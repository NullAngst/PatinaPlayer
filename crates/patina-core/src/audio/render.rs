//! The real-time half of the engine. Output backends call
//! [`Renderer::render`] from their audio thread. Nothing in here locks,
//! allocates (after construction) or waits.
//!
//! Stages live here in the order from the design doc. Milestone 1 runs the
//! fade gain and volume; leveling, the transition mixer, amp stack, EQ,
//! channel tools, resampler, limiter and dither slot in at their marked
//! places as their milestones land.

use std::sync::atomic::Ordering;

use rtrb::Consumer;

use super::{SharedRef, StreamFormat};

pub struct Renderer {
    ring: Consumer<f64>,
    shared: SharedRef,
    format: StreamFormat,
    /// Interleaved f64 scratch, the result of the last render call.
    buf: Vec<f64>,
    /// Current fade gain, 0.0 to 1.0.
    fade: f64,
    /// Current volume gain, smoothed toward the target per block.
    volume: f64,
    /// Frames the last render call actually took from the ring.
    last_from_ring: usize,
    last_len: usize,
}

/// Largest block a backend asks for in one go. PipeWire quanta top out at
/// 8192 frames by default; this leaves headroom for odd configs.
pub const MAX_BLOCK_FRAMES: usize = 16384;

impl Renderer {
    pub fn new(ring: Consumer<f64>, shared: SharedRef, format: StreamFormat) -> Self {
        let volume = shared.volume.load();
        Self {
            ring,
            shared,
            format,
            buf: vec![0.0; MAX_BLOCK_FRAMES * format.channels as usize],
            fade: 0.0,
            volume,
            last_from_ring: 0,
            last_len: 0,
        }
    }

    /// The block the last [`render`](Self::render) call produced.
    pub fn last_block(&self) -> &[f64] {
        &self.buf[..self.last_len]
    }

    /// Frames the last [`render`](Self::render) call took from the ring.
    /// The rest of that block was silence.
    pub fn last_from_ring(&self) -> usize {
        self.last_from_ring
    }

    pub fn format(&self) -> StreamFormat {
        self.format
    }

    /// Render `frames` frames. Returns interleaved f64 in [-1, 1].
    /// Asking for more than [`MAX_BLOCK_FRAMES`] gets you that many at most;
    /// call again for the rest.
    pub fn render(&mut self, frames: usize) -> &[f64] {
        let frames = frames.min(MAX_BLOCK_FRAMES);
        let ch = self.format.channels as usize;
        let n = frames * ch;
        let shared = &*self.shared;
        self.last_from_ring = 0;
        self.last_len = n;

        // A pending flush beats everything else: throw the ring away and
        // play silence until the decoder refills it.
        let req = shared.flush_req.load(Ordering::Acquire);
        if req != shared.flush_ack.load(Ordering::Relaxed) {
            let avail = self.ring.slots();
            if let Ok(chunk) = self.ring.read_chunk(avail) {
                chunk.commit_all();
            }
            shared
                .consumed_frames
                .fetch_add((avail / ch) as u64, Ordering::Relaxed);
            shared.flush_ack.store(req, Ordering::Release);
            self.fade = 0.0;
            shared.silent.store(true, Ordering::Relaxed);
            self.buf[..n].fill(0.0);
            return &self.buf[..n];
        }

        let want_sound = shared.playing.load(Ordering::Relaxed);
        if !want_sound && self.fade <= 0.0 {
            // Paused and fully faded: leave the ring alone so playback
            // resumes at the exact sample it stopped on.
            shared.silent.store(true, Ordering::Relaxed);
            self.buf[..n].fill(0.0);
            return &self.buf[..n];
        }

        // Pull whole frames only.
        let avail = (self.ring.slots() / ch * ch).min(n);
        let got = match self.ring.read_chunk(avail) {
            Ok(chunk) => {
                let (a, b) = chunk.as_slices();
                self.buf[..a.len()].copy_from_slice(a);
                self.buf[a.len()..a.len() + b.len()].copy_from_slice(b);
                chunk.commit_all();
                avail
            }
            Err(_) => 0,
        };
        self.buf[got..n].fill(0.0);
        self.last_from_ring = got / ch;
        shared
            .consumed_frames
            .fetch_add((got / ch) as u64, Ordering::Relaxed);
        if got < n {
            if shared.end_of_queue.load(Ordering::Acquire) {
                shared.drained.store(true, Ordering::Release);
            } else if want_sound {
                shared.underrun.store(true, Ordering::Relaxed);
            }
        }

        // 2. Leveling gain plus preamp            (milestone 7)
        // 3. Transition mixer, crossfade          (milestone 10)
        // 4. Amp stack                            (milestone 11)
        // 5. EQ                                   (milestone 8)
        // 6. Channel tools                        (later)

        // Fade (pause, resume, seek, stop) and 7. Volume, as one gain ramp.
        let fade_frames = (shared.fade_ms.load(Ordering::Relaxed) as f64 * self.format.rate as f64
            / 1000.0)
            .max(1.0);
        let fade_step = 1.0 / fade_frames;
        let fade_target = if want_sound { 1.0 } else { 0.0 };
        let vol_target = shared.volume.load();
        let vol_step = (vol_target - self.volume) / frames.max(1) as f64;

        for f in 0..frames {
            if self.fade < fade_target {
                self.fade = (self.fade + fade_step).min(1.0);
            } else if self.fade > fade_target {
                self.fade = (self.fade - fade_step).max(0.0);
            }
            self.volume += vol_step;
            // Equal-power-ish fade curve: sin² sounds smoother than linear.
            let fg = (self.fade * std::f64::consts::FRAC_PI_2).sin();
            let g = fg * fg * self.volume;
            for s in &mut self.buf[f * ch..(f + 1) * ch] {
                *s *= g;
            }
        }
        self.volume = vol_target;
        shared.silent.store(self.fade <= 0.0, Ordering::Relaxed);

        // 8. Resample                             (milestone 10)
        // 9. True-peak limiter                    (milestone 8)
        // Until the limiter exists, hard-clip so a hot master never wraps.
        for s in &mut self.buf[..n] {
            *s = s.clamp(-1.0, 1.0);
        }
        // 10. Dither and convert happens in the backend for its sample format.

        &self.buf[..n]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Shared;
    use std::sync::Arc;

    fn setup() -> (rtrb::Producer<f64>, Renderer, SharedRef) {
        let fmt = StreamFormat {
            rate: 1000,
            channels: 2,
            mask: 0b11,
        };
        let (p, c) = rtrb::RingBuffer::new(4096);
        let shared = Arc::new(Shared::default());
        shared.fade_ms.store(10, Ordering::Relaxed); // 10 frames at 1 kHz
        (p, Renderer::new(c, shared.clone(), fmt), shared)
    }

    #[test]
    fn paused_does_not_consume() {
        let (mut p, mut r, shared) = setup();
        for _ in 0..200 {
            p.push(0.5).unwrap();
        }
        let out = r.render(50);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(shared.consumed_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn fade_in_reaches_unity() {
        let (mut p, mut r, shared) = setup();
        for _ in 0..200 {
            p.push(0.5).unwrap();
        }
        shared.playing.store(true, Ordering::Relaxed);
        let out = r.render(50).to_vec();
        assert!(out[0] < 0.1, "starts quiet");
        assert!((out[out.len() - 1] - 0.5).abs() < 1e-9, "ends at unity");
        assert_eq!(shared.consumed_frames.load(Ordering::Relaxed), 50);
    }

    #[test]
    fn flush_discards_and_acks() {
        let (mut p, mut r, shared) = setup();
        for _ in 0..200 {
            p.push(0.5).unwrap();
        }
        shared.flush_req.store(1, Ordering::Release);
        r.render(10);
        assert_eq!(shared.flush_ack.load(Ordering::Acquire), 1);
        assert_eq!(shared.consumed_frames.load(Ordering::Relaxed), 100);
        assert_eq!(p.slots(), 4096);
    }
}
