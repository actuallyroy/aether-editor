#!/bin/sh
# Aether installer for Linux.
#
#   curl -fsSL https://actuallyroy.github.io/aether-editor/install.sh | sh
#
# Downloads the latest release binary, installs it to ~/.local/bin (or
# /usr/local/bin when run as root), and registers a desktop launcher with the
# logo so Aether shows up in your application menu. No package manager needed.
#
# Env overrides:
#   AETHER_INSTALL_DIR   target dir for the binary (default: see above)
#   AETHER_VERSION       release tag to install (default: latest)
set -eu

REPO="actuallyroy/aether-editor"
ASSET="aether-linux-x86_64"

# --- arch check ------------------------------------------------------------
arch="$(uname -m)"
case "$arch" in
    x86_64|amd64) : ;;
    *)
        echo "Error: Aether's Linux release is x86_64 only; detected '$arch'." >&2
        echo "Build from source instead: https://github.com/$REPO" >&2
        exit 1
        ;;
esac

# --- pick install location -------------------------------------------------
if [ -n "${AETHER_INSTALL_DIR:-}" ]; then
    bindir="$AETHER_INSTALL_DIR"
elif [ "$(id -u)" -eq 0 ]; then
    bindir="/usr/local/bin"
else
    bindir="$HOME/.local/bin"
fi

datahome="${XDG_DATA_HOME:-$HOME/.local/share}"
if [ "$(id -u)" -eq 0 ]; then datahome="/usr/local/share"; fi
appsdir="$datahome/applications"
icondir="$datahome/icons/hicolor/scalable/apps"

# --- resolve version + URLs ------------------------------------------------
if [ -n "${AETHER_VERSION:-}" ]; then
    base="https://github.com/$REPO/releases/download/$AETHER_VERSION"
else
    base="https://github.com/$REPO/releases/latest/download"
fi
bin_url="$base/$ASSET"
icon_url="https://raw.githubusercontent.com/$REPO/main/logo.svg"

echo "Installing Aether to $bindir/aether ..."
mkdir -p "$bindir" "$appsdir" "$icondir"

# --- download binary -------------------------------------------------------
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
if command -v curl >/dev/null 2>&1; then
    curl -fSL --progress-bar "$bin_url" -o "$tmp"
elif command -v wget >/dev/null 2>&1; then
    wget -q --show-progress "$bin_url" -O "$tmp"
else
    echo "Error: need curl or wget to download." >&2
    exit 1
fi
chmod +x "$tmp"
mv "$tmp" "$bindir/aether"
trap - EXIT

# --- desktop integration ---------------------------------------------------
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$icon_url" -o "$icondir/aether.svg" 2>/dev/null || true
else
    wget -q "$icon_url" -O "$icondir/aether.svg" 2>/dev/null || true
fi

cat > "$appsdir/aether.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Aether
GenericName=Code Editor
Comment=A GPU-native, VSCode-compatible code editor
Exec=$bindir/aether %F
Icon=aether
Terminal=false
Categories=Development;TextEditor;IDE;
MimeType=text/plain;inode/directory;
StartupNotify=true
StartupWMClass=aether
Keywords=editor;code;text;development;programming;
EOF

command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database -q "$appsdir" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -q "$datahome/icons/hicolor" 2>/dev/null || true

echo ""
echo "✓ Aether installed to $bindir/aether"
case ":$PATH:" in
    *":$bindir:"*) echo "  Run it with: aether" ;;
    *)
        echo "  Note: $bindir is not on your PATH. Add it with:"
        echo "    echo 'export PATH=\"$bindir:\$PATH\"' >> ~/.profile && . ~/.profile"
        echo "  Or run directly: $bindir/aether"
        ;;
esac
echo "  It should also appear in your application menu as \"Aether\"."
