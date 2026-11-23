#!/usr/bin/env bash
# Patch pipewire-sys for FreeBSD, for the FreeBSD build.
#
# FreeBSD's libutil has a pw_init() of its own (the passwd library), so the
# FreeBSD port of PipeWire renames PipeWire's pw_init and pw_deinit to
# pipewire_init and pipewire_deinit, and leaves pw_init as a macro.
# bindgen doesn't turn macros into functions, so pipewire-sys has no
# pw_init on FreeBSD and the pipewire crate fails to build:
#
#   error[E0425]: cannot find function `pw_init` in crate `pw_sys`
#
# pipewire-sys-freebsd.patch has bindgen pick up the real names, and gives
# them their usual names on FreeBSD only. Nothing changes on Linux, where
# no pipewire_init exists for it to pick up.
#
# This fetches pipewire-sys at the version Cargo.lock has, checks it
# against the lockfile's checksum, applies the patch, and prints the
# folder. build-freebsd.sh hands that folder to cargo as a [patch], so the
# patch follows whatever version `cargo update` picked.
#
# Usage: packaging/patch-pipewire-freebsd.sh OUTDIR
#
# Prints nothing when pipewire-sys handles FreeBSD itself now (its source
# already names pipewire_init): the build goes on with it as it is. Fails
# when the patch doesn't apply and it still doesn't. See "The FreeBSD
# build" in maintenance.md for both cases.
set -euo pipefail
export LC_ALL=C

CRATE=pipewire-sys
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PATCH_FILE="$ROOT/packaging/pipewire-sys-freebsd.patch"
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
for c in tar patch awk grep mkdir rm; do
    command -v "$c" >/dev/null 2>&1 || missing="$missing $c"
done
if ! command -v curl >/dev/null 2>&1 && ! command -v fetch >/dev/null 2>&1; then
    missing="$missing curl"
fi
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1 &&
    ! command -v sha256 >/dev/null 2>&1; then
    missing="$missing sha256sum"
fi
[ -z "$missing" ] || die "missing commands:$missing"

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -- "$1" | awk '{print $1}'
    else
        # FreeBSD's own.
        sha256 -q -- "$1"
    fi
}

download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$1" "$2"
    else
        fetch -q -o "$1" "$2"
    fi
}

[ -f "$LOCK" ] || die "no Cargo.lock. Run cargo generate-lockfile (or cargo update) first."
[ -f "$PATCH_FILE" ] || die "no $PATCH_FILE"

# The crate's version and checksum, from its [[package]] entry.
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

tmp="$(mktemp "${TMPDIR:-/tmp}/pipewire-sys.XXXXXX")"
trap 'rm -f -- "$tmp"' EXIT

say "Fetching $CRATE $version to patch it for FreeBSD"
download "$tmp" "https://static.crates.io/crates/$CRATE/$CRATE-$version.crate" ||
    die "couldn't download $CRATE $version from crates.io"
got="$(sha256 "$tmp")"
[ "$got" = "$checksum" ] || die "$CRATE $version doesn't match Cargo.lock's checksum (got $got)"

rm -rf -- "$dir"
mkdir -p -- "$OUT"
tar -xzf "$tmp" -C "$OUT"
if [ ! -f "$dir/build.rs" ] || [ ! -f "$dir/src/lib.rs" ]; then
    die "$CRATE $version has no build.rs or src/lib.rs; its layout changed"
fi

# Upstream took a fix of its own: nothing to do.
if grep -q 'pipewire_init' "$dir/build.rs" "$dir/src/lib.rs"; then
    rm -rf -- "$dir"
    say "$CRATE $version handles FreeBSD's pipewire_init itself. Building with it as it is; delete the patch (see maintenance.md)."
    exit 0
fi

if patch -p1 -N -d "$dir" <"$PATCH_FILE" >&2; then
    : >"$dir/.patina-patched"
    say "Patched $CRATE $version for FreeBSD"
    printf '%s\n' "$dir"
    exit 0
fi

rm -rf -- "$dir"
die "pipewire-sys-freebsd.patch no longer applies to $CRATE $version. Redo it against this version (see maintenance.md)."
