#!/usr/bin/env bash
# Service integration test with a private IPC endpoint and a throwaway data dir.
# Usage: scripts/it-daemon.sh   (builds first; needs network access to cdn.kernel.org)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
(cd "$ROOT" && cargo build --quiet)
B="$ROOT/target/debug"
T=$(mktemp -d)
# Short runtime dir: Unix socket paths are limited to 108 bytes.
RT=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/raudo-it.XXXX")
D=""
trap 'kill $D 2>/dev/null || true; rm -rf "$T" "$RT"' EXIT
cd "$T"; mkdir -p data dl
export XDG_RUNTIME_DIR=$RT XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache RUST_LOG=warn
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
