//! The real-time stages of the chain: the EQ, the amp stack, the channel
//! tools and the true-peak limiter.
//! Both run inside the renderer on the output thread, so nothing here
//! allocates after construction, locks, or waits.

pub mod amp;
pub mod biquad;
pub mod channel;
pub mod limiter;
pub mod resample;

use biquad::{Band, Coeffs, State};

/// Graphic EQ tops out at 31 bands, parametric at 20.
pub const MAX_BANDS: usize = 31;
/// Bypass and on glide over this long, so A/B doesn't click.
const MIX_MS: f64 = 20.0;
/// A change to a band glides over this long, so dragging a handle fast
/// doesn't step the filters (the "zipper" noise).
const GLIDE_MS: f64 = 30.0;
/// Coefficients move in steps this many frames apart while gliding.
const GLIDE_CHUNK: usize = 16;

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
    /// What the filters run now; they walk toward `goal` while gliding.
    coeffs: [Coeffs; MAX_BANDS],
    goal: [Coeffs; MAX_BANDS],
    goal_preamp: f64,
    /// Frames of glide left.
    glide_left: usize,
    glide_len: usize,
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
            goal: [Coeffs::IDENTITY; MAX_BANDS],
            goal_preamp: 1.0,
            glide_left: 0,
            glide_len: (rate as f64 * GLIDE_MS / 1000.0).max(1.0) as usize,
            state: vec![State::default(); MAX_BANDS * ch.max(1)],
            look: vec![State::default(); MAX_BANDS * ch.max(1)],
            preamp: 1.0,
            mix: 0.0,
            target: 0.0,
            mix_step: 1.0 / (rate as f64 * MIX_MS / 1000.0).max(1.0),
            dry: vec![0.0; max_samples],
        }
    }

    /// New settings. The filters keep their memory and glide from where
    /// they are to the new curve, so a band being dragged moves smoothly
    /// instead of restarting or stepping.
    pub fn set(&mut self, p: &EqParams) {
        let n = p.n.min(MAX_BANDS);
        for b in 0..n {
            self.goal[b] = Coeffs::design(&p.bands[b], self.rate);
        }
        for b in n..MAX_BANDS {
            self.goal[b] = Coeffs::IDENTITY;
        }
        // Bands joining start flat and glide in, from clean memory.
        for b in self.n..n {
            self.coeffs[b] = Coeffs::IDENTITY;
            for s in &mut self.state[b * self.ch..(b + 1) * self.ch] {
                *s = State::default();
            }
        }
        // Bands leaving glide to flat, then drop out once there.
        self.n = self.n.max(n);
        self.goal_preamp = 10f64.powf(p.preamp_db / 20.0);
        self.target = if p.active { 1.0 } else { 0.0 };
        if self.mix <= 0.0 && self.target <= 0.0 {
            // Bypassed and silent: nothing to glide from.
            self.snap();
        } else {
            self.glide_left = self.glide_len;
        }
    }

    /// Jump straight to the goal.
    fn snap(&mut self) {
        self.coeffs = self.goal;
        self.preamp = self.goal_preamp;
        self.glide_left = 0;
        self.trim();
    }

    /// Stop running trailing bands that have reached flat.
    fn trim(&mut self) {
        while self.n > 0 && self.goal[self.n - 1] == Coeffs::IDENTITY {
            self.n -= 1;
        }
    }

    /// Forget the filter memory: after a seek or a stop.
    pub fn reset(&mut self) {
        for s in &mut self.state {
            *s = State::default();
        }
        self.mix = self.target;
        self.snap();
    }

    /// One glide step covering `frames` frames.
    fn glide(&mut self, frames: usize) {
        if self.glide_left == 0 {
            return;
        }
        let t = (frames as f64 / self.glide_left as f64).min(1.0);
        for b in 0..self.n {
            let (c, g) = (&mut self.coeffs[b], &self.goal[b]);
            c.b0 += (g.b0 - c.b0) * t;
            c.b1 += (g.b1 - c.b1) * t;
            c.b2 += (g.b2 - c.b2) * t;
            c.a1 += (g.a1 - c.a1) * t;
            c.a2 += (g.a2 - c.a2) * t;
        }
        self.preamp += (self.goal_preamp - self.preamp) * t;
        self.glide_left = self.glide_left.saturating_sub(frames);
        if self.glide_left == 0 {
            self.snap();
        }
    }

    /// The filters over `buf`, gliding the coefficients as they go.
    fn run_wet(&mut self, buf: &mut [f64]) {
        let ch = self.ch;
        if self.glide_left == 0 {
            Self::wet(
                &self.coeffs[..self.n],
                &mut self.state,
                ch,
                self.preamp,
                buf,
            );
            return;
        }
        for chunk in buf.chunks_mut(GLIDE_CHUNK * ch) {
            self.glide(chunk.len() / ch);
            Self::wet(
                &self.coeffs[..self.n],
                &mut self.state,
                ch,
                self.preamp,
                chunk,
            );
        }
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
            self.run_wet(buf);
            return;
        }
        // Gliding between on and bypassed: blend the two.
        let mut dry = std::mem::take(&mut self.dry);
        dry[..buf.len()].copy_from_slice(buf);
        self.run_wet(buf);
        let len = buf.len();
        for (wf, df) in buf.chunks_exact_mut(ch).zip(dry[..len].chunks_exact(ch)) {
            if self.mix < self.target {
                self.mix = (self.mix + self.mix_step).min(self.target);
            } else if self.mix > self.target {
                self.mix = (self.mix - self.mix_step).max(self.target);
            }
            for (w, d) in wf.iter_mut().zip(df) {
                *w = *d + (*w - *d) * self.mix;
            }
        }
        self.dry = dry;
        if self.mix <= 0.0 {
            // Fully bypassed now: start clean next time.
            for s in &mut self.state {
                *s = State::default();
            }
            self.snap();
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

    /// Dragging a band in many small steps, one per block, comes out as
    /// smooth as one move: no step between blocks bigger than the signal's
    /// own movement allows.
    #[test]
    fn fast_drags_glide_instead_of_stepping() {
        let rate = 48000;
        let mut eq = EqRt::new(2, rate, 1 << 16);
        eq.set(&params(true, 0.0));
        eq.reset();
        // A low tone, so any step stands out against the slow waveform.
        let n = 48000;
        let mut b: Vec<f64> = (0..n)
            .flat_map(|i| {
                let s = 0.25 * (2.0 * std::f64::consts::PI * 100.0 * i as f64 / rate as f64).sin();
                [s, s]
            })
            .collect();
        let block = 256;
        for (k, chunk) in b.chunks_mut(block * 2).enumerate() {
            // A band swept at 100 Hz, +12 dB and back, every block.
            let g = 12.0 * ((k as f64 * 0.37).sin()).abs();
            let mut p = params(true, g);
            p.bands[0].freq = 100.0;
            eq.set(&p);
            eq.process(chunk);
        }
        // Largest sample-to-sample jump. A 100 Hz sine at up to 4x
        // amplitude moves at most about 0.013 per sample at 48 kHz.
        let jump = b
            .chunks_exact(2)
            .zip(b.chunks_exact(2).skip(1))
            .map(|(a, c)| (c[0] - a[0]).abs())
            .fold(0.0f64, f64::max);
        assert!(jump < 0.03, "step of {jump}");
    }

    #[test]
    fn a_change_lands_exactly_after_the_glide() {
        let mut eq = EqRt::new(2, 48000, 1 << 16);
        eq.set(&params(true, 0.0));
        eq.reset();
        eq.set(&params(true, 6.0206));
        let mut b = sine(24000);
        eq.process(&mut b);
        let tail = b[b.len() - 2000..]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()));
        assert!((tail - 0.5).abs() < 0.002, "{tail}");
        assert_eq!(eq.coeffs[0], eq.goal[0]);
    }

    #[test]
    fn changes_sound_only_when_on_and_not_flat() {
        assert!(!params(false, 6.0).changes_sound());
        assert!(!params(true, 0.0).changes_sound());
        assert!(params(true, 1.0).changes_sound());
    }
}
