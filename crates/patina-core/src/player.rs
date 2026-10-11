//! The core thread. Frontends send it [`Command`]s and read its
//! [`Snapshot`]; it owns the queue, the decoder thread and the output.
//! The TUI, the GUI and MPRIS all talk to this, which is how they stay
//! feature-identical without writing anything twice.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, unbounded};

use crate::audio::feeder::{self, FeedCmd, FeedMsg, Segment};
use crate::audio::output::{self, Output, OutputConfig};
use crate::audio::render::Renderer;
use crate::audio::{Shared, SharedRef, gain_to_db, volume_to_gain};
use crate::queue::{EntryId, Queue, Repeat};
use crate::track::TrackInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Replace the queue with files (tags read in the background) and
    /// start playing the one at index `start`.
    SetQueue {
        paths: Vec<PathBuf>,
        start: usize,
    },
    /// Add files to the end of the queue.
    Append(Vec<PathBuf>),
    /// Replace the queue with tracks whose tags are known (from the
    /// library) and start at index `start`.
    SetQueueTracks {
        tracks: Vec<TrackInfo>,
        start: usize,
    },
    AppendTracks(Vec<TrackInfo>),
    /// Insert right after the current track.
    PlayNext(Vec<TrackInfo>),
    /// Play this queue entry from the start.
    PlayEntry(EntryId),
    /// Move an entry to a new position in play order.
    MoveEntry {
        id: EntryId,
        to: usize,
    },
    /// Remove an entry. Removing the playing track moves on to the next.
    RemoveEntry(EntryId),
    ClearQueue,
    TogglePause,
    Play,
    Pause,
    Stop,
    Next,
    /// Restart the track if more than 3 seconds in, otherwise go back one.
    Back,
    Seek(Duration),
    /// Relative seek in seconds, negative goes back.
    SeekBy(f64),
    /// Volume slider position, 0.0 to 1.0.
    SetVolume(f64),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    /// Fade length for pause, resume, seek and stop. 0 turns fades off.
    SetFadeMs(u32),
    Quit,
}

#[derive(Debug, Clone)]
pub enum Event {
    TrackChanged(Option<EntryId>),
    StateChanged(PlayState),
    QueueChanged,
    Error(String),
    /// The last track in the queue finished.
    Finished,
}

/// What the frontends draw. Cloned out on every read; it's small.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub state: PlayState,
    pub current: Option<EntryId>,
    pub track: Option<TrackInfo>,
    pub position: Duration,
    pub duration: Option<Duration>,
    /// Slider position, 0.0 to 1.0.
    pub volume: f64,
    pub volume_db: f64,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// Output device in use, "PipeWire" or "ALSA default f32".
    pub output: Option<String>,
    /// Playing but the ring ran dry.
    pub buffering: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    pub output: OutputConfig,
    pub volume: f64,
    pub fade_ms: u32,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            output: OutputConfig::default(),
            volume: 0.8,
            fade_ms: 100,
        }
    }
}

/// Called when a track has been listened to for half its length or four
/// minutes, whichever comes first. Counts real listening: time skipped by
/// seeking forward doesn't count.
pub type PlayedHook = Box<dyn Fn(&TrackInfo) + Send>;

#[derive(Default)]
pub struct Hooks {
    pub played: Option<PlayedHook>,
    /// Replaces the output backend, for tests that capture the audio.
    pub output: Option<OutputFactory>,
}

/// Handle to the running core. Cheap to clone.
#[derive(Clone)]
pub struct Player {
    cmds: Sender<Command>,
    events: Receiver<Event>,
    snapshot: Arc<Mutex<Snapshot>>,
    queue: Arc<Mutex<Queue>>,
}

impl Player {
    pub fn spawn(cfg: PlayerConfig) -> Self {
        Self::spawn_with(cfg, Hooks::default())
    }

    pub fn spawn_with(cfg: PlayerConfig, hooks: Hooks) -> Self {
        let (cmd_tx, cmd_rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let queue = Arc::new(Mutex::new(Queue::new()));
        let core = Core::new(cfg, hooks, cmd_rx, ev_tx, snapshot.clone(), queue.clone());
        std::thread::Builder::new()
            .name("patina-core".into())
            .spawn(move || core.run())
            .expect("spawn core thread");
        Self {
            cmds: cmd_tx,
            events: ev_rx,
            snapshot,
            queue,
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.cmds.send(cmd);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Locked view of the queue, for drawing. Keep the guard short.
    pub fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap()
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }
}

pub type OutputFactory =
    Box<dyn Fn(&OutputConfig, Renderer) -> Result<Box<dyn Output>, String> + Send>;

struct Core {
    cfg: PlayerConfig,
    factory: Option<OutputFactory>,
    played_hook: Option<PlayedHook>,
    /// Listening time on the current track, and whether it was counted.
    listened: Duration,
    counted: bool,
    last_pos: Option<Duration>,
    seg_start: Option<u64>,
    cmds: Receiver<Command>,
    events: Sender<Event>,
    snapshot: Arc<Mutex<Snapshot>>,
    queue: Arc<Mutex<Queue>>,
    shared: SharedRef,
    feed_tx: Sender<FeedCmd>,
    feed_rx: Receiver<FeedMsg>,
    output: Option<Box<dyn Output>>,
    segments: VecDeque<Segment>,
    state: PlayState,
    current: Option<EntryId>,
    volume: f64,
    muted: bool,
    /// Turn sound back on once the decoder announces the new segment.
    resume_on_segment: bool,
    last_error: Option<String>,
    probe_tx: Sender<Vec<(u64, PathBuf)>>,
}

impl Core {
    fn new(
        cfg: PlayerConfig,
        hooks: Hooks,
        cmds: Receiver<Command>,
        events: Sender<Event>,
        snapshot: Arc<Mutex<Snapshot>>,
        queue: Arc<Mutex<Queue>>,
    ) -> Self {
        let shared: SharedRef = Arc::new(Shared::default());
        shared.fade_ms.store(cfg.fade_ms, Ordering::Relaxed);
        shared.volume.store(volume_to_gain(cfg.volume));
        let (feed_tx, feed_cmd_rx) = unbounded();
        let (feed_msg_tx, feed_rx) = unbounded();
        feeder::spawn(shared.clone(), queue.clone(), feed_cmd_rx, feed_msg_tx);
        let probe_tx = spawn_prober(queue.clone(), events.clone());
        let volume = cfg.volume;
        Self {
            cfg,
            factory: hooks.output,
            played_hook: hooks.played,
            listened: Duration::ZERO,
            counted: false,
            last_pos: None,
            seg_start: None,
            cmds,
            events,
            snapshot,
            queue,
            shared,
            feed_tx,
            feed_rx,
            output: None,
            segments: VecDeque::new(),
            state: PlayState::Stopped,
            current: None,
            volume,
            muted: false,
            resume_on_segment: false,
            last_error: None,
            probe_tx,
        }
    }

    fn run(mut self) {
        let tick = Duration::from_millis(30);
        loop {
            select! {
                recv(self.cmds) -> cmd => match cmd {
                    Ok(Command::Quit) | Err(_) => break,
                    Ok(c) => self.command(c),
                },
                recv(self.feed_rx) -> msg => {
                    if let Ok(m) = msg { self.feed_msg(m) }
                },
                default(tick) => {}
            }
            self.update();
        }
        self.fade_out_and_wait();
        let _ = self.feed_tx.send(FeedCmd::Quit);
        self.output = None;
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn set_state(&mut self, s: PlayState) {
        if self.state != s {
            self.state = s;
            self.emit(Event::StateChanged(s));
        }
    }

    fn apply_volume(&self) {
        let g = if self.muted {
            0.0
        } else {
            volume_to_gain(self.volume)
        };
        self.shared.volume.store(g);
    }

    /// Ramp the output down and wait for it to go quiet, so the next
    /// thing that happens to the stream doesn't pop.
    fn fade_out_and_wait(&self) {
        let was = self.shared.playing.swap(false, Ordering::Relaxed);
        if !was || self.output.is_none() {
            return;
        }
        let fade = self.shared.fade_ms.load(Ordering::Relaxed) as u64;
        let deadline = Instant::now() + Duration::from_millis(fade + 60);
        while !self.shared.silent.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn position(&self) -> Duration {
        let Some(seg) = self.segments.back() else {
            return Duration::ZERO;
        };
        let consumed = self.shared.consumed_frames.load(Ordering::Relaxed);
        // Use the segment that is actually playing, not the newest.
        let seg = self
            .segments
            .iter()
            .rev()
            .find(|s| s.start_frame <= consumed)
            .unwrap_or(seg);
        let frames = consumed.saturating_sub(seg.start_frame);
        seg.start_pos + Duration::from_secs_f64(frames as f64 / seg.rate.max(1) as f64)
    }

    fn start_entry(&mut self, entry: EntryId, pos: Duration) {
        self.fade_out_and_wait();
        self.segments.clear();
        self.resume_on_segment = true;
        self.set_state(PlayState::Playing);
        let _ = self.feed_tx.send(FeedCmd::Start { entry, pos });
    }

    fn seek(&mut self, pos: Duration) {
        if self.state == PlayState::Stopped {
            if let Some(cur) = self.current {
                self.start_entry(cur, pos);
            }
            return;
        }
        let pos = match self.snapshot.lock().unwrap().duration {
            Some(d) => pos.min(d.saturating_sub(Duration::from_millis(250))),
            None => pos,
        };
        let playing = self.state == PlayState::Playing;
        self.fade_out_and_wait();
        self.segments.clear();
        self.resume_on_segment = playing;
        let _ = self.feed_tx.send(FeedCmd::Seek(pos));
    }

    fn stop(&mut self) {
        self.fade_out_and_wait();
        self.resume_on_segment = false;
        let _ = self.feed_tx.send(FeedCmd::Stop);
        self.segments.clear();
        self.set_state(PlayState::Stopped);
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::SetQueue { paths, start } => {
                self.stop();
                let added = {
                    let mut q = self.queue.lock().unwrap();
                    q.clear();
                    q.append(paths)
                };
                let _ = self.probe_tx.send(added.clone());
                self.emit(Event::QueueChanged);
                if let Some(&(id, _)) = added.get(start) {
                    self.start_entry(id, Duration::ZERO);
                }
            }
            Command::SetQueueTracks { tracks, start } => {
                self.stop();
                let ids = {
                    let mut q = self.queue.lock().unwrap();
                    q.clear();
                    q.append_tracks(tracks)
                };
                self.emit(Event::QueueChanged);
                if let Some(&id) = ids.get(start) {
                    self.start_entry(id, Duration::ZERO);
                }
            }
            Command::AppendTracks(tracks) => {
                self.queue.lock().unwrap().append_tracks(tracks);
                self.emit(Event::QueueChanged);
            }
            Command::PlayNext(tracks) => {
                self.queue.lock().unwrap().insert_next(tracks);
                self.emit(Event::QueueChanged);
            }
            Command::MoveEntry { id, to } => {
                if self.queue.lock().unwrap().move_to(id, to) {
                    self.emit(Event::QueueChanged);
                }
            }
            Command::RemoveEntry(id) => {
                let next = {
                    let q = self.queue.lock().unwrap();
                    (self.current == Some(id)).then(|| q.step(id, 1)).flatten()
                };
                let was_current = self.current == Some(id);
                if !self.queue.lock().unwrap().remove(id) {
                    return;
                }
                if was_current && self.state != PlayState::Stopped {
                    match next {
                        Some(n) => self.start_entry(n, Duration::ZERO),
                        None => self.stop(),
                    }
                }
                if was_current {
                    self.current = None;
                }
                self.emit(Event::QueueChanged);
            }
            Command::ClearQueue => {
                self.stop();
                self.queue.lock().unwrap().clear();
                self.current = None;
                self.emit(Event::QueueChanged);
            }
            Command::Append(paths) => {
                let added = self.queue.lock().unwrap().append(paths);
                let _ = self.probe_tx.send(added);
                self.emit(Event::QueueChanged);
            }
            Command::PlayEntry(id) => {
                if self.queue.lock().unwrap().contains(id) {
                    self.start_entry(id, Duration::ZERO);
                }
            }
            Command::TogglePause => match self.state {
                PlayState::Playing => self.command(Command::Pause),
                _ => self.command(Command::Play),
            },
            Command::Play => match self.state {
                PlayState::Paused => {
                    self.shared.playing.store(true, Ordering::Relaxed);
                    self.set_state(PlayState::Playing);
                }
                PlayState::Stopped => {
                    let first = {
                        let q = self.queue.lock().unwrap();
                        self.current
                            .filter(|&c| q.contains(c))
                            .or_else(|| q.first())
                    };
                    if let Some(e) = first {
                        self.start_entry(e, Duration::ZERO);
                    }
                }
                PlayState::Playing => {}
            },
            Command::Pause => {
                if self.state == PlayState::Playing {
                    self.shared.playing.store(false, Ordering::Relaxed);
                    self.resume_on_segment = false;
                    self.set_state(PlayState::Paused);
                }
            }
            Command::Stop => self.stop(),
            Command::Next => {
                let Some(cur) = self.current else { return };
                let next = self.queue.lock().unwrap().step(cur, 1);
                if let Some(n) = next {
                    self.start_entry(n, Duration::ZERO);
                }
            }
            Command::Back => {
                let Some(cur) = self.current else { return };
                if self.state != PlayState::Stopped && self.position() > Duration::from_secs(3) {
                    self.seek(Duration::ZERO);
                    return;
                }
                let prev = self.queue.lock().unwrap().step(cur, -1);
                match prev {
                    Some(p) => self.start_entry(p, Duration::ZERO),
                    None => self.seek(Duration::ZERO),
                }
            }
            Command::Seek(pos) => self.seek(pos),
            Command::SeekBy(secs) => {
                let now = self.position().as_secs_f64();
                self.seek(Duration::from_secs_f64((now + secs).max(0.0)));
            }
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.muted = false;
                self.apply_volume();
            }
            Command::ToggleMute => {
                self.muted = !self.muted;
                self.apply_volume();
            }
            Command::ToggleShuffle => {
                let mut q = self.queue.lock().unwrap();
                let on = !q.shuffle();
                q.set_shuffle(on);
                drop(q);
                self.emit(Event::QueueChanged);
            }
            Command::CycleRepeat => {
                let mut q = self.queue.lock().unwrap();
                q.repeat = q.repeat.cycle();
                q.version += 1;
            }
            Command::SetFadeMs(ms) => self.shared.fade_ms.store(ms.min(2000), Ordering::Relaxed),
            Command::Quit => {}
        }
    }

    fn feed_msg(&mut self, msg: FeedMsg) {
        match msg {
            FeedMsg::Open { format, consumer } => {
                self.output = None;
                let renderer = Renderer::new(consumer, self.shared.clone(), format);
                let res = match &self.factory {
                    Some(f) => f(&self.cfg.output, renderer),
                    None => output::open(&self.cfg.output, renderer).map_err(|e| e.to_string()),
                };
                match res {
                    Ok(o) => {
                        log::info!(
                            "output open: {} at {} Hz, {} ch",
                            o.describe(),
                            format.rate,
                            format.channels
                        );
                        self.output = Some(o);
                    }
                    Err(e) => {
                        self.report(format!("can't open output: {e}"));
                        self.stop();
                    }
                }
            }
            FeedMsg::Segment(seg) => {
                if self.resume_on_segment && self.state == PlayState::Playing {
                    self.shared.playing.store(true, Ordering::Relaxed);
                    self.resume_on_segment = false;
                }
                self.segments.push_back(seg);
            }
            FeedMsg::Flushed => {}
            FeedMsg::TrackError { error, .. } => self.report(error),
            FeedMsg::EndOfQueue => {}
        }
    }

    fn report(&mut self, msg: String) {
        log::warn!("{msg}");
        self.last_error = Some(msg.clone());
        self.emit(Event::Error(msg));
    }

    fn reset_listening(&mut self) {
        self.listened = Duration::ZERO;
        self.counted = false;
        self.last_pos = None;
    }

    /// Add up time actually heard. A jump of more than two seconds between
    /// ticks is a seek, not listening.
    fn count_listening(&mut self, pos: Duration, track: Option<&TrackInfo>) {
        if let Some(last) = self.last_pos
            && pos > last
            && pos - last < Duration::from_secs(2)
        {
            self.listened += pos - last;
        }
        self.last_pos = Some(pos);
        let Some(info) = track else { return };
        if self.counted {
            return;
        }
        let threshold = info
            .duration
            .map(|d| (d / 2).min(Duration::from_secs(240)))
            .unwrap_or(Duration::from_secs(240));
        if self.listened >= threshold && !threshold.is_zero() {
            self.counted = true;
            if let Some(h) = &self.played_hook {
                h(info);
            }
        }
    }

    /// Runs every tick: work out what's playing and refresh the snapshot.
    fn update(&mut self) {
        let consumed = self.shared.consumed_frames.load(Ordering::Relaxed);

        // Drop segments that are fully behind the play head.
        while self.segments.len() > 1 && self.segments[1].start_frame <= consumed {
            self.segments.pop_front();
        }
        let playing_seg = self
            .segments
            .front()
            .filter(|s| s.start_frame <= consumed || self.segments.len() == 1);
        let entry = playing_seg.map(|s| s.entry);
        let seg_info = playing_seg.map(|s| (s.start_frame, s.start_pos.is_zero()));
        // A segment that starts a track from zero is a fresh listen, which
        // also covers Repeat One playing the same entry again.
        if let Some((start, from_zero)) = seg_info {
            if from_zero && Some(start) != self.seg_start {
                self.reset_listening();
            }
            self.seg_start = Some(start);
        }
        if entry.is_some() && entry != self.current {
            self.reset_listening();
            self.current = entry;
            self.queue.lock().unwrap().set_current(entry);
            self.emit(Event::TrackChanged(entry));
        }

        if self.shared.drained.swap(false, Ordering::AcqRel) {
            self.shared.playing.store(false, Ordering::Relaxed);
            self.shared.end_of_queue.store(false, Ordering::Release);
            self.segments.clear();
            self.output = None;
            self.set_state(PlayState::Stopped);
            self.emit(Event::Finished);
        }

        if let Some(e) = self.output.as_ref().and_then(|o| o.error()) {
            self.output = None;
            self.report(format!("output failed: {e}"));
            self.stop();
        }

        let buffering =
            self.shared.underrun.swap(false, Ordering::Relaxed) && self.state == PlayState::Playing;
        let position = self.position();
        let track = self.segments.front().map(|s| s.info.clone());
        if self.state == PlayState::Playing {
            self.count_listening(position, track.as_ref());
        } else {
            self.last_pos = None;
        }
        let (shuffle, repeat) = {
            let q = self.queue.lock().unwrap();
            (q.shuffle(), q.repeat)
        };
        let gain = if self.muted {
            0.0
        } else {
            volume_to_gain(self.volume)
        };
        let mut snap = self.snapshot.lock().unwrap();
        snap.state = self.state;
        snap.current = self.current;
        if track.is_some() || self.state != PlayState::Stopped {
            snap.duration = track.as_ref().and_then(|t| t.duration);
            snap.track = track;
        }
        snap.position = if self.state == PlayState::Stopped {
            Duration::ZERO
        } else {
            position
        };
        snap.volume = self.volume;
        snap.volume_db = gain_to_db(gain);
        snap.muted = self.muted;
        snap.shuffle = shuffle;
        snap.repeat = repeat;
        snap.output = self.output.as_ref().map(|o| o.describe());
        snap.buffering = buffering;
        snap.last_error = self.last_error.clone();
    }
}

/// Reads tags for newly queued files in the background, so a big folder
/// shows up instantly and fills in its titles a moment later.
fn spawn_prober(queue: Arc<Mutex<Queue>>, events: Sender<Event>) -> Sender<Vec<(u64, PathBuf)>> {
    let (tx, rx) = unbounded::<Vec<(u64, PathBuf)>>();
    std::thread::Builder::new()
        .name("patina-probe".into())
        .spawn(move || {
            for batch in rx {
                let mut last_emit = Instant::now();
                for (id, path) in batch {
                    let info =
                        TrackInfo::probe(&path).unwrap_or_else(|_| TrackInfo::unprobed(&path));
                    queue.lock().unwrap().set_info(id, info);
                    if last_emit.elapsed() > Duration::from_millis(100) {
                        let _ = events.send(Event::QueueChanged);
                        last_emit = Instant::now();
                    }
                }
                let _ = events.send(Event::QueueChanged);
            }
        })
        .expect("spawn probe thread");
    tx
}
