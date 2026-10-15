//! Sample rate conversion, as a stream: push decoded audio in, take
//! resampled audio out, and at the end of a song flush the rest, so the
//! output is exactly as long as the input times the ratio. The
//! resampler's own start-up delay is trimmed, so the first output sample
//! lines up with the first input sample and gapless joins stay gapless.
//!
//! The edges need care. A filter looks at samples on both sides of the
//! one it's working out, and at the start and end of a stream one side is
//! missing. Fed silence there, the first and last few samples come out at
//! about half level: a dip, and a click at a join. So the stream is
//! extended past both edges by an odd reflection of its own first and
//! last samples (the curve continued through the edge point), the
//! filter is run over that, and the extra output is thrown away. Songs
//! that follow on at the same rate don't need even that: the player keeps
//! one stream running across them, which is exactly the same as
//! resampling the whole album in one go.
//!
//! Three qualities, all from rubato:
//! - Fast: cubic polynomial interpolation. Cheap, and no anti-aliasing
//!   filter, so content near the top of the band can fold back.
//! - Balanced: a 128-tap windowed sinc. Clean for music, light on CPU.
//! - Best: rubato's FFT resampler, a long sinc filter run by FFT. The
//!   cleanest of the three, at a fixed ratio between two whole rates,
//!   which is all a music player needs.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, Indexing, PolynomialDegree, Resampler,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Fast,
    #[default]
    Balanced,
    Best,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Fast, Quality::Balanced, Quality::Best];

    pub fn as_str(self) -> &'static str {
        match self {
            Quality::Fast => "fast",
            Quality::Balanced => "balanced",
            Quality::Best => "best",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Fast => "Fast",
            Quality::Balanced => "Balanced",
            Quality::Best => "Best",
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Quality::Fast,
            2 => Quality::Best,
            _ => Quality::Balanced,
        }
    }
}

impl std::str::FromStr for Quality {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Quality::ALL
            .into_iter()
            .find(|q| q.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("unknown resampler quality \"{s}\" (fast, balanced, best)"))
    }
}

const CHUNK: usize = 1024;
/// Input frames of reflection ahead of the stream, at least.
const PRE: usize = 1024;
/// Input frames kept for the reflection past the end.
const EXT: usize = 2048;

/// A running conversion from one rate to another.
pub struct Stream {
    r: Box<dyn Resampler<f64>>,
    ch: usize,
    pub from: u32,
    pub to: u32,
    ratio: f64,
    /// Input not yet a whole chunk.
    pending: Vec<f64>,
    out: Vec<f64>,
    /// Output frames still to drop: the resampler's start-up delay, and
    /// the output of the reflection ahead of the stream.
    skip: usize,
    /// The resampler's delay in output frames, fraction and all.
    delay: f64,
    in_frames: u64,
    out_frames: u64,
    /// The first push primes the edge.
    primed: bool,
    /// The last input frames, for the reflection past the end.
    hist: Vec<f64>,
    /// Input and output frames in the smallest whole ratio (147 to 160
    /// for 44.1 to 48 kHz).
    step: (usize, usize),
}

impl Stream {
    pub fn new(from: u32, to: u32, channels: u16, q: Quality) -> Result<Self, String> {
        let ch = channels.max(1) as usize;
        let ratio = to as f64 / from as f64;
        let r: Box<dyn Resampler<f64>> = match q {
            Quality::Fast => Box::new(
                Async::<f64>::new_poly(
                    ratio,
                    1.0,
                    PolynomialDegree::Cubic,
                    CHUNK,
                    ch,
                    FixedAsync::Input,
                )
                .map_err(|e| e.to_string())?,
            ),
            Quality::Balanced => {
                let p = SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: None,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 256,
                    window: WindowFunction::BlackmanHarris2,
                };
                Box::new(
                    Async::<f64>::new_sinc(ratio, 1.0, &p, CHUNK, ch, FixedAsync::Input)
                        .map_err(|e| e.to_string())?,
                )
            }
            Quality::Best => Box::new(
                Fft::<f64>::new(from as usize, to as usize, CHUNK, ch, FixedSync::Input)
                    .map_err(|e| e.to_string())?,
            ),
        };
        let mut r = r;
        let delay = measure_delay(r.as_mut(), ch);
        let out = vec![0.0; r.output_frames_max() * ch];
        let g = gcd(from as usize, to as usize).max(1);
        Ok(Self {
            r,
            ch,
            from,
            to,
            ratio,
            pending: Vec::with_capacity(CHUNK * ch * 2),
            out,
            skip: 0,
            delay,
            in_frames: 0,
            out_frames: 0,
            primed: false,
            hist: Vec::with_capacity(EXT * ch * 2),
            step: (from as usize / g, to as usize / g),
        })
    }

    /// Run a reflection of the stream's first samples through ahead of
    /// it, and drop what comes out for it and for the filter's delay.
    ///
    /// That delay is rarely a whole number of output frames (278.64 for
    /// the long sinc at 44.1 to 48 kHz), and dropping a rounded number
    /// would shift the whole song by the fraction. So the reflection's
    /// length is picked to make reflection plus delay land on a whole
    /// frame, to within 1/147 of a frame at 44.1 to 48.
    fn prime(&mut self, input: &[f64]) {
        self.primed = true;
        let (si, so) = self.step;
        let frames = input.len() / self.ch;
        let ratio = so as f64 / si as f64;
        let span = si.min(4096);
        let pre = (PRE..PRE + span)
            .min_by(|&a, &b| {
                let off = |p: usize| {
                    let x = p as f64 * ratio + self.delay;
                    (x - x.round()).abs()
                };
                off(a).total_cmp(&off(b))
            })
            .unwrap_or(PRE);
        let ch = self.ch;
        if frames == 0 {
            return;
        }
        for k in (1..=pre).rev() {
            let k = k.min(frames - 1);
            for c in 0..ch {
                self.pending.push(2.0 * input[c] - input[k * ch + c]);
            }
        }
        self.skip = (pre as f64 * ratio + self.delay).round() as usize;
    }

    /// Start over, as after a seek: nothing pending, the delay trimmed
    /// again.
    pub fn reset(&mut self) {
        self.r.reset();
        self.pending.clear();
        self.hist.clear();
        self.skip = 0;
        self.in_frames = 0;
        self.out_frames = 0;
        self.primed = false;
    }

    pub fn channels(&self) -> usize {
        self.ch
    }

    /// Output frames still to come for the input pushed so far.
    pub fn owed(&self) -> u64 {
        self.out_len(self.in_frames).saturating_sub(self.out_frames)
    }

    fn run(&mut self, partial: Option<usize>, out: &mut Vec<f64>) {
        let need = self.r.input_frames_next();
        let ch = self.ch;
        if self.pending.len() < need * ch {
            self.pending.resize(need * ch, 0.0);
        }
        let cap = self.out.len() / ch;
        let input = InterleavedSlice::new(&self.pending[..need * ch], ch, need)
            .expect("input sized to the chunk");
        let mut output = InterleavedSlice::new_mut(&mut self.out[..], ch, cap)
            .expect("output sized to the maximum");
        let idx = partial.map(|n| Indexing::new().partial_len(n));
        let (read, written) = match self
            .r
            .process_into_buffer(&input, &mut output, idx.as_ref())
        {
            Ok(x) => x,
            Err(e) => {
                log::warn!("resampler: {e}");
                (need, 0)
            }
        };
        let consumed = match partial {
            Some(n) => n.min(read),
            None => read,
        };
        self.pending
            .drain(..(consumed * ch).min(self.pending.len()));
        if partial.is_some() {
            self.pending.clear();
        }
        let mut frames = written;
        let mut from = 0;
        if self.skip > 0 {
            let s = self.skip.min(frames);
            self.skip -= s;
            from = s;
            frames -= s;
        }
        out.extend_from_slice(&self.out[from * ch..(from + frames) * ch]);
        self.out_frames += frames as u64;
    }

    /// Resample `input` (interleaved), appending to `out`.
    pub fn push(&mut self, input: &[f64], out: &mut Vec<f64>) {
        if input.is_empty() {
            return;
        }
        if !self.primed {
            self.prime(input);
        }
        self.pending.extend_from_slice(input);
        self.in_frames += (input.len() / self.ch) as u64;
        self.hist.extend_from_slice(input);
        if self.hist.len() > 2 * EXT * self.ch {
            self.hist.drain(..self.hist.len() - EXT * self.ch);
        }
        self.drain(out);
    }

    fn drain(&mut self, out: &mut Vec<f64>) {
        while self.pending.len() >= self.r.input_frames_next() * self.ch {
            self.run(None, out);
        }
    }

    /// End of the song: the rest out, to exactly the right length.
    pub fn finish(&mut self, out: &mut Vec<f64>) {
        let want = (self.in_frames as f64 * self.ratio).round() as u64;
        let ch = self.ch;
        let start = out.len();
        // The reflection past the end, through the filter like the rest.
        let n = self.hist.len() / ch;
        if n >= 2 {
            let last = &self.hist[(n - 1) * ch..n * ch];
            let ext: Vec<f64> = (1..n.min(EXT + 1))
                .flat_map(|k| (0..ch).map(move |c| (k, c)))
                .map(|(k, c)| 2.0 * last[c] - self.hist[(n - 1 - k) * ch + c])
                .collect();
            self.pending.extend_from_slice(&ext);
            self.drain(out);
        }
        let have = self.pending.len() / ch;
        if have > 0 {
            self.run(Some(have), out);
        }
        // The tail is still inside the filter: push silence through.
        let mut guard = 0;
        while self.out_frames < want && guard < 64 {
            self.run(Some(0), out);
            guard += 1;
        }
        if self.out_frames > want {
            let extra = (self.out_frames - want) as usize;
            let keep = (out.len() - start) / ch - extra.min((out.len() - start) / ch);
            out.truncate(start + keep * ch);
            self.out_frames = want;
        }
    }

    /// Output frames for this many input frames.
    pub fn out_len(&self, frames: u64) -> u64 {
        (frames as f64 * self.ratio).round() as u64
    }
}

/// The resampler's delay, in output frames with the fraction, worked out
/// rather than taken from `output_delay`, which rounds it (and for the
/// sinc and polynomial ones is a frame out besides). A slow ramp goes
/// through; every filter here passes a straight line straight, so each
/// output frame says which input position it came from. Two points on the
/// line give the delay with any gain error cancelled.
fn measure_delay(r: &mut dyn Resampler<f64>, ch: usize) -> f64 {
    let ratio = r.resample_ratio();
    let mut out_all: Vec<f64> = Vec::new();
    let mut n0 = 0usize;
    let mut buf = vec![0.0; r.output_frames_max() * ch];
    for _ in 0..8 {
        let need = r.input_frames_next();
        let input: Vec<f64> = (n0..n0 + need)
            .flat_map(|n| std::iter::repeat_n(n as f64 * 1e-4, ch))
            .collect();
        let cap = buf.len() / ch;
        let (Ok(i), Ok(mut o)) = (
            InterleavedSlice::new(&input[..], ch, need),
            InterleavedSlice::new_mut(&mut buf[..], ch, cap),
        ) else {
            break;
        };
        let Ok((_, w)) = r.process_into_buffer(&i, &mut o, None) else {
            break;
        };
        out_all.extend(buf[..w * ch].iter().step_by(ch));
        n0 += need;
    }
    r.reset();
    let n = out_all.len();
    if n < 16 {
        return r.output_delay() as f64;
    }
    let (m1, m2) = (n / 2, n * 3 / 4);
    let slope = (out_all[m2] - out_all[m1]) / (m2 - m1) as f64;
    if slope.abs() < 1e-12 {
        return r.output_delay() as f64;
    }
    // y(m) = slope * (m - delay), with slope = 1e-4 / ratio in an ideal
    // filter.
    let _ = ratio;
    m1 as f64 - out_all[m1] / slope
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize) -> Vec<f64> {
        (0..frames)
            .flat_map(|i| {
                let s = 0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin();
                [s, s]
            })
            .collect()
    }

    /// The frequency of a stereo sine, by counting rising zero crossings.
    fn freq(buf: &[f64], rate: u32) -> f64 {
        let l: Vec<f64> = buf.chunks(2).map(|f| f[0]).collect();
        let n = l.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        n as f64 * rate as f64 / l.len() as f64
    }

    #[test]
    fn every_quality_keeps_pitch_and_length() {
        for q in Quality::ALL {
            let input = sine(44100, 1000.0, 44100 * 2 + 777);
            let mut s = Stream::new(44100, 48000, 2, q).unwrap();
            let mut out = Vec::new();
            // Odd pieces, the way a decoder hands them over.
            for c in input.chunks(2 * 1153) {
                s.push(c, &mut out);
            }
            s.finish(&mut out);
            let want = ((44100 * 2 + 777) as f64 * 48000.0 / 44100.0).round() as usize;
            assert_eq!(out.len() / 2, want, "{q:?}");
            let f = freq(&out[4800 * 2..out.len() - 4800 * 2], 48000);
            assert!((f - 1000.0).abs() < 2.0, "{q:?}: {f}");
            // Lined up: the first output samples are the start of the
            // sine, not silence.
            let early = out[20..200].iter().fold(0.0f64, |m, x| m.max(x.abs()));
            assert!(early > 0.2, "{q:?}: starts silent ({early})");
        }
    }

    #[test]
    fn edges_come_out_at_full_level() {
        // A sine at its peak right at the start: the first output sample
        // should be near the peak, not half of it.
        for q in Quality::ALL {
            let input: Vec<f64> = (0..44100)
                .flat_map(|i| {
                    let s = 0.5 * (2.0 * std::f64::consts::PI * 200.0 * i as f64 / 44100.0).cos();
                    [s, s]
                })
                .collect();
            let mut s = Stream::new(44100, 48000, 2, q).unwrap();
            let mut out = Vec::new();
            s.push(&input, &mut out);
            s.finish(&mut out);
            assert!((out[0] - 0.5).abs() < 0.01, "{q:?} start {}", out[0]);
            let n = out.len() / 2;
            let t = (n - 1) as f64 / 48000.0;
            let end = 0.5 * (2.0 * std::f64::consts::PI * 200.0 * t).cos();
            assert!(
                (out[(n - 1) * 2] - end).abs() < 0.01,
                "{q:?} end {} vs {end}",
                out[(n - 1) * 2]
            );
        }
    }

    #[test]
    fn lined_up_to_a_fraction_of_a_sample() {
        // The output is the input's own curve, sampled on the new grid,
        // with no time shift: checked against the exact sine, every
        // quality, up and down, odd ratios too.
        let f = |t: f64| 0.4 * (2.0 * std::f64::consts::PI * 660.0 * t).sin();
        for (from, to) in [
            (44100u32, 48000u32),
            (48000, 44100),
            (44100, 96000),
            (96000, 44100),
        ] {
            for q in Quality::ALL {
                let input: Vec<f64> = (0..from as usize)
                    .flat_map(|i| {
                        let s = f(i as f64 / from as f64);
                        [s, s]
                    })
                    .collect();
                let mut s = Stream::new(from, to, 2, q).unwrap();
                let mut out = Vec::new();
                for c in input.chunks(4096 * 2) {
                    s.push(c, &mut out);
                }
                s.finish(&mut out);
                let n = out.len() / 2;
                let e = (0..n)
                    .map(|i| (out[i * 2] - f(i as f64 / to as f64)).abs())
                    .fold(0.0, f64::max);
                // A 0.01-sample shift alone would be 3e-4 here.
                assert!(e < 2e-4, "{from}->{to} {q:?}: off by {e}");
            }
        }
    }

    #[test]
    fn better_qualities_keep_out_aliasing() {
        // 96 to 44.1 kHz with a 30 kHz tone, which has no place at 44.1:
        // a proper filter takes it out, interpolation alone folds it down
        // to 14.1 kHz.
        let input: Vec<f64> = (0..96000)
            .flat_map(|i| {
                let s = 0.5 * (2.0 * std::f64::consts::PI * 30000.0 * i as f64 / 96000.0).sin();
                [s, s]
            })
            .collect();
        let rms = |q| {
            let mut s = Stream::new(96000, 44100, 2, q).unwrap();
            let mut out = Vec::new();
            s.push(&input, &mut out);
            s.finish(&mut out);
            let mid = &out[8000..out.len() - 8000];
            (mid.iter().map(|x| x * x).sum::<f64>() / mid.len() as f64).sqrt()
        };
        let (fast, bal, best) = (
            rms(Quality::Fast),
            rms(Quality::Balanced),
            rms(Quality::Best),
        );
        assert!(fast > 0.01, "fast {fast}");
        assert!(bal < 1e-3, "balanced {bal}");
        assert!(best < 1e-4, "best {best}");
    }

    #[test]
    fn level_is_kept() {
        let input = sine(96000, 1000.0, 96000);
        let mut s = Stream::new(96000, 44100, 2, Quality::Balanced).unwrap();
        let mut out = Vec::new();
        s.push(&input, &mut out);
        s.finish(&mut out);
        let mid = &out[10000..out.len() - 10000];
        let peak = mid.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
    }

    #[test]
    fn reset_starts_clean() {
        let mut s = Stream::new(44100, 48000, 2, Quality::Fast).unwrap();
        let mut out = Vec::new();
        s.push(&sine(44100, 500.0, 5000), &mut out);
        s.reset();
        let mut out2 = Vec::new();
        s.push(&sine(44100, 500.0, 44100), &mut out2);
        s.finish(&mut out2);
        assert_eq!(out2.len() / 2, 48000);
    }
}
