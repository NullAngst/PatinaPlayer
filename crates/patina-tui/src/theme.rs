//! The TUI's colours: Runout's palette (ui/src/styles.css) by default, or
//! one of the others the GUI uses: plain light, dark and OLED black, and the
//! two kawaii ones. Truecolor where the terminal
//! says it supports it, nearest xterm-256 match everywhere else.

use patina_core::config::Theme as Kind;
use ratatui::style::Color;

pub struct Theme {
    pub kind: Kind,
    pub bg: Color,
    pub surface: Color,
    pub raised: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    /// The main accent: verdigris in Runout, blue in the plain themes, pink
    /// in the kawaii ones.
    pub verdigris: Color,
    pub verdigris_dim: Color,
    /// The warm accent, also used for warnings.
    pub copper: Color,
    pub copper_dim: Color,
    pub wave_raw: Color,
}

type Palette = [(u8, u8, u8); 11];

const RUNOUT: Palette = [
    (0x0e, 0x19, 0x17), // bg
    (0x13, 0x22, 0x20), // surface
    (0x17, 0x2a, 0x27), // raised
    (0x2a, 0x3d, 0x39), // border
    (0xe6, 0xec, 0xe9), // text
    (0x8f, 0xa3, 0x9d), // muted
    (0x5f, 0xb8, 0xa5), // verdigris
    (0x2f, 0x5f, 0x55), // verdigris-dim
    (0xd4, 0x88, 0x4a), // copper
    (0x5a, 0x3a, 0x22), // copper-dim
    (0x4a, 0x5f, 0x5a), // wave-raw
];

/// Sakura Milk: cream and strawberry pink. The accents are a shade deeper
/// than the GUI's so they still read as text on a light background.
const KAWAII_LIGHT: Palette = [
    (0xff, 0xf8, 0xf5), // bg
    (0xff, 0xf0, 0xf2), // surface
    (0xff, 0xe3, 0xea), // raised
    (0xf3, 0xc6, 0xd3), // border
    (0x5a, 0x3e, 0x47), // text
    (0xa7, 0x7d, 0x8a), // muted
    (0xe0, 0x55, 0x8a), // pink
    (0xf7, 0xc1, 0xd3), // pink, soft
    (0xd2, 0x7a, 0x2c), // apricot
    (0xf6, 0xdc, 0xc0), // apricot, soft
    (0xd3, 0xb3, 0xbe), // faint
];

/// Sakura Milk after dark: cocoa plum with the same pink and butter.
const KAWAII_DARK: Palette = [
    (0x24, 0x1a, 0x22), // bg
    (0x2d, 0x20, 0x29), // surface
    (0x3b, 0x2a, 0x36), // raised
    (0x55, 0x3d, 0x4c), // border
    (0xfb, 0xe9, 0xef), // text
    (0xc4, 0x9a, 0xab), // muted
    (0xff, 0x8f, 0xb3), // pink
    (0x6b, 0x3a, 0x52), // pink, deep
    (0xff, 0xd2, 0x7a), // butter
    (0x5c, 0x4a, 0x2a), // butter, deep
    (0x7a, 0x5a, 0x6a), // faint
];

/// Plain light: white and cool grey, one blue. Accents a shade deeper
/// than the GUI's, as text on white.
const LIGHT: Palette = [
    (0xff, 0xff, 0xff), // bg
    (0xf5, 0xf6, 0xf8), // surface
    (0xe6, 0xe9, 0xee), // raised
    (0xd3, 0xd7, 0xde), // border
    (0x1d, 0x1f, 0x23), // text
    (0x62, 0x67, 0x70), // muted
    (0x25, 0x5f, 0xc8), // blue
    (0xc6, 0xd8, 0xf7), // blue, soft
    (0xb0, 0x52, 0x10), // orange
    (0xf1, 0xd6, 0xc2), // orange, soft
    (0xc3, 0xc8, 0xd0), // faint
];

/// Plain dark: neutral charcoal and the same blue.
const DARK: Palette = [
    (0x1c, 0x1d, 0x21), // bg
    (0x24, 0x26, 0x2b), // surface
    (0x2d, 0x30, 0x36), // raised
    (0x3a, 0x3d, 0x44), // border
    (0xe8, 0xe9, 0xec), // text
    (0x9a, 0x9e, 0xa7), // muted
    (0x5b, 0x9c, 0xf5), // blue
    (0x2b, 0x46, 0x70), // blue, deep
    (0xe0, 0x9a, 0x5b), // orange
    (0x5a, 0x3e, 0x26), // orange, deep
    (0x4a, 0x4e, 0x57), // faint
];

/// OLED: true black, so the screen's pixels are off. Only the selection
/// and borders light up.
const OLED: Palette = [
    (0x00, 0x00, 0x00), // bg
    (0x00, 0x00, 0x00), // surface
    (0x14, 0x14, 0x14), // raised
    (0x26, 0x26, 0x26), // border
    (0xf2, 0xf2, 0xf2), // text
    (0x8a, 0x8a, 0x8a), // muted
    (0x6a, 0xa8, 0xff), // blue
    (0x1d, 0x33, 0x54), // blue, deep
    (0xf0, 0xa8, 0x68), // orange
    (0x3d, 0x2a, 0x18), // orange, deep
    (0x33, 0x33, 0x33), // faint
];

impl Theme {
    pub fn new(kind: Kind) -> Self {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        let pal = match kind {
            Kind::Runout => &RUNOUT,
            Kind::KawaiiLight => &KAWAII_LIGHT,
            Kind::KawaiiDark => &KAWAII_DARK,
            Kind::Light => &LIGHT,
            // The terminal has no system colors to follow.
            Kind::Dark | Kind::System => &DARK,
            Kind::Oled => &OLED,
        };
        let c = |i: usize| {
            let (r, g, b) = pal[i];
            if truecolor {
                Color::Rgb(r, g, b)
            } else {
                Color::Indexed(nearest_256(r, g, b))
            }
        };
        Self {
            kind,
            bg: c(0),
            surface: c(1),
            raised: c(2),
            border: c(3),
            text: c(4),
            muted: c(5),
            verdigris: c(6),
            verdigris_dim: c(7),
            copper: c(8),
            copper_dim: c(9),
            wave_raw: c(10),
        }
    }
}

/// Nearest xterm-256 index, searching the 6x6x6 cube and the grey ramp.
/// The first 16 are skipped since terminals remap them freely.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let dist = |a: (u8, u8, u8)| {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        d(a.0, r) + d(a.1, g) + d(a.2, b)
    };
    let mut best = (16u8, i32::MAX);
    for (ri, &rv) in LEVELS.iter().enumerate() {
        for (gi, &gv) in LEVELS.iter().enumerate() {
            for (bi, &bv) in LEVELS.iter().enumerate() {
                let d = dist((rv, gv, bv));
                if d < best.1 {
                    best = ((16 + 36 * ri + 6 * gi + bi) as u8, d);
                }
            }
        }
    }
    for i in 0..24u8 {
        let v = 8 + 10 * i;
        let d = dist((v, v, v));
        if d < best.1 {
            best = (232 + i, d);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_maps_to_ramp() {
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
    }

    #[test]
    fn oled_is_black_in_256_colours() {
        assert_eq!(nearest_256(0, 0, 0), 16);
    }
}
