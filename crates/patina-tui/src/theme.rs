//! Runout's palette (ui/src/styles.css), dark only. Truecolor where the
//! terminal says it supports it, nearest xterm-256 match everywhere else.

use ratatui::style::Color;

pub struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub raised: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub verdigris: Color,
    pub verdigris_dim: Color,
    pub copper: Color,
    pub copper_dim: Color,
    pub wave_raw: Color,
}

const PALETTE: [(u8, u8, u8); 11] = [
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

impl Theme {
    pub fn detect() -> Self {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        let c = |i: usize| {
            let (r, g, b) = PALETTE[i];
            if truecolor {
                Color::Rgb(r, g, b)
            } else {
                Color::Indexed(nearest_256(r, g, b))
            }
        };
        Self {
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
}
