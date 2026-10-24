#!/usr/bin/env bash
# Installs raudo for the current user (no root):
#   ~/.local/bin/raudo-{daemon,cli,nmhost}
#   D-Bus activation file, so the daemon starts on first use
#   Native Messaging manifests for the browsers found in $HOME
# Usage: scripts/install-dev.sh [--uninstall]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$HOME/.local/bin"
DBUS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services"
HOST_NAME="org.raudo.nmhost"
CHROME_ID="boloeecmdjhgdkcahinhhpakjhiljbhi"
FIREFOX_ID="raudo@raudo.org"

# Chromium-family config dirs; a manifest is written only where the browser exists.
CHROMIUM_DIRS=(
    "$HOME/.config/google-chrome"
    "$HOME/.config/google-chrome-beta"
    "$HOME/.config/chromium"
    "$HOME/.config/BraveSoftware/Brave-Browser"
    "$HOME/.config/microsoft-edge"
    "$HOME/.config/vivaldi"
)
# Firefox reads user native manifests from ~/.mozilla even when its profile uses the
# XDG layout (~/.config/mozilla/firefox); verified on Firefox 158. A ~/.mozilla
# without a firefox/ subdirectory does not move the profile back to the legacy layout.
FIREFOX_NM_DIR="$HOME/.mozilla/native-messaging-hosts"
# Stale location written by earlier versions of this script.
OLD_FIREFOX_NM_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/mozilla/native-messaging-hosts"

# Flatpak Chromium-family browsers ("app id:config dir"). They are sandboxed: each gets
# a static copy of the host inside its own data dir, and D-Bus access to org.raudo.Manager
# only (no flatpak-spawn --host, which would let the browser run any host command).
FLATPAK_CHROMIUM=(
    "com.brave.Browser:BraveSoftware/Brave-Browser"
    "org.chromium.Chromium:chromium"
    "com.google.Chrome:google-chrome"
    "com.microsoft.Edge:microsoft-edge"
    "com.vivaldi.Vivaldi:vivaldi"
)
STATIC_TARGET="x86_64-unknown-linux-musl"

flatpak_installed() {
    command -v flatpak >/dev/null && flatpak info "$1" >/dev/null 2>&1
}

uninstall() {
    for entry in "${FLATPAK_CHROMIUM[@]}"; do
        app=${entry%%:*}
        cfg=${entry#*:}
        rm -f "$HOME/.var/app/$app/config/$cfg/NativeMessagingHosts/$HOST_NAME.json"
        rm -rf "$HOME/.var/app/$app/data/raudo"
        flatpak_installed "$app" && flatpak override --user --no-talk-name=org.raudo.Manager "$app" 2>/dev/null || true
    done
    rm -f "$BIN/raudo-daemon" "$BIN/raudo-cli" "$BIN/raudo-nmhost" "$DBUS_DIR/org.raudo.Manager.service"
    for d in "${CHROMIUM_DIRS[@]}"; do rm -f "$d/NativeMessagingHosts/$HOST_NAME.json"; done
    rm -f "$FIREFOX_NM_DIR/$HOST_NAME.json" "$OLD_FIREFOX_NM_DIR/$HOST_NAME.json"
    echo "raudo removed (downloads database in ${XDG_DATA_HOME:-$HOME/.local/share}/raudo kept)."
}

if [[ "${1:-}" == "--uninstall" ]]; then
    uninstall
    exit 0
fi

echo "==> building release binaries"
(cd "$ROOT" && cargo build --release --quiet -p raudo-daemon -p raudo-cli -p raudo-nmhost)

echo "==> installing binaries into $BIN"
mkdir -p "$BIN"
for b in raudo-daemon raudo-cli raudo-nmhost; do
    install -m 755 "$ROOT/target/release/$b" "$BIN/$b"
done

echo "==> D-Bus activation"
mkdir -p "$DBUS_DIR"
cat > "$DBUS_DIR/org.raudo.Manager.service" <<EOF
[D-BUS Service]
Name=org.raudo.Manager
Exec=$BIN/raudo-daemon
EOF
# dbus-broker only picks up new .service files after a reload.
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true

echo "==> Native Messaging manifests"
for d in "${CHROMIUM_DIRS[@]}"; do
    [[ -d "$d" ]] || continue
    mkdir -p "$d/NativeMessagingHosts"
    cat > "$d/NativeMessagingHosts/$HOST_NAME.json" <<EOF
{
  "name": "$HOST_NAME",
  "description": "raudo download manager",
  "path": "$BIN/raudo-nmhost",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$CHROME_ID/"]
}
EOF
    echo "    $d"
done
rm -f "$OLD_FIREFOX_NM_DIR/$HOST_NAME.json"
rmdir "$OLD_FIREFOX_NM_DIR" 2>/dev/null || true
if command -v firefox >/dev/null || [[ -d "$HOME/.mozilla" || -d "${XDG_CONFIG_HOME:-$HOME/.config}/mozilla" ]]; then
    mkdir -p "$FIREFOX_NM_DIR"
    cat > "$FIREFOX_NM_DIR/$HOST_NAME.json" <<EOF
{
  "name": "$HOST_NAME",
  "description": "raudo download manager",
  "path": "$BIN/raudo-nmhost",
  "type": "stdio",
  "allowed_extensions": ["$FIREFOX_ID"]
}
EOF
    echo "    $FIREFOX_NM_DIR"
fi

flatpak_apps=()
for entry in "${FLATPAK_CHROMIUM[@]}"; do
    flatpak_installed "${entry%%:*}" && flatpak_apps+=("$entry")
done
if ((${#flatpak_apps[@]})); then
    echo "==> static host for Flatpak browsers"
    rustup target add "$STATIC_TARGET" >/dev/null 2>&1 || true
    (cd "$ROOT" && cargo build --release --quiet -p raudo-nmhost --target "$STATIC_TARGET")
    for entry in "${flatpak_apps[@]}"; do
        app=${entry%%:*}
        cfg=${entry#*:}
        host_bin="$HOME/.var/app/$app/data/raudo/raudo-nmhost"
        install -D -m 755 "$ROOT/target/$STATIC_TARGET/release/raudo-nmhost" "$host_bin"
        nm_dir="$HOME/.var/app/$app/config/$cfg/NativeMessagingHosts"
        mkdir -p "$nm_dir"
        cat > "$nm_dir/$HOST_NAME.json" <<EOF
{
  "name": "$HOST_NAME",
  "description": "raudo download manager",
  "path": "$host_bin",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$CHROME_ID/"]
}
EOF
        flatpak override --user --talk-name=org.raudo.Manager "$app"
        # The sandbox cannot read the project folder: keep a copy of the extension it can
        # load unpacked. Re-running this script updates it (then reload it in the browser).
        rm -rf "$HOME/.var/app/$app/data/raudo/extension"
        cp -r "$ROOT/extension" "$HOME/.var/app/$app/data/raudo/extension"
        echo "    $app (flatpak) — extension: $HOME/.var/app/$app/data/raudo/extension"
    done
fi

cat <<EOF

Done. Load the extension from: $ROOT/extension
  Chrome/Chromium: chrome://extensions → Developer mode → Load unpacked
  Firefox:         about:debugging#/runtime/this-firefox → Load Temporary Add-on → manifest.json
Snap browsers and Flatpak Firefox are not supported yet.
EOF
