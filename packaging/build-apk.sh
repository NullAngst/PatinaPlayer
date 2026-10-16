#!/usr/bin/env bash
# Build the Android app: PatinaPlayer-<version>-arm64.apk in target/apk/.
#
# No Gradle. The pieces are few enough to put together by hand: the Rust
# library (the whole app) built with the NDK, the four Java classes in
# android/java compiled and dexed, the manifest and icons linked by aapt2,
# then zipalign and apksigner.
#
# Usage: packaging/build-apk.sh
#
# Needs: the Android SDK with build-tools and a platform (ANDROID_HOME),
# the NDK (ANDROID_NDK_ROOT, or ANDROID_NDK_LATEST_HOME as on GitHub's
# runners), a JDK 17 or newer, rustup's aarch64-linux-android target, and
# whatever build-ffmpeg-android.sh needs.
#
# Environment:
#   VERSION              version name, like v0.3.0 or 0.3.0 (default: the
#                        workspace version from Cargo.toml)
#   SKIP_FFMPEG=1        reuse target/ffmpeg-android if it's already built
#   ANDROID_API          the lowest Android the app runs on, as an API
#                        level (default 28, Android 9)
#   Signing, all four or none:
#   ANDROID_KEYSTORE           path to the release keystore
#   ANDROID_KEYSTORE_PASSWORD  the keystore's password
#   ANDROID_KEY_ALIAS          the key's alias in it
#   ANDROID_KEY_PASSWORD       the key's password (often the same)
#   Without them the APK is signed with a debug key kept in
#   target/android-debug.keystore. It installs fine, but an APK signed
#   with a different key can't update it later.
#   REQUIRE_RELEASE_KEY=1  fail instead of falling back to the debug key
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
API="${ANDROID_API:-28}"
TARGET=aarch64-linux-android
OUT="$ROOT/target/apk"
WORK="$ROOT/target/apk-build"
FFMPEG="$ROOT/target/ffmpeg-android"

die() { echo "error: $*" >&2; exit 1; }

# ----- Find the SDK and NDK -----

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$SDK" ] && [ -d "$SDK" ] || die "no Android SDK. Set ANDROID_HOME to the SDK's folder."
NDK="${ANDROID_NDK_ROOT:-${ANDROID_NDK_LATEST_HOME:-${ANDROID_NDK_HOME:-}}}"
if [ -z "$NDK" ] && [ -d "$SDK/ndk" ]; then
    NDK="$(ls -d "$SDK"/ndk/*/ 2>/dev/null | sort -V | tail -n1)"
fi
[ -n "$NDK" ] && [ -d "$NDK/toolchains/llvm/prebuilt" ] ||
    die "no NDK. Set ANDROID_NDK_ROOT, or install one with sdkmanager \"ndk;<version>\"."
NDK="${NDK%/}"
export ANDROID_NDK_ROOT="$NDK"

# The newest build-tools and platform installed: no versions to bump.
BT="$(ls -d "$SDK"/build-tools/*/ 2>/dev/null | sort -V | tail -n1)"
BT="${BT%/}"
[ -x "$BT/aapt2" ] || die "no build-tools in $SDK/build-tools. Install them with sdkmanager."
PLATFORM="$(ls -d "$SDK"/platforms/android-*/ 2>/dev/null |
    grep -E '/android-[0-9]+/$' | sort -V | tail -n1)"
PLATFORM="${PLATFORM%/}"
[ -f "$PLATFORM/android.jar" ] || die "no platform in $SDK/platforms. Install one with sdkmanager."
ANDROID_JAR="$PLATFORM/android.jar"
TARGET_SDK="${PLATFORM##*android-}"
[ "$TARGET_SDK" -ge 34 ] || die "platform android-$TARGET_SDK is too old; the app needs 34 or newer to build."

TC="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
CLANG="$TC/bin/$TARGET$API-clang"
[ -x "$CLANG" ] || die "$CLANG is missing. Is ANDROID_API ($API) supported by this NDK?"

echo "SDK:         $SDK"
echo "NDK:         $NDK"
echo "Build tools: ${BT##*/}"
echo "Platform:    android-$TARGET_SDK (minimum android-$API)"

# ----- Version -----

VERSION="${VERSION:-$(awk -F'"' '/^\[workspace.package\]/{w=1} w && /^version/{print $2; exit}' "$ROOT/Cargo.toml")}"
VERSION="${VERSION#v}"
# Version code: 1.2.3 -> 1002003. Android only installs a higher one over
# a lower one, and this keeps them in step with the tags.
IFS=. read -r vmaj vmin vpat _ <<<"${VERSION%%-*}.0.0.0"
vmaj="${vmaj//[^0-9]/}"; vmin="${vmin//[^0-9]/}"; vpat="${vpat//[^0-9]/}"
VERSION_CODE=$(( ${vmaj:-0} * 1000000 + ${vmin:-0} * 1000 + ${vpat:-0} ))
[ "$VERSION_CODE" -gt 0 ] || VERSION_CODE=1
echo "Version:     $VERSION (code $VERSION_CODE)"

# ----- FFmpeg -----

if [ "${SKIP_FFMPEG:-}" != 1 ] || [ ! -f "$FFMPEG/lib/libavcodec.a" ]; then
    bash "$ROOT/packaging/build-ffmpeg-android.sh" "$FFMPEG"
fi

# ----- The Rust library -----

export "CC_${TARGET//-/_}=$CLANG"
export "CXX_${TARGET//-/_}=$CLANG++"
export "AR_${TARGET//-/_}=$TC/bin/llvm-ar"
export "RANLIB_${TARGET//-/_}=$TC/bin/llvm-ranlib"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CLANG"
# 16 KB pages: Android 15 devices can use them, and Play asks for it.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"
# bindgen (FFmpeg's bindings) parses the headers as Android's.
export "BINDGEN_EXTRA_CLANG_ARGS_${TARGET//-/_}=--sysroot=$TC/sysroot --target=$TARGET$API"
export CARGO_NDK_SYSROOT_PATH="$TC/sysroot"
# pkg-config sees only the Android FFmpeg, and only for the Android build
# (build scripts for the build machine keep the system's).
export PKG_CONFIG_ALLOW_CROSS=1
export "PKG_CONFIG_PATH_${TARGET//-/_}=$FFMPEG/lib/pkgconfig"
export "PKG_CONFIG_LIBDIR_${TARGET//-/_}=$FFMPEG/lib/pkgconfig"

cargo build --release --target "$TARGET" -p patina-android --features static-ffmpeg
SO="$ROOT/target/$TARGET/release/libpatina_android.so"
[ -f "$SO" ] || die "the build made no $SO"

# ----- Stage the APK's files -----

rm -rf "$WORK"
mkdir -p "$WORK/lib/arm64-v8a" "$WORK/gen" "$WORK/classes" "$WORK/dex" "$OUT"
"$TC/bin/llvm-strip" --strip-unneeded -o "$WORK/lib/arm64-v8a/libpatina_android.so" "$SO"

# Libraries the NDK ships that the app needs at run time (the C++
# runtime, if anything links it shared). System libraries aren't copied.
NDK_LIBS="$TC/sysroot/usr/lib/$TARGET"
for lib in $("$TC/bin/llvm-readelf" -d "$SO" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p'); do
    if [ -f "$NDK_LIBS/$lib" ]; then
        echo "Bundling $lib"
        cp "$NDK_LIBS/$lib" "$WORK/lib/arm64-v8a/"
    fi
done

# Resources and manifest.
"$BT/aapt2" compile --dir "$ROOT/android/res" -o "$WORK/res.zip"
"$BT/aapt2" link -o "$WORK/base.apk" \
    -I "$ANDROID_JAR" \
    --manifest "$ROOT/android/AndroidManifest.xml" \
    --min-sdk-version "$API" \
    --target-sdk-version "$TARGET_SDK" \
    --version-code "$VERSION_CODE" \
    --version-name "$VERSION" \
    --java "$WORK/gen" \
    "$WORK/res.zip"

# Java, then dex.
find "$ROOT/android/java" "$WORK/gen" -name '*.java' > "$WORK/sources.txt"
javac --release 11 -nowarn -encoding UTF-8 \
    -cp "$ANDROID_JAR" -d "$WORK/classes" @"$WORK/sources.txt"
D8="$BT/d8"
[ -x "$D8" ] || die "no d8 in $BT"
"$D8" --release --min-api "$API" --lib "$ANDROID_JAR" --output "$WORK/dex" \
    $(find "$WORK/classes" -name '*.class')

# Put it together.
cp "$WORK/base.apk" "$WORK/unaligned.apk"
(cd "$WORK/dex" && zip -q -X "$WORK/unaligned.apk" classes*.dex)
(cd "$WORK" && zip -q -X -r "$WORK/unaligned.apk" lib)

# 16 KB alignment for the native library where zipalign knows how.
if "$BT/zipalign" -P 16 -f 4 "$WORK/unaligned.apk" "$WORK/aligned.apk" 2>/dev/null; then
    :
else
    "$BT/zipalign" -p -f 4 "$WORK/unaligned.apk" "$WORK/aligned.apk"
fi

# ----- Sign -----

APK="$OUT/PatinaPlayer-$VERSION-arm64.apk"
rm -f "$APK"
if [ -n "${ANDROID_KEYSTORE:-}" ]; then
    [ -f "$ANDROID_KEYSTORE" ] || die "ANDROID_KEYSTORE points at $ANDROID_KEYSTORE, which isn't there"
    [ -n "${ANDROID_KEYSTORE_PASSWORD:-}" ] && [ -n "${ANDROID_KEY_ALIAS:-}" ] ||
        die "ANDROID_KEYSTORE is set but ANDROID_KEYSTORE_PASSWORD or ANDROID_KEY_ALIAS isn't"
    export ANDROID_KEY_PASSWORD="${ANDROID_KEY_PASSWORD:-$ANDROID_KEYSTORE_PASSWORD}"
    echo "Signing with the release key ($ANDROID_KEY_ALIAS)"
    "$BT/apksigner" sign \
        --ks "$ANDROID_KEYSTORE" \
        --ks-key-alias "$ANDROID_KEY_ALIAS" \
        --ks-pass env:ANDROID_KEYSTORE_PASSWORD \
        --key-pass env:ANDROID_KEY_PASSWORD \
        --out "$APK" "$WORK/aligned.apk"
else
    [ "${REQUIRE_RELEASE_KEY:-}" != 1 ] ||
        die "no release key. Set ANDROID_KEYSTORE and friends (see the signing guide)."
    DEBUG_KS="$ROOT/target/android-debug.keystore"
    if [ ! -f "$DEBUG_KS" ]; then
        keytool -genkeypair -keystore "$DEBUG_KS" -storepass android -keypass android \
            -alias debug -keyalg RSA -keysize 4096 -validity 10000 \
            -dname "CN=PatinaPlayer debug" >/dev/null 2>&1
    fi
    echo "warning: no release key set, signing with the debug key in $DEBUG_KS" >&2
    "$BT/apksigner" sign \
        --ks "$DEBUG_KS" --ks-key-alias debug \
        --ks-pass pass:android --key-pass pass:android \
        --out "$APK" "$WORK/aligned.apk"
fi
rm -f "$APK.idsig"
# The fingerprint goes in the log, to check against your key's.
"$BT/apksigner" verify --print-certs "$APK" > "$WORK/certs.txt" || die "the signed APK doesn't verify"
grep -iE "certificate DN|SHA-256" "$WORK/certs.txt" || true

echo
echo "Built $APK ($(du -h "$APK" | cut -f1))"
