#!/usr/bin/env bash
# Builds the downloadable packages of a release into dist/:
#   boost-<version>-linux-x86_64.tar.gz    app + browser host + install.sh
#   boost-<version>-windows-x86_64.zip     app + browser host + install.ps1
#   boost-<version>-extension-chromium.zip the extension, to load unpacked
#   boost-<version>-extension-firefox.xpi  the signed Firefox add-on, when there is one
# The signed Firefox add-on (web-ext-artifacts/boost-<ext version>.xpi, from
# scripts/sign-extension.sh) goes into both app packages when it exists.
# Needs the x86_64-unknown-linux-musl and x86_64-pc-windows-gnu Rust targets
# (the latter with mingw64-gcc).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
EXT_VERSION=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['version'])" extension/manifest.json)
XPI="web-ext-artifacts/boost-$EXT_VERSION.xpi"
DIST="$ROOT/dist"
WORK="$DIST/work"
rm -rf "$WORK"
mkdir -p "$WORK"

echo "==> building (Linux, Linux static host, Windows)"
cargo build --release --quiet -p raudo-app -p raudo-nmhost -p raudo-cli
cargo build --release --quiet -p raudo-nmhost --target x86_64-unknown-linux-musl
cargo build --release --quiet -p raudo-app -p raudo-nmhost -p raudo-cli --target x86_64-pc-windows-gnu

common() {
    local dir=$1
    cp -r extension "$dir/extension"
    cp LICENSE README.md "$dir/"
    if [[ -f "$XPI" ]]; then
        cp "$XPI" "$dir/boost.xpi"
    else
        echo "    (no signed $XPI: the Firefox add-on is not included)"
    fi
}

echo "==> Linux package"
L="$WORK/boost-$VERSION-linux-x86_64"
mkdir -p "$L/bin/flatpak" "$L/icons"
install -m 755 target/release/{boost,raudo-nmhost,raudo-cli} "$L/bin/"
install -m 755 target/x86_64-unknown-linux-musl/release/raudo-nmhost "$L/bin/flatpak/"
cp crates/raudo-app/icons/{32x32,128x128,128x128@2x,icon}.png "$L/icons/"
install -m 755 scripts/install.sh "$L/install.sh"
common "$L"
tar -C "$WORK" -czf "$DIST/boost-$VERSION-linux-x86_64.tar.gz" "$(basename "$L")"

echo "==> Windows package"
W="$WORK/boost-$VERSION-windows-x86_64"
mkdir -p "$W"
cp target/x86_64-pc-windows-gnu/release/{boost,raudo-nmhost,raudo-cli}.exe "$W/"
# GNU builds load WebView2 through this DLL, which Windows does not ship.
cp target/x86_64-pc-windows-gnu/release/WebView2Loader.dll "$W/"
cp scripts/install.ps1 "$W/"
common "$W"
(cd "$WORK" && zip -qr "$DIST/boost-$VERSION-windows-x86_64.zip" "$(basename "$W")")

echo "==> browser extensions"
(cd extension && zip -qr "$DIST/boost-$VERSION-extension-chromium.zip" .)
[[ -f "$XPI" ]] && cp "$XPI" "$DIST/boost-$VERSION-extension-firefox.xpi"

rm -rf "$WORK"
(cd "$DIST" && sha256sum boost-"$VERSION"-* > SHA256SUMS)
ls -la "$DIST"
