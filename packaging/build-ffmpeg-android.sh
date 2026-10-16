#!/usr/bin/env bash
# Build the static, audio-only FFmpeg for the Android app, with Mbed TLS
# for HTTPS streams.
#
# Same FFmpeg as the AppImage's (packaging/build-ffmpeg.sh): every audio
# decoder, every demuxer, parser and protocol, the few filters the player
# uses, no video and no encoders. The differences: it's cross-compiled
# with the NDK for arm64, and TLS is Mbed TLS instead of GnuTLS, since
# Mbed TLS builds for Android with nothing but CMake.
#
# Usage: packaging/build-ffmpeg-android.sh [PREFIX]
#   PREFIX defaults to target/ffmpeg-android. FFmpeg and Mbed TLS both
#   install there.
#
# Needs: the Android NDK (ANDROID_NDK_ROOT, or ANDROID_NDK_LATEST_HOME as
# on GitHub's runners), cmake, make, git and curl.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${1:-$ROOT/target/ffmpeg-android}"
WORK="${FFMPEG_ANDROID_WORK:-$ROOT/target/ffmpeg-android-src}"
JOBS="${JOBS:-$(nproc)}"
API="${ANDROID_API:-28}"
NDK="${ANDROID_NDK_ROOT:-${ANDROID_NDK_LATEST_HOME:-${ANDROID_NDK_HOME:-}}}"

if [ -z "$NDK" ] || [ ! -d "$NDK/toolchains/llvm/prebuilt" ]; then
    echo "error: no NDK found. Set ANDROID_NDK_ROOT to the NDK's folder." >&2
    exit 1
fi
TC="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
CC="$TC/bin/aarch64-linux-android$API-clang"
CXX="$TC/bin/aarch64-linux-android$API-clang++"
if [ ! -x "$CC" ]; then
    echo "error: $CC is missing. Is ANDROID_API ($API) supported by this NDK?" >&2
    exit 1
fi

rm -rf "$WORK" "$PREFIX"
mkdir -p "$WORK" "$PREFIX"

# ----- Mbed TLS -----

# The newest 3.x release. 4.x dropped the API FFmpeg uses.
tag="$(git ls-remote --tags --refs https://github.com/Mbed-TLS/mbedtls 'mbedtls-3.*' |
    sed 's#.*refs/tags/##' | grep -E '^mbedtls-3\.[0-9]+\.[0-9]+$' | sort -V | tail -n1)"
echo "Mbed TLS: $tag -> $PREFIX"
# The release tarball has the generated files a plain clone lacks.
curl -fsSL "https://github.com/Mbed-TLS/mbedtls/releases/download/$tag/$tag.tar.bz2" |
    tar xj -C "$WORK"
cmake -S "$WORK/$tag" -B "$WORK/mbedtls-build" \
    -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
    -DANDROID_ABI=arm64-v8a \
    -DANDROID_PLATFORM="android-$API" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DUSE_SHARED_MBEDTLS_LIBRARY=OFF \
    -DUSE_STATIC_MBEDTLS_LIBRARY=ON \
    -DENABLE_PROGRAMS=OFF \
    -DENABLE_TESTING=OFF
cmake --build "$WORK/mbedtls-build" -j"$JOBS"
cmake --install "$WORK/mbedtls-build"

# ----- FFmpeg -----

# release/MAJOR.MINOR of the bindings, as for the AppImage.
branches="$(git ls-remote --heads https://github.com/FFmpeg/FFmpeg 'release/*' |
    sed 's#.*refs/heads/release/##' | sort -V)"
want="$(awk '/^name = "ffmpeg-sys-next"/ { getline; gsub(/[^0-9.]/, ""); print; exit }' \
    "$ROOT/Cargo.lock" 2>/dev/null | cut -d. -f1,2)"
if [ -n "$want" ] && grep -qx "$want" <<<"$branches"; then
    branch="$want"
else
    branch="$(tail -n1 <<<"$branches")"
fi
echo "FFmpeg: release/$branch -> $PREFIX"

git clone --quiet --depth 1 -b "release/$branch" https://github.com/FFmpeg/FFmpeg "$WORK/ffmpeg"
cd "$WORK/ffmpeg"

decoders="$(awk '/\/\* audio codecs \*\//{a=1} /\/\* subtitles \*\//{a=0} a' libavcodec/allcodecs.c |
    sed -nE 's/.*ff_([a-z0-9_]+)_decoder;.*/\1/p' | sort -u | paste -sd, -)"
count="$(tr ',' '\n' <<<"$decoders" | wc -l)"
if [ "$count" -lt 100 ]; then
    echo "warning: found only $count audio decoders, enabling all decoders" >&2
    decoder_args=(--enable-decoders)
else
    echo "Audio decoders: $count"
    decoder_args=(--enable-decoder="$decoders")
fi

# pkg-config is pointed at the prefix only, so configure finds this Mbed
# TLS and nothing from the build machine.
PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig" PKG_CONFIG_PATH="" \
./configure \
    --prefix="$PREFIX" \
    --target-os=android --arch=aarch64 --enable-cross-compile \
    --cc="$CC" --cxx="$CXX" \
    --ar="$TC/bin/llvm-ar" --nm="$TC/bin/llvm-nm" \
    --ranlib="$TC/bin/llvm-ranlib" --strip="$TC/bin/llvm-strip" \
    --pkg-config=pkg-config \
    --extra-cflags="-I$PREFIX/include -fPIC" \
    --extra-ldflags="-L$PREFIX/lib" \
    --enable-static --disable-shared --enable-pic \
    --disable-programs --disable-doc --disable-debug \
    --disable-autodetect --enable-zlib \
    --enable-version3 --enable-mbedtls \
    --disable-avdevice --disable-swscale \
    --disable-everything \
    "${decoder_args[@]}" \
    --enable-demuxers --enable-parsers --enable-protocols \
    --enable-filter=abuffer,abuffersink,aformat,aresample,anull,atempo,silenceremove \
    --enable-swresample

make -j"$JOBS"
make install
echo "Static FFmpeg for Android installed in $PREFIX"
