//! The visualizer's numbers, from the samples [`crate::audio::scope`]
//! keeps: a spectrum in log-spaced bars, and a short stretch of the wave.
//! Drawing them is the window's job.
//!
//! Both are leveled for looks, not measured: the bars on a fixed dB range,
//! the wave by its recent peak, so a quiet passage at low volume still
//! moves.
//!
//! Each frame reads the samples being heard at that moment, worked out
//! from when the renderer's blocks came, rather than the newest ones: the
//! renderer writes a whole output buffer at once, and with a big
//! `buffer_ms` the newest samples only changed a few times a second, which
//! made the picture jump. How fast the bars fall goes by time too, so it
//! looks the same at any frame rate.

use std::sync::Arc;
use std::time::Instant;

use realfft::{RealFftPlanner, RealToComplex};

use crate::audio::scope::{self, Scope};

/// FFT size: about 43 ms at 48 kHz, fine enough for the low bars.
const N: usize = 2048;
/// Points in the wave.
pub const WAVE_POINTS: usize = 160;
/// The bars cover this range, in Hz.
const LOW: f32 = 40.0;
const HIGH: f32 = 16_000.0;
/// dB below full scale that reads as an empty bar.
const FLOOR_DB: f32 = 66.0;

/// What one frame draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    /// 0 to 1, low to high.
    pub bars: Vec<f32>,
    /// -1 to 1, oldest first.
    pub wave: Vec<f32>,
}

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    samples: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
    bars: Vec<f32>,
    /// The wave's recent peak, falling slowly.
    peak: f32,
    /// The sample being heard as of the last frame, and when that was.
    head: Option<(f64, Instant)>,
    /// When the last frame was drawn, and how long ago that was in 30ths
    /// of a second: the falls are tuned per 30th.
    last: Option<Instant>,
    frames: f32,
}

/// The frame rate the bars' and peak's falls are tuned for.
const TUNED_FPS: f32 = 30.0;

impl Analyzer {
    pub fn new(bars: usize) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
            .collect();
        Self {
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            samples: vec![0.0; N],
            bars: vec![0.0; bars.max(1)],
            peak: 0.05,
            head: None,
            last: None,
            frames: 1.0,
        }
    }

    /// Read the scope and work out the next frame. `playing` false lets
    /// the bars fall away.
    pub fn frame(&mut self, scope: &Scope, playing: bool) -> Frame {
        let now = Instant::now();
        self.frames = self
            .last
            .map(|t| (now.duration_since(t).as_secs_f32() * TUNED_FPS).clamp(0.1, 10.0))
            .unwrap_or(1.0);
        self.last = Some(now);
        let c = scope.clock();
        let rate = c.rate;
        if !playing || rate == 0 {
            self.samples.iter_mut().for_each(|s| *s = 0.0);
            self.head = None;
        } else {
            let head = self.heard(c, now);
            self.head = Some((head, now));
            scope.read_to(head as usize, &mut self.samples);
        }
        // No rate yet: the renderer hasn't sent anything since the scope
        // came on (it's all silence then anyway).
        self.analyze(if rate == 0 { 48_000 } else { rate })
    }

    /// The sample being heard now. A block written at `at` starts playing
    /// about a block later, once the one before it is out, so what's heard
    /// trails the newest sample by two blocks, less the time since. The
    /// frame-to-frame estimate runs on its own clock and leans toward that,
    /// so it glides between blocks; it jumps only when far off (a seek, a
    /// new song, a pause).
    fn heard(&self, c: scope::Clock, now: Instant) -> f64 {
        let rate = f64::from(c.rate);
        let pos = c.pos as f64;
        let since = scope::now_nanos().saturating_sub(c.at) as f64 / 1e9;
        let target = (pos - 2.0 * c.block as f64 + since * rate).min(pos);
        let head = match self.head {
            Some((h, t)) => {
                let moved = h + now.duration_since(t).as_secs_f64() * rate;
                if (moved - target).abs() > rate * 0.15 {
                    target
                } else {
                    moved + (target - moved) * 0.1
                }
            }
            None => target,
        };
        // Never past what's written, nor further back than is kept.
        head.min(pos).max(pos - (scope::LEN - N) as f64).max(0.0)
    }

    /// The frame for whatever's in `samples`, at `rate`.
    fn analyze(&mut self, rate: u32) -> Frame {
        for ((i, s), w) in self.input.iter_mut().zip(&self.samples).zip(&self.window) {
            *i = s * w;
        }
        let _ =
            self.fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch);
        let rate = rate.max(8_000);
        let bin_hz = rate as f32 / N as f32;
        let n = self.bars.len();
        let high = HIGH.min(rate as f32 / 2.0);
        // The last bin a band can start at, so every band has at least one.
        let last = self.spectrum.len() - 1;
        // A Hann window halves the amplitude; this makes a full-scale sine
        // read as 0 dB.
        let scale = 4.0 / N as f32;
        for (b, bar) in self.bars.iter_mut().enumerate() {
            let f0 = LOW * (high / LOW).powf(b as f32 / n as f32);
            let f1 = LOW * (high / LOW).powf((b + 1) as f32 / n as f32);
            let lo = ((f0 / bin_hz) as usize).clamp(1, last);
            let hi = ((f1 / bin_hz).ceil() as usize).clamp(lo + 1, last + 1);
            let mag = self.spectrum[lo..hi]
                .iter()
                .map(|c| c.norm())
                .fold(0.0f32, f32::max)
                * scale;
            // Highs carry less energy: tilt them up a little, as most
            // analyzers do, so the right side isn't always empty.
            let tilt = 3.0 * (b as f32 / n as f32);
            let db = 20.0 * mag.max(1e-9).log10() + tilt;
            let v = ((db + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0);
            // Up at once, down gently: by the same amount each 30th of a
            // second, whatever the frame rate.
            let keep = 0.82f32.powf(self.frames);
            *bar = if v > *bar {
                v
            } else {
                *bar * keep + v * (1.0 - keep)
            };
        }

        // The wave: the newest stretch, leveled by its recent peak.
        let span = N / 4;
        let tail = &self.samples[N - span..];
        let peak = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.peak = if peak > self.peak {
            peak
        } else {
            (self.peak * 0.97f32.powf(self.frames)).max(peak).max(0.05)
        };
        let step = span as f32 / WAVE_POINTS as f32;
        let wave = (0..WAVE_POINTS)
            .map(|i| (tail[(i as f32 * step) as usize] / self.peak).clamp(-1.0, 1.0))
            .collect();
        Frame {
            bars: self.bars.clone(),
            wave,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f32, rate: u32, amp: f32) -> Vec<f32> {
        (0..N)
            .map(|i| amp * (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn a_tone_lights_its_own_bar() {
        let mut a = Analyzer::new(32);
        a.samples = sine(1000.0, 48_000, 0.5);
        let f = a.analyze(48_000);
        let top = f
            .bars
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .unwrap()
            .0;
        // 1 kHz on a 40 Hz to 16 kHz log scale sits a little past half way.
        let want = (32.0 * (1000f32 / 40.0).ln() / (16_000f32 / 40.0).ln()) as usize;
        assert!(top.abs_diff(want) <= 1, "bar {top}, wanted about {want}");
        assert!(f.bars[top] > 0.8);
        assert!(f.bars[2] < 0.3, "a bass bar lit: {}", f.bars[2]);
    }

    #[test]
    fn silence_is_flat_and_the_bars_fall() {
        let mut a = Analyzer::new(16);
        a.samples = sine(200.0, 44_100, 0.8);
        let loud = a.analyze(44_100);
        a.samples = vec![0.0; N];
        let next = a.analyze(44_100);
        let max = |f: &Frame| f.bars.iter().cloned().fold(0.0, f32::max);
        assert!(max(&next) < max(&loud), "the bars didn't fall");
        assert!(next.wave.iter().all(|&s| s == 0.0));
        for _ in 0..60 {
            a.analyze(44_100);
        }
        assert!(max(&a.analyze(44_100)) < 0.01);
    }

    #[test]
    fn any_rate_or_none_at_all_is_drawn() {
        // The scope just came on and nothing has played: rate 0. This
        // panicked in 2026.11.13's build.
        let s = Scope::default();
        s.set_on(true);
        let mut a = Analyzer::new(40);
        let f = a.frame(&s, true);
        assert_eq!(f.bars.len(), 40);
        for rate in [1, 100, 8_000, 22_050, 44_100, 192_000, 384_000] {
            a.samples = sine(440.0, rate.max(8_000), 0.5);
            let f = a.analyze(rate);
            assert!(f.bars.iter().all(|b| (0.0..=1.0).contains(b)), "{rate}");
        }
    }

    #[test]
    fn a_quiet_wave_is_leveled_up() {
        let mut a = Analyzer::new(8);
        a.samples = sine(440.0, 48_000, 0.1);
        let f = a.analyze(48_000);
        assert_eq!(f.wave.len(), WAVE_POINTS);
        let peak = f.wave.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.9, "peak {peak}");
    }
}
