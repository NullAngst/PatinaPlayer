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
//!
//! Radio is the exception to "end of stream means next track". A live
//! stream that ends or errors is reconnected with backoff, trying each of
//! the station's mirrors in turn, until it plays again or you stop it.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use rtrb::{Consumer, Producer, RingBuffer};

use super::decoder::{OpenOptions, Source};
use super::output::BitPerfect;
use super::{Depth, SharedRef, StreamFormat};
use crate::queue::{EntryId, Queue};
use crate::track::{MediaKind, TrackInfo};

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
    /// Open a new ring (and so a new output) at the next track, even if
    /// the format is the same. For output settings that changed.
    NewRing,
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
    /// Source seconds per output second. 1 except for sped-up podcasts.
    pub tempo: f64,
    /// Position comes from [`FeedMsg::Mark`]s rather than frame counting,
    /// since silence trimming makes the mapping uneven.
    pub marked: bool,
    pub info: TrackInfo,
}

#[allow(clippy::large_enum_variant)] // a few messages a second at most
pub enum FeedMsg {
    /// A new ring at a new format. The core opens an output for it.
    Open {
        format: StreamFormat,
        consumer: Consumer<f64>,
        /// Play this ring through the bit-perfect output, for a source of
        /// this depth. None is the normal shared output.
        exclusive: Option<Depth>,
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
    /// A live stream's now-playing title changed.
    Title {
        entry: EntryId,
        title: String,
    },
    /// A live stream dropped; the next try is in `wait`.
    Reconnecting {
        entry: EntryId,
        attempt: u32,
        wait: Duration,
        error: String,
    },
    /// Source position at a point in the ring, for filtered playback.
    Mark {
        at_frame: u64,
        pos: Duration,
    },
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

/// A live stream waiting to be reconnected.
struct Retry {
    entry: EntryId,
    attempt: u32,
    at: Instant,
}

struct Feeder {
    shared: SharedRef,
    queue: Arc<Mutex<Queue>>,
    cmds: Receiver<FeedCmd>,
    msgs: Sender<FeedMsg>,
    source: Option<Source>,
    entry: EntryId,
    /// What the queue says about the open entry, with the technical fields
    /// filled in from the stream.
    entry_info: TrackInfo,
    ring: Option<Producer<f64>>,
    format: Option<StreamFormat>,
    /// The open ring plays bit-perfect, for this depth.
    exclusive: Option<Depth>,
    /// Frames pushed into the ring, in `consumed_frames` coordinates.
    written: u64,
    /// Decoded samples not yet in the ring.
    pending: Vec<f64>,
    pending_off: usize,
    stashed: Option<FeedCmd>,
    retry: Option<Retry>,
    last_title_poll: Instant,
    last_mark: u64,
    /// The current segment: first ring frame, track position there, rate.
    seg: (u64, Duration, u32),
}

/// Longest we wait for the renderer to acknowledge a flush. If the output
/// is wedged, we stop waiting and start a fresh ring instead.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

/// How many skips in a row before giving up on a queue full of broken files.
const MAX_CONSECUTIVE_ERRORS: usize = 25;

/// Reconnect backoff: 1, 2, 4, 8, 16, then every 30 s.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.saturating_sub(1).min(5)).min(30))
}

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
            entry_info: TrackInfo::default(),
            ring: None,
            format: None,
            exclusive: None,
            written: 0,
            pending: Vec::with_capacity(32768),
            pending_off: 0,
            stashed: None,
            retry: None,
            last_title_poll: Instant::now(),
            last_mark: 0,
            seg: (0, Duration::ZERO, 0),
        }
    }

    fn next_cmd(&mut self) -> Result<Option<FeedCmd>, ()> {
        if let Some(c) = self.stashed.take() {
            return Ok(Some(c));
        }
        if self.source.is_some() {
            return match self.cmds.try_recv() {
                Ok(c) => Ok(Some(c)),
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => Err(()),
            };
        }
        // Nothing open: wait for a command, or until a reconnect is due.
        match &self.retry {
            Some(r) => {
                let wait = r.at.saturating_duration_since(Instant::now());
                match self.cmds.recv_timeout(wait) {
                    Ok(c) => Ok(Some(c)),
                    Err(RecvTimeoutError::Timeout) => Ok(None),
                    Err(RecvTimeoutError::Disconnected) => Err(()),
                }
            }
            None => self.cmds.recv().map(Some).map_err(|_| ()),
        }
    }

    fn run(mut self) {
        loop {
            let Ok(cmd) = self.next_cmd() else { return };
            if let Some(cmd) = cmd {
                // Commands mean "now": clear the abort the core raised so
                // the next network read isn't cut short.
                self.shared.abort_io.store(false, Ordering::Relaxed);
                match cmd {
                    FeedCmd::Quit => return,
                    FeedCmd::NewRing => self.format = None,
                    FeedCmd::Stop => {
                        self.retry = None;
                        self.flush();
                        self.source = None;
                        let _ = self.msgs.send(FeedMsg::Flushed);
                    }
                    FeedCmd::Start { entry, pos } => {
                        self.retry = None;
                        self.flush();
                        self.source = None;
                        let _ = self.msgs.send(FeedMsg::Flushed);
                        self.open_from(entry, pos);
                    }
                    FeedCmd::Seek(pos) => {
                        if self.source.as_ref().is_some_and(|s| s.is_live()) {
                            continue;
                        }
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
            } else if let Some(r) = &self.retry
                && Instant::now() >= r.at
            {
                let (entry, attempt) = (r.entry, r.attempt);
                self.reconnect(entry, attempt);
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
            match self.open(entry, pos, 1) {
                Opened::Yes | Opened::Interrupted => return,
                Opened::Retrying => return,
                Opened::No => {}
            }
            pos = Duration::ZERO;
            match self.queue.lock().unwrap().step(entry, 1) {
                Some(next) => entry = next,
                None => break,
            }
        }
        self.finish_queue();
    }

    fn open_options(&self, info: &TrackInfo) -> OpenOptions {
        let podcast = info.kind == MediaKind::Podcast;
        OpenOptions {
            live: info.is_live(),
            tempo: if podcast {
                self.shared.podcast_tempo.load()
            } else {
                1.0
            },
            trim_silence: podcast && self.shared.podcast_trim.load(Ordering::Relaxed),
            abort: Some(self.shared.abort_io.clone()),
        }
    }

    /// Open a radio station: resolve its URL, then try each stream.
    fn open_station(&self, info: &TrackInfo) -> Result<Source, String> {
        let url = info.url.clone().ok_or("station has no URL")?;
        let resolved = crate::radio::resolve(&url).map_err(|e| e.to_string())?;
        let mut last = String::new();
        for s in &resolved.streams {
            if self.shared.abort_io.load(Ordering::Relaxed) {
                return Err("interrupted".into());
            }
            match Source::open_with(s, self.open_options(info)) {
                Ok(src) => return Ok(src),
                Err(e) => {
                    log::info!("mirror {s} failed: {e}");
                    last = e.to_string();
                }
            }
        }
        Err(format!("no stream answered ({last})"))
    }

    /// Open one entry. Sends the Segment (and a new ring if needed).
    /// `attempt` numbers the reconnect a failure here would schedule.
    fn open(&mut self, entry: EntryId, pos: Duration, attempt: u32) -> Opened {
        let Some(info) = self
            .queue
            .lock()
            .unwrap()
            .get(entry)
            .map(|e| e.info.clone())
        else {
            return Opened::No;
        };
        let label = info
            .url
            .clone()
            .unwrap_or_else(|| info.path.display().to_string());
        // Whether this is the file as it sits on disk (or as the server
        // has it, untouched), which is what bit-perfect needs.
        let mut original = info.kind == MediaKind::File;
        let opened = if info.is_live() {
            self.open_station(&info)
        } else if let Some(r) = &info.remote {
            // A server song: the URL carries a login, so it's made now.
            crate::remote::registry()
                .stream(r)
                .map_err(|e| e.to_string())
                .and_then(|(url, raw)| {
                    original &= raw;
                    Source::open_with(&url, self.open_options(&info)).map_err(|e| e.to_string())
                })
        } else {
            match info.location() {
                Some(loc) => {
                    Source::open_with(&loc, self.open_options(&info)).map_err(|e| e.to_string())
                }
                None => Err("nothing to open".into()),
            }
        };
        let mut src = match opened {
            Ok(s) => s,
            Err(e) if info.is_live() => {
                // A station that doesn't answer gets retried, not skipped.
                self.schedule_retry(entry, attempt, e);
                return Opened::Retrying;
            }
            Err(e) => {
                self.queue.lock().unwrap().mark_failed(entry);
                let _ = self.msgs.send(FeedMsg::TrackError {
                    entry,
                    error: format!("{label}: {e}"),
                });
                return Opened::No;
            }
        };
        if pos > Duration::ZERO
            && let Err(e) = src.seek(pos)
        {
            log::warn!("seek failed in {label}: {e}");
        }

        // Local files: the file's own tags are the truth. Streams: keep
        // what the queue knows (station name, episode title) and take the
        // technical fields from the stream.
        let merged = match info.kind {
            MediaKind::File if info.remote.is_none() => {
                let mut i = src.info().clone();
                i.kind = MediaKind::File;
                i
            }
            _ => {
                let s = src.info();
                TrackInfo {
                    codec: s.codec.clone(),
                    sample_rate: s.sample_rate,
                    channels: s.channels,
                    bits: s.bits,
                    kbps: s.kbps.or(info.kbps),
                    lossless: s.lossless,
                    duration: info.duration.or(s.duration).filter(|_| !info.is_live()),
                    album: info.album.clone().or_else(|| s.album.clone()),
                    ..info.clone()
                }
            }
        };
        self.queue.lock().unwrap().set_info(entry, merged.clone());

        let fmt = src.format();
        // Bit-perfect is for local files played as they are: never radio,
        // never a sped-up or trimmed podcast.
        let mode = BitPerfect::from_u8(self.shared.bit_perfect.load(Ordering::Relaxed));
        let exclusive = (mode.wanted(false) && original && src.source_position().is_none())
            .then(|| src.depth());
        if self.format != Some(fmt) || self.exclusive != exclusive {
            if !self.drain_ring() {
                // A command arrived while draining; it's stashed and wins.
                return Opened::Interrupted;
            }
            self.exclusive = exclusive;
            self.new_ring(fmt);
        }
        self.entry = entry;
        self.entry_info = merged;
        self.source = Some(src);
        self.last_mark = 0;
        self.announce(pos);
        Opened::Yes
    }

    fn schedule_retry(&mut self, entry: EntryId, attempt: u32, error: String) {
        let wait = backoff(attempt);
        log::info!("stream dropped ({error}), retry {attempt} in {wait:?}");
        let _ = self.msgs.send(FeedMsg::Reconnecting {
            entry,
            attempt,
            wait,
            error,
        });
        self.retry = Some(Retry {
            entry,
            attempt,
            at: Instant::now() + wait,
        });
    }

    fn reconnect(&mut self, entry: EntryId, attempt: u32) {
        self.retry = None;
        if !self.queue.lock().unwrap().contains(entry) {
            return;
        }
        if let Opened::Yes = self.open(entry, Duration::ZERO, attempt + 1) {
            log::info!("stream back after {attempt} attempt(s)");
        }
    }

    fn announce(&mut self, pos: Duration) {
        let Some(src) = &self.source else { return };
        let marked = src.source_position().is_some();
        let tempo = if self.entry_info.kind == MediaKind::Podcast {
            self.shared.podcast_tempo.load().clamp(0.5, 3.0)
        } else {
            1.0
        };
        let start_frame = self.written
            + (self.pending.len() - self.pending_off) as u64
                / self.format.map(|f| f.channels as u64).unwrap_or(1);
        self.seg = (start_frame, pos, src.format().rate);
        let _ = self.msgs.send(FeedMsg::Segment(Segment {
            entry: self.entry,
            start_frame,
            start_pos: pos,
            rate: src.format().rate,
            tempo,
            marked,
            info: self.entry_info.clone(),
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
            exclusive: self.exclusive,
        });
    }

    /// Decode a chunk if needed and push what fits. Sleeps a little when
    /// the ring is full so this thread doesn't spin.
    fn pump(&mut self) {
        let live = self.source.as_ref().is_some_and(|s| s.is_live());
        if live && self.last_title_poll.elapsed() > Duration::from_millis(500) {
            self.last_title_poll = Instant::now();
            if let Some(title) = self.source.as_mut().unwrap().poll_title() {
                let _ = self.msgs.send(FeedMsg::Title {
                    entry: self.entry,
                    title,
                });
            }
        }
        if self.pending_off >= self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
            let res = self.source.as_mut().unwrap().decode_into(&mut self.pending);
            match res {
                Ok(0) if live => {
                    self.source = None;
                    self.schedule_retry(self.entry, 1, "stream ended".into());
                    return;
                }
                Ok(0) => {
                    self.next_track();
                    return;
                }
                Ok(_) => {
                    if self.source.as_mut().unwrap().take_format_change() {
                        self.follow_format_change();
                    }
                }
                Err(e) if live => {
                    self.source = None;
                    self.schedule_retry(self.entry, 1, e.to_string());
                    return;
                }
                Err(e) => {
                    let label = self
                        .entry_info
                        .url
                        .clone()
                        .unwrap_or_else(|| self.entry_info.path.display().to_string());
                    log::warn!("decode error in {label}: {e}");
                    let _ = self.msgs.send(FeedMsg::TrackError {
                        entry: self.entry,
                        error: format!("{label}: {e}"),
                    });
                    self.next_track();
                    return;
                }
            }
        }
        if self.push_pending() == 0 {
            std::thread::sleep(Duration::from_millis(10));
        } else {
            self.mark();
        }
    }

    /// The stream switched rate or channels. What's in the ring plays out in
    /// the old format, then a new ring and output take the new one, and the
    /// track carries on from where it was.
    fn follow_format_change(&mut self) {
        let Some(fmt) = self.source.as_ref().map(|s| s.format()) else {
            return;
        };
        // The freshly decoded samples are already in the new format: keep
        // them out of the old ring.
        let fresh = std::mem::take(&mut self.pending);
        let off = std::mem::replace(&mut self.pending_off, 0);
        let pos = self.position_at(self.written);
        if !self.drain_ring() {
            self.pending = fresh;
            self.pending_off = off;
            return;
        }
        self.new_ring(fmt);
        self.entry_info.sample_rate = fmt.rate;
        self.entry_info.channels = fmt.channels;
        self.queue
            .lock()
            .unwrap()
            .set_info(self.entry, self.entry_info.clone());
        // Announced with nothing pending, so the segment starts at the
        // first of the fresh samples.
        self.announce(pos);
        self.pending = fresh;
        self.pending_off = off;
    }

    /// Track position of ring frame `frame`, from the current segment.
    fn position_at(&self, frame: u64) -> Duration {
        let (start, pos, rate) = self.seg;
        if rate == 0 {
            return pos;
        }
        pos + Duration::from_secs_f64(frame.saturating_sub(start) as f64 / rate as f64)
    }

    /// For filtered playback, tell the core where in the source the audio
    /// just pushed ends. A few marks a second keeps the position honest.
    fn mark(&mut self) {
        let Some(pos) = self.source.as_ref().and_then(|s| s.source_position()) else {
            return;
        };
        let rate = self.format.map(|f| f.rate as u64).unwrap_or(44100);
        if self.pending_off < self.pending.len() || self.written < self.last_mark + rate / 4 {
            return;
        }
        self.last_mark = self.written;
        let _ = self.msgs.send(FeedMsg::Mark {
            at_frame: self.written,
            pos,
        });
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

enum Opened {
    Yes,
    No,
    /// A live stream that didn't answer; a retry is scheduled.
    Retrying,
    /// A command arrived while waiting for the old ring to drain.
    Interrupted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        let s: Vec<u64> = (1..=8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(s, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }
}
