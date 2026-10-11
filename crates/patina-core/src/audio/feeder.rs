//! The decoder thread. Opens tracks, decodes them back to back into the
//! ring, and moves straight on to the next queue entry at end of file.
//! That straight hand-off is what makes playback gapless: the last sample
//! of one track and the first sample of the next sit next to each other
//! in the ring, with nothing in between.
//!
//! When the next track has a different sample rate or channel layout, the
//! ring is left to drain and a new one (and a new output stream) is opened
//! at the new format. That join has a short gap, which is the honest
//! result until the resampler lands.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use rtrb::{Consumer, Producer, RingBuffer};

use super::decoder::Source;
use super::{SharedRef, StreamFormat};
use crate::queue::{EntryId, Queue};
use crate::track::TrackInfo;

#[derive(Debug)]
pub enum FeedCmd {
    /// Start playing queue entry `entry` at `pos`.
    Start {
        entry: EntryId,
        pos: Duration,
    },
    /// Seek within the current track.
    Seek(Duration),
    /// Stop decoding and empty the ring.
    Stop,
    Quit,
}

/// Where a track starts in the stream of frames the renderer consumes.
#[derive(Debug, Clone)]
pub struct Segment {
    pub entry: EntryId,
    /// Value of `consumed_frames` at which this track's first frame plays.
    pub start_frame: u64,
    /// Track position of that first frame (non-zero after a seek).
    pub start_pos: Duration,
    pub rate: u32,
    pub info: TrackInfo,
}

pub enum FeedMsg {
    /// A new ring at a new format. The core opens an output for it.
    Open {
        format: StreamFormat,
        consumer: Consumer<f64>,
    },
    Segment(Segment),
    /// The ring was emptied for a Start, Seek or Stop.
    Flushed,
    TrackError {
        entry: EntryId,
        error: String,
    },
    /// Every queued track has been decoded.
    EndOfQueue,
}

pub fn spawn(
    shared: SharedRef,
    queue: Arc<Mutex<Queue>>,
    cmds: Receiver<FeedCmd>,
    msgs: Sender<FeedMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("patina-decoder".into())
        .spawn(move || Feeder::new(shared, queue, cmds, msgs).run())
        .expect("spawn decoder thread")
}

struct Feeder {
    shared: SharedRef,
    queue: Arc<Mutex<Queue>>,
    cmds: Receiver<FeedCmd>,
    msgs: Sender<FeedMsg>,
    source: Option<Source>,
    entry: EntryId,
    ring: Option<Producer<f64>>,
    format: Option<StreamFormat>,
    /// Frames pushed into the ring, in `consumed_frames` coordinates.
    written: u64,
    /// Decoded samples not yet in the ring.
    pending: Vec<f64>,
    pending_off: usize,
    stashed: Option<FeedCmd>,
}

/// Longest we wait for the renderer to acknowledge a flush. If the output
/// is wedged, we stop waiting and start a fresh ring instead.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

/// How many skips in a row before giving up on a queue full of broken files.
const MAX_CONSECUTIVE_ERRORS: usize = 25;

impl Feeder {
    fn new(
        shared: SharedRef,
        queue: Arc<Mutex<Queue>>,
        cmds: Receiver<FeedCmd>,
        msgs: Sender<FeedMsg>,
    ) -> Self {
        Self {
            shared,
            queue,
            cmds,
            msgs,
            source: None,
            entry: 0,
            ring: None,
            format: None,
            written: 0,
            pending: Vec::with_capacity(32768),
            pending_off: 0,
            stashed: None,
        }
    }

    fn run(mut self) {
        loop {
            // Idle: block on the next command. Busy: just peek.
            let cmd = if let Some(c) = self.stashed.take() {
                Some(c)
            } else if self.source.is_none() {
                match self.cmds.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            } else {
                match self.cmds.try_recv() {
                    Ok(c) => Some(c),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            };

            if let Some(cmd) = cmd {
                match cmd {
                    FeedCmd::Quit => return,
                    FeedCmd::Stop => {
                        self.flush();
                        self.source = None;
                        let _ = self.msgs.send(FeedMsg::Flushed);
                    }
                    FeedCmd::Start { entry, pos } => {
                        self.flush();
                        self.source = None;
                        let _ = self.msgs.send(FeedMsg::Flushed);
                        self.open_from(entry, pos);
                    }
                    FeedCmd::Seek(pos) => {
                        self.flush();
                        let _ = self.msgs.send(FeedMsg::Flushed);
                        let entry = self.entry;
                        let ok = self.source.as_mut().map(|s| s.seek(pos).is_ok());
                        match ok {
                            Some(true) => self.announce(pos),
                            // Seek failed (or nothing open): reopen there.
                            _ => {
                                self.source = None;
                                self.open_from(entry, pos);
                            }
                        }
                    }
                }
                continue;
            }

            if self.source.is_some() {
                self.pump();
            }
        }
    }

    /// Ask the renderer to drop everything in the ring and wait for it.
    fn flush(&mut self) {
        self.pending.clear();
        self.pending_off = 0;
        self.shared.end_of_queue.store(false, Ordering::Release);
        self.shared.drained.store(false, Ordering::Release);
        if self.ring.is_none() {
            return;
        }
        let token = self.shared.flush_req.load(Ordering::Relaxed) + 1;
        self.shared.flush_req.store(token, Ordering::Release);
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        while self.shared.flush_ack.load(Ordering::Acquire) != token {
            if Instant::now() > deadline {
                log::warn!("output didn't acknowledge flush, starting a new ring");
                // Mark it acknowledged so a future renderer doesn't flush
                // the new ring on its first call.
                self.shared.flush_ack.store(token, Ordering::Release);
                self.ring = None;
                self.format = None;
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.written = self.shared.consumed_frames.load(Ordering::Acquire);
    }

    /// Open `entry` at `pos`, skipping forward past files that won't open.
    fn open_from(&mut self, mut entry: EntryId, pos: Duration) {
        let mut pos = pos;
        for _ in 0..MAX_CONSECUTIVE_ERRORS {
            if self.open(entry, pos) {
                return;
            }
            pos = Duration::ZERO;
            match self.queue.lock().unwrap().step(entry, 1) {
                Some(next) => entry = next,
                None => break,
            }
        }
        self.finish_queue();
    }

    /// Open one entry. Sends the Segment (and a new ring if needed).
    fn open(&mut self, entry: EntryId, pos: Duration) -> bool {
        let Some(path) = self.queue.lock().unwrap().path(entry) else {
            return false;
        };
        let mut src = match Source::open(&path) {
            Ok(s) => s,
            Err(e) => {
                self.queue.lock().unwrap().mark_failed(entry);
                let _ = self.msgs.send(FeedMsg::TrackError {
                    entry,
                    error: format!("{}: {e}", path.display()),
                });
                return false;
            }
        };
        if pos > Duration::ZERO
            && let Err(e) = src.seek(pos)
        {
            log::warn!("seek failed in {}: {e}", path.display());
        }
        self.queue
            .lock()
            .unwrap()
            .set_info(entry, src.info().clone());

        let fmt = src.format();
        if self.format != Some(fmt) {
            if !self.drain_ring() {
                // A command arrived while draining; it's stashed and wins.
                return true;
            }
            self.new_ring(fmt);
        }
        self.entry = entry;
        self.source = Some(src);
        self.announce(pos);
        true
    }

    fn announce(&mut self, pos: Duration) {
        let Some(src) = &self.source else { return };
        let _ = self.msgs.send(FeedMsg::Segment(Segment {
            entry: self.entry,
            start_frame: self.written
                + (self.pending.len() - self.pending_off) as u64
                    / self.format.map(|f| f.channels as u64).unwrap_or(1),
            start_pos: pos,
            rate: src.format().rate,
            info: src.info().clone(),
        }));
    }

    /// Wait until the renderer has played everything in the current ring.
    /// Returns false if a command arrived first.
    fn drain_ring(&mut self) -> bool {
        if self.ring.is_none() {
            return true;
        }
        // Push what's left of the previous track first.
        while self.pending_off < self.pending.len() {
            if !self.push_pending_blocking() {
                return false;
            }
        }
        loop {
            let consumed = self.shared.consumed_frames.load(Ordering::Acquire);
            if consumed >= self.written {
                return true;
            }
            if let Ok(c) = self.cmds.try_recv() {
                self.stashed = Some(c);
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn new_ring(&mut self, fmt: StreamFormat) {
        let (prod, cons) = RingBuffer::new(fmt.ring_capacity());
        self.ring = Some(prod);
        self.format = Some(fmt);
        self.written = self.shared.consumed_frames.load(Ordering::Acquire);
        let _ = self.msgs.send(FeedMsg::Open {
            format: fmt,
            consumer: cons,
        });
    }

    /// Decode a chunk if needed and push what fits. Sleeps a little when
    /// the ring is full so this thread doesn't spin.
    fn pump(&mut self) {
        if self.pending_off >= self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
            let res = self.source.as_mut().unwrap().decode_into(&mut self.pending);
            match res {
                Ok(0) => {
                    self.next_track();
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    let path = self.source.as_ref().unwrap().info().path.clone();
                    log::warn!("decode error in {}: {e}", path.display());
                    let _ = self.msgs.send(FeedMsg::TrackError {
                        entry: self.entry,
                        error: format!("{}: {e}", path.display()),
                    });
                    self.next_track();
                    return;
                }
            }
        }
        if self.push_pending() == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Push as much pending audio as fits. Returns samples pushed.
    fn push_pending(&mut self) -> usize {
        let Some(ring) = self.ring.as_mut() else {
            self.pending_off = self.pending.len();
            return 0;
        };
        let ch = self.format.map(|f| f.channels as usize).unwrap_or(1);
        let left = self.pending.len() - self.pending_off;
        // Whole frames only, so the renderer never sees half a frame.
        let n = (ring.slots().min(left) / ch) * ch;
        if n == 0 {
            return 0;
        }
        let mut chunk = ring.write_chunk_uninit(n).expect("slots checked");
        let src = &self.pending[self.pending_off..self.pending_off + n];
        let (a, b) = chunk.as_mut_slices();
        for (d, s) in a.iter_mut().zip(src) {
            d.write(*s);
        }
        for (d, s) in b.iter_mut().zip(&src[a.len()..]) {
            d.write(*s);
        }
        // SAFETY: every slot in both slices was just written.
        unsafe { chunk.commit_all() };
        self.pending_off += n;
        self.written += (n / ch) as u64;
        n
    }

    fn push_pending_blocking(&mut self) -> bool {
        while self.pending_off < self.pending.len() {
            if self.push_pending() == 0 {
                if let Ok(c) = self.cmds.try_recv() {
                    self.stashed = Some(c);
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        true
    }

    /// End of the current file: move straight on to whatever's next.
    fn next_track(&mut self) {
        let next = self.queue.lock().unwrap().upcoming(self.entry);
        self.source = None;
        match next {
            Some(n) => self.open_from(n, Duration::ZERO),
            None => self.finish_queue(),
        }
    }

    fn finish_queue(&mut self) {
        self.source = None;
        // Everything decoded is either in the ring or in pending. Pending
        // has to go in before the renderer may call it done.
        if self.ring.is_some() && !self.push_pending_blocking() {
            return;
        }
        self.shared.end_of_queue.store(true, Ordering::Release);
        let _ = self.msgs.send(FeedMsg::EndOfQueue);
    }
}
