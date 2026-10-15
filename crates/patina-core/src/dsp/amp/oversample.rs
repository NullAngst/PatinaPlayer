//! Oversampling for the amp stack. Saturation makes harmonics, and any
//! that land past half the sample rate fold back down as aliasing: harsh,
//! out-of-tune tones that real hardware never makes. Running the devices
//! at 2, 4 or 8 times the rate gives the harmonics room, and the way back
//! down filters them off before they can fold.
//!
//! Both directions use one linear-phase FIR, a Kaiser-windowed sinc with
//! its cutoff at the original Nyquist, 48 taps per phase. Up and down
//! together delay the sound by exactly 48 frames at the original rate,
//! whatever the factor, so changing the factor doesn't move anything.

/// Taps per phase, and so the round trip's delay in frames.
pub const TAPS_PER_PHASE: usize = 48;

pub struct Oversampler {
    n: usize,
    ch: usize,
    /// L = TAPS_PER_PHASE * n + 1 coefficients, DC gain 1.
    h: Vec<f64>,
    /// Polyphase view of `h` for the way up: phase p, tap q at
    /// `p * (K + 1) + q`, scaled by n. Missing taps are zero.
    up: Vec<f64>,
    /// Per channel: the last K + 1 input samples, twice over, so a
    /// window of them is always contiguous.
    up_hist: Vec<f64>,
    up_pos: usize,
    /// Per channel: the last L + n high-rate samples, twice over.
    down_hist: Vec<f64>,
    /// Ring length for `down_hist`.
    ring: usize,
    down_pos: usize,
}

/// A dot product with four running sums, which the compiler can keep in
/// vector registers (a single sum can't be reordered, so it can't).
#[inline]
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut s = [0.0f64; 4];
    let ca = a.chunks_exact(4);
    let cb = b.chunks_exact(4);
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        s[0] += x[0] * y[0];
        s[1] += x[1] * y[1];
        s[2] += x[2] * y[2];
        s[3] += x[3] * y[3];
    }
    let mut t = (s[0] + s[1]) + (s[2] + s[3]);
    for (x, y) in ra.iter().zip(rb) {
        t += x * y;
    }
    t
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-14 * sum {
            break;
        }
    }
    sum
}

impl Oversampler {
    /// `n` is 1, 2, 4 or 8. At 1 it's a plain delay of the same length,
    /// so every factor delays the same.
    pub fn new(n: usize, ch: usize) -> Self {
        let n = n.max(1);
        let ch = ch.max(1);
        let k = TAPS_PER_PHASE;
        let l = k * n + 1;
        let mid = (l - 1) as f64 / 2.0;
        // Cutoff at the original Nyquist: 0.5 / n of the high rate.
        let fc = 0.5 / n as f64;
        let beta = 7.0;
        let i0b = bessel_i0(beta);
        let mut h: Vec<f64> = (0..l)
            .map(|i| {
                let t = i as f64 - mid;
                let sinc = if t.abs() < 1e-12 {
                    2.0 * fc
                } else {
                    (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t)
                };
                let r = t / mid.max(1.0);
                let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
                sinc * w
            })
            .collect();
        let sum: f64 = h.iter().sum();
        for c in &mut h {
            *c /= sum;
        }
        let mut up = vec![0.0; n * (k + 1)];
        for p in 0..n {
            for q in 0..=k {
                let i = p + q * n;
                if i < l {
                    up[p * (k + 1) + q] = h[i] * n as f64;
                }
            }
        }
        // At factor 1 the way down is a plain delay line of K frames.
        let ring = if n == 1 { k + 1 } else { l + n };
        Self {
            n,
            ch,
            h,
            up,
            up_hist: vec![0.0; ch * 2 * (k + 1)],
            up_pos: 0,
            down_hist: vec![0.0; ch * 2 * ring],
            down_pos: 0,
            ring,
        }
    }

    pub fn factor(&self) -> usize {
        self.n
    }

    pub fn reset(&mut self) {
        self.up_hist.fill(0.0);
        self.down_hist.fill(0.0);
        self.up_pos = 0;
        self.down_pos = 0;
    }

    /// One input frame up to `n` high-rate frames, written to `out` as
    /// `out[p * ch + c]`. At factor 1 the delay all happens on the way
    /// down, so this is a copy.
    pub fn up(&mut self, frame: &[f64], out: &mut [f64]) {
        let (n, ch, k) = (self.n, self.ch, TAPS_PER_PHASE);
        if n == 1 {
            out[..ch].copy_from_slice(&frame[..ch]);
            return;
        }
        let w = k + 1;
        // Newest sample at window index 0 when read backwards: store at
        // pos and pos + w, then read pos..pos+w as newest-first.
        self.up_pos = if self.up_pos == 0 {
            w - 1
        } else {
            self.up_pos - 1
        };
        for c in 0..ch {
            let base = c * 2 * w;
            self.up_hist[base + self.up_pos] = frame[c];
            self.up_hist[base + self.up_pos + w] = frame[c];
            let x = &self.up_hist[base + self.up_pos..base + self.up_pos + w];
            for p in 0..n {
                out[p * ch + c] = dot(&self.up[p * w..(p + 1) * w], x);
            }
        }
    }

    /// `n` processed high-rate frames (as [`up`](Self::up) laid them out)
    /// back down to one frame.
    pub fn down(&mut self, hi: &[f64], frame: &mut [f64]) {
        let (n, ch, r) = (self.n, self.ch, self.ring);
        // Push the n samples oldest first, so the newest (p = n - 1) ends
        // at window index 0.
        for p in 0..n {
            self.down_pos = if self.down_pos == 0 {
                r - 1
            } else {
                self.down_pos - 1
            };
            for c in 0..ch {
                let base = c * 2 * r;
                self.down_hist[base + self.down_pos] = hi[p * ch + c];
                self.down_hist[base + self.down_pos + r] = hi[p * ch + c];
            }
        }
        if n == 1 {
            // The sample from K frames ago.
            for (c, out) in frame.iter_mut().enumerate().take(ch) {
                *out = self.down_hist[c * 2 * r + self.down_pos + TAPS_PER_PHASE];
            }
            return;
        }
        // The output lines up with this frame's p = 0 sample, n - 1 places
        // back from the newest. The ring is L + n long, so the whole
        // window (L + n - 1 samples from the newest) is contiguous.
        let off = n - 1;
        for (c, out) in frame.iter_mut().enumerate().take(ch) {
            let base = c * 2 * r + self.down_pos + off;
            *out = dot(&self.h, &self.down_hist[base..base + self.h.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(n: usize, input: &[f64]) -> Vec<f64> {
        let mut o = Oversampler::new(n, 1);
        let mut hi = vec![0.0; n];
        let mut out = Vec::new();
        let mut f = [0.0];
        for &x in input {
            o.up(&[x], &mut hi);
            o.down(&hi, &mut f);
            out.push(f[0]);
        }
        out
    }

    #[test]
    fn round_trip_is_a_clean_delay() {
        // A 1 kHz and a 15 kHz tone at 44.1 kHz come back as themselves,
        // exactly TAPS_PER_PHASE frames late, at every factor.
        for n in [1, 2, 4, 8] {
            for freq in [1000.0, 15000.0] {
                let x: Vec<f64> = (0..8000)
                    .map(|i| 0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / 44100.0).sin())
                    .collect();
                let y = run(n, &x);
                let d = TAPS_PER_PHASE;
                let err = (1000..7000)
                    .map(|i| (y[i] - x[i - d]).abs())
                    .fold(0.0, f64::max);
                assert!(err < 2e-3, "{n}x {freq} Hz: {err}");
            }
        }
    }

    #[test]
    fn images_are_filtered() {
        // Upsampled 4x, a 10 kHz tone at 44.1 kHz has its first image at
        // 34.1 kHz; the filter takes it below -60 dB.
        let n = 4;
        let mut o = Oversampler::new(n, 1);
        let mut hi = vec![0.0; n];
        let mut all = Vec::new();
        for i in 0..20000 {
            let x = (2.0 * std::f64::consts::PI * 10000.0 * i as f64 / 44100.0).sin();
            o.up(&[x], &mut hi);
            all.extend_from_slice(&hi);
        }
        let fs = 44100.0 * n as f64;
        let mag = |f: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (j, s) in all[8000..].iter().enumerate() {
                let a = 2.0 * std::f64::consts::PI * f * j as f64 / fs;
                re += s * a.cos();
                im += s * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        let ratio = mag(34100.0) / mag(10000.0);
        assert!(20.0 * ratio.log10() < -60.0, "{}", 20.0 * ratio.log10());
    }
}
