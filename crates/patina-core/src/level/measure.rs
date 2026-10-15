//! Measuring a song: decode it start to end and run it through an EBU R128
//! meter for integrated loudness and true peak, the figures ReplayGain 2
//! is made from.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ebur128::{Channel, EbuR128, Mode};

use super::{Gains, Origin, REFERENCE_LUFS};
use crate::audio::decoder::{OpenOptions, Source};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// Integrated loudness, LUFS.
    pub lufs: f64,
    /// True peak, linear (1.0 is full scale; can go over).
    pub peak: f64,
    pub seconds: f64,
}

impl Measurement {
    /// As a ReplayGain 2 track gain.
    pub fn gain(&self) -> f64 {
        REFERENCE_LUFS - self.lufs
    }

    pub fn gains(&self) -> Gains {
        Gains {
            track_gain: Some(self.gain()),
            track_peak: Some(self.peak),
            track_from: Some(Origin::Measured),
            ..Default::default()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MeasureError {
    #[error("{0}")]
    Decode(String),
    #[error("stopped")]
    Stopped,
    #[error("no sound to measure")]
    Silent,
}

/// Decode a file and measure it. `stop` ends it early.
pub fn measure_file(path: &Path, stop: &Arc<AtomicBool>) -> Result<Measurement, MeasureError> {
    let opts = OpenOptions {
        abort: Some(stop.clone()),
        ..Default::default()
    };
    let mut src = Source::open_with(&path.to_string_lossy(), opts)
        .map_err(|e| MeasureError::Decode(e.to_string()))?;
    let mut meter = Meter::new(src.format().channels, src.format().rate)?;
    let mut buf = Vec::with_capacity(32768);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(MeasureError::Stopped);
        }
        buf.clear();
        let n = src
            .decode_into(&mut buf)
            .map_err(|e| MeasureError::Decode(e.to_string()))?;
        if n == 0 {
            break;
        }
        if src.take_format_change() {
            let f = src.format();
            meter.fold();
            meter.restart(f.channels, f.rate)?;
        }
        meter.add(&buf)?;
    }
    meter.finish()
}

/// A meter that survives a stream changing format partway (rare, but some
/// files do), by keeping each part's state for the final figure.
pub struct Meter {
    cur: EbuR128,
    done: Vec<EbuR128>,
    frames: u64,
    rate: u32,
    seconds: f64,
}

impl Meter {
    pub fn new(channels: u16, rate: u32) -> Result<Self, MeasureError> {
        Ok(Self {
            cur: Self::meter(channels, rate)?,
            done: Vec::new(),
            frames: 0,
            rate,
            seconds: 0.0,
        })
    }

    fn meter(channels: u16, rate: u32) -> Result<EbuR128, MeasureError> {
        let mut m = EbuR128::new(u32::from(channels), rate, Mode::I | Mode::TRUE_PEAK)
            .map_err(|e| MeasureError::Decode(format!("can't measure this format: {e:?}")))?;
        if channels == 1 {
            let _ = m.set_channel(0, Channel::DualMono);
        }
        Ok(m)
    }

    pub fn add(&mut self, interleaved: &[f64]) -> Result<(), MeasureError> {
        let ch = self.cur.channels().max(1) as usize;
        self.frames += (interleaved.len() / ch) as u64;
        self.cur
            .add_frames_f64(interleaved)
            .map_err(|e| MeasureError::Decode(format!("{e:?}")))
    }

    fn fold(&mut self) {
        self.seconds += self.frames as f64 / self.rate.max(1) as f64;
        self.frames = 0;
    }

    fn restart(&mut self, channels: u16, rate: u32) -> Result<(), MeasureError> {
        let next = Self::meter(channels, rate)?;
        self.done.push(std::mem::replace(&mut self.cur, next));
        self.rate = rate;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Measurement, MeasureError> {
        self.fold();
        let all: Vec<&EbuR128> = self.done.iter().chain(std::iter::once(&self.cur)).collect();
        let lufs = EbuR128::loudness_global_multiple(all.iter().copied())
            .map_err(|e| MeasureError::Decode(format!("{e:?}")))?;
        if !lufs.is_finite() {
            return Err(MeasureError::Silent);
        }
        let mut peak = 0.0f64;
        for m in &all {
            for c in 0..m.channels() {
                peak = peak.max(m.true_peak(c).unwrap_or(0.0));
            }
        }
        Ok(Measurement {
            lufs,
            peak,
            seconds: self.seconds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_measures_as_expected() {
        // A full-scale 1 kHz sine in both channels is -3.01 LUFS per
        // channel, summed over two: about 0 LUFS. At half amplitude, -6 dB.
        let rate = 48000;
        let mut m = Meter::new(2, rate).unwrap();
        let mut v = Vec::new();
        for i in 0..rate as usize * 5 {
            let s = 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate as f64).sin();
            v.push(s);
            v.push(s);
        }
        m.add(&v).unwrap();
        let r = m.finish().unwrap();
        assert!((r.lufs + 6.0).abs() < 0.2, "{}", r.lufs);
        assert!((r.peak - 0.5).abs() < 0.01, "{}", r.peak);
        assert!((r.seconds - 5.0).abs() < 1e-6);
        assert!((r.gain() + 12.0).abs() < 0.2);
    }

    #[test]
    fn silence_has_no_loudness() {
        let mut m = Meter::new(2, 44100).unwrap();
        m.add(&vec![0.0; 44100 * 2 * 3]).unwrap();
        assert!(matches!(m.finish(), Err(MeasureError::Silent)));
    }
}
