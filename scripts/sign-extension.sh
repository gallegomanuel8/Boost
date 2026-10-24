#!/usr/bin/env bash
# Signs the Firefox extension on addons.mozilla.org (unlisted: signed, not published)
# and opens the resulting .xpi in Firefox.
# Keys: https://addons.mozilla.org/developers/addon/api/key/
# They are read from the terminal (or WEB_EXT_API_KEY / WEB_EXT_API_SECRET), never stored.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.local/opt/node/bin:$PATH"

if [[ -z "${WEB_EXT_API_KEY:-}" ]]; then
    read -rp "Emisor JWT (user:...): " WEB_EXT_API_KEY </dev/tty
fi
if [[ -z "${WEB_EXT_API_SECRET:-}" ]]; then
    read -rsp "Secreto JWT: " WEB_EXT_API_SECRET </dev/tty
    echo
fi
# Pasted values often carry stray spaces or newlines.
WEB_EXT_API_KEY=$(tr -d '[:space:]' <<<"$WEB_EXT_API_KEY")
WEB_EXT_API_SECRET=$(tr -d '[:space:]' <<<"$WEB_EXT_API_SECRET")
[[ "$WEB_EXT_API_KEY" =~ ^user:[0-9]+:[0-9]+$ ]] || {
    echo "El emisor debe tener la forma user:NNNN:NNN" >&2
    exit 1
}
[[ "$WEB_EXT_API_SECRET" =~ ^[0-9a-f]{64}$ ]] || {
    echo "El secreto debe tener 64 caracteres hexadecimales (se leyeron ${#WEB_EXT_API_SECRET})." >&2
    exit 1
}
export WEB_EXT_API_KEY WEB_EXT_API_SECRET

VERSION=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['version'])" "$ROOT/extension/manifest.json")
OUT="$ROOT/web-ext-artifacts"
cd "$ROOT"
npx -y web-ext@10.7.0 sign -s extension --channel unlisted --artifacts-dir "$OUT"

XPI=$(ls -t "$OUT"/*-"$VERSION".xpi | head -1)
mv "$XPI" "$OUT/boost-$VERSION.xpi"
echo "Firmado: $OUT/boost-$VERSION.xpi"
command -v firefox >/dev/null && firefox "$OUT/boost-$VERSION.xpi"
