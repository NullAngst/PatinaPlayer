//! The true-peak limiter, last before the output. It looks ahead in the
//! ring (never adding delay of its own), finds where the signal would go
//! over the ceiling, inter-sample peaks included, and brings the gain
//! down along a short ramp that arrives exactly when the peak does. Then
//! it lets go over about a tenth of a second.
//!
//! Below the ceiling it's exactly unity: every sample passes bit for bit,
//! which the gapless tests check with the limiter in the chain.
//!
//! Inter-sample peaks are found by 4x oversampling with a windowed-sinc
//! interpolator, the same idea as the ITU-R BS.1770 true-peak meter.

/// How far ahead it looks, and so how long the attack ramp is.
pub const LOOKAHEAD_MS: f64 = 2.0;
/// Frames of context the interpolator needs either side of a frame.
pub const TAPS_HALF: usize = 4;
const TAPS: usize = 2 * TAPS_HALF;
const RELEASE_MS: f64 = 100.0;
/// Most lookahead the buffers are sized for (at 768 kHz).
pub const MAX_LOOKAHEAD: usize = 1536;

pub struct Limiter {
    ch: usize,
    la: usize,
    release: f64,
    /// The gain on the last sample.
    gain: f64,
    /// The last frames of input before this block, for the interpolator.
    hist: Vec<f64>,
    /// History + block + lookahead, gathered.
    x: Vec<f64>,
    /// Per frame of `x` (past the history): the gain that frame needs.
    need: Vec<f64>,
    /// Per output frame: the most the gain may be.
    cap: Vec<f64>,
    /// Interpolation filters for the points a quarter, a half and three
    /// quarters of the way to the next frame.
    phases: [[f64; TAPS]; 3],
}

impl Limiter {
    pub fn new(ch: usize, rate: u32, max_frames: usize) -> Self {
        let ch = ch.max(1);
        let la = ((rate as f64 * LOOKAHEAD_MS / 1000.0).round() as usize).clamp(1, MAX_LOOKAHEAD);
        let span = TAPS_HALF + max_frames + la + TAPS_HALF;
        let mut phases = [[0.0; TAPS]; 3];
        for (p, row) in phases.iter_mut().enumerate() {
            let frac = (p + 1) as f64 / 4.0;
            let mut sum = 0.0;
            for (k, c) in row.iter_mut().enumerate() {
                // Tap k sits at frame offset k - (TAPS_HALF - 1).
                let t = k as f64 - (TAPS_HALF as f64 - 1.0) - frac;
                let sinc = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
                };
                // Hann window over the taps.
                let w = 0.5 + 0.5 * (std::f64::consts::PI * t / (TAPS_HALF as f64 + 1.0)).cos();
                *c = sinc * w;
                sum += *c;
            }
            for c in row.iter_mut() {
                *c /= sum;
            }
        }
        Self {
            ch,
            la,
            release: 1.0 - (-1.0 / (rate as f64 * RELEASE_MS / 1000.0)).exp(),
            gain: 1.0,
            hist: vec![0.0; TAPS_HALF * ch],
            x: vec![0.0; span * ch],
            need: vec![1.0; span],
            cap: vec![1.0; max_frames],
            phases,
        }
    }

    /// Frames past the block it wants to see.
    pub fn lookahead(&self) -> usize {
        self.la + TAPS_HALF
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.hist.fill(0.0);
    }

    /// Limit `buf` (interleaved, whole frames) to `ceiling` (linear),
    /// knowing that `ahead` comes next. Returns the deepest gain reduction
    /// in the block, in dB (0 when nothing was touched).
    pub fn process(&mut self, buf: &mut [f64], ahead: &[f64], ceiling: f64) -> f64 {
        let ch = self.ch;
        let frames = buf.len() / ch;
        if frames == 0 {
            return 0.0;
        }
        let la = self.la;
        let want_ahead = (la + TAPS_HALF) * ch;
        // Gather: history, the block, what comes next (silence past the end).
        let h = TAPS_HALF * ch;
        self.x[..h].copy_from_slice(&self.hist);
        self.x[h..h + buf.len()].copy_from_slice(buf);
        let got = ahead.len().min(want_ahead);
        self.x[h + buf.len()..h + buf.len() + got].copy_from_slice(&ahead[..got]);
        self.x[h + buf.len() + got..h + buf.len() + want_ahead].fill(0.0);
        // Keep this block's tail for the next call's interpolation.
        let tail = buf.len().saturating_sub(h);
        if buf.len() >= h {
            self.hist.copy_from_slice(&buf[tail..]);
        } else {
            self.hist.copy_within(buf.len().., 0);
            let keep = h - buf.len();
            self.hist[keep..].copy_from_slice(buf);
        }

        // What each frame, block and lookahead, needs.
        let scan = frames + la;
        let mut any = false;
        for k in 0..scan {
            let base = (TAPS_HALF + k) * ch;
            let mut p = 0.0f64;
            for c in 0..ch {
                let s = self.x[base + c].abs();
                p = p.max(s);
            }
            if p > ceiling * 0.5 {
                // Only loud spots can have inter-sample peaks over the
                // ceiling; skip the interpolation for the rest.
                for c in 0..ch {
                    for ph in &self.phases {
                        let mut acc = 0.0;
                        for (t, coef) in ph.iter().enumerate() {
                            let f = TAPS_HALF + k + t + 1 - TAPS_HALF;
                            acc += coef * self.x[f * ch + c];
                        }
                        p = p.max(acc.abs());
                    }
                }
            }
            self.need[k] = if p > ceiling {
                any = true;
                ceiling / p
            } else {
                1.0
            };
        }

        // The most the gain may be at each output frame: every coming peak
        // pulls it down along a ramp that lands on its need right on time.
        let cap = &mut self.cap[..frames];
        cap.fill(1.0);
        if any {
            for j in 0..scan {
                let g = self.need[j];
                if g >= 1.0 {
                    continue;
                }
                let from = j.saturating_sub(la);
                let to = j.min(frames - 1);
                for (i, c) in cap.iter_mut().enumerate().take(to + 1).skip(from) {
                    let r = g + (1.0 - g) * (j - i) as f64 / la as f64;
                    if r < *c {
                        *c = r;
                    }
                }
            }
        }

        let mut lowest = 1.0f64;
        for (f, c) in buf.chunks_exact_mut(ch).zip(cap.iter()) {
            if *c < self.gain {
                self.gain = *c;
            } else if self.gain < 1.0 {
                self.gain += (*c - self.gain) * self.release;
                if self.gain > 0.9999 && *c >= 1.0 {
                    // Within a thousandth of a dB: back to exact unity, so
                    // quiet passages after a peak are bit for bit again.
                    self.gain = 1.0;
                }
            }
            if self.gain < 1.0 {
                lowest = lowest.min(self.gain);
                for s in f {
                    *s *= self.gain;
                }
            }
        }
        if lowest < 1.0 {
            -20.0 * lowest.log10()
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f64, freq: f64) -> Vec<f64> {
        (0..n)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / 48000.0).sin();
                [s, s]
            })
            .collect()
    }

    /// Run a whole signal through in blocks, looking ahead like the
    /// renderer does.
    fn run(l: &mut Limiter, sig: &[f64], block: usize, ceiling: f64) -> (Vec<f64>, f64) {
        let mut out = Vec::new();
        let mut gr = 0.0f64;
        let mut at = 0;
        while at < sig.len() {
            let end = (at + block * 2).min(sig.len());
            let mut b = sig[at..end].to_vec();
            let ahead_end = (end + l.lookahead() * 2).min(sig.len());
            gr = gr.max(l.process(&mut b, &sig[end..ahead_end], ceiling));
            out.extend(b);
            at = end;
        }
        (out, gr)
    }

    #[test]
    fn under_the_ceiling_is_bit_exact() {
        let mut l = Limiter::new(2, 48000, 4096);
        let sig = tone(48000, 0.8, 997.0);
        let (out, gr) = run(&mut l, &sig, 1000, 0.891);
        assert_eq!(out, sig);
        assert_eq!(gr, 0.0);
    }

    #[test]
    fn peaks_stay_under_the_ceiling() {
        let mut l = Limiter::new(2, 48000, 4096);
        // 6 dB over a -1 dB ceiling.
        let sig = tone(48000, 1.78, 997.0);
        let ceiling = 10f64.powf(-1.0 / 20.0);
        let (out, gr) = run(&mut l, &sig, 777, ceiling);
        let peak = out.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(peak <= ceiling + 1e-9, "{peak}");
        assert!(gr > 5.0 && gr < 8.0, "{gr}");
    }

    #[test]
    fn inter_sample_peaks_are_caught() {
        // A sine at a quarter of the sample rate, phased so every sample
        // lands at 0.707 of the true peak: the samples all sit under the
        // ceiling while the wave between them doesn't.
        let mut l = Limiter::new(1, 48000, 4096);
        let sig: Vec<f64> = (0..48000)
            .map(|i| {
                1.2 * (std::f64::consts::PI / 2.0 * i as f64 + std::f64::consts::PI / 4.0).sin()
            })
            .collect();
        let sample_peak = sig.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(sample_peak < 0.9);
        let mut out = Vec::new();
        for c in sig.chunks(1000) {
            let mut b = c.to_vec();
            l.process(&mut b, &[], 0.9);
            out.extend(b);
        }
        // True peak of the output: about 1.2 x gain. It's been brought
        // down to the ceiling.
        let s = out[40000..].iter().fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(
            s * 2f64.sqrt() <= 0.9 * 1.01,
            "true peak {}",
            s * 2f64.sqrt()
        );
    }

    #[test]
    fn lets_go_after_the_peak() {
        let mut l = Limiter::new(2, 48000, 4096);
        let mut sig = tone(4800, 1.5, 997.0);
        sig.extend(tone(48000, 0.3, 997.0));
        let (out, _) = run(&mut l, &sig, 512, 0.9);
        // A second after the loud part, it's back to unity.
        let a = &out[out.len() - 2000..];
        let b = &sig[sig.len() - 2000..];
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x, y);
        }
    }
}
