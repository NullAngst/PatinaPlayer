#!/usr/bin/env bash
# Patch Slint's Android backend for mice, for the APK build.
#
# Slint's Android backend (the i-slint-backend-android-activity crate)
# panics on a mouse wheel: scroll events land on a todo!(). It also takes
# a mouse click as a finger touching down on top of the button press, so a
# right click pressed the left button as well. slint-android-mouse.patch
# fixes both.
#
# This fetches the backend's source at the version Cargo.lock has, checks
# it against the lockfile's checksum, applies the patch, and prints the
# folder. build-apk.sh hands that folder to cargo as a [patch], so the
# patch follows whatever Slint version `cargo update` picked.
#
# Usage: packaging/patch-slint-android.sh OUTDIR
#
# Prints nothing when the patch no longer applies and the todo!() is gone,
# since then Slint has changed that code itself: the build goes on with
# Slint as it is, and a warning says to check a mouse on a phone. Fails
# when the patch doesn't apply and the todo!() is still there. See
# "Slint's Android mouse patch" in maintenance.md for both cases.
set -euo pipefail
export LC_ALL=C

CRATE=i-slint-backend-android-activity
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PATCH_FILE="$ROOT/packaging/slint-android-mouse.patch"
LOCK="$ROOT/Cargo.lock"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

say() {
    printf '%s\n' "$*" >&2
}

[ "$#" -eq 1 ] || die "usage: $0 OUTDIR"
OUT=$1

missing=""
for c in curl tar patch awk grep mkdir rm; do
    command -v "$c" >/dev/null 2>&1 || missing="$missing $c"
done
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    missing="$missing sha256sum"
fi
[ -z "$missing" ] || die "missing commands:$missing"

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- "$1" | awk '{print $1}'
    else
        shasum -a 256 -- "$1" | awk '{print $1}'
    fi
}

[ -f "$LOCK" ] || die "no Cargo.lock. Run cargo generate-lockfile (or cargo update) first."
[ -f "$PATCH_FILE" ] || die "no $PATCH_FILE"

# The backend's version and checksum, from its [[package]] entry.
entry="$(awk -v want="$CRATE" '
    /^\[\[package\]\]/ { name = ""; ver = ""; sum = ""; next }
    /^name = /     { name = $3; gsub(/"/, "", name) }
    /^version = /  { ver = $3; gsub(/"/, "", ver) }
    /^checksum = / { sum = $3; gsub(/"/, "", sum) }
    name == want && ver != "" && sum != "" { print ver, sum; exit }
' "$LOCK")"
[ -n "$entry" ] || die "Cargo.lock has no $CRATE from crates.io. Run cargo update first."
version=${entry% *}
checksum=${entry#* }

dir="$OUT/$CRATE-$version"
# Patched already, on an earlier run.
if [ -f "$dir/.patina-patched" ]; then
    printf '%s\n' "$dir"
    exit 0
fi

tmp="$(mktemp "${TMPDIR:-/tmp}/slint-android.XXXXXX")"
trap 'rm -f -- "$tmp"' EXIT

say "Fetching $CRATE $version to patch it for mice"
curl -fsSL --retry 3 -o "$tmp" "https://static.crates.io/crates/$CRATE/$CRATE-$version.crate" ||
    die "couldn't download $CRATE $version from crates.io"
got="$(sha256 "$tmp")"
[ "$got" = "$checksum" ] || die "$CRATE $version doesn't match Cargo.lock's checksum (got $got)"

rm -rf -- "$dir"
mkdir -p -- "$OUT"
tar -xzf "$tmp" -C "$OUT"
[ -f "$dir/androidwindowadapter.rs" ] || die "$CRATE $version has no androidwindowadapter.rs; its layout changed"

todo=0
if grep -q 'MotionAction::Scroll => todo!()' "$dir/androidwindowadapter.rs"; then
    todo=1
fi

if patch -p1 -N -d "$dir" <"$PATCH_FILE" >&2; then
    : >"$dir/.patina-patched"
    say "Patched $CRATE $version for mice"
    printf '%s\n' "$dir"
    exit 0
fi

rm -rf -- "$dir"
if [ "$todo" -eq 1 ]; then
    die "slint-android-mouse.patch no longer applies to $CRATE $version, and a mouse wheel still crashes it. Redo the patch against this version (see maintenance.md)."
fi
say "warning: slint-android-mouse.patch doesn't apply to $CRATE $version, and Slint handles scroll events now. Building with Slint as it is. Check a mouse's wheel and right click on a phone; if both work, delete the patch (see maintenance.md)."
