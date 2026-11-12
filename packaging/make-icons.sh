#!/usr/bin/env bash
# Render every PNG icon from the SVGs in assets/. Run it after changing an
# SVG, and commit the PNGs it writes; the builds use the PNGs as they are.
#
#   bash packaging/make-icons.sh
#
# The sources:
#   assets/patinaplayer.svg          the icon, for 48 px and up
#   assets/patinaplayer-small.svg    a simpler drawing for 16 to 32 px
#   assets/android-foreground.svg    the Android launcher's foreground layer
#   assets/android-monochrome.svg    the same, one colour, for themed icons
#
# Needs rsvg-convert (librsvg2-bin on Debian and Ubuntu, librsvg2-tools on
# Fedora, librsvg on Arch).
set -euo pipefail

if ! command -v rsvg-convert >/dev/null 2>&1; then
    printf '%s\n' "Missing: rsvg-convert" >&2
    exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)" || exit 1
A="$ROOT/assets"
RES="$ROOT/android/res"

render() { # svg size out
    mkdir -p "$(dirname "$3")"
    rsvg-convert -w "$2" -h "$2" -- "$1" >"$3"
    printf '%s\n' "${3#"$ROOT"/}"
}

# The window, the tray, and the hicolor theme in the AppImage.
render "$A/patinaplayer.svg" 256 "$A/patinaplayer.png"
render "$A/patinaplayer-small.svg" 64 "$A/patinaplayer-small.png"
for s in 16 22 24 32; do
    render "$A/patinaplayer-small.svg" "$s" "$A/icons/${s}x${s}/patinaplayer.png"
done
for s in 48 64 128 512; do
    render "$A/patinaplayer.svg" "$s" "$A/icons/${s}x${s}/patinaplayer.png"
done

# Android: the plain icon at 48 dp, and the adaptive layers at 108 dp, per
# screen density.
while IFS=: read -r density legacy adaptive; do
    render "$A/patinaplayer.svg" "$legacy" "$RES/mipmap-$density/ic_launcher.png"
    render "$A/android-foreground.svg" "$adaptive" "$RES/mipmap-$density/ic_launcher_foreground.png"
    render "$A/android-monochrome.svg" "$adaptive" "$RES/mipmap-$density/ic_launcher_monochrome.png"
done <<'EOF'
mdpi:48:108
hdpi:72:162
xhdpi:96:216
xxhdpi:144:324
xxxhdpi:192:432
EOF
