//! The EQ as you set it: graphic or parametric bands, the preamp, presets,
//! imports from AutoEq and Equalizer APO, and which output device gets
//! which EQ. [`EqSettings::params`] turns it into what the renderer runs.

pub mod import;
pub mod presets;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dsp::biquad::{Band, Coeffs, Kind};
use crate::dsp::{EqParams, MAX_BANDS};

/// Graphic band gains go from -12 to +12 dB.
pub const GRAPHIC_RANGE: f64 = 12.0;
/// Parametric bands, at most.
pub const MAX_PARAMETRIC: usize = 20;
/// Parametric gain range, wider than the graphic one: AutoEq corrections
/// go past 12 dB now and then.
pub const PARAMETRIC_RANGE: f64 = 24.0;
pub const FREQ_MIN: f64 = 10.0;
pub const FREQ_MAX: f64 = 24000.0;
pub const Q_MIN: f64 = 0.05;
pub const Q_MAX: f64 = 50.0;
pub const PREAMP_RANGE: f64 = 24.0;
/// The rate curves and the auto preamp are worked out at.
pub const CURVE_RATE: u32 = 48000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Graphic,
    Parametric,
}

/// ISO band centres: octaves, two-thirds octaves and third octaves.
pub const BANDS_10: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
pub const BANDS_15: [f64; 15] = [
    25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1000.0, 1600.0, 2500.0, 4000.0, 6300.0,
    10000.0, 16000.0,
];
pub const BANDS_31: [f64; 31] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0, 12500.0, 16000.0, 20000.0,
];

/// The centres for a graphic EQ of `n` bands (10, 15 or 31).
pub fn centres(n: usize) -> &'static [f64] {
    match n {
        15 => &BANDS_15,
        31 => &BANDS_31,
        _ => &BANDS_10,
    }
}

/// Q for a band `octaves` wide, so neighbours meet at their -3 dB points.
fn q_for(octaves: f64) -> f64 {
    let n = 2f64.powf(octaves);
    n.sqrt() / (n - 1.0)
}

fn graphic_q(n: usize) -> f64 {
    match n {
        15 => q_for(2.0 / 3.0),
        31 => q_for(1.0 / 3.0),
        _ => q_for(1.0),
    }
}

/// "1k", "16k", "31.5", "250" for band labels.
pub fn freq_label(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 1e-9 {
            format!("{}k", k as u32)
        } else {
            format!("{k:.1}k").replace(".0k", "k")
        }
    } else if (f - f.round()).abs() < 1e-9 {
        format!("{}", f as u32)
    } else {
        format!("{f:.1}")
    }
}

/// Everything about the EQ you can set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    /// Off is bypassed.
    pub on: bool,
    pub mode: Mode,
    /// 10, 15 or 31.
    pub graphic_bands: usize,
    /// dB per graphic band.
    pub graphic: Vec<f64>,
    pub parametric: Vec<Band>,
    /// dB, before the bands.
    pub preamp: f64,
    /// Take the largest boost off the preamp too, so the EQ can't push the
    /// signal into the limiter. Off by default: with it on, raising a band
    /// turns everything else down by as much, so a boost sounds quieter
    /// overall. The limiter catches what a boost would clip.
    pub auto_preamp: bool,
    /// The preset it came from, for showing; empty for your own tweaks.
    pub preset: String,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            on: true,
            mode: Mode::Graphic,
            graphic_bands: 10,
            graphic: vec![0.0; 10],
            parametric: Vec::new(),
            preamp: 0.0,
            auto_preamp: false,
            preset: "Flat".into(),
        }
    }
}

impl EqSettings {
    /// Fix anything out of range: band counts, gains, frequencies.
    pub fn normalized(mut self) -> Self {
        if ![10, 15, 31].contains(&self.graphic_bands) {
            self.graphic_bands = 10;
        }
        self.graphic.resize(self.graphic_bands, 0.0);
        for g in &mut self.graphic {
            *g = clean(*g).clamp(-GRAPHIC_RANGE, GRAPHIC_RANGE);
        }
        self.parametric.truncate(MAX_PARAMETRIC);
        for b in &mut self.parametric {
            *b = clamp_band(*b);
        }
        self.preamp = clean(self.preamp).clamp(-PREAMP_RANGE, PREAMP_RANGE);
        self
    }

    /// The bands that run, whichever mode is on.
    pub fn bands(&self) -> Vec<Band> {
        match self.mode {
            Mode::Graphic => {
                let q = graphic_q(self.graphic_bands);
                centres(self.graphic_bands)
                    .iter()
                    .zip(&self.graphic)
                    .map(|(&freq, &gain)| Band {
                        kind: Kind::Peak,
                        freq,
                        gain,
                        q,
                    })
                    .collect()
            }
            Mode::Parametric => self.parametric.clone(),
        }
    }

    /// The bands' combined response at `freq`, in dB, without the preamp.
    pub fn response_at(&self, freq: f64) -> f64 {
        response(&self.coeffs(), freq)
    }

    fn coeffs(&self) -> Vec<Coeffs> {
        self.bands()
            .iter()
            .map(|b| Coeffs::design(b, CURVE_RATE))
            .collect()
    }

    /// The response curve from 20 Hz to 20 kHz, `points` of it, log-spaced,
    /// as (Hz, dB). Bands only; the preamp moves the whole thing.
    pub fn curve(&self, points: usize) -> Vec<(f64, f64)> {
        let c = self.coeffs();
        log_points(points)
            .into_iter()
            .map(|f| (f, response(&c, f)))
            .collect()
    }

    /// The largest boost anywhere from 20 Hz to 20 kHz.
    pub fn max_boost(&self) -> f64 {
        let c = self.coeffs();
        log_points(400)
            .into_iter()
            .map(|f| response(&c, f))
            .fold(0.0f64, f64::max)
    }

    /// The preamp that runs: yours, and the auto part when it's on.
    pub fn total_preamp(&self) -> f64 {
        let auto = if self.auto_preamp {
            -self.max_boost()
        } else {
            0.0
        };
        self.preamp + auto
    }

    /// What the renderer runs.
    pub fn params(&self) -> EqParams {
        let bands = self.bands();
        let mut p = EqParams {
            active: self.on,
            preamp_db: self.total_preamp(),
            n: bands.len().min(MAX_BANDS),
            ..Default::default()
        };
        for (slot, b) in p.bands.iter_mut().zip(bands) {
            *slot = b;
        }
        p
    }

    /// Switch the graphic EQ to `n` bands, keeping the curve: each new band
    /// takes the old curve's value at its centre.
    pub fn set_graphic_bands(&mut self, n: usize) {
        if n == self.graphic_bands || ![10, 15, 31].contains(&n) {
            return;
        }
        let old: Vec<(f64, f64)> = centres(self.graphic_bands)
            .iter()
            .copied()
            .zip(self.graphic.iter().copied())
            .collect();
        self.graphic = centres(n)
            .iter()
            .map(|&f| interp_log(&old, f).clamp(-GRAPHIC_RANGE, GRAPHIC_RANGE))
            .map(|g| (g * 10.0).round() / 10.0)
            .collect();
        self.graphic_bands = n;
    }

    /// Whether these are the same sound, ignoring on, the preset name and
    /// whichever mode isn't showing.
    pub fn same_sound(&self, other: &EqSettings) -> bool {
        self.mode == other.mode
            && (self.preamp - other.preamp).abs() < 1e-9
            && self.auto_preamp == other.auto_preamp
            && match self.mode {
                Mode::Graphic => {
                    self.graphic_bands == other.graphic_bands && self.graphic == other.graphic
                }
                Mode::Parametric => self.parametric == other.parametric,
            }
    }

    pub fn is_flat(&self) -> bool {
        self.preamp.abs() < 1e-9 && self.bands().iter().all(Band::is_flat)
    }
}

/// Keep a band inside what the EQ allows.
pub fn clamp_band(mut b: Band) -> Band {
    b.freq = clean(b.freq).clamp(FREQ_MIN, FREQ_MAX);
    b.gain = clean(b.gain).clamp(-PARAMETRIC_RANGE, PARAMETRIC_RANGE);
    b.q = clean(b.q).clamp(Q_MIN, Q_MAX);
    b
}

fn clean(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

fn response(c: &[Coeffs], f: f64) -> f64 {
    c.iter().map(|c| c.response_db(f, CURVE_RATE)).sum()
}

pub fn log_points(n: usize) -> Vec<f64> {
    let n = n.max(2);
    let (lo, hi) = (20f64.ln(), 20000f64.ln());
    (0..n)
        .map(|i| (lo + (hi - lo) * i as f64 / (n - 1) as f64).exp())
        .collect()
}

/// Straight-line interpolation on a log frequency axis, flat past the ends.
pub fn interp_log(points: &[(f64, f64)], f: f64) -> f64 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if f <= first.0 {
        return first.1;
    }
    for w in points.windows(2) {
        let ((f0, g0), (f1, g1)) = (w[0], w[1]);
        if f <= f1 {
            let t = (f.ln() - f0.ln()) / (f1.ln() - f0.ln()).max(1e-12);
            return g0 + (g1 - g0) * t;
        }
    }
    points.last().map(|p| p.1).unwrap_or(0.0)
}

/// Fit the graphic EQ's bands to a target curve, so the sound that comes
/// out (overlapping bands and all) follows it. A few rounds of nudging
/// each band by what's still missing at its centre.
pub fn fit_graphic(target: &[(f64, f64)], n: usize) -> (Vec<f64>, bool) {
    let cs = centres(n);
    let want: Vec<f64> = cs.iter().map(|&f| interp_log(target, f)).collect();
    let mut s = EqSettings {
        graphic_bands: n,
        graphic: want.clone(),
        ..Default::default()
    };
    for _ in 0..40 {
        let got: Vec<f64> = cs.iter().map(|&f| s.response_at(f)).collect();
        for i in 0..n {
            s.graphic[i] =
                (s.graphic[i] + 0.6 * (want[i] - got[i])).clamp(-GRAPHIC_RANGE, GRAPHIC_RANGE);
        }
    }
    let clipped = want.iter().any(|w| w.abs() > GRAPHIC_RANGE + 0.05);
    let gains = s
        .graphic
        .iter()
        .map(|g| (g * 10.0).round() / 10.0)
        .collect();
    (gains, clipped)
}

/// What config.toml's `[eq]` holds: the EQ for devices with none of their
/// own, and which device gets which preset.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EqConfig {
    #[serde(flatten)]
    pub settings: EqSettings,
    /// Output device (as PipeWire or ALSA names it) to preset name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<String, String>,
}

/// The output the sound is going to, as far as the player can tell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputDevice {
    /// Stable id: a PipeWire node name like
    /// "alsa_output.usb-Sennheiser_HD650-00.analog-stereo", or "alsa:hw:1,0".
    pub key: String,
    /// For showing: "HD 650 Analog Stereo".
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphic_curve_follows_the_sliders() {
        let mut s = EqSettings::default();
        s.graphic[5] = 6.0; // 1 kHz
        assert!(
            (s.response_at(1000.0) - 6.0).abs() < 0.5,
            "{}",
            s.response_at(1000.0)
        );
        assert!(s.response_at(31.5).abs() < 0.3);
        assert!((s.max_boost() - s.response_at(1000.0)).abs() < 0.2);
        // Auto preamp takes the boost off.
        s.auto_preamp = true;
        let p = s.params();
        assert!((p.preamp_db + s.max_boost()).abs() < 1e-9);
        s.auto_preamp = false;
        assert_eq!(s.params().preamp_db, 0.0);
    }

    #[test]
    fn band_count_change_keeps_the_curve() {
        let mut s = EqSettings {
            graphic: vec![0.0, 0.0, 0.0, 0.0, 0.0, 6.0, 6.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        s.set_graphic_bands(31);
        assert_eq!(s.graphic.len(), 31);
        let at = |f: f64| s.graphic[BANDS_31.iter().position(|&x| x == f).unwrap()];
        assert_eq!(at(1000.0), 6.0);
        assert_eq!(at(2000.0), 6.0);
        assert_eq!(at(125.0), 0.0);
        assert!((at(1600.0) - 6.0).abs() < 1e-9);
        assert!(at(500.0) == 0.0 && at(630.0) > 0.0);
    }

    #[test]
    fn fitting_a_target_comes_close() {
        let target = vec![(20.0, 6.0), (200.0, 6.0), (2000.0, -3.0), (20000.0, -3.0)];
        let (g, clipped) = fit_graphic(&target, 31);
        assert!(!clipped);
        let s = EqSettings {
            graphic_bands: 31,
            graphic: g,
            ..Default::default()
        };
        for f in [40.0, 100.0, 5000.0, 10000.0] {
            let want = interp_log(&target, f);
            assert!(
                (s.response_at(f) - want).abs() < 1.0,
                "{f}: {} vs {want}",
                s.response_at(f)
            );
        }
    }

    #[test]
    fn normalizing_fixes_bad_values() {
        let s = EqSettings {
            graphic_bands: 12,
            graphic: vec![40.0; 3],
            parametric: vec![
                Band {
                    kind: Kind::Peak,
                    freq: 1e9,
                    gain: f64::NAN,
                    q: 0.0
                };
                30
            ],
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.graphic.len(), 10);
        assert_eq!(s.graphic[0], 12.0);
        assert_eq!(s.parametric.len(), 20);
        assert_eq!(s.parametric[0].freq, FREQ_MAX);
        assert_eq!(s.parametric[0].gain, 0.0);
    }

    #[test]
    fn labels() {
        assert_eq!(freq_label(31.5), "31.5");
        assert_eq!(freq_label(1000.0), "1k");
        assert_eq!(freq_label(12500.0), "12.5k");
        assert_eq!(freq_label(250.0), "250");
    }
}
