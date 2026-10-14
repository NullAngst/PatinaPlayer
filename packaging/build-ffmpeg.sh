#!/usr/bin/env bash
# Build a small, static, audio-only FFmpeg for the AppImage.
#
# The AppImage links FFmpeg statically so it doesn't depend on whichever
# FFmpeg the host has, or drag in the hundred-odd video, subtitle and device
# libraries a distro FFmpeg pulls along. This build has every audio decoder,
# every demuxer, parser and protocol, the few filters the player uses, and
# GnuTLS for HTTPS streams. No video decoders, no encoders, no devices.
#
# Usage: packaging/build-ffmpeg.sh [PREFIX]
#   PREFIX defaults to target/ffmpeg-static. Point PKG_CONFIG_PATH at
#   PREFIX/lib/pkgconfig and build with the static-ffmpeg feature to use it.
#
# The FFmpeg branch follows the ffmpeg-sys-next version in Cargo.lock, so a
# `cargo update` that moves the bindings to a new FFmpeg moves this too.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${1:-$ROOT/target/ffmpeg-static}"
SRC="${FFMPEG_SRC:-$ROOT/target/ffmpeg-src}"
JOBS="${JOBS:-$(nproc)}"

# release/MAJOR.MINOR of the bindings, if FFmpeg has that branch; otherwise
# the newest release branch there is.
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

rm -rf "$SRC"
git clone --quiet --depth 1 -b "release/$branch" https://github.com/FFmpeg/FFmpeg "$SRC"
cd "$SRC"

# Every audio decoder FFmpeg has: the native ones listed between "audio
# codecs" and "subtitles" in allcodecs.c (audio, PCM, DPCM and ADPCM).
decoders="$(awk '/\/\* audio codecs \*\//{a=1} /\/\* subtitles \*\//{a=0} a' libavcodec/allcodecs.c |
    sed -nE 's/.*ff_([a-z0-9_]+)_decoder;.*/\1/p' | sort -u | paste -sd, -)"
count="$(tr ',' '\n' <<<"$decoders" | wc -l)"
if [ "$count" -lt 100 ]; then
    # The file changed shape. Better a bigger build than a missing codec.
    echo "warning: found only $count audio decoders, enabling all decoders" >&2
    decoder_args=(--enable-decoders)
else
    echo "Audio decoders: $count"
    decoder_args=(--enable-decoder="$decoders")
fi

./configure \
    --prefix="$PREFIX" \
    --enable-static --disable-shared --enable-pic \
    --disable-programs --disable-doc --disable-debug \
    --disable-autodetect --enable-zlib --enable-gnutls \
    --disable-avdevice --disable-swscale \
    --disable-everything \
    "${decoder_args[@]}" \
    --enable-demuxers --enable-parsers --enable-protocols \
    --enable-filter=abuffer,abuffersink,aformat,aresample,anull,atempo,silenceremove \
    --enable-swresample

make -j"$JOBS"
make install
echo "Static FFmpeg installed in $PREFIX"
