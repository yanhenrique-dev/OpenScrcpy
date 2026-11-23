#!/usr/bin/env bash
# Builds a Linux AppImage for the native GPUI shell.
#
# Phase 5 spike: this is the packaging path we intend to replace with a
# release-grade script (cargo-packager or a dedicated CI job). It bundles the
# shared libraries linuxdeploy finds, writes the desktop entry and the AppRun
# wrapper, then produces the type-2 AppImage.
#
# Usage:
#   scripts/package-gpui-appimage.sh [path/to/openscrcpy]
#
# Environment:
#   APPIMAGE_TOOL     linuxdeploy-plugin-appimage AppImage (appimagetool host)
#   LINUXDEPLOY       linuxdeploy AppImage
#   GPUI_BIN          release binary (defaults to the crate target dir)
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
crate="$root/crates/open-scrcpy-gpui"
out_dir="$root/artifacts/gpui"
app_dir="$out_dir/OpenScrcpy.AppDir"
output="${out_dir}/OpenScrcpy-GPUI-x86_64.AppImage"

bin="${1:-${GPUI_BIN:-$crate/target/release/openscrcpy}}"
if [[ ! -x "$bin" ]]; then
    echo "package-gpui: release binary not found at $bin" >&2
    echo "package-gpui: build it with 'cargo build --release' in $crate" >&2
    exit 1
fi

appimage_tool="${APPIMAGE_TOOL:-$HOME/.cache/tauri/linuxdeploy-plugin-appimage.AppImage}"
linuxdeploy="${LINUXDEPLOY:-$HOME/.cache/tauri/linuxdeploy-x86_64.AppImage}"
icon="$root/src-tauri/icons/128x128.png"
for required in "$appimage_tool" "$linuxdeploy" "$icon"; do
    if [[ ! -e "$required" ]]; then
        echo "package-gpui: missing $required" >&2
        exit 1
    fi
done

rm -rf "$app_dir"
mkdir -p \
    "$app_dir/usr/bin" \
    "$app_dir/usr/share/applications" \
    "$app_dir/usr/share/icons/hicolor/128x128/apps"

install -m 755 "$bin" "$app_dir/usr/bin/openscrcpy"
install -m 644 "$icon" "$app_dir/usr/share/icons/hicolor/128x128/apps/openscrcpy.png"

cat > "$app_dir/usr/share/applications/openscrcpy.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=OpenScrcpy
Comment=Control your Android device from the desktop
Exec=openscrcpy
Icon=openscrcpy
Terminal=false
Categories=Utility;
StartupWMClass=openscrcpy
DESKTOP

cat > "$app_dir/AppRun" <<'APPRUN'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$HERE/usr/bin/openscrcpy" "$@"
APPRUN
chmod 755 "$app_dir/AppRun"

ln -sf usr/share/applications/openscrcpy.desktop "$app_dir/OpenScrcpy.desktop"
ln -sf usr/share/icons/hicolor/128x128/apps/openscrcpy.png "$app_dir/openscrcpy.png"
ln -sf openscrcpy.png "$app_dir/.DirIcon"

ARCH=x86_64 NO_STRIP=1 APPIMAGE_EXTRACT_AND_RUN=1 \
    "$linuxdeploy" \
    --appdir "$app_dir" \
    --executable "$app_dir/usr/bin/openscrcpy" \
    --desktop-file "$app_dir/usr/share/applications/openscrcpy.desktop" \
    --icon-file "$app_dir/usr/share/icons/hicolor/128x128/apps/openscrcpy.png"

ARCH=x86_64 NO_STRIP=1 \
    LDAI_OUTPUT="$output" \
    "$appimage_tool" --appimage-extract-and-run --appdir="$app_dir"

echo "package-gpui: $output"
