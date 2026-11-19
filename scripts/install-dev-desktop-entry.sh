#!/usr/bin/env bash
# Install a desktop entry for the development build so GNOME (Wayland) shows the fo-foca icon
# and name in the dock / alt-tab. Release bundles (AppImage, .deb) ship their own entry.
# Remove with: rm ~/.local/share/applications/io.github.moacyrricardo.fofoca.desktop
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$dir"
cat > "$dir/io.github.moacyrricardo.fofoca.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=fo-foca (dev)
Comment=Local AI meeting notes
Exec=$repo/target/debug/fofoca
Icon=$repo/app/src-tauri/icons/icon.png
StartupWMClass=fofoca
Categories=Office;AudioVideo;
Terminal=false
DESKTOP
update-desktop-database "$dir" 2>/dev/null || true
echo "installed $dir/io.github.moacyrricardo.fofoca.desktop"
