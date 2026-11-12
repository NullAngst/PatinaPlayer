#!/usr/bin/env bash
# Build PatinaPlayer-<version>-x86_64.AppImage from this checkout.
#
# Both apps go in one AppImage: it starts the desktop app, and runs the
# terminal app when started with --tui or through a symlink named
# patinaplayer-tui. FFmpeg is built small and linked in statically (see
# build-ffmpeg.sh). Everything else the binaries link is bundled by
# linuxdeploy, except a few libraries that have to come from the host:
#
#   libpipewire   talks to the PipeWire daemon that's running; a bundled
#                 copy can mismatch it and its plugins
#   GnuTLS        reads the host's certificate store and crypto settings,
#                 and its own dependencies come along with it
#   ALSA, OpenGL, fontconfig, freetype, glibc
#                 on linuxdeploy's standard exclude list for the same reason
#
# Usage: packaging/build-appimage.sh
# Environment:
#   VERSION              version in the file name (default: git describe)
#   UPDATE_INFORMATION   embed AppImage update info and write a .zsync too
#   SKIP_FFMPEG=1        reuse target/ffmpeg-static if it's already built
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${VERSION:-$(git describe --tags --always 2>/dev/null ||
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)}"
VERSION="${VERSION#v}"
FFMPEG="$ROOT/target/ffmpeg-static"
TOOLS="$ROOT/target/appimage-tools"
APPDIR="$ROOT/target/AppDir"
BUILD="$ROOT/target/appimage-build"
OUT="$ROOT/target/appimage"

# 1. FFmpeg.
if [ "${SKIP_FFMPEG:-}" != 1 ] || [ ! -f "$FFMPEG/lib/libavcodec.a" ]; then
    bash "$ROOT/packaging/build-ffmpeg.sh" "$FFMPEG"
fi

# 2. Both binaries, linked against it. A separate target dir, so this
#    doesn't throw away your normal build.
PKG_CONFIG_PATH="$FFMPEG/lib/pkgconfig" cargo build --release \
    --target-dir "$BUILD" \
    -p patina-gui -p patina-tui \
    --features patina-gui/static-ffmpeg,patina-tui/static-ffmpeg
if ldd "$BUILD/release/patinaplayer" | grep -q libavcodec; then
    echo "error: the build linked the system FFmpeg instead of the static one" >&2
    exit 1
fi

# 3. linuxdeploy, newest build, every time.
mkdir -p "$TOOLS"
curl -fsSL -o "$TOOLS/linuxdeploy" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
chmod +x "$TOOLS/linuxdeploy"

# Every size in assets/icons/, so the hicolor theme inside has the simple
# drawing at 16 to 32 px instead of a shrunk copy of the big one.
SIZED_ICONS=()
for f in "$ROOT"/assets/icons/*/patinaplayer.png; do
    [ -e "$f" ] && SIZED_ICONS+=(--icon-file "$f")
done

# 4. AppDir and AppImage. Extract-and-run, so it works without FUSE (CI
#    runners, containers).
rm -rf "$APPDIR"
mkdir -p "$OUT"
# A copy with the execute bit set, in case it got lost on the way here.
install -m 755 "$ROOT/packaging/AppRun" "$TOOLS/AppRun"
export APPIMAGE_EXTRACT_AND_RUN=1
export LINUXDEPLOY_OUTPUT_VERSION="$VERSION"
if [ -n "${UPDATE_INFORMATION:-}" ]; then
    export LDAI_UPDATE_INFORMATION="$UPDATE_INFORMATION"
fi
(
    cd "$OUT"
    rm -f PatinaPlayer-*.AppImage PatinaPlayer-*.AppImage.zsync
    # First the AppDir, without packing it.
    "$TOOLS/linuxdeploy" \
        --appdir "$APPDIR" \
        --executable "$BUILD/release/patinaplayer" \
        --executable "$BUILD/release/patinaplayer-tui" \
        --desktop-file "$ROOT/assets/patinaplayer.desktop" \
        --icon-file "$ROOT/assets/patinaplayer.png" \
        --icon-file "$ROOT/assets/patinaplayer.svg" \
        "${SIZED_ICONS[@]}" \
        --custom-apprun "$TOOLS/AppRun" \
        --exclude-library 'libpipewire-0.3.so*' \
        --exclude-library 'libgnutls.so*' \
        --exclude-library 'libnettle.so*' --exclude-library 'libhogweed.so*' \
        --exclude-library 'libp11-kit.so*' --exclude-library 'libtasn1.so*' \
        --exclude-library 'libidn2.so*' --exclude-library 'libunistring.so*' \
        --exclude-library 'libgmp.so*' --exclude-library 'libffi.so*'

    # linuxdeploy puts the SVG at the top of the AppDir, and appimagetool
    # only makes .DirIcon from a PNG there. Without .DirIcon the AppImage
    # has no icon in file managers, Gear Lever or AppImageLauncher, so the
    # 256 px PNG goes on top instead.
    rm -f "$APPDIR/patinaplayer.svg"
    ln -sf usr/share/icons/hicolor/256x256/apps/patinaplayer.png "$APPDIR/patinaplayer.png"
    ln -sf patinaplayer.png "$APPDIR/.DirIcon"

    # Then pack it.
    "$TOOLS/linuxdeploy" --appdir "$APPDIR" --output appimage
)
# Fail here rather than ship an AppImage without its icon again.
if [ ! -f "$APPDIR/.DirIcon" ]; then
    printf '%s\n' "error: .DirIcon is missing from the AppDir" >&2
    exit 1
fi
ls -l "$OUT"
