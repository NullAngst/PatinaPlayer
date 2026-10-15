//! Second-order IIR filters from Robert Bristow-Johnson's Audio EQ
//! Cookbook: the peak, shelf, pass and notch shapes every EQ is built
//! from. Coefficients are worked out for the stream's own sample rate, and
//! the same coefficients give the curve the EQ view draws, so what you see
//! is what plays.

use serde::{Deserialize, Serialize};

/// What a band does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Peak,
        Kind::LowShelf,
        Kind::HighShelf,
        Kind::LowPass,
        Kind::HighPass,
        Kind::Notch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Peak => "Peak",
            Kind::LowShelf => "Low shelf",
            Kind::HighShelf => "High shelf",
            Kind::LowPass => "Low pass",
            Kind::HighPass => "High pass",
            Kind::Notch => "Notch",
        }
    }

    /// Short form, for narrow places: "PK", "LS".
    pub fn short(self) -> &'static str {
        match self {
            Kind::Peak => "PK",
            Kind::LowShelf => "LS",
            Kind::HighShelf => "HS",
            Kind::LowPass => "LP",
            Kind::HighPass => "HP",
            Kind::Notch => "NO",
        }
    }

    /// Whether the gain does anything for this shape.
    pub fn has_gain(self) -> bool {
        matches!(self, Kind::Peak | Kind::LowShelf | Kind::HighShelf)
    }
}

/// One band's settings, independent of sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band {
    #[serde(rename = "type", default)]
    pub kind: Kind,
    /// Hz.
    pub freq: f64,
    /// dB. Ignored by the pass and notch shapes.
    #[serde(default)]
    pub gain: f64,
    #[serde(default = "default_q")]
    pub q: f64,
}

fn default_q() -> f64 {
    std::f64::consts::FRAC_1_SQRT_2
}

impl Default for Band {
    fn default() -> Self {
        Self {
            kind: Kind::Peak,
            freq: 1000.0,
            gain: 0.0,
            q: default_q(),
        }
    }
}

impl Band {
    /// Does nothing at all: a peak or shelf at 0 dB.
    pub fn is_flat(&self) -> bool {
        self.kind.has_gain() && self.gain.abs() < 1e-9
    }
}

/// Normalised coefficients (a0 = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// The cookbook formulas. A band at or past Nyquist can't be built at
    /// this rate: a peak or notch there does nothing, a low pass passes
    /// everything, a high pass is held just under Nyquist.
    pub fn design(b: &Band, rate: u32) -> Coeffs {
        let fs = rate as f64;
        let q = b.q.clamp(0.025, 100.0);
        let mut f = b.freq.max(1.0);
        if f >= fs * 0.49 {
            match b.kind {
                Kind::HighPass => f = fs * 0.49,
                Kind::HighShelf => f = fs * 0.49,
                _ => return Coeffs::IDENTITY,
            }
        }
        if b.is_flat() {
            return Coeffs::IDENTITY;
        }
        let w0 = 2.0 * std::f64::consts::PI * f / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a = 10f64.powf(b.gain / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match b.kind {
            Kind::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            Kind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            Kind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
            Kind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Kind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Kind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        };
        Coeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Gain at `freq` Hz, in dB.
    pub fn response_db(&self, freq: f64, rate: u32) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / rate as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        // H(e^jw) = (b0 + b1 e^-jw + b2 e^-2jw) / (1 + a1 e^-jw + a2 e^-2jw)
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let num = nr * nr + ni * ni;
        let den = (dr * dr + di * di).max(1e-300);
        10.0 * (num / den).max(1e-30).log10()
    }
}

/// One channel's filter memory (transposed direct form II, which behaves
/// well when the coefficients change under it).
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    s1: f64,
    s2: f64,
}

impl State {
    #[inline]
    pub fn run(&mut self, c: &Coeffs, x: f64) -> f64 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Clear values too small to matter, so a filter ringing down into
    /// silence never lands on denormals (slow on x86).
    #[inline]
    pub fn settle(&mut self) {
        if self.s1.abs() < 1e-25 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-25 {
            self.s2 = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(kind: Kind, freq: f64, gain: f64, q: f64) -> Band {
        Band {
            kind,
            freq,
            gain,
            q,
        }
    }

    #[test]
    fn peak_hits_its_gain_at_its_frequency() {
        let c = Coeffs::design(&band(Kind::Peak, 1000.0, 6.0, 1.0), 48000);
        assert!((c.response_db(1000.0, 48000) - 6.0).abs() < 1e-6);
        assert!(c.response_db(30.0, 48000).abs() < 0.1);
        assert!(c.response_db(15000.0, 48000).abs() < 0.2);
    }

    #[test]
    fn shelves_reach_their_gain_far_from_the_corner() {
        let ls = Coeffs::design(&band(Kind::LowShelf, 100.0, 5.0, 0.707), 44100);
        assert!((ls.response_db(10.0, 44100) - 5.0).abs() < 0.1);
        assert!(ls.response_db(5000.0, 44100).abs() < 0.1);
        let hs = Coeffs::design(&band(Kind::HighShelf, 8000.0, -4.0, 0.707), 44100);
        assert!((hs.response_db(20000.0, 44100) + 4.0).abs() < 0.3);
        assert!(hs.response_db(100.0, 44100).abs() < 0.1);
    }

    #[test]
    fn pass_and_notch_shapes() {
        let lp = Coeffs::design(&band(Kind::LowPass, 1000.0, 0.0, 0.707), 48000);
        assert!((lp.response_db(1000.0, 48000) + 3.01).abs() < 0.05);
        assert!(lp.response_db(10000.0, 48000) < -35.0);
        let hp = Coeffs::design(&band(Kind::HighPass, 100.0, 0.0, 0.707), 48000);
        assert!(hp.response_db(10.0, 48000) < -35.0);
        let no = Coeffs::design(&band(Kind::Notch, 1000.0, 0.0, 10.0), 48000);
        assert!(no.response_db(1000.0, 48000) < -60.0);
    }

    #[test]
    fn flat_and_out_of_range_bands_are_identity() {
        assert_eq!(
            Coeffs::design(&band(Kind::Peak, 1000.0, 0.0, 1.0), 48000),
            Coeffs::IDENTITY
        );
        assert_eq!(
            Coeffs::design(&band(Kind::Peak, 30000.0, 6.0, 1.0), 48000),
            Coeffs::IDENTITY
        );
    }

    #[test]
    fn filtering_matches_the_response() {
        // A 1 kHz sine through a +6 dB peak comes out twice as big.
        let c = Coeffs::design(&band(Kind::Peak, 1000.0, 6.0206, 1.0), 48000);
        let mut st = State::default();
        let mut peak = 0.0f64;
        for i in 0..48000 {
            let x = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48000.0).sin() * 0.25;
            let y = st.run(&c, x);
            if i > 24000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - 0.5).abs() < 0.001, "{peak}");
    }
}
