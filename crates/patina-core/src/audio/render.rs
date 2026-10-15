//! The real-time half of the engine. Output backends call
//! [`Renderer::render`] from their audio thread. Nothing in here locks,
//! allocates (after construction) or waits.
//!
//! Stages live here in the order from the design doc: EQ, then fade and
//! volume, then the true-peak limiter. Leveling runs earlier, in the
//! decoder thread; the transition mixer, amp stack, channel tools and
//! resampler slot in at their marked places as their milestones land.
//!
//! The limiter looks ahead by peeking further into the ring than the block
//! it takes, so it adds no delay and the stream's timing is untouched.

use std::sync::atomic::Ordering;

use rtrb::Consumer;

use super::{SharedRef, StreamFormat};
use crate::dsp::EqRt;
use crate::dsp::limiter::{Limiter, MAX_LOOKAHEAD, TAPS_HALF};

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
    last_flush: bool,
    eq: EqRt,
    eq_gen: u64,
    limiter: Limiter,
    /// Audio past the block, peeked from the ring for the limiter.
    ahead: Vec<f64>,
}

/// Largest block a backend asks for in one go. PipeWire quanta top out at
/// 8192 frames by default; this leaves headroom for odd configs.
pub const MAX_BLOCK_FRAMES: usize = 16384;

impl Renderer {
    pub fn new(ring: Consumer<f64>, shared: SharedRef, format: StreamFormat) -> Self {
        let volume = shared.volume.load();
        let ch = format.channels as usize;
        let mut eq = EqRt::new(
            ch,
            format.rate,
            (MAX_BLOCK_FRAMES + MAX_LOOKAHEAD + 2 * TAPS_HALF) * ch,
        );
        let eq_gen = shared.eq_gen.load(Ordering::Acquire);
        eq.set(&shared.eq.lock().unwrap());
        eq.reset();
        Self {
            ring,
            format,
            buf: vec![0.0; MAX_BLOCK_FRAMES * ch],
            fade: 0.0,
            volume,
            last_from_ring: 0,
            last_len: 0,
            last_flush: false,
            eq,
            eq_gen,
            limiter: Limiter::new(ch, format.rate, MAX_BLOCK_FRAMES),
            ahead: vec![0.0; (MAX_LOOKAHEAD + 2 * TAPS_HALF) * ch],
            shared,
        }
    }

    /// Take the current volume as-is, with no ramp from the old one. For a
    /// renderer that hasn't played anything yet.
    pub fn sync_volume(&mut self) {
        self.volume = self.shared.volume.load();
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
    /// call again for the rest. When the ring runs short the block is
    /// padded with silence, which is what a shared device wants.
    pub fn render(&mut self, frames: usize) -> &[f64] {
        let n = self.render_inner(frames, false);
        &self.buf[..n]
    }

    /// Like [`render`](Self::render), but returns only what came out of the
    /// ring: nothing while paused, flushed or starved, and never padding.
    /// The exclusive output uses this, since every sample it writes has to
    /// be the music.
    pub fn render_exact(&mut self, frames: usize) -> &[f64] {
        let n = self.render_inner(frames, true);
        &self.buf[..n]
    }

    /// True if the last render threw the ring away for a seek or stop.
    pub fn last_was_flush(&self) -> bool {
        self.last_flush
    }

    fn render_inner(&mut self, frames: usize, exact: bool) -> usize {
        let ch = self.format.channels as usize;
        let mut frames = frames.min(MAX_BLOCK_FRAMES);
        let shared = &*self.shared;
        if !exact {
            let g = shared.eq_gen.load(Ordering::Acquire);
            if g != self.eq_gen
                && let Ok(p) = shared.eq.try_lock()
            {
                // try_lock: if the core is writing right now, the next
                // block picks it up. Never wait on the output thread.
                self.eq.set(&p);
                self.eq_gen = g;
            }
        }
        self.last_from_ring = 0;
        self.last_flush = false;
        if exact {
            let want_sound = shared.playing.load(Ordering::Relaxed);
            let flushing = shared.flush_req.load(Ordering::Acquire)
                != shared.flush_ack.load(Ordering::Relaxed);
            if !flushing && (want_sound || self.fade > 0.0) {
                frames = frames.min(self.ring.slots() / ch);
                if frames == 0 && shared.end_of_queue.load(Ordering::Acquire) {
                    shared.drained.store(true, Ordering::Release);
                }
            }
        }
        let n = frames * ch;
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
            self.eq.reset();
            self.limiter.reset();
            shared.silent.store(true, Ordering::Relaxed);
            self.last_flush = true;
            if exact {
                self.last_len = 0;
                return 0;
            }
            self.buf[..n].fill(0.0);
            return n;
        }

        let want_sound = shared.playing.load(Ordering::Relaxed);
        if !want_sound && self.fade <= 0.0 {
            // Paused and fully faded: leave the ring alone so playback
            // resumes at the exact sample it stopped on. Volume jumps
            // straight to its target while silent, so sound starts at the
            // right level instead of ramping from wherever it was.
            self.volume = shared.volume.load();
            shared.silent.store(true, Ordering::Relaxed);
            if exact {
                self.last_len = 0;
                return 0;
            }
            self.buf[..n].fill(0.0);
            return n;
        }

        // Pull whole frames only, and peek past them for the limiter.
        let look = if exact {
            0
        } else {
            self.limiter.lookahead() * ch
        };
        let slots = self.ring.slots() / ch * ch;
        let avail = slots.min(n);
        let peek = slots.min(n + look);
        let mut ahead_len = 0;
        let got = match self.ring.read_chunk(peek) {
            Ok(chunk) => {
                let (a, b) = chunk.as_slices();
                for (i, s) in a.iter().chain(b.iter()).enumerate() {
                    if i < avail {
                        self.buf[i] = *s;
                    } else {
                        self.ahead[i - avail] = *s;
                    }
                }
                ahead_len = peek - avail;
                chunk.commit(avail);
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

        // 2. Leveling gain: applied by the decoder thread before the ring,
        //    so it switches at the exact sample one track becomes the next.
        // 3. Transition mixer, crossfade          (later)
        // 4. Amp stack                            (milestone 13)
        // 5. EQ
        if !exact {
            self.eq.process(&mut self.buf[..n]);
        }
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

        // 8. Resample                             (milestone 12)
        // 9. True-peak limiter. It sees what's coming through the same EQ
        //    and gain, so it can be down by the time a peak arrives.
        if !exact && shared.limiter_on.load(Ordering::Relaxed) {
            let ahead = &mut self.ahead[..ahead_len];
            self.eq.preview(ahead);
            let fg = (self.fade * std::f64::consts::FRAC_PI_2).sin();
            let g = fg * fg * vol_target;
            if g != 1.0 {
                for s in ahead.iter_mut() {
                    *s *= g;
                }
            }
            let ceiling = shared.limiter_ceiling.load();
            let gr = self.limiter.process(&mut self.buf[..n], ahead, ceiling);
            if gr > shared.limiter_gr.load() {
                shared.limiter_gr.store(gr);
            }
        }
        // Whatever's left over full scale (limiter off) is clipped, never
        // wrapped.
        for s in &mut self.buf[..n] {
            *s = s.clamp(-1.0, 1.0);
        }
        // 10. Dither and convert happens in the backend for its sample format.

        n
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

    #[test]
    fn exact_render_never_pads() {
        let (mut p, mut r, shared) = setup();
        shared.fade_ms.store(0, Ordering::Relaxed);
        // Paused: nothing at all.
        for _ in 0..20 {
            p.push(0.25).unwrap();
        }
        assert!(r.render_exact(50).is_empty());
        // Playing with 10 frames in the ring: exactly those 10, untouched.
        shared.playing.store(true, Ordering::Relaxed);
        let out = r.render_exact(50).to_vec();
        assert_eq!(out.len(), 20);
        assert!(out.iter().all(|&s| s == 0.25), "{out:?}");
        // Starved: nothing, and no silence invented.
        assert!(r.render_exact(50).is_empty());
    }
}
