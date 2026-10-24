#!/usr/bin/env bash
# Installs raudo for the current user (no root):
#   ~/.local/bin/raudo-{daemon,cli,nmhost}
#   session autostart entry for the service (the browser host also starts it on demand)
#   Native Messaging manifests for the browsers found in $HOME
# Usage: scripts/install-dev.sh [--uninstall]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$HOME/.local/bin"
# Left by versions that used D-Bus activation; removed on install.
OLD_DBUS_SERVICE="${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services/org.raudo.Manager.service"
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/raudo-daemon.desktop"
HOST_NAME="org.raudo.nmhost"
CHROME_ID="boloeecmdjhgdkcahinhhpakjhiljbhi"

# Regular browsers are registered by `raudo-nmhost register`. Firefox reads user
# manifests from ~/.mozilla even when its profile uses the XDG layout
# (~/.config/mozilla/firefox); verified on Firefox 158.
# Stale location written by earlier versions of this script.
OLD_FIREFOX_NM_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/mozilla/native-messaging-hosts"

# Flatpak Chromium-family browsers ("app id:config dir"). They are sandboxed: each gets
# a static copy of the host inside its own data dir, and access to $XDG_RUNTIME_DIR/raudo
# only, where the service socket and token live (no flatpak-spawn --host, which would
# let the browser run any host command).
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
        if flatpak_installed "$app"; then
            flatpak override --user --nofilesystem=xdg-run/raudo "$app" 2>/dev/null || true
            flatpak override --user --no-talk-name=org.raudo.Manager "$app" 2>/dev/null || true
        fi
    done
    [[ -x "$BIN/raudo-nmhost" ]] && "$BIN/raudo-nmhost" unregister >/dev/null
    rm -f "$BIN/raudo-daemon" "$BIN/raudo-cli" "$BIN/raudo-nmhost" "$OLD_DBUS_SERVICE" "$AUTOSTART"
    rm -f "$OLD_FIREFOX_NM_DIR/$HOST_NAME.json"
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

if [[ -f "$OLD_DBUS_SERVICE" ]]; then
    rm -f "$OLD_DBUS_SERVICE"
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
fi

echo "==> autostart with the session"
# Sandboxed browsers cannot start the service themselves; this keeps it available.
mkdir -p "$(dirname "$AUTOSTART")"
cat > "$AUTOSTART" <<EOF
[Desktop Entry]
Type=Application
Name=raudo
Comment=raudo download service
Exec=$BIN/raudo-daemon
NoDisplay=true
X-GNOME-Autostart-enabled=true
EOF

echo "==> Native Messaging manifests"
rm -f "$OLD_FIREFOX_NM_DIR/$HOST_NAME.json"
rmdir "$OLD_FIREFOX_NM_DIR" 2>/dev/null || true
# Same code the Windows installer uses (there it writes the registry).
"$BIN/raudo-nmhost" register | sed 's/^registered: /    /'

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
        flatpak override --user --no-talk-name=org.raudo.Manager "$app" 2>/dev/null || true
        flatpak override --user --filesystem=xdg-run/raudo "$app"
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
