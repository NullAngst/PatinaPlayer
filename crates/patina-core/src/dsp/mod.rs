//! The real-time stages of the chain: the EQ and the true-peak limiter.
//! Both run inside the renderer on the output thread, so nothing here
//! allocates after construction, locks, or waits.

pub mod amp;
pub mod biquad;
pub mod limiter;
pub mod resample;

use biquad::{Band, Coeffs, State};

/// Graphic EQ tops out at 31 bands, parametric at 20.
pub const MAX_BANDS: usize = 31;
/// Bypass and on glide over this long, so A/B doesn't click.
const MIX_MS: f64 = 20.0;

/// What the renderer needs to run the EQ: plain values, so it can be
/// copied out of shared state without allocating.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqParams {
    /// Off means bypassed: the sound goes through untouched.
    pub active: bool,
    /// The total preamp, auto part included, in dB.
    pub preamp_db: f64,
    pub n: usize,
    pub bands: [Band; MAX_BANDS],
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            active: false,
            preamp_db: 0.0,
            n: 0,
            bands: [Band::default(); MAX_BANDS],
        }
    }
}

impl EqParams {
    /// On, and doing something: a band off flat or a preamp.
    pub fn changes_sound(&self) -> bool {
        self.active
            && (self.preamp_db.abs() > 1e-9 || self.bands[..self.n].iter().any(|b| !b.is_flat()))
    }
}

/// The EQ as the renderer runs it, for one stream format.
pub struct EqRt {
    ch: usize,
    rate: u32,
    n: usize,
    coeffs: [Coeffs; MAX_BANDS],
    /// Band-major: band b, channel c at `b * ch + c`.
    state: Vec<State>,
    /// Scratch copy for running the lookahead without moving the real state.
    look: Vec<State>,
    preamp: f64,
    /// 0 is bypassed, 1 is fully on.
    mix: f64,
    target: f64,
    mix_step: f64,
    dry: Vec<f64>,
}

impl EqRt {
    pub fn new(ch: usize, rate: u32, max_samples: usize) -> Self {
        Self {
            ch,
            rate,
            n: 0,
            coeffs: [Coeffs::IDENTITY; MAX_BANDS],
            state: vec![State::default(); MAX_BANDS * ch.max(1)],
            look: vec![State::default(); MAX_BANDS * ch.max(1)],
            preamp: 1.0,
            mix: 0.0,
            target: 0.0,
            mix_step: 1.0 / (rate as f64 * MIX_MS / 1000.0).max(1.0),
            dry: vec![0.0; max_samples],
        }
    }

    /// New settings. The filters keep their memory, so a band being
    /// dragged moves smoothly instead of restarting.
    pub fn set(&mut self, p: &EqParams) {
        let n = p.n.min(MAX_BANDS);
        for b in 0..n {
            self.coeffs[b] = Coeffs::design(&p.bands[b], self.rate);
        }
        for b in n..MAX_BANDS {
            self.coeffs[b] = Coeffs::IDENTITY;
        }
        if n > self.n {
            for s in &mut self.state[self.n * self.ch..n * self.ch] {
                *s = State::default();
            }
        }
        self.n = n;
        self.preamp = 10f64.powf(p.preamp_db / 20.0);
        self.target = if p.active { 1.0 } else { 0.0 };
    }

    /// Forget the filter memory: after a seek or a stop.
    pub fn reset(&mut self) {
        for s in &mut self.state {
            *s = State::default();
        }
        self.mix = self.target;
    }

    fn wet(coeffs: &[Coeffs], state: &mut [State], ch: usize, preamp: f64, buf: &mut [f64]) {
        let n = coeffs.len();
        for f in buf.chunks_exact_mut(ch) {
            for (c, s) in f.iter_mut().enumerate() {
                let mut x = *s * preamp;
                for b in 0..n {
                    x = state[b * ch + c].run(&coeffs[b], x);
                }
                *s = x;
            }
        }
        for s in state.iter_mut() {
            s.settle();
        }
    }

    pub fn process(&mut self, buf: &mut [f64]) {
        let ch = self.ch;
        if self.mix <= 0.0 && self.target <= 0.0 {
            return;
        }
        if self.mix >= 1.0 && self.target >= 1.0 {
            Self::wet(
                &self.coeffs[..self.n],
                &mut self.state,
                ch,
                self.preamp,
                buf,
            );
            return;
        }
        // Gliding between on and bypassed: blend the two.
        let dry = &mut self.dry[..buf.len()];
        dry.copy_from_slice(buf);
        Self::wet(
            &self.coeffs[..self.n],
            &mut self.state,
            ch,
            self.preamp,
            buf,
        );
        for (wf, df) in buf.chunks_exact_mut(ch).zip(dry.chunks_exact(ch)) {
            if self.mix < self.target {
                self.mix = (self.mix + self.mix_step).min(self.target);
            } else if self.mix > self.target {
                self.mix = (self.mix - self.mix_step).max(self.target);
            }
            for (w, d) in wf.iter_mut().zip(df) {
                *w = *d + (*w - *d) * self.mix;
            }
        }
        if self.mix <= 0.0 {
            // Fully bypassed now: start clean next time.
            for s in &mut self.state {
                *s = State::default();
            }
        }
    }

    /// Run audio that hasn't played yet through a copy of the filters, for
    /// the limiter to look ahead at. The real filters don't move.
    pub fn preview(&mut self, buf: &mut [f64]) {
        if self.mix <= 0.0 && self.target <= 0.0 {
            return;
        }
        let ch = self.ch;
        self.look.copy_from_slice(&self.state);
        let m = self.target.max(self.mix);
        let dry = &mut self.dry[..buf.len()];
        dry.copy_from_slice(buf);
        Self::wet(&self.coeffs[..self.n], &mut self.look, ch, self.preamp, buf);
        if m < 1.0 {
            for (w, d) in buf.iter_mut().zip(dry.iter()) {
                *w = *d + (*w - *d) * m;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::biquad::Kind;
    use super::*;

    fn params(active: bool, gain: f64) -> EqParams {
        let mut p = EqParams {
            active,
            n: 1,
            ..Default::default()
        };
        p.bands[0] = Band {
            kind: Kind::Peak,
            freq: 1000.0,
            gain,
            q: 1.0,
        };
        p
    }

    fn sine(n: usize) -> Vec<f64> {
        (0..n)
            .flat_map(|i| {
                let s = 0.25 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48000.0).sin();
                [s, s]
            })
            .collect()
    }

    #[test]
    fn bypassed_is_untouched() {
        let mut eq = EqRt::new(2, 48000, 8192);
        eq.set(&params(false, 6.0));
        let mut b = sine(2048);
        let orig = b.clone();
        eq.process(&mut b);
        assert_eq!(b, orig);
    }

    #[test]
    fn on_glides_in_then_applies_fully() {
        let mut eq = EqRt::new(2, 48000, 1 << 16);
        eq.set(&params(true, 6.0206));
        let mut b = sine(24000);
        eq.process(&mut b);
        let tail = b[b.len() - 2000..]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()));
        assert!((tail - 0.5).abs() < 0.002, "{tail}");
        // The first samples are barely touched: the glide starts at dry.
        assert!((b[2] - sine(2)[2]).abs() < 0.01);
    }

    #[test]
    fn changes_sound_only_when_on_and_not_flat() {
        assert!(!params(false, 6.0).changes_sound());
        assert!(!params(true, 0.0).changes_sound());
        assert!(params(true, 1.0).changes_sound());
    }
}
