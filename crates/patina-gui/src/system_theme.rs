//! The System theme: a palette built from the system's own colors.
//!
//! On Android 12 and later that's Material You. The system hands out five
//! tonal palettes made from the wallpaper (three accents, two neutrals),
//! each in thirteen shades, and the phone is in light or dark mode. This
//! picks shades from them for each of the app's palette slots, the way
//! Material 3 builds its light and dark schemes. Anywhere else, or on an
//! older Android, the System theme looks like plain Dark.

use slint::Color;

use crate::Palette;

/// The system's colors. Each palette is ARGB, in Android's shade order:
/// 0, 10, 50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000 (white to
/// black).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemColors {
    pub accent1: [u32; 13],
    pub accent2: [u32; 13],
    pub accent3: [u32; 13],
    pub neutral1: [u32; 13],
    pub neutral2: [u32; 13],
    /// The system is in dark mode.
    pub dark: bool,
}

impl SystemColors {
    /// From one flat list: the five palettes in the order above, then 1
    /// for dark mode or 0 for light. None if it's the wrong length.
    pub fn from_flat(v: &[i32]) -> Option<Self> {
        if v.len() != 5 * 13 + 1 {
            return None;
        }
        let pal = |i: usize| -> [u32; 13] {
            let mut out = [0u32; 13];
            for (k, o) in out.iter_mut().enumerate() {
                *o = v[i * 13 + k] as u32;
            }
            out
        };
        Some(Self {
            accent1: pal(0),
            accent2: pal(1),
            accent3: pal(2),
            neutral1: pal(3),
            neutral2: pal(4),
            dark: v[65] != 0,
        })
    }
}

/// A shade by Android's number.
fn shade(p: &[u32; 13], n: u32) -> Color {
    let i = match n {
        0 => 0,
        10 => 1,
        50 => 2,
        100 => 3,
        200 => 4,
        300 => 5,
        400 => 6,
        500 => 7,
        600 => 8,
        700 => 9,
        800 => 10,
        900 => 11,
        _ => 12,
    };
    let c = p[i];
    Color::from_argb_u8(
        255,
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    )
}

fn rgb(c: u32) -> Color {
    Color::from_argb_u8(255, (c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn alpha(c: Color, a: u8) -> Color {
    Color::from_argb_u8(a, c.red(), c.green(), c.blue())
}

/// Part way from `a` to `b`.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::from_argb_u8(
        255,
        l(a.red(), b.red()),
        l(a.green(), b.green()),
        l(a.blue(), b.blue()),
    )
}

/// The palette for the System theme, and whether it's a light one.
pub fn palette(c: Option<&SystemColors>) -> (Palette, bool) {
    let Some(c) = c else {
        return (plain_dark(), false);
    };
    let (a1, a2, a3, n1, n2) = (&c.accent1, &c.accent2, &c.accent3, &c.neutral1, &c.neutral2);
    if c.dark {
        let bg = shade(n1, 900);
        let surface = mix(bg, shade(n1, 800), 0.35);
        let raised = mix(bg, shade(n1, 800), 0.7);
        let p = Palette {
            bg,
            surface,
            raised,
            border: shade(n2, 700),
            text: shade(n1, 100),
            muted: shade(n2, 300),
            verdigris: shade(a1, 200),
            verdigris_dim: shade(a1, 700),
            copper: shade(a3, 200),
            copper_dim: shade(a3, 800),
            wave: shade(n2, 600),
            on_accent: shade(a1, 800),
            playing_bg: alpha(shade(a1, 200), 0x1f),
            accent_text: shade(a1, 200),
            hover: alpha(raised, 0x99),
            sel_idle: alpha(raised, 0x80),
            accent_hover: shade(a1, 100),
            chip_hover: shade(n1, 800),
            accent_edge: Color::from_argb_u8(0, 0, 0, 0),
            scrim: Color::from_argb_u8(0xcc, 0, 0, 0),
            shadow: Color::from_argb_u8(0x90, 0, 0, 0),
            mint: shade(a2, 200),
            butter: shade(a3, 100),
            dots: shade(n1, 800),
        };
        (p, false)
    } else {
        let p = Palette {
            bg: shade(n1, 10),
            surface: shade(n1, 50),
            raised: shade(n2, 100),
            border: shade(n2, 200),
            text: shade(n1, 900),
            muted: shade(n2, 600),
            verdigris: shade(a1, 600),
            verdigris_dim: shade(a1, 100),
            copper: shade(a3, 600),
            copper_dim: shade(a3, 100),
            wave: shade(n2, 300),
            on_accent: shade(a1, 0),
            playing_bg: alpha(shade(a1, 600), 0x14),
            accent_text: shade(a1, 700),
            hover: shade(n1, 50),
            sel_idle: shade(n1, 50),
            accent_hover: shade(a1, 500),
            chip_hover: shade(n2, 100),
            accent_edge: Color::from_argb_u8(0, 0, 0, 0),
            scrim: alpha(shade(n1, 900), 0x66),
            shadow: Color::from_argb_u8(0x2e, 0, 0, 0),
            mint: shade(a2, 600),
            butter: shade(a3, 500),
            dots: shade(n1, 100),
        };
        (p, true)
    }
}

/// The plain Dark theme's palette (as in app.slint), for when there are no
/// system colors to follow.
fn plain_dark() -> Palette {
    Palette {
        bg: rgb(0x1c1d21),
        surface: rgb(0x24262b),
        raised: rgb(0x2d3036),
        border: rgb(0x3a3d44),
        text: rgb(0xe8e9ec),
        muted: rgb(0x9a9ea7),
        verdigris: rgb(0x5b9cf5),
        verdigris_dim: rgb(0x2b4670),
        copper: rgb(0xe09a5b),
        copper_dim: rgb(0x5a3e26),
        wave: rgb(0x4a4e57),
        on_accent: rgb(0x0e1420),
        playing_bg: Color::from_argb_u8(0x1f, 0x5b, 0x9c, 0xf5),
        accent_text: rgb(0x6aa6f7),
        hover: Color::from_argb_u8(0x99, 0x2a, 0x2c, 0x31),
        sel_idle: Color::from_argb_u8(0x80, 0x2a, 0x2c, 0x31),
        accent_hover: rgb(0x74adf7),
        chip_hover: rgb(0x33363c),
        accent_edge: Color::from_argb_u8(0, 0, 0, 0),
        scrim: Color::from_argb_u8(0xb3, 0, 0, 0),
        shadow: Color::from_argb_u8(0x90, 0, 0, 0),
        mint: rgb(0x5cc4a8),
        butter: rgb(0xe5c15a),
        dots: rgb(0x2a2c31),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(base: u32) -> [u32; 13] {
        // White to black, tinted.
        let mut out = [0u32; 13];
        for (i, o) in out.iter_mut().enumerate() {
            let v = 255 - (i as u32 * 255 / 12);
            *o = 0xff00_0000 | (v << 16) | (v << 8) | (v & base);
        }
        out
    }

    #[test]
    fn flat_list_reads_back() {
        let mut v: Vec<i32> = Vec::new();
        for _ in 0..5 {
            v.extend(ramp(0xff).iter().map(|&c| c as i32));
        }
        v.push(1);
        let c = SystemColors::from_flat(&v).unwrap();
        assert!(c.dark);
        assert_eq!(c.neutral2[12] & 0xffffff, 0);
        assert!(SystemColors::from_flat(&v[..10]).is_none());
        // Dark mode: light text on a dark ground.
        let (p, light) = palette(Some(&c));
        assert!(!light);
        assert!(p.text.red() > p.bg.red());
        let (p, light) = palette(Some(&SystemColors { dark: false, ..c }));
        assert!(light);
        assert!(p.text.red() < p.bg.red());
    }
}
