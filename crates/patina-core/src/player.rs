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
use crate::audio::output::exclusive::{self, ExclusiveInfo, ExclusiveOutput};
use crate::audio::output::{self, Backend, BitPerfect, Output, OutputConfig};
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
        /// Where in that track to begin, for resuming a podcast episode.
        pos: Duration,
    },
    AppendTracks(Vec<TrackInfo>),
    /// Replace the queue and play it from the top, either in the given
    /// order (shuffle off) or shuffled (shuffle on). Leaves shuffle set that
    /// way, so toggling it later flips between the two.
    PlayAll {
        tracks: Vec<TrackInfo>,
        shuffle: bool,
    },
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
    /// Bit-perfect mode and device. A local file playing now switches over
    /// at once, from where it is.
    SetBitPerfect {
        mode: BitPerfect,
        device: Option<String>,
    },
    /// Output backend, device and buffer. The output reopens at once.
    SetOutput {
        backend: Backend,
        device: Option<String>,
        buffer_ms: u32,
    },
    /// Podcast speed (0.5 to 3, pitch kept) and silence trimming. Takes
    /// effect at once on a playing episode.
    SetPodcastPlayback {
        tempo: f64,
        trim_silence: bool,
    },
    /// Leveling settings. The playing track's gain follows within a
    /// couple of seconds (what's already decoded plays out).
    SetLeveling(crate::level::LevelConfig),
    /// A mix is playing, or stopped: Smart leveling treats a mix like
    /// shuffle.
    SetMixing(bool),
    /// EQ settings, as the renderer runs them. Heard within one output
    /// block.
    SetEq(Box<crate::dsp::EqParams>),
    /// The true-peak limiter: on or off, and its ceiling in dBTP.
    SetLimiter {
        on: bool,
        ceiling_db: f64,
    },
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
    /// A live stream's now-playing title changed.
    NowPlaying(String),
    /// A track was listened to long enough to count as a play.
    Played(PathBuf),
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
    /// Playing a live stream: no duration, no seeking.
    pub live: bool,
    /// Now-playing title from the stream (radio).
    pub live_title: Option<String>,
    /// Set while a dropped stream is waiting to reconnect.
    pub reconnect: Option<Reconnect>,
    /// Podcast playback speed and trimming, as currently set.
    pub podcast_tempo: f64,
    pub podcast_trim: bool,
    pub bit_perfect: BitPerfect,
    /// Set while the exclusive output is playing.
    pub exclusive: Option<ExclusiveInfo>,
    /// Why bit-perfect was wanted for this track but isn't happening.
    pub bit_perfect_refused: Option<String>,
    /// Volume can't move: exclusive output on a DAC with no volume control.
    pub volume_locked: bool,
    /// The leveling gain on what's playing, and where it came from.
    pub level: Option<LevelView>,
    /// Where the sound is going, for per-device EQ. Kept from the last
    /// output while nothing is open.
    pub output_device: Option<crate::eq::OutputDevice>,
    /// How hard the limiter worked lately, in dB of gain reduction. Held
    /// for a moment after it lets go, so it can be seen.
    pub limiting_db: f64,
}

/// The leveling gain for the bottom bar: "-4.2 dB album".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelView {
    pub gain_db: f64,
    pub source: crate::level::Source,
}

impl LevelView {
    /// "-4.2 dB album", or "leveling off (bit-perfect)".
    pub fn text(&self) -> String {
        use crate::level::Source;
        match self.source {
            Source::Off => "leveling off".into(),
            Source::BitPerfect => "leveling off (bit-perfect)".into(),
            s => format!("{:+.1} dB {}", self.gain_db, s.label()),
        }
    }

    /// Just the number, for a narrow bar: "-4.2 dB", "-6.0 dB?" while
    /// it's a guess.
    pub fn short(&self) -> String {
        use crate::level::Source;
        match self.source {
            Source::Off | Source::BitPerfect => String::new(),
            Source::Fallback => format!("{:+.1} dB?", self.gain_db),
            Source::Live => format!("~{:+.1} dB", self.gain_db),
            _ => format!("{:+.1} dB", self.gain_db),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Reconnect {
    pub attempt: u32,
    pub at: Instant,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    pub output: OutputConfig,
    pub volume: f64,
    pub fade_ms: u32,
    pub podcast_tempo: f64,
    pub podcast_trim: bool,
    pub leveling: crate::level::LevelConfig,
    pub eq: crate::dsp::EqParams,
    pub limiter_on: bool,
    /// dBTP, -6 to 0.
    pub limiter_ceiling_db: f64,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            output: OutputConfig::default(),
            volume: 0.8,
            fade_ms: 100,
            podcast_tempo: 1.0,
            podcast_trim: false,
            leveling: crate::level::LevelConfig::default(),
            eq: crate::dsp::EqParams::default(),
            limiter_on: true,
            limiter_ceiling_db: -1.0,
        }
    }
}

/// Called when a track has been listened to for half its length or four
/// minutes, whichever comes first. Counts real listening: time skipped by
/// seeking forward doesn't count.
pub type PlayedHook = Box<dyn Fn(&TrackInfo) + Send>;

/// Called with a podcast episode's position while it plays (every few
/// seconds, and when it stops or changes), and `finished` once it's done.
pub type ProgressHook = Box<dyn Fn(&TrackInfo, Duration, bool) + Send>;

#[derive(Default)]
pub struct Hooks {
    pub played: Option<PlayedHook>,
    pub progress: Option<ProgressHook>,
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
    progress_hook: Option<ProgressHook>,
    /// The podcast episode being tracked for resume, its last position,
    /// and when it was last reported.
    progress: Option<(TrackInfo, Duration, Instant)>,
    marks: VecDeque<(u64, Duration)>,
    live_title: Option<String>,
    reconnect: Option<Reconnect>,
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
    /// The reason the last exclusive open failed, while that track plays.
    exclusive_refused: Option<String>,
    last_device: Option<crate::eq::OutputDevice>,
    /// The limiter's deepest reduction lately, and when it was seen.
    limit_hold: (f64, Instant),
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
        shared
            .podcast_tempo
            .store(cfg.podcast_tempo.clamp(0.5, 3.0));
        shared
            .podcast_trim
            .store(cfg.podcast_trim, Ordering::Relaxed);
        shared
            .bit_perfect
            .store(cfg.output.bit_perfect.to_u8(), Ordering::Relaxed);
        shared.level.set(&cfg.leveling);
        shared.set_eq(cfg.eq);
        shared.limiter_on.store(cfg.limiter_on, Ordering::Relaxed);
        shared
            .limiter_ceiling
            .store(10f64.powf(cfg.limiter_ceiling_db.clamp(-6.0, 0.0) / 20.0));
        let (feed_tx, feed_cmd_rx) = unbounded();
        let (feed_msg_tx, feed_rx) = unbounded();
        feeder::spawn(shared.clone(), queue.clone(), feed_cmd_rx, feed_msg_tx);
        let probe_tx = spawn_prober(queue.clone(), events.clone());
        let volume = cfg.volume;
        Self {
            cfg,
            factory: hooks.output,
            played_hook: hooks.played,
            progress_hook: hooks.progress,
            progress: None,
            marks: VecDeque::new(),
            live_title: None,
            reconnect: None,
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
            exclusive_refused: None,
            last_device: None,
            limit_hold: (0.0, Instant::now()),
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
        self.flush_progress(false);
        self.send_feed(FeedCmd::Quit);
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
        let slider = if self.muted { 0.0 } else { self.volume };
        if let Some(o) = self.output.as_ref().filter(|o| o.exclusive().is_some()) {
            // Bit-perfect: the samples go out at unity. The DAC's own mixer
            // does the volume if it has one; otherwise volume stays at
            // 100% (mute still mutes).
            o.set_hw_volume(slider);
            self.shared.volume.store(if self.muted { 0.0 } else { 1.0 });
            return;
        }
        self.shared.volume.store(volume_to_gain(slider));
    }

    /// Start the current entry over from where it is, so a changed output
    /// setting takes effect. Paused stays paused.
    fn restart_here(&mut self) {
        let Some(cur) = self.current else { return };
        if self.state == PlayState::Stopped || self.current_is_live() {
            return;
        }
        let pos = self.position();
        let paused = self.state == PlayState::Paused;
        self.start_entry(cur, pos);
        if paused {
            self.resume_on_segment = false;
            self.set_state(PlayState::Paused);
        }
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

    /// Tell the decoder something, cutting short any network read it's
    /// stuck in so it hears about it now.
    fn send_feed(&self, cmd: FeedCmd) {
        self.shared.abort_io.store(true, Ordering::Relaxed);
        let _ = self.feed_tx.send(cmd);
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
        let rate = seg.rate.max(1) as f64;
        // Filtered playback: start from the newest mark already played.
        if seg.marked
            && let Some(&(at, pos)) = self
                .marks
                .iter()
                .rev()
                .find(|(at, _)| *at <= consumed && *at >= seg.start_frame)
        {
            let frames = consumed - at;
            return pos + Duration::from_secs_f64(frames as f64 / rate * seg.tempo);
        }
        let frames = consumed.saturating_sub(seg.start_frame);
        seg.start_pos + Duration::from_secs_f64(frames as f64 / rate * seg.tempo)
    }

    /// Report the tracked podcast episode's position, and forget it.
    fn flush_progress(&mut self, finished: bool) {
        if let Some((info, pos, _)) = self.progress.take()
            && let Some(h) = &self.progress_hook
        {
            let done = finished
                || info.duration.is_some_and(|d| {
                    pos + Duration::from_secs(30) >= d
                        || pos.as_secs_f64() >= d.as_secs_f64() * 0.95
                });
            h(&info, pos, done);
        }
    }

    fn clear_segments(&mut self) {
        self.segments.clear();
        self.marks.clear();
    }

    fn start_entry(&mut self, entry: EntryId, pos: Duration) {
        self.fade_out_and_wait();
        self.flush_progress(false);
        self.clear_segments();
        self.live_title = None;
        self.reconnect = None;
        self.resume_on_segment = true;
        self.set_state(PlayState::Playing);
        self.send_feed(FeedCmd::Start { entry, pos });
    }

    fn current_is_live(&self) -> bool {
        self.segments.front().is_some_and(|s| s.info.is_live()) || self.reconnect.is_some()
    }

    fn seek(&mut self, pos: Duration) {
        if self.state == PlayState::Stopped {
            if let Some(cur) = self.current {
                self.start_entry(cur, pos);
            }
            return;
        }
        if self.current_is_live() {
            return;
        }
        let pos = match self.snapshot.lock().unwrap().duration {
            Some(d) => pos.min(d.saturating_sub(Duration::from_millis(250))),
            None => pos,
        };
        let playing = self.state == PlayState::Playing;
        self.fade_out_and_wait();
        self.clear_segments();
        self.resume_on_segment = playing;
        self.send_feed(FeedCmd::Seek(pos));
    }

    fn stop(&mut self) {
        self.fade_out_and_wait();
        self.flush_progress(false);
        self.resume_on_segment = false;
        self.send_feed(FeedCmd::Stop);
        self.clear_segments();
        self.live_title = None;
        self.reconnect = None;
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
            Command::PlayAll { tracks, shuffle } => {
                self.stop();
                let first = {
                    let mut q = self.queue.lock().unwrap();
                    q.clear();
                    q.set_shuffle(shuffle);
                    q.append_tracks(tracks);
                    q.at(0).map(|e| e.id)
                };
                self.emit(Event::QueueChanged);
                if let Some(id) = first {
                    self.start_entry(id, Duration::ZERO);
                }
            }
            Command::SetQueueTracks { tracks, start, pos } => {
                self.stop();
                let ids = {
                    let mut q = self.queue.lock().unwrap();
                    q.clear();
                    q.append_tracks(tracks)
                };
                self.emit(Event::QueueChanged);
                if let Some(&id) = ids.get(start) {
                    self.start_entry(id, pos);
                }
            }
            Command::SetPodcastPlayback {
                tempo,
                trim_silence,
            } => {
                let tempo = tempo.clamp(0.5, 3.0);
                let changed = (self.shared.podcast_tempo.load() - tempo).abs() > 1e-3
                    || self.shared.podcast_trim.load(Ordering::Relaxed) != trim_silence;
                self.shared.podcast_tempo.store(tempo);
                self.shared
                    .podcast_trim
                    .store(trim_silence, Ordering::Relaxed);
                // A playing episode is reopened where it is, so the change
                // is heard now rather than two seconds of buffer later.
                let podcast = self
                    .segments
                    .front()
                    .is_some_and(|s| s.info.kind == crate::track::MediaKind::Podcast);
                if changed && podcast && self.state != PlayState::Stopped {
                    let pos = self.position();
                    let entry = self.segments.front().unwrap().entry;
                    let playing = self.state == PlayState::Playing;
                    self.fade_out_and_wait();
                    self.clear_segments();
                    self.resume_on_segment = playing;
                    self.send_feed(FeedCmd::Start { entry, pos });
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
                    // Save the episode position on pause too.
                    if let Some((info, pos, _)) = &self.progress
                        && let Some(h) = &self.progress_hook
                    {
                        h(info, *pos, false);
                    }
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
            Command::SetBitPerfect { mode, device } => {
                self.cfg.output.bit_perfect = mode;
                self.cfg.output.bit_perfect_device = device;
                self.shared
                    .bit_perfect
                    .store(mode.to_u8(), Ordering::Relaxed);
                self.exclusive_refused = None;
                // Close whatever's open, so the next open goes the new way.
                self.restart_here();
            }
            Command::SetLeveling(c) => {
                let was_off = self.shared.level.mode() == crate::level::LevelMode::Off;
                self.cfg.leveling = c.clone();
                self.shared.level.set(&c);
                // Auto bit-perfect goes on and off with leveling.
                let now_off = c.mode == crate::level::LevelMode::Off;
                if was_off != now_off && self.cfg.output.bit_perfect == BitPerfect::Auto {
                    self.exclusive_refused = None;
                    self.restart_here();
                }
            }
            Command::SetEq(p) => {
                let before = self.shared.dsp_active();
                self.shared.set_eq(*p);
                // Auto bit-perfect goes on and off with what changes the
                // sound.
                if before != self.shared.dsp_active()
                    && self.cfg.output.bit_perfect == BitPerfect::Auto
                {
                    self.exclusive_refused = None;
                    self.restart_here();
                }
            }
            Command::SetLimiter { on, ceiling_db } => {
                self.shared.limiter_on.store(on, Ordering::Relaxed);
                self.shared
                    .limiter_ceiling
                    .store(10f64.powf(ceiling_db.clamp(-6.0, 0.0) / 20.0));
            }
            Command::SetMixing(on) => {
                let l = &self.shared.level;
                if l.mixing.swap(on, Ordering::Relaxed) != on {
                    l.generation.fetch_add(1, Ordering::Release);
                }
            }
            Command::SetOutput {
                backend,
                device,
                buffer_ms,
            } => {
                self.cfg.output.backend = backend;
                self.cfg.output.device = device;
                self.cfg.output.buffer_ms = buffer_ms.clamp(50, 1000);
                self.send_feed(FeedCmd::NewRing);
                self.restart_here();
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
            FeedMsg::Open {
                format,
                consumer,
                exclusive,
            } => {
                self.output = None;
                if exclusive.is_some() {
                    // Unity from the very first sample. A renderer ramps
                    // from the volume it was made with.
                    self.shared.volume.store(if self.muted { 0.0 } else { 1.0 });
                }
                let mut renderer = Renderer::new(consumer, self.shared.clone(), format);
                if let Some(depth) = exclusive {
                    match ExclusiveOutput::try_start(
                        &self.cfg.output,
                        depth,
                        renderer,
                        self.shared.clone(),
                    ) {
                        Ok(o) => {
                            log::info!(
                                "bit-perfect output: {} for {}",
                                o.describe(),
                                exclusive::describe(depth, format.rate)
                            );
                            self.exclusive_refused = None;
                            self.output = Some(Box::new(o));
                            self.apply_volume();
                            return;
                        }
                        Err(refused) => {
                            let (reason, r) = *refused;
                            // Never quietly: say why, then play through the
                            // shared output so the music doesn't stop.
                            let msg = format!(
                                "Bit-perfect unavailable for {}: {reason}",
                                exclusive::describe(depth, format.rate)
                            );
                            if self.exclusive_refused.as_deref() != Some(msg.as_str()) {
                                self.emit(Event::Error(msg.clone()));
                            }
                            self.exclusive_refused = Some(msg);
                            renderer = r;
                            let slider = if self.muted { 0.0 } else { self.volume };
                            self.shared.volume.store(volume_to_gain(slider));
                            renderer.sync_volume();
                        }
                    }
                }
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
                        self.apply_volume();
                    }
                    Err(e) => {
                        self.report(format!("can't open output: {e}"));
                        self.stop();
                    }
                }
            }
            FeedMsg::Segment(seg) => {
                self.reconnect = None;
                if self.resume_on_segment && self.state == PlayState::Playing {
                    self.shared.playing.store(true, Ordering::Relaxed);
                    self.resume_on_segment = false;
                }
                self.segments.push_back(seg);
            }
            FeedMsg::Flushed => {}
            FeedMsg::TrackError { error, .. } => self.report(error),
            FeedMsg::EndOfQueue => {}
            FeedMsg::Title { entry, title } => {
                if self.segments.front().is_some_and(|s| s.entry == entry) {
                    self.live_title = Some(title.clone());
                    self.emit(Event::NowPlaying(title));
                }
            }
            FeedMsg::Reconnecting {
                attempt,
                wait,
                error,
                ..
            } => {
                self.reconnect = Some(Reconnect {
                    attempt,
                    at: Instant::now() + wait,
                    error,
                });
            }
            FeedMsg::Mark { at_frame, pos } => {
                self.marks.push_back((at_frame, pos));
                let consumed = self.shared.consumed_frames.load(Ordering::Relaxed);
                // Keep the newest played mark and everything ahead of it.
                while self.marks.len() > 2 && self.marks[1].0 <= consumed {
                    self.marks.pop_front();
                }
            }
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
            // After the hook, so anything that reloads on this event sees
            // the new count.
            self.emit(Event::Played(info.path.clone()));
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
            // Natural end of a podcast episode (gapless hand-off).
            let natural = self.current.is_some();
            self.flush_progress(natural);
            self.live_title = None;
            self.reset_listening();
            self.current = entry;
            self.queue.lock().unwrap().set_current(entry);
            self.emit(Event::TrackChanged(entry));
        }

        if self.shared.drained.swap(false, Ordering::AcqRel) {
            self.flush_progress(true);
            self.shared.playing.store(false, Ordering::Relaxed);
            // Close the output before clearing end-of-queue: the exclusive
            // output plays out what's left in the device when it sees it.
            self.output = None;
            self.shared.end_of_queue.store(false, Ordering::Release);
            self.segments.clear();
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
        let playing_level = self
            .segments
            .front()
            .filter(|_| self.state != PlayState::Stopped)
            .map(|s| s.level);
        if self.state == PlayState::Playing {
            self.count_listening(position, track.as_ref());
        } else {
            self.last_pos = None;
        }
        if let Some(info) = track
            .as_ref()
            .filter(|t| t.kind == crate::track::MediaKind::Podcast)
            && self.state != PlayState::Stopped
            && !self.segments.is_empty()
        {
            let now = Instant::now();
            let last = self.progress.as_ref().map(|p| p.2).unwrap_or(now);
            self.progress = Some((info.clone(), position, last));
            if now.duration_since(last) >= Duration::from_secs(5) {
                if let Some(h) = &self.progress_hook {
                    h(info, position, false);
                }
                if let Some(p) = &mut self.progress {
                    p.2 = now;
                }
            }
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
        snap.live = snap.track.as_ref().is_some_and(|t| t.is_live());
        snap.live_title = self.live_title.clone();
        snap.reconnect = self.reconnect.clone();
        snap.podcast_tempo = self.shared.podcast_tempo.load();
        snap.podcast_trim = self.shared.podcast_trim.load(Ordering::Relaxed);
        snap.bit_perfect = self.cfg.output.bit_perfect;
        snap.exclusive = self.output.as_ref().and_then(|o| o.exclusive());
        snap.volume_locked = snap.exclusive.as_ref().is_some_and(|e| !e.hw_volume);
        if let Some(d) = self.output.as_ref().and_then(|o| o.device()) {
            self.last_device = Some(d);
        }
        snap.output_device = self.last_device.clone();
        let gr = self.shared.limiter_gr.load();
        self.shared.limiter_gr.store(0.0);
        let now = Instant::now();
        if gr >= self.limit_hold.0
            || now.duration_since(self.limit_hold.1) > Duration::from_millis(1500)
        {
            self.limit_hold = (gr, now);
        }
        snap.limiting_db = self.limit_hold.0;
        snap.level = playing_level.map(|l| LevelView {
            gain_db: if l.source == crate::level::Source::Live {
                self.shared.level.live_db.load()
            } else {
                l.gain_db
            },
            source: l.source,
        });
        snap.bit_perfect_refused = if snap.exclusive.is_none()
            && self.cfg.output.bit_perfect != BitPerfect::Off
            && snap
                .track
                .as_ref()
                .is_some_and(|t| t.kind == crate::track::MediaKind::File)
        {
            self.exclusive_refused.clone()
        } else {
            None
        };
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
