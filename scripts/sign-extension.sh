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
[[ "$WEB_EXT_API_KEY" == user:* && -n "$WEB_EXT_API_SECRET" ]] || {
    echo "Claves vacías o con formato incorrecto (el emisor empieza por 'user:')." >&2
    exit 1
}
export WEB_EXT_API_KEY WEB_EXT_API_SECRET

VERSION=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['version'])" "$ROOT/extension/manifest.json")
OUT="$ROOT/web-ext-artifacts"
cd "$ROOT"
npx -y web-ext@10.7.0 sign -s extension --channel unlisted --artifacts-dir "$OUT"

XPI=$(ls -t "$OUT"/*-"$VERSION".xpi | head -1)
mv "$XPI" "$OUT/raudo-$VERSION.xpi"
echo "Firmado: $OUT/raudo-$VERSION.xpi"
command -v firefox >/dev/null && firefox "$OUT/raudo-$VERSION.xpi"
