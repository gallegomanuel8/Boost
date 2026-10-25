#!/usr/bin/env bash
# Builds the extension zips to upload to the browser stores, into dist/:
#   boost-extension-<version>-chrome-web-store.zip   Chrome Web Store (Chrome, Brave)
#   boost-extension-<version>-edge-addons.zip        Microsoft Edge Add-ons
#   boost-extension-<version>-amo-source.zip         addons.mozilla.org (listed)
# The stores assign their own extension ID, so the Chromium zips drop `key`
# (which pins the ID used when loading the extension unpacked) and the
# Firefox-only fields; see docs/tiendas/TIENDAS.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['version'])" extension/manifest.json)
DIST="$ROOT/dist"
WORK="$DIST/stores-work"
rm -rf "$WORK"
mkdir -p "$WORK"

chromium_zip() {
    local out=$1
    rm -rf "$WORK/ext"
    cp -r extension "$WORK/ext"
    python3 - "$WORK/ext/manifest.json" <<'PY'
import json, sys
p = sys.argv[1]
m = json.load(open(p))
m.pop("key", None)                      # the store assigns the ID
m.pop("browser_specific_settings", None)  # Firefox only
m["background"].pop("scripts", None)      # Firefox's background page; Chromium uses the service worker
json.dump(m, open(p, "w"), indent=2, ensure_ascii=False)
PY
    (cd "$WORK/ext" && zip -qr "$out" .)
}

chromium_zip "$DIST/boost-extension-$VERSION-chrome-web-store.zip"
chromium_zip "$DIST/boost-extension-$VERSION-edge-addons.zip"
(cd extension && zip -qr "$DIST/boost-extension-$VERSION-amo-source.zip" .)

rm -rf "$WORK"
ls -la "$DIST"/boost-extension-"$VERSION"-*
