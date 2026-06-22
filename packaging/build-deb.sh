#!/usr/bin/env bash
# Build a Debian package (.deb) for Aether from an already-built binary.
#
# Usage: packaging/build-deb.sh <binary-path> <version> <output-dir>
#   binary-path  path to the compiled `aether` executable
#   version      package version without the leading 'v' (e.g. 0.4.9)
#   output-dir   directory to write aether_<version>_amd64.deb into
#
# Produces a single self-contained .deb installable with
#   sudo apt install ./aether_<version>_amd64.deb
# It drops the binary in /usr/bin, a .desktop launcher in the app menu, and the
# scalable SVG icon into the hicolor theme so the launcher shows the logo.
set -euo pipefail

BIN="${1:?binary path required}"
VERSION="${2:?version required}"
OUTDIR="${3:?output dir required}"
ARCH="${ARCH:-amd64}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
chmod 755 "$STAGE"  # mktemp gives 700; package root should be world-readable

# --- file tree -------------------------------------------------------------
install -Dm755 "$BIN"                       "$STAGE/usr/bin/aether"
install -Dm644 "$ROOT/packaging/aether.desktop" \
                                            "$STAGE/usr/share/applications/aether.desktop"
install -Dm644 "$ROOT/logo.svg" \
        "$STAGE/usr/share/icons/hicolor/scalable/apps/aether.svg"

# Installed size in KiB, for the control file.
INSTALLED_SIZE="$(du -ks "$STAGE/usr" | cut -f1)"

# --- copyright (GPL-3.0) ---------------------------------------------------
install -Dm644 /dev/stdin "$STAGE/usr/share/doc/aether/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: aether
Source: https://github.com/actuallyroy/aether-editor

Files: *
Copyright: actuallyroy
License: GPL-3.0-or-later
 This program is free software: you can redistribute it and/or modify it under
 the terms of the GNU General Public License as published by the Free Software
 Foundation, either version 3 of the License, or (at your option) any later
 version. On Debian systems the full text is in /usr/share/common-licenses/GPL-3.
EOF

# --- control ---------------------------------------------------------------
# Runtime shared-library deps mirror the build deps in release.yml: GTK (rfd
# file dialogs), xkbcommon/wayland/X (winit), fontconfig (glyphon), and GL/EGL
# (wgpu's GL fallback). These cover Ubuntu 22.04+/Debian 12+.
install -Dm644 /dev/stdin "$STAGE/DEBIAN/control" <<EOF
Package: aether
Version: $VERSION
Section: editors
Priority: optional
Architecture: $ARCH
Maintainer: actuallyroy <claude01hyd@gmail.com>
Installed-Size: $INSTALLED_SIZE
Depends: libc6, libgtk-3-0, libxkbcommon0, libwayland-client0, libxcb1, libx11-6, libfontconfig1, libgl1
Homepage: https://github.com/actuallyroy/aether-editor
Description: GPU-native, VSCode-compatible code editor
 Aether is a fast, native code editor written in Rust and rendered on the GPU
 with wgpu. It offers a VSCode-familiar workflow — explorer, tabs, command
 palette, search, source control, and an integrated terminal — without the
 Electron overhead, shipping as a single self-contained executable.
EOF

# Refresh the icon cache and desktop database after (un)install so the launcher
# appears without a re-login.
install -Dm755 /dev/stdin "$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "configure" ]; then
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q /usr/share/applications 2>/dev/null || true
    fi
fi
EOF
install -Dm755 /dev/stdin "$STAGE/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "remove" ] || [ "$1" = "purge" ]; then
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q /usr/share/applications 2>/dev/null || true
    fi
fi
EOF

# --- build -----------------------------------------------------------------
mkdir -p "$OUTDIR"
DEB="$OUTDIR/aether_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$STAGE" "$DEB"
echo "Built $DEB"
