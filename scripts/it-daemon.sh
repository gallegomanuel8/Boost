#!/usr/bin/env bash
# Daemon integration test on a private D-Bus session and a throwaway data dir.
# Usage: scripts/it-daemon.sh   (builds first; needs network access to cdn.kernel.org)
set -euo pipefail

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" || -z "${RAUDO_IT_INNER:-}" ]]; then
    cargo build --quiet
    RAUDO_IT_INNER=1 exec dbus-run-session -- "$0" "$@"
fi

B="$(cd "$(dirname "$0")/.." && pwd)/target/debug"
T=$(mktemp -d)
trap 'kill $D 2>/dev/null || true; rm -rf "$T"' EXIT
cd "$T"; mkdir -p data dl
export XDG_DATA_HOME=$T/data RUST_LOG=warn
BASE=https://cdn.kernel.org/pub/linux/kernel/v6.x
cli() { "$B/raudo-cli" "$@"; }

"$B/raudo-daemon" & D=$!; sleep 1
cli limit 3072
cli add -d dl $BASE/linux-6.19.tar.xz >/dev/null
cli add -d dl --paused $BASE/sha256sums.asc >/dev/null
sleep 4
cli list | grep -q "1  downloading" || { echo "FAIL: not downloading"; exit 1; }

cli pause 1; sleep 1
cli list | grep -q "1  paused" || { echo "FAIL: not paused"; exit 1; }
cli resume 1; cli resume 2; sleep 3

kill -TERM $D; wait $D || true
"$B/raudo-daemon" & D=$!; sleep 1
cli limit 0
cli list | grep -q "1  downloading" || { echo "FAIL: not resumed after restart"; exit 1; }

for _ in $(seq 120); do cli list | grep -q downloading || break; sleep 1; done
cli list
(cd dl && grep " linux-6.19.tar.xz$" sha256sums.asc | sha256sum -c -)

cli rm 2 --delete
[[ ! -e dl/sha256sums.asc ]] || { echo "FAIL: file not deleted"; exit 1; }
echo "OK"
