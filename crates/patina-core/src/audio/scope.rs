//! A tap on what the renderer sends out, for drawing a visualizer.
//!
//! The renderer writes what it makes, mixed to mono, into a ring of
//! atomics: no lock, nothing allocated, nothing waited on, so the audio
//! thread never feels it. It only does so while someone's watching
//! ([`Scope::set_on`]).
//!
//! The renderer writes in blocks as big as the output's buffer: with a
//! large `buffer_ms`, a fifth of a second or more at once. So each write
//! also notes when it came and how big it was ([`Scope::clock`]), and the
//! window reads the samples at the point it works out is being heard now
//! ([`Scope::read_to`]), moving along smoothly between blocks, instead of
//! the newest ones, which only change once a block.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Samples kept: two blocks of the biggest buffer (a third of a second
/// each at 192 kHz) and a 2048-point FFT, with room to spare.
pub const LEN: usize = 1 << 17;

/// Nanoseconds since the first call, as one clock for the renderer's
/// writes and the window's reads.
pub fn now_nanos() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// Where the writes stand: see [`Scope::clock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    /// Samples ever written.
    pub pos: usize,
    /// The last write's size, in samples.
    pub block: usize,
    /// When the last write came, from [`now_nanos`].
    pub at: u64,
    /// Their rate; 0 before anything has played.
    pub rate: u32,
}

pub struct Scope {
    on: AtomicBool,
    rate: AtomicU32,
    /// Samples ever written; the newest is at `(pos - 1) % LEN`.
    pos: AtomicUsize,
    block: AtomicUsize,
    at: AtomicU64,
    buf: Box<[AtomicU32]>,
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope").field("on", &self.is_on()).finish()
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            on: AtomicBool::new(false),
            rate: AtomicU32::new(0),
            pos: AtomicUsize::new(0),
            block: AtomicUsize::new(0),
            at: AtomicU64::new(0),
            buf: (0..LEN).map(|_| AtomicU32::new(0)).collect(),
        }
    }
}

impl Scope {
    /// Start or stop collecting.
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// From the renderer: an interleaved block at `rate`.
    pub(crate) fn push(&self, block: &[f64], channels: usize, rate: u32) {
        let ch = channels.max(1);
        self.rate.store(rate, Ordering::Relaxed);
        let mut pos = self.pos.load(Ordering::Relaxed);
        for f in block.chunks_exact(ch) {
            let m = f.iter().sum::<f64>() / ch as f64;
            self.buf[pos % LEN].store((m as f32).to_bits(), Ordering::Relaxed);
            pos = pos.wrapping_add(1);
        }
        self.block.store(block.len() / ch, Ordering::Relaxed);
        self.at.store(now_nanos(), Ordering::Relaxed);
        self.pos.store(pos, Ordering::Release);
    }

    /// Where the writes stand, for working out what's being heard.
    pub fn clock(&self) -> Clock {
        let pos = self.pos.load(Ordering::Acquire);
        Clock {
            pos,
            block: self.block.load(Ordering::Relaxed),
            at: self.at.load(Ordering::Relaxed),
            rate: self.rate.load(Ordering::Relaxed),
        }
    }

    /// The `out.len()` samples up to sample `end` (counted as in
    /// [`Clock::pos`]), oldest first. What's no longer kept, or not yet
    /// written, reads as silence.
    pub fn read_to(&self, end: usize, out: &mut [f32]) {
        let pos = self.pos.load(Ordering::Acquire);
        let n = out.len();
        for (i, o) in out.iter_mut().enumerate() {
            // The sample's place, if it has one: not before the first.
            let at = (end + i).checked_sub(n);
            *o = match at {
                Some(at) if at < pos && pos - at <= LEN => {
                    f32::from_bits(self.buf[at % LEN].load(Ordering::Relaxed))
                }
                _ => 0.0,
            };
        }
    }

    /// The newest `out.len()` samples, oldest first, and their rate (0
    /// before anything has played). A sample being written as it's read
    /// may be from either side of the write, which a picture never shows.
    pub fn latest(&self, out: &mut [f32]) -> u32 {
        let n = out.len().min(LEN);
        let pos = self.pos.load(Ordering::Acquire);
        let start = pos.wrapping_sub(n);
        for (i, o) in out.iter_mut().take(n).enumerate() {
            *o = f32::from_bits(self.buf[start.wrapping_add(i) % LEN].load(Ordering::Relaxed));
        }
        self.rate.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_samples_come_back_mixed_to_mono() {
        let s = Scope::default();
        let block: Vec<f64> = (0..10).flat_map(|i| [i as f64, i as f64 + 1.0]).collect();
        s.push(&block, 2, 48_000);
        let mut out = [0f32; 3];
        assert_eq!(s.latest(&mut out), 48_000);
        assert_eq!(out, [7.5, 8.5, 9.5]);
    }

    #[test]
    fn reading_up_to_a_point() {
        let s = Scope::default();
        let block: Vec<f64> = (0..10).map(|i| i as f64).collect();
        s.push(&block, 1, 48_000);
        let c = s.clock();
        assert_eq!((c.pos, c.block, c.rate), (10, 10, 48_000));
        let mut out = [9f32; 4];
        s.read_to(6, &mut out);
        assert_eq!(out, [2.0, 3.0, 4.0, 5.0]);
        // Before the first sample, and past the last: silence.
        s.read_to(2, &mut out);
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0]);
        s.read_to(12, &mut out);
        assert_eq!(out, [8.0, 9.0, 0.0, 0.0]);
    }

    #[test]
    fn it_wraps_around() {
        let s = Scope::default();
        let block: Vec<f64> = (0..LEN + 5).map(|i| i as f64).collect();
        s.push(&block, 1, 44_100);
        let mut out = [0f32; 2];
        s.latest(&mut out);
        assert_eq!(out, [(LEN + 3) as f32, (LEN + 4) as f32]);
    }
}
