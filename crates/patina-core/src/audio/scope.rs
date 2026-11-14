//! A tap on what the renderer sends out, for drawing a visualizer.
//!
//! The renderer writes the last few thousand samples, mixed to mono, into
//! a ring of atomics: no lock, nothing allocated, nothing waited on, so the
//! audio thread never feels it. It only does so while someone's watching
//! ([`Scope::set_on`]). The window reads the newest samples whenever it
//! draws a frame.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Samples kept: enough for a 2048-point FFT with room to spare.
pub const LEN: usize = 4096;

pub struct Scope {
    on: AtomicBool,
    rate: AtomicU32,
    /// Samples ever written; the newest is at `(pos - 1) % LEN`.
    pos: AtomicUsize,
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
        self.pos.store(pos, Ordering::Release);
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
    fn it_wraps_around() {
        let s = Scope::default();
        let block: Vec<f64> = (0..LEN + 5).map(|i| i as f64).collect();
        s.push(&block, 1, 44_100);
        let mut out = [0f32; 2];
        s.latest(&mut out);
        assert_eq!(out, [(LEN + 3) as f32, (LEN + 4) as f32]);
    }
}
