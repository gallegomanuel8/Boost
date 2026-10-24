#!/usr/bin/env bash
# Installs Boost for the current user (no root):
#   ~/.local/bin/boost (the app) and raudo-{nmhost,cli,daemon}
#   application menu entry and the boost:// link that launches it
#   Native Messaging manifests for the browsers found in $HOME (Flatpak ones too)
#
# Works from the source tree (scripts/install.sh: builds first) and from the
# downloaded Linux package (./install.sh: uses the binaries in bin/).
# Usage: install.sh [--uninstall]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
STATIC_TARGET="x86_64-unknown-linux-musl"
if [[ -f "$HERE/../Cargo.toml" ]]; then
    FROM_SOURCE=1
    ROOT="$(cd "$HERE/.." && pwd)"
    BIN_SRC="$ROOT/target/release"
    STATIC_HOST="$ROOT/target/$STATIC_TARGET/release/raudo-nmhost"
    ICON_SRC="$ROOT/crates/raudo-app/icons"
else
    FROM_SOURCE=0
    ROOT="$HERE"
    BIN_SRC="$ROOT/bin"
    STATIC_HOST="$ROOT/bin/flatpak/raudo-nmhost"
    ICON_SRC="$ROOT/icons"
fi
EXTENSION="$ROOT/extension"
BIN="$HOME/.local/bin"
# Left by versions that used D-Bus activation; removed on install.
OLD_DBUS_SERVICE="${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services/org.raudo.Manager.service"
# Left by versions that started raudo-daemon at login; removed on install.
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/raudo-daemon.desktop"
HOST_NAME="org.raudo.nmhost"
CHROME_ID="boloeecmdjhgdkcahinhhpakjhiljbhi"
APPS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
# The app was called raudo before Boost: its launcher and icon are removed on install.
# Icons named just "boost" lose against icon themes that ship a "boost" action icon
# (Breeze, Papirus...), so they are installed under the app ID instead.
OLD_APP=("$BIN/raudo" "$APPS_DIR/org.raudo.app.desktop" "$ICONS/128x128/apps/raudo.png"
    "$ICONS"/{32x32,128x128,256x256,512x512}/apps/boost.png)

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
    rm -f "${OLD_APP[@]}" "$BIN/boost" "$BIN/raudo-daemon" "$BIN/raudo-cli" "$BIN/raudo-nmhost" "$OLD_DBUS_SERVICE" "$AUTOSTART"
    rm -f "$APPS_DIR/org.boost.app.desktop" "$ICONS"/{32x32,128x128,256x256,512x512}/apps/org.boost.app.png
    rm -f "$OLD_FIREFOX_NM_DIR/$HOST_NAME.json"
    rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/boost"
    echo "Boost removed (downloads database in ${XDG_DATA_HOME:-$HOME/.local/share}/raudo kept)."
}

if [[ "${1:-}" == "--uninstall" ]]; then
    uninstall
    exit 0
fi

if ((FROM_SOURCE)); then
    echo "==> building release binaries"
    (cd "$ROOT" && cargo build --release --quiet -p raudo-app -p raudo-daemon -p raudo-cli -p raudo-nmhost)
fi

echo "==> installing binaries into $BIN"
mkdir -p "$BIN"
for b in boost raudo-daemon raudo-cli raudo-nmhost; do
    [[ -f "$BIN_SRC/$b" ]] && install -m 755 "$BIN_SRC/$b" "$BIN/$b"
done

if [[ -f "$OLD_DBUS_SERVICE" ]]; then
    rm -f "$OLD_DBUS_SERVICE"
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
fi

echo "==> application menu entry"
rm -f "${OLD_APP[@]}"
install -D -m 644 "$ICON_SRC/32x32.png" "$ICONS/32x32/apps/org.boost.app.png"
install -D -m 644 "$ICON_SRC/128x128.png" "$ICONS/128x128/apps/org.boost.app.png"
install -D -m 644 "$ICON_SRC/128x128@2x.png" "$ICONS/256x256/apps/org.boost.app.png"
install -D -m 644 "$ICON_SRC/icon.png" "$ICONS/512x512/apps/org.boost.app.png"
mkdir -p "$APPS_DIR"
cat > "$APPS_DIR/org.boost.app.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Boost
GenericName=Gestor de descargas
Comment=Descarga ficheros y vídeos con varias conexiones
Exec=$BIN/boost %u
Icon=org.boost.app
Terminal=false
Categories=Network;FileTransfer;
StartupWMClass=boost
MimeType=x-scheme-handler/boost;
EOF
update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true
xdg-mime default org.boost.app.desktop x-scheme-handler/boost
gtk-update-icon-cache -q -t "$ICONS" >/dev/null 2>&1 || true
# KDE caches menu entries and their icons.
kbuildsycoca6 >/dev/null 2>&1 || true

echo "==> browser extension files"
# Fixed place for the unpacked extension (Chromium browsers load it from here) and the
# signed Firefox add-on (Preferencias → Extensiones web installs it from here).
SHARE="${XDG_DATA_HOME:-$HOME/.local/share}/boost"
rm -rf "$SHARE/extension"
mkdir -p "$SHARE"
cp -r "$EXTENSION" "$SHARE/extension"
if ((FROM_SOURCE)); then
    XPI=$(ls -t "$ROOT"/web-ext-artifacts/boost-*.xpi 2>/dev/null | head -1 || true)
else
    XPI="$ROOT/boost.xpi"
fi
[[ -n "$XPI" && -f "$XPI" ]] && install -m 644 "$XPI" "$SHARE/boost.xpi"

# Boost runs only while the app is open: no background service at login. The
# extension offers to launch it (sandboxed browsers through the boost: link).
rm -f "$AUTOSTART"

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
    if ((FROM_SOURCE)); then
        rustup target add "$STATIC_TARGET" >/dev/null 2>&1 || true
        (cd "$ROOT" && cargo build --release --quiet -p raudo-nmhost --target "$STATIC_TARGET")
    fi
    for entry in "${flatpak_apps[@]}"; do
        app=${entry%%:*}
        cfg=${entry#*:}
        host_bin="$HOME/.var/app/$app/data/raudo/raudo-nmhost"
        install -D -m 755 "$STATIC_HOST" "$host_bin"
        nm_dir="$HOME/.var/app/$app/config/$cfg/NativeMessagingHosts"
        mkdir -p "$nm_dir"
        cat > "$nm_dir/$HOST_NAME.json" <<EOF
{
  "name": "$HOST_NAME",
  "description": "Boost download manager",
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
        cp -r "$EXTENSION" "$HOME/.var/app/$app/data/raudo/extension"
        echo "    $app (flatpak) — extension: $HOME/.var/app/$app/data/raudo/extension"
    done
fi

cat <<EOF

Boost instalado. Ábrelo desde el menú de aplicaciones.
Extensión del navegador:
  Firefox:                Boost → Preferencias → Extensiones web → Instalar extensión
  Chrome/Brave/Edge:      chrome://extensions → Modo de desarrollador → Cargar descomprimida →
                          $SHARE/extension
                          (navegadores Flatpak: la carpeta indicada arriba para cada uno)
Los navegadores Snap y Firefox en Flatpak todavía no son compatibles.
EOF
