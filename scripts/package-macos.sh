#!/usr/bin/env bash
# Builds Boost for macOS into dist/:
#   Boost.app                                  app + browser host + CLI + extension
#   boost-<version>-macos-<arch>.dmg          the app, to drag into Applications
# The app registers its browser host and copies the extension out of the bundle
# each time it starts, so the .dmg needs no installer.
# The signed Firefox add-on (web-ext-artifacts/boost-<ext version>.xpi, from
# scripts/sign-extension.sh) goes into the bundle; when it is not there, the one
# of this version's GitHub release is used if it holds the same extension version.
# The bundle is signed ad hoc (not notarized): the first time, open it with
# right click → Open, or allow it in System Settings → Privacy & Security.
# Usage: package-macos.sh [--install]   (--install also copies it to ~/Applications)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
EXT_VERSION=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['version'])" extension/manifest.json)
XPI="web-ext-artifacts/boost-$EXT_VERSION.xpi"
ARCH=$(uname -m)
DIST="$ROOT/dist"
APP="$DIST/Boost.app"
WORK="$DIST/work"
rm -rf "$APP" "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$WORK"

echo "==> building ($ARCH)"
cargo build --release --quiet -p raudo-app -p raudo-nmhost -p raudo-cli
cp target/release/{boost,raudo-nmhost,raudo-cli} "$APP/Contents/MacOS/"

echo "==> icon"
ICONSET="$WORK/icon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z $size $size crates/raudo-app/icons/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    if ((double <= 512)); then
        sips -z $double $double crates/raudo-app/icons/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
    fi
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

echo "==> bundle"
cp -r extension "$APP/Contents/Resources/extension"
if [[ ! -f "$XPI" ]]; then
    RELEASE="https://github.com/gallegomanuel8/Boost/releases/download/v$VERSION"
    REMOTE="boost-$VERSION-extension-firefox.xpi"
    if curl -fsSL -o "$WORK/$REMOTE" "$RELEASE/$REMOTE" && curl -fsSL -o "$WORK/SHA256SUMS" "$RELEASE/SHA256SUMS" \
        && (cd "$WORK" && grep " $REMOTE\$" SHA256SUMS | shasum -a 256 -c --status) \
        && [[ "$(unzip -p "$WORK/$REMOTE" manifest.json | python3 -c "import json,sys;print(json.load(sys.stdin)['version'])")" == "$EXT_VERSION" ]]; then
        mkdir -p web-ext-artifacts
        cp "$WORK/$REMOTE" "$XPI"
        echo "    (signed Firefox add-on taken from the v$VERSION release)"
    fi
fi
if [[ -f "$XPI" ]]; then
    cp "$XPI" "$APP/Contents/Resources/boost.xpi"
else
    echo "    (no signed $XPI: the Firefox add-on is not included)"
fi
cat >"$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>es</string>
    <key>CFBundleDisplayName</key><string>Boost</string>
    <key>CFBundleName</key><string>Boost</string>
    <key>CFBundleExecutable</key><string>boost</string>
    <key>CFBundleIdentifier</key><string>org.boost.app</string>
    <key>CFBundleIconFile</key><string>icon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleURLTypes</key>
    <array>
        <dict>
            <key>CFBundleURLName</key><string>org.boost.app</string>
            <key>CFBundleURLSchemes</key><array><string>boost</string></array>
        </dict>
    </array>
</dict>
</plist>
EOF
codesign --force --deep --sign - "$APP"

echo "==> disk image"
DMG="$DIST/boost-$VERSION-macos-$ARCH.dmg"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "Boost $VERSION" -srcfolder "$STAGE" -format UDZO "$DMG"
rm -rf "$WORK"

if [[ "${1:-}" == "--install" ]]; then
    echo "==> installing in ~/Applications"
    mkdir -p "$HOME/Applications"
    rm -rf "$HOME/Applications/Boost.app"
    cp -R "$APP" "$HOME/Applications/"
    "$HOME/Applications/Boost.app/Contents/MacOS/raudo-nmhost" register
fi

echo "==> done: $APP, $DMG"
