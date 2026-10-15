//! The live normalizer: for what can't be measured ahead of time, which
//! is internet radio, podcasts, and (if you turn it on) music that hasn't
//! been measured yet.
//!
//! It follows the stream's short-term loudness (the last 3 seconds, EBU
//! R128) and moves the gain toward the target slowly, so it rides the
//! level of a station without pumping on every drum hit. Quick for the
//! first few seconds, so a station that starts loud settles fast, then
//! about a dB a second. It holds still through silence, so the gap
//! between talk and music isn't boosted up to the target. It's audibly
//! worse than a measured gain, which is why music doesn't use it by
//! default.

use ebur128::{Channel, EbuR128, Mode};

use super::{db_to_lin, lin_to_db};

/// How often the gain moves, in seconds of audio.
const STEP_S: f64 = 0.1;
/// Seconds of the quick start, and how fast it moves then and after.
const WARMUP_S: f64 = 6.0;
const FAST_DB_PER_S: f64 = 6.0;
const SLOW_DB_PER_S: f64 = 1.0;
/// Quieter than this is silence: hold the gain.
const SILENCE_LUFS: f64 = -45.0;
/// How far it will go either way.
pub const MIN_DB: f64 = -20.0;
pub const MAX_DB: f64 = 8.0;

pub struct LiveNorm {
    meter: EbuR128,
    channels: usize,
    rate: u32,
    target: f64,
    /// Where the gain is heading, in dB.
    gain_db: f64,
    /// The linear gain applied to the last sample.
    applied: f64,
    /// Frames since the gain last moved.
    since: usize,
    /// Seconds of audio seen.
    elapsed: f64,
}

impl LiveNorm {
    /// None if the meter can't take this format (no channels, odd rate).
    pub fn new(channels: u16, rate: u32, start_db: f64, target_lufs: f64) -> Option<Self> {
        let mut meter = EbuR128::new(u32::from(channels), rate, Mode::S).ok()?;
        if channels == 1 {
            // A mono stream is heard from both speakers.
            meter.set_channel(0, Channel::DualMono).ok()?;
        }
        let start_db = start_db.clamp(MIN_DB, MAX_DB);
        Some(Self {
            meter,
            channels: channels as usize,
            rate,
            target: target_lufs,
            gain_db: start_db,
            applied: db_to_lin(start_db),
            since: 0,
            elapsed: 0.0,
        })
    }

    /// The same normalizer at a new format, keeping its gain.
    pub fn reformat(&self, channels: u16, rate: u32) -> Option<Self> {
        let mut n = Self::new(channels, rate, self.gain_db, self.target)?;
        n.applied = self.applied;
        n.elapsed = self.elapsed;
        Some(n)
    }

    pub fn set_target(&mut self, target_lufs: f64) {
        self.target = target_lufs;
    }

    /// The gain right now, in dB.
    pub fn gain_db(&self) -> f64 {
        lin_to_db(self.applied)
    }

    /// Measure a block of interleaved samples and apply the gain to it.
    pub fn process(&mut self, buf: &mut [f64]) {
        let ch = self.channels.max(1);
        let step = ((self.rate as f64 * STEP_S) as usize).max(1);
        let mut at = 0;
        let frames = buf.len() / ch;
        while at < frames {
            let n = (step - self.since).min(frames - at);
            let block = &mut buf[at * ch..(at + n) * ch];
            // The meter hears the stream as it comes, before our gain.
            let _ = self.meter.add_frames_f64(block);
            let want = db_to_lin(self.gain_db);
            let inc = (want - self.applied) / n as f64;
            for f in block.chunks_exact_mut(ch) {
                self.applied += inc;
                for s in f {
                    *s *= self.applied;
                }
            }
            self.applied = want;
            self.since += n;
            at += n;
            if self.since >= step {
                self.since = 0;
                self.elapsed += STEP_S;
                self.adjust();
            }
        }
    }

    fn adjust(&mut self) {
        // Short-term loudness needs 3 s of audio; momentary (400 ms) gets
        // the quick start going sooner.
        let loud = if self.elapsed < 3.0 {
            self.meter.loudness_momentary()
        } else {
            self.meter.loudness_shortterm()
        };
        let Ok(l) = loud else { return };
        if !l.is_finite() || l < SILENCE_LUFS {
            return;
        }
        let want = (self.target - l).clamp(MIN_DB, MAX_DB);
        let rate = if self.elapsed < WARMUP_S {
            FAST_DB_PER_S
        } else {
            SLOW_DB_PER_S
        };
        let max = rate * STEP_S;
        self.gain_db += (want - self.gain_db).clamp(-max, max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz sine, stereo, at `amp`.
    fn sine(rate: u32, secs: f64, amp: f64) -> Vec<f64> {
        let n = (rate as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = amp * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate as f64).sin();
            v.push(s);
            v.push(s);
        }
        v
    }

    fn loudness(buf: &[f64], rate: u32) -> f64 {
        let mut m = EbuR128::new(2, rate, Mode::I).unwrap();
        m.add_frames_f64(buf).unwrap();
        m.loudness_global().unwrap()
    }

    #[test]
    fn settles_on_the_target() {
        let rate = 48000;
        // About -9 LUFS: a loud station.
        let mut n = LiveNorm::new(2, rate, -6.0, -18.0).unwrap();
        let mut all = sine(rate, 30.0, 0.5);
        for chunk in all.chunks_mut(4096) {
            n.process(chunk);
        }
        let tail = &all[all.len() - rate as usize * 2 * 5..];
        let l = loudness(tail, rate);
        assert!((l + 18.0).abs() < 1.0, "settled at {l} LUFS");
    }

    #[test]
    fn holds_through_silence() {
        let rate = 48000;
        let mut n = LiveNorm::new(2, rate, -6.0, -18.0).unwrap();
        let mut quiet = vec![0.0; rate as usize * 2 * 20];
        n.process(&mut quiet);
        assert!((n.gain_db() + 6.0).abs() < 1e-6, "{}", n.gain_db());
    }

    #[test]
    fn moves_slowly_after_warmup() {
        let rate = 48000;
        let mut n = LiveNorm::new(2, rate, -6.0, -18.0).unwrap();
        let mut a = sine(rate, 20.0, 0.5);
        n.process(&mut a);
        let before = n.gain_db();
        // The stream drops 20 dB: one second later the gain has moved by
        // about a dB, not jumped.
        let mut b = sine(rate, 4.0, 0.05);
        n.process(&mut b);
        let moved = n.gain_db() - before;
        assert!(moved > 0.0 && moved < 4.5, "moved {moved} dB in 4 s");
    }
}
