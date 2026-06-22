#!/usr/bin/env bash
# Build a portable AppImage for Aether from an already-built binary.
#
# Usage: packaging/build-appimage.sh <binary-path> <version> <output-dir>
#
# The AppImage is a single executable that runs on most Linux distros with no
# install step: `chmod +x Aether-x86_64.AppImage && ./Aether-x86_64.AppImage`.
set -euo pipefail

BIN="${1:?binary path required}"
VERSION="${2:?version required}"
OUTDIR="${3:?output dir required}"
ARCH="${ARCH:-x86_64}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APPDIR="$WORK/Aether.AppDir"

# --- AppDir tree -----------------------------------------------------------
install -Dm755 "$BIN"                           "$APPDIR/usr/bin/aether"
install -Dm644 "$ROOT/packaging/aether.desktop" "$APPDIR/usr/share/applications/aether.desktop"
install -Dm644 "$ROOT/logo.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/aether.svg"

# appimagetool looks for the desktop file and icon at the AppDir root.
cp "$APPDIR/usr/share/applications/aether.desktop" "$APPDIR/aether.desktop"
cp "$ROOT/logo.svg" "$APPDIR/aether.svg"
ln -s aether.svg "$APPDIR/.DirIcon"

# AppRun: resolve our own location, then exec the bundled binary.
install -Dm755 /dev/stdin "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/aether" "$@"
EOF

# --- appimagetool ----------------------------------------------------------
TOOL="$WORK/appimagetool"
TOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage"
curl -fsSL "$TOOL_URL" -o "$TOOL"
chmod +x "$TOOL"

mkdir -p "$OUTDIR"
OUT="$OUTDIR/Aether-${ARCH}.AppImage"
# --appimage-extract-and-run avoids needing FUSE inside CI containers.
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT"
echo "Built $OUT"
