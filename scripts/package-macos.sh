#!/usr/bin/env bash
# Builds Boost for macOS into dist/:
#   Boost.app                                  app + browser host + CLI + extension
#   boost-<version>-macos-<arch>.dmg          the app, to drag into Applications
#   Boost.app.tar.gz + .sig                    the update the app installs by itself
# The app registers its browser host and copies the extension out of the bundle
# each time it starts, so the .dmg needs no installer.
# The signed Firefox add-on (web-ext-artifacts/boost-<ext version>.xpi, from
# scripts/sign-extension.sh) goes into the bundle; when it is not there, the one
# of this version's GitHub release is used if it holds the same extension version.
#
# Signing: with a "Developer ID Application" certificate in the keychain the app
# and the .dmg are signed with it (hardened runtime) and, when the notary
# credentials are saved as the keychain profile "boost-notary", notarized and
# stapled, so they open with no Gatekeeper warning. Without the certificate the
# bundle is signed ad hoc: the first time it is opened with right click → Open.
#   xcrun notarytool store-credentials boost-notary --apple-id <id> --team-id <team>
#
# Updates: Boost.app.tar.gz is signed with the updater key (~/.config/boost/updater.key,
# its password in the keychain item "boost-updater"); the app checks it against
# crates/raudo-app/updater/public-key.txt.
#
# Usage: package-macos.sh [--install] [--publish]
#   --install  also copies it to ~/Applications
#   --publish  uploads the .dmg and the update to the GitHub release v<version>
#              (which must exist) and adds them to its SHA256SUMS and latest.json
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
EXT_VERSION=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['version'])" extension/manifest.json)
XPI="web-ext-artifacts/boost-$EXT_VERSION.xpi"
ARCH=$(uname -m)
INSTALL=0
PUBLISH=0
for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        --publish) PUBLISH=1 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done
# By its SHA-1: names with accents ("Rodríguez") do not reach codesign intact.
DEVELOPER_ID=$(security find-identity -v -p codesigning | grep '"Developer ID Application: ' | head -1 || true)
IDENTITY=$(awk '{print $2}' <<<"$DEVELOPER_ID")
IDENTITY_NAME=$(sed -n 's/.*"\(.*\)"/\1/p' <<<"$DEVELOPER_ID")
UPDATER_KEY="$HOME/.config/boost/updater.key"
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
if [[ ! -f "$XPI" ]]; then
    # Firefox only installs add-ons Mozilla has signed: until this version is
    # signed (scripts/sign-extension.sh), ship the newest signed one.
    OLDER=$(ls -t web-ext-artifacts/*.xpi 2>/dev/null | head -1 || true)
    if [[ -n "$OLDER" ]]; then
        echo "    WARNING: no signed Firefox add-on $EXT_VERSION; shipping $(basename "$OLDER") (sign it with scripts/sign-extension.sh)"
        XPI="$OLDER"
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
if [[ -n "$IDENTITY" ]]; then
    echo "==> signing with $IDENTITY_NAME"
    # Inner executables first, each with the hardened runtime notarization requires.
    for exe in raudo-nmhost raudo-cli boost; do
        codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP/Contents/MacOS/$exe"
    done
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
    codesign --verify --strict --deep "$APP"
else
    echo "==> signing ad hoc (no Developer ID Application certificate in the keychain)"
    codesign --force --deep --sign - "$APP"
fi

echo "==> disk image"
DMG="$DIST/boost-$VERSION-macos-$ARCH.dmg"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "Boost $VERSION" -srcfolder "$STAGE" -format UDZO "$DMG"

if [[ -n "$IDENTITY" ]]; then
    codesign --force --timestamp --sign "$IDENTITY" "$DMG"
    if xcrun notarytool history --keychain-profile boost-notary >/dev/null 2>&1; then
        echo "==> notarizing (a few minutes)"
        # The disk image carries the app: both are notarized and get the ticket.
        xcrun notarytool submit "$DMG" --keychain-profile boost-notary --wait
        xcrun stapler staple "$DMG"
        xcrun stapler staple "$APP"
        spctl --assess --type open --context context:primary-signature -v "$DMG"
        spctl --assess --type execute -v "$APP"
    else
        echo "    (no notary profile \"boost-notary\": signed but not notarized)"
    fi
fi

echo "==> update package"
UPDATE="$DIST/Boost.app.tar.gz"
rm -f "$UPDATE" "$UPDATE.sig"
# No AppleDouble (._*) files in the archive.
COPYFILE_DISABLE=1 tar -czf "$UPDATE" -C "$DIST" Boost.app
if [[ -f "$UPDATER_KEY" ]]; then
    CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD=$(security find-generic-password -s boost-updater -w) \
        "$HOME/.cargo/bin/cargo-packager" signer sign --private-key "$UPDATER_KEY" "$UPDATE" -q
else
    echo "    (no $UPDATER_KEY: the update is not signed and cannot be published)"
fi
rm -rf "$WORK"

if ((PUBLISH)); then
    TAG="v$VERSION"
    REPO="gallegomanuel8/Boost"
    [[ -f "$UPDATE.sig" ]] || { echo "the update is not signed" >&2; exit 1; }
    gh release view "$TAG" -R "$REPO" >/dev/null || { echo "release $TAG does not exist" >&2; exit 1; }
    echo "==> publishing to $TAG"
    PUB="$DIST/publish"
    rm -rf "$PUB"
    mkdir -p "$PUB"
    cp "$DMG" "$UPDATE" "$UPDATE.sig" "$PUB/"
    # Keep what the other platforms put in SHA256SUMS and latest.json.
    gh release download "$TAG" -R "$REPO" -p SHA256SUMS -D "$PUB" 2>/dev/null || touch "$PUB/SHA256SUMS"
    gh release download "$TAG" -R "$REPO" -p latest.json -D "$PUB" 2>/dev/null || true
    (cd "$PUB" && grep -v -e " $(basename "$DMG")\$" -e " Boost.app.tar.gz\$" SHA256SUMS >SHA256SUMS.new || true
        shasum -a 256 "$(basename "$DMG")" Boost.app.tar.gz >>SHA256SUMS.new && mv SHA256SUMS.new SHA256SUMS)
    python3 - "$PUB" "$TAG" "$REPO" "aarch64" <<'PY'
import datetime, json, os, sys
pub, tag, repo, arch = sys.argv[1:]
path = os.path.join(pub, "latest.json")
manifest = json.load(open(path)) if os.path.exists(path) else {}
manifest["version"] = tag.lstrip("v")
manifest["notes"] = f"https://github.com/{repo}/releases/tag/{tag}"
manifest["pub_date"] = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
manifest.setdefault("platforms", {})[f"macos-{arch}"] = {
    "url": f"https://github.com/{repo}/releases/download/{tag}/Boost.app.tar.gz",
    "signature": open(os.path.join(pub, "Boost.app.tar.gz.sig")).read().strip(),
    "format": "app",
}
json.dump(manifest, open(path, "w"), indent=2)
PY
    gh release upload "$TAG" -R "$REPO" --clobber "$PUB/$(basename "$DMG")" "$PUB/Boost.app.tar.gz" "$PUB/SHA256SUMS" "$PUB/latest.json"
    rm -rf "$PUB"
fi

if ((INSTALL)); then
    echo "==> installing in ~/Applications"
    mkdir -p "$HOME/Applications"
    rm -rf "$HOME/Applications/Boost.app"
    cp -R "$APP" "$HOME/Applications/"
    "$HOME/Applications/Boost.app/Contents/MacOS/raudo-nmhost" register
fi

echo "==> done: $APP, $DMG, $UPDATE"
