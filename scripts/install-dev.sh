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
# Classic layout and the XDG layout used by recent Firefox releases.
FIREFOX_DIRS=("$HOME/.mozilla" "${XDG_CONFIG_HOME:-$HOME/.config}/mozilla")

uninstall() {
    rm -f "$BIN/raudo-daemon" "$BIN/raudo-cli" "$BIN/raudo-nmhost" "$DBUS_DIR/org.raudo.Manager.service"
    for d in "${CHROMIUM_DIRS[@]}"; do rm -f "$d/NativeMessagingHosts/$HOST_NAME.json"; done
    for d in "${FIREFOX_DIRS[@]}"; do rm -f "$d/native-messaging-hosts/$HOST_NAME.json"; done
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
for d in "${FIREFOX_DIRS[@]}"; do
    [[ -d "$d" ]] || continue
    mkdir -p "$d/native-messaging-hosts"
    cat > "$d/native-messaging-hosts/$HOST_NAME.json" <<EOF
{
  "name": "$HOST_NAME",
  "description": "raudo download manager",
  "path": "$BIN/raudo-nmhost",
  "type": "stdio",
  "allowed_extensions": ["$FIREFOX_ID"]
}
EOF
    echo "    $d"
done

cat <<EOF

Done. Load the extension from: $ROOT/extension
  Chrome/Chromium: chrome://extensions → Developer mode → Load unpacked
  Firefox:         about:debugging#/runtime/this-firefox → Load Temporary Add-on → manifest.json
Flatpak/Snap browsers are sandboxed and cannot start native hosts this way.
EOF
