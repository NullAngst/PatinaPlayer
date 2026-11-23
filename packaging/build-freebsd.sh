#!/usr/bin/env bash
# Build PatinaPlayer-<version>-freebsd-<arch>.tar.gz on FreeBSD.
#
# Both apps, the desktop one and the terminal one, with FFmpeg linked in
# statically (see build-ffmpeg.sh), so the tarball doesn't depend on which
# FFmpeg the system has. The rest comes from packages at run time:
# PipeWire, alsa-lib, GnuTLS, fontconfig, freetype, libxkbcommon, Wayland
# or X11, Mesa. README-FreeBSD.txt in the tarball lists them.
#
# pipewire-sys gets patched for FreeBSD's renamed pw_init on the way: see
# patch-pipewire-freebsd.sh.
#
# Needs (pkg install): rust git gmake pkgconf nasm gnutls llvm pipewire
#   alsa-lib libxkbcommon wayland fontconfig freetype2 libX11 libXcursor
#   libXrandr libXi mesa-libs curl bash
#
# Usage: packaging/build-freebsd.sh
# Environment:
#   VERSION         version in the file name (default: git describe)
#   SKIP_FFMPEG=1   reuse target/ffmpeg-static if it's already built
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 1

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

[ "$(uname -s)" = FreeBSD ] || die "this builds the FreeBSD tarball, on FreeBSD. Use build-appimage.sh on Linux."

missing=""
for c in cargo git gmake pkg-config tar ldd uname sysctl; do
    command -v "$c" >/dev/null 2>&1 || missing="$missing $c"
done
[ -z "$missing" ] || die "missing commands:$missing (pkg install rust git gmake pkgconf)"

VERSION="${VERSION:-$(git describe --tags --always 2>/dev/null ||
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)}"
VERSION="${VERSION#v}"
ARCH="$(uname -m)"
FFMPEG="$ROOT/target/ffmpeg-static"
BUILD="$ROOT/target/freebsd-build"
NAME="PatinaPlayer-$VERSION-freebsd-$ARCH"
STAGE="$ROOT/target/freebsd/$NAME"
OUT="$ROOT/target/freebsd"

# 1. FFmpeg. FreeBSD's make isn't GNU make, which FFmpeg needs.
if [ "${SKIP_FFMPEG:-}" != 1 ] || [ ! -f "$FFMPEG/lib/libavcodec.a" ]; then
    MAKE=gmake JOBS="${JOBS:-$(sysctl -n hw.ncpu)}" bash "$ROOT/packaging/build-ffmpeg.sh" "$FFMPEG"
fi

# 2. pipewire-sys, patched, as a [patch] for this build only. While it
#    builds, Cargo.lock lists the patched copy by its path, so the lockfile
#    is put back afterwards.
PW_DIR="$(bash "$ROOT/packaging/patch-pipewire-freebsd.sh" "$ROOT/target/pipewire-freebsd")"
CARGO_PATCH=()
if [ -n "$PW_DIR" ]; then
    CARGO_PATCH=(--config "patch.crates-io.pipewire-sys.path=\"$PW_DIR\"")
fi
LOCK_SAVED="$ROOT/target/Cargo.lock.before-freebsd"
mkdir -p "$ROOT/target"
cp -- "$ROOT/Cargo.lock" "$LOCK_SAVED"
trap 'cp -- "$LOCK_SAVED" "$ROOT/Cargo.lock"' EXIT

# 3. Both binaries. A separate target dir, so this doesn't throw away
#    your normal build.
PKG_CONFIG_PATH="$FFMPEG/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" cargo build --release \
    --target-dir "$BUILD" \
    -p patina-gui -p patina-tui \
    --features patina-gui/static-ffmpeg,patina-tui/static-ffmpeg \
    "${CARGO_PATCH[@]}"
if ldd "$BUILD/release/patinaplayer" | grep -q libavcodec; then
    die "the build linked the system FFmpeg instead of the static one"
fi

# 4. The tarball: bin/, the desktop entry and the icons, laid out like
#    /usr/local so it can be copied there as it is.
rm -rf -- "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/applications"
cp -- "$BUILD/release/patinaplayer" "$BUILD/release/patinaplayer-tui" "$STAGE/bin/"
cp -- "$ROOT/assets/patinaplayer.desktop" "$STAGE/share/applications/"
for f in "$ROOT"/assets/icons/*/patinaplayer.png; do
    [ -e "$f" ] || continue
    size="$(basename -- "$(dirname -- "$f")")"
    mkdir -p "$STAGE/share/icons/hicolor/$size/apps"
    cp -- "$f" "$STAGE/share/icons/hicolor/$size/apps/"
done
mkdir -p "$STAGE/share/icons/hicolor/scalable/apps"
cp -- "$ROOT/assets/patinaplayer.svg" "$STAGE/share/icons/hicolor/scalable/apps/"
cp -- "$ROOT/LICENSE" "$STAGE/"
cat >"$STAGE/README-FreeBSD.txt" <<TXT
PatinaPlayer $VERSION for FreeBSD ($ARCH)

What it needs, from packages:

  pkg install pipewire alsa-lib gnutls fontconfig freetype2 libxkbcommon \\
      wayland libX11 libXcursor libXrandr libXi mesa-libs

Install, for everyone:

  cp -R bin share /usr/local/

or just for you (with ~/.local/bin on your PATH):

  mkdir -p ~/.local && cp -R bin share ~/.local/

Then run patinaplayer (the desktop app) or patinaplayer-tui (the
terminal app). Sound goes through PipeWire when it's running, and through
alsa-lib otherwise; alsa-lib on FreeBSD plays to OSS (/dev/dsp).
TXT

mkdir -p "$OUT"
tar -czf "$OUT/$NAME.tar.gz" -C "$OUT" "$NAME"
printf 'Built %s\n' "$OUT/$NAME.tar.gz"
