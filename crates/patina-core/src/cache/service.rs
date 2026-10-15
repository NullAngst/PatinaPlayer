//! The cache's own thread: one download queue for the song playing, the
//! songs coming up, podcast episodes and pinned songs, in that order.
//!
//! One download runs at a time, so the most urgent one gets the whole
//! line. Something more urgent pauses it (its `.part` stays and resumes
//! later with a range request): the song you just pressed play on always
//! goes first, and the next song in the queue goes ahead of songs further
//! down. A download that nothing wants any more is cancelled and its
//! `.part` removed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};

use super::fetch::{self, Outcome, Progress, State};
use super::reader::PartReader;
use super::store::{self, Pin, Stored, now};
use super::{CacheConfig, MB, Opening, PrefetchMode, SongKey, Tier};
use crate::library::{LibError, Library};
use crate::remote::{RemoteError, StreamFormat, registry};

/// A size to plan with for a song the index has no size for: about a
/// four-minute FLAC.
const GUESS_SIZE: u64 = 30 * MB;

/// The most queue entries prefetch looks at.
pub const LOOKAHEAD: usize = 1000;

/// How often to ask NetworkManager whether the connection is metered.
const METERED_EVERY: Duration = Duration::from_secs(30);

/// At most this often, a "something changed" while a download moves.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

pub(super) struct Shared {
    pub index: RwLock<HashMap<SongKey, Stored>>,
    pub cfg: RwLock<CacheConfig>,
    /// Songs pinned for offline, so a song you play that's also pinned
    /// lands in the offline folder, even with the cache turned off.
    pub wanted: RwLock<HashSet<SongKey>>,
    tx: Sender<Cmd>,
    status: Mutex<Status>,
}

/// What can be downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JobKey {
    Song(SongKey),
    Episode(i64),
}

/// Why something is downloading. The order is the priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobKind {
    /// Playing right now.
    Play,
    /// Coming up in the queue.
    Prefetch,
    /// A podcast episode you asked for.
    Episode,
    /// Pinned for offline.
    Offline,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Play => "playing",
            JobKind::Prefetch => "up next",
            JobKind::Episode => "episode",
            JobKind::Offline => "offline",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveRow {
    pub key: JobKey,
    pub kind: JobKind,
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueRow {
    pub key: JobKey,
    pub kind: JobKind,
    pub label: String,
    pub paused: bool,
    pub size: Option<u64>,
    /// Why the last try failed; it's tried again later.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PinRow {
    pub pin: Pin,
    /// Server songs it stands for. Local files need no keeping.
    pub songs: usize,
    /// How many of them are in the offline folder.
    pub stored: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeState {
    Queued,
    Downloading { done: u64, total: Option<u64> },
    Paused,
}

/// What the Downloads view draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub cache_bytes: u64,
    pub cache_files: usize,
    pub offline_bytes: u64,
    pub offline_files: usize,
    pub cap: u64,
    pub active: Option<ActiveRow>,
    /// The first couple of hundred waiting downloads, in the order they'll go.
    pub queue: Vec<QueueRow>,
    /// How many are waiting in all.
    pub waiting: usize,
    /// Everything but the playing song is paused.
    pub paused: bool,
    /// The connection is metered and background downloads are holding.
    pub metered_hold: bool,
    pub pins: Vec<PinRow>,
    pub episodes: HashMap<i64, EpisodeState>,
}

#[derive(Debug, Clone)]
pub enum CacheEvent {
    /// Something the Downloads view or the offline marks show changed.
    Changed,
    EpisodeDone(i64),
    Error(String),
    /// A download from this server failed: worth checking it's still there.
    ServerTrouble(String),
}

pub(super) enum PlayStart {
    Stored {
        path: PathBuf,
        original: bool,
    },
    Growing {
        dest: PathBuf,
        part: PathBuf,
        progress: Arc<Progress>,
        url: Option<String>,
        original: bool,
    },
}

enum PlayError {
    /// The server can't be reached or said no: the song can't play.
    Server(String),
    /// Something on this side (a full disk, say): stream it instead.
    Local(String),
}

enum Cmd {
    Play {
        key: SongKey,
        reply: Sender<Result<PlayStart, PlayError>>,
    },
    Upcoming(Vec<SongKey>),
    Touch(SongKey),
    Config(CacheConfig),
    Servers(Vec<String>),
    RenameServer(String, String),
    Pin(Pin),
    Unpin(i64),
    Resync,
    Episode(i64),
    CancelEpisode(i64),
    /// Pause (true) or resume one download, or all of them (None).
    Pause(Option<JobKey>, bool),
    Cancel(JobKey),
    ClearCache,
    Finished {
        run: u64,
        key: JobKey,
        result: Result<Outcome, String>,
    },
    Quit,
}

/// Where things go. Tests point these at a temporary folder.
#[derive(Debug, Clone)]
pub struct Roots {
    pub cache: PathBuf,
    pub offline: PathBuf,
    pub podcasts: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            cache: super::cache_root(),
            offline: super::offline_root(),
            podcasts: crate::config::offline_dir().join("podcasts"),
        }
    }
}

pub struct CacheService {
    shared: Arc<Shared>,
    events: Receiver<CacheEvent>,
}

impl CacheService {
    /// Start the cache for the servers in the config. The player reaches
    /// it from then on.
    pub fn start(db: &Path, cfg: CacheConfig, servers: Vec<String>) -> Result<Self, LibError> {
        Self::start_at(db, cfg, servers, Roots::default())
    }

    pub fn start_at(
        db: &Path,
        cfg: CacheConfig,
        servers: Vec<String>,
        roots: Roots,
    ) -> Result<Self, LibError> {
        let lib = Library::open(db)?;
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let shared = Arc::new(Shared {
            index: RwLock::new(HashMap::new()),
            cfg: RwLock::new(cfg.clone()),
            wanted: RwLock::new(HashSet::new()),
            tx: tx.clone(),
            status: Mutex::new(Status::default()),
        });
        let mut w = Worker {
            shared: shared.clone(),
            lib,
            events: ev_tx,
            tx,
            roots,
            cfg,
            servers: servers.into_iter().collect(),
            playing: None,
            play_open: false,
            upcoming: Vec::new(),
            window: Vec::new(),
            meta: HashMap::new(),
            pins: Vec::new(),
            wanted: Vec::new(),
            pin_songs: HashMap::new(),
            episodes: VecDeque::new(),
            paused: HashSet::new(),
            paused_all: false,
            metered: false,
            metered_checked: None,
            failed: HashMap::new(),
            active: None,
            stopping: HashMap::new(),
            parts: HashMap::new(),
            next_run: 1,
            http: fetch::client(),
            last_progress: Instant::now(),
        };
        w.load();
        std::thread::Builder::new()
            .name("patina-cache".into())
            .spawn(move || w.run(rx))
            .expect("spawn cache thread");
        super::set_handle(Some(shared.clone()));
        Ok(Self {
            shared,
            events: ev_rx,
        })
    }

    pub fn events(&self) -> &Receiver<CacheEvent> {
        &self.events
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    pub fn config(&self) -> CacheConfig {
        self.shared.cfg.read().unwrap().clone()
    }

    fn send(&self, c: Cmd) {
        let _ = self.shared.tx.send(c);
    }

    pub fn set_config(&self, cfg: CacheConfig) {
        *self.shared.cfg.write().unwrap() = cfg.clone();
        self.send(Cmd::Config(cfg));
    }

    /// The configured servers. Songs kept for any other server go.
    pub fn set_servers(&self, names: Vec<String>) {
        self.send(Cmd::Servers(names));
    }

    /// A server was renamed: its kept songs and pins follow it.
    pub fn rename_server(&self, old: &str, new: &str) {
        self.send(Cmd::RenameServer(old.into(), new.into()));
    }

    /// Keep these songs, coming up in this order, downloaded ahead.
    pub fn upcoming(&self, songs: Vec<SongKey>) {
        self.send(Cmd::Upcoming(songs));
    }

    pub fn pin(&self, p: Pin) {
        self.send(Cmd::Pin(p));
    }

    pub fn unpin(&self, id: i64) {
        self.send(Cmd::Unpin(id));
    }

    /// The index changed (a sync finished): pins may stand for different
    /// songs now, and a server that came back may let failed downloads go.
    pub fn resync(&self) {
        self.send(Cmd::Resync);
    }

    pub fn download_episode(&self, id: i64) {
        self.shared
            .status
            .lock()
            .unwrap()
            .episodes
            .insert(id, EpisodeState::Queued);
        self.send(Cmd::Episode(id));
    }

    pub fn cancel_episode(&self, id: i64) {
        self.shared.status.lock().unwrap().episodes.remove(&id);
        self.send(Cmd::CancelEpisode(id));
    }

    /// Pause or resume one download, or all of them but the song playing.
    pub fn pause(&self, key: Option<JobKey>, pause: bool) {
        self.send(Cmd::Pause(key, pause));
    }

    pub fn cancel(&self, key: JobKey) {
        self.send(Cmd::Cancel(key));
    }

    /// Empty the cache. Pinned songs stay.
    pub fn clear_cache(&self) {
        self.send(Cmd::ClearCache);
    }

    /// Songs in the offline folder and the cache, by key.
    pub fn stored(&self) -> HashMap<SongKey, Tier> {
        self.shared
            .index
            .read()
            .unwrap()
            .iter()
            .map(|(k, s)| (k.clone(), s.tier))
            .collect()
    }
}

impl Drop for CacheService {
    fn drop(&mut self) {
        super::set_handle(None);
        self.send(Cmd::Quit);
    }
}

/// The player's way in: see [`super::open`].
pub(super) fn open(
    shared: &Arc<Shared>,
    key: SongKey,
    abort: Option<Arc<AtomicBool>>,
) -> Option<Result<Opening, String>> {
    let on_disk = shared
        .index
        .read()
        .unwrap()
        .get(&key)
        .filter(|s| s.file.is_file())
        .map(|s| (s.file.clone(), s.original));
    if let Some((path, original)) = on_disk {
        let _ = shared.tx.send(Cmd::Touch(key));
        return Some(Ok(Opening::File { path, original }));
    }
    let cache_on = shared.cfg.read().unwrap().size_mb > 0;
    if !cache_on && !shared.wanted.read().unwrap().contains(&key) {
        return None;
    }
    let (tx, rx) = bounded(1);
    shared.tx.send(Cmd::Play { key, reply: tx }).ok()?;
    let started = Instant::now();
    let reply = loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(r) => break r,
            Err(RecvTimeoutError::Timeout) => {
                if abort.as_ref().is_some_and(|a| a.load(Ordering::Relaxed)) {
                    return Some(Err("interrupted".into()));
                }
                if started.elapsed() > Duration::from_secs(30) {
                    log::warn!("the cache didn't answer; streaming instead");
                    return None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    };
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    match reply {
        Ok(PlayStart::Stored { path, original }) => Some(Ok(Opening::File { path, original })),
        Ok(PlayStart::Growing {
            dest,
            part,
            progress,
            url,
            original,
        }) => match PartReader::open(&dest, &part, progress, url, abort) {
            Ok(reader) => Some(Ok(Opening::Growing {
                reader: Box::new(reader),
                name: name(&dest),
                original,
            })),
            Err(e) => {
                log::warn!("can't read the download of {}: {e}", dest.display());
                None
            }
        },
        Err(PlayError::Server(e)) => Some(Err(e)),
        Err(PlayError::Local(e)) => {
            log::warn!("the cache can't take this song ({e}); streaming instead");
            None
        }
    }
}

/// What the index says about a song, for naming its file and planning.
#[derive(Debug, Clone)]
struct Meta {
    path: Option<PathBuf>,
    size: Option<u64>,
    label: String,
}

struct Active {
    run: u64,
    key: JobKey,
    kind: JobKind,
    stop: Arc<AtomicBool>,
    progress: Arc<Progress>,
    dest: PathBuf,
    tier: Tier,
    original: bool,
    format: String,
    url: String,
    label: String,
}

struct Failure {
    retry_at: Instant,
    error: String,
    tries: u32,
    /// The retry time came and a new plan was made for it.
    due: bool,
}

/// Why a download couldn't start.
enum StartError {
    /// The server: away, refusing, or the song is gone.
    Server(String),
    /// This side: a folder that can't be made, a full disk.
    Local(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Server(e) | StartError::Local(e) => f.write_str(e),
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    lib: Library,
    events: Sender<CacheEvent>,
    tx: Sender<Cmd>,
    roots: Roots,
    cfg: CacheConfig,
    servers: HashSet<String>,
    playing: Option<SongKey>,
    /// The playing song still needs its download. Once it's in (or it
    /// was on disk already), clearing the cache doesn't fetch it again.
    play_open: bool,
    upcoming: Vec<SongKey>,
    /// The part of `upcoming` prefetch keeps, nearest first.
    window: Vec<SongKey>,
    meta: HashMap<SongKey, Meta>,
    pins: Vec<Pin>,
    /// Pinned songs, in pin order.
    wanted: Vec<SongKey>,
    pin_songs: HashMap<i64, Vec<SongKey>>,
    episodes: VecDeque<i64>,
    paused: HashSet<JobKey>,
    paused_all: bool,
    metered: bool,
    metered_checked: Option<Instant>,
    failed: HashMap<JobKey, Failure>,
    active: Option<Active>,
    /// Downloads told to stop that haven't said they have yet.
    stopping: HashMap<JobKey, Active>,
    /// Progress of every download with a `.part` on disk, so a reader and
    /// a resumed download share one.
    parts: HashMap<JobKey, Arc<Progress>>,
    next_run: u64,
    http: reqwest::blocking::Client,
    last_progress: Instant,
}

impl Worker {
    fn emit(&self, e: CacheEvent) {
        let _ = self.events.send(e);
    }

    // ----- Startup -----

    /// Read what's kept, and drop rows whose file is gone (a cleaner went
    /// through ~/.cache) or whose server isn't configured any more.
    fn load(&mut self) {
        let rows = store::load(&self.lib.conn).unwrap_or_else(|e| {
            log::warn!("can't read the cache index: {e}");
            Vec::new()
        });
        let mut idx = HashMap::new();
        for s in rows {
            if !s.file.is_file() || !self.servers.contains(&s.key.server) {
                if s.file.is_file() {
                    let _ = std::fs::remove_file(&s.file);
                }
                let _ = store::remove(&self.lib.conn, &s.key);
                continue;
            }
            idx.insert(s.key.clone(), s);
        }
        *self.shared.index.write().unwrap() = idx;
        // Half downloads of cached songs from last time: nothing wants them
        // yet, and the cache folder is no place to collect them.
        remove_parts(&self.roots.cache);
        self.resolve_pins();
        self.publish();
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            let cmd = match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            match cmd {
                Some(Cmd::Quit) => break,
                Some(c) => self.handle(c),
                None => {}
            }
            self.tick();
        }
        // Stop downloads; their .parts resume next time.
        for a in self.active.iter().chain(self.stopping.values()) {
            a.stop.store(true, Ordering::Relaxed);
        }
    }

    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Quit => {}
            Cmd::Play { key, reply } => {
                let r = self.play(key);
                let _ = reply.send(r);
            }
            Cmd::Upcoming(keys) => {
                if keys != self.upcoming {
                    self.upcoming = keys;
                    self.learn(&self.upcoming.clone());
                    self.plan();
                }
            }
            Cmd::Touch(key) => {
                let t = now();
                if let Some(s) = self.shared.index.write().unwrap().get_mut(&key) {
                    s.last_used = t;
                }
                let _ = store::touch(&self.lib.conn, &key, t);
                self.playing = Some(key);
                self.play_open = false;
                self.plan();
            }
            Cmd::Config(cfg) => {
                self.cfg = cfg;
                self.plan();
                self.evict();
                self.publish();
            }
            Cmd::Servers(names) => {
                self.servers = names.into_iter().collect();
                self.drop_unknown_servers();
                self.resolve_pins();
                self.plan();
            }
            Cmd::RenameServer(old, new) => self.rename_server(&old, &new),
            Cmd::Pin(p) => {
                match store::add_pin(&self.lib.conn, &p) {
                    Ok(_) => {}
                    Err(e) => self.emit(CacheEvent::Error(format!("can't pin: {e}"))),
                }
                self.resolve_pins();
                self.plan();
            }
            Cmd::Unpin(id) => {
                let _ = store::remove_pin(&self.lib.conn, id);
                self.resolve_pins();
                self.plan();
            }
            Cmd::Resync => {
                // Songs that failed because a server was away get another go.
                self.failed.retain(|_, f| f.tries > 1);
                self.resolve_pins();
                self.plan();
            }
            Cmd::Episode(id) => {
                let k = JobKey::Episode(id);
                self.failed.remove(&k);
                self.paused.remove(&k);
                if !self.episodes.contains(&id) {
                    self.episodes.push_back(id);
                }
                self.plan();
            }
            Cmd::CancelEpisode(id) => {
                self.episodes.retain(|&e| e != id);
                let k = JobKey::Episode(id);
                self.paused.remove(&k);
                self.failed.remove(&k);
                self.plan();
            }
            Cmd::Pause(None, p) => {
                self.paused_all = p;
                if !p {
                    self.paused.clear();
                    self.failed.clear();
                }
                self.plan();
            }
            Cmd::Pause(Some(k), p) => {
                if p {
                    self.paused.insert(k);
                } else {
                    self.paused.remove(&k);
                    self.failed.remove(&k);
                }
                self.plan();
            }
            Cmd::Cancel(k) => {
                match &k {
                    JobKey::Episode(id) => {
                        let id = *id;
                        self.episodes.retain(|&e| e != id);
                    }
                    // A song coming up stays skipped until the queue moves;
                    // a pinned one stays pinned (unpin it to stop it).
                    JobKey::Song(s) => {
                        self.upcoming.retain(|u| u != s);
                    }
                }
                self.paused.remove(&k);
                self.plan();
            }
            Cmd::ClearCache => self.clear_cache(),
            Cmd::Finished { run, key, result } => self.finished(run, key, result),
        }
    }

    fn tick(&mut self) {
        if self.cfg.pause_on_metered
            && self
                .metered_checked
                .is_none_or(|t| t.elapsed() >= METERED_EVERY)
        {
            self.metered_checked = Some(Instant::now());
            let m = super::metered::is_metered();
            if m != self.metered {
                self.metered = m;
                log::info!("connection is {}metered", if m { "" } else { "no longer " });
                self.plan();
            }
        }
        // Failures whose retry time came.
        let now = Instant::now();
        let mut due = false;
        for f in self.failed.values_mut() {
            if !f.due && f.retry_at <= now {
                f.due = true;
                due = true;
            }
        }
        if due {
            self.plan();
        }
        if self.active.is_some() && self.last_progress.elapsed() >= PROGRESS_EVERY {
            self.last_progress = Instant::now();
            self.publish();
        }
    }

    // ----- What to download -----

    /// Look up what the index knows about songs not seen yet.
    fn learn(&mut self, keys: &[SongKey]) {
        let mut by_server: HashMap<&str, Vec<String>> = HashMap::new();
        for k in keys {
            if !self.meta.contains_key(k) {
                by_server.entry(&k.server).or_default().push(k.id.clone());
            }
        }
        let mut found = Vec::new();
        for (server, ids) in by_server {
            let rows = self.lib.tracks_by_remote(server, &ids).unwrap_or_default();
            for (id, row) in ids.into_iter().zip(rows) {
                let key = SongKey::new(server, id);
                let meta = match row {
                    Some(t) => Meta {
                        path: Some(t.path.clone()),
                        size: (t.size > 0).then_some(t.size),
                        label: match &t.artist {
                            Some(a) => format!("{} · {a}", t.title),
                            None => t.title.clone(),
                        },
                    },
                    None => Meta {
                        path: None,
                        size: None,
                        label: format!("song {} on {}", key.id, key.server),
                    },
                };
                found.push((key, meta));
            }
        }
        self.meta.extend(found);
    }

    fn size_of(&self, k: &SongKey) -> u64 {
        if let Some(s) = self.shared.index.read().unwrap().get(k) {
            return s.size;
        }
        self.meta.get(k).and_then(|m| m.size).unwrap_or(GUESS_SIZE)
    }

    fn stored(&self, k: &SongKey) -> Option<Tier> {
        self.shared.index.read().unwrap().get(k).map(|s| s.tier)
    }

    /// The songs prefetch keeps: the next ones in the queue, nearest
    /// first, until the size or the count runs out.
    fn compute_window(&mut self) {
        self.window.clear();
        if !self.cfg.prefetch_on() {
            return;
        }
        let budget = self.cfg.prefetch_bytes();
        let (mut bytes, mut n) = (0u64, 0u32);
        let mut seen = HashSet::new();
        for k in &self.upcoming {
            if Some(k) == self.playing.as_ref() || !seen.insert(k) {
                continue;
            }
            match self.cfg.prefetch {
                PrefetchMode::Tracks if n >= self.cfg.prefetch_tracks => break,
                PrefetchMode::Size => {
                    let size = self.size_of(k);
                    if bytes + size > budget {
                        break;
                    }
                    bytes += size;
                }
                PrefetchMode::Tracks => {}
            }
            self.window.push(k.clone());
            n += 1;
        }
    }

    fn blocked(&self, k: &JobKey) -> bool {
        self.paused.contains(k)
            || self
                .failed
                .get(k)
                .is_some_and(|f| f.retry_at > Instant::now())
    }

    /// Everything that should download, most urgent first.
    fn wants(&self) -> Vec<(JobKey, JobKind)> {
        let mut v = Vec::new();
        if let Some(p) = &self.playing
            && self.play_open
            && self.stored(p).is_none()
        {
            let k = JobKey::Song(p.clone());
            if self.active.as_ref().is_some_and(|a| a.key == k) || !self.blocked(&k) {
                v.push((k, JobKind::Play));
            }
        }
        // Explicit episode downloads aren't background: they go on metered
        // connections too. Pausing everything holds them.
        let metered_hold = self.cfg.pause_on_metered && self.metered;
        if !self.paused_all {
            if !metered_hold {
                for s in &self.window {
                    let k = JobKey::Song(s.clone());
                    if self.stored(s).is_none() && !self.blocked(&k) {
                        v.push((k, JobKind::Prefetch));
                    }
                }
            }
            for id in &self.episodes {
                let k = JobKey::Episode(*id);
                if !self.blocked(&k) {
                    v.push((k, JobKind::Episode));
                }
            }
            if !metered_hold {
                for s in &self.wanted {
                    let k = JobKey::Song(s.clone());
                    if self.stored(s) != Some(Tier::Offline)
                        && !self.blocked(&k)
                        && !v.iter().any(|(x, _)| *x == k)
                    {
                        v.push((k, JobKind::Offline));
                    }
                }
            }
        }
        v
    }

    /// Work out the window and what should run, and make it so.
    fn plan(&mut self) {
        self.compute_window();
        *self.shared.wanted.write().unwrap() = self.wanted.iter().cloned().collect();
        let wants = self.wants();
        // A running download nothing wants goes. One that something more
        // urgent wants the line for steps aside, and picks up later.
        if let Some(a) = &self.active {
            let mine = wants.iter().position(|(k, _)| *k == a.key);
            let stop = match (mine, wants.first()) {
                (None, _) => true,
                (Some(0), _) => false,
                (Some(i), Some((_, top))) => {
                    let my_kind = wants[i].1;
                    // The next song goes ahead of songs further down.
                    *top < my_kind
                        || (*top == JobKind::Prefetch
                            && my_kind == JobKind::Prefetch
                            && self
                                .window
                                .first()
                                .is_some_and(|w| wants[0].0 == JobKey::Song(w.clone())))
                }
                (Some(_), None) => true,
            };
            if stop {
                let a = self.active.take().unwrap();
                a.stop.store(true, Ordering::Relaxed);
                self.stopping.insert(a.key.clone(), a);
            } else if let Some(i) = mine {
                // Same download, maybe for a new reason (prefetch to play).
                let kind = wants[i].1;
                if let Some(a) = self.active.as_mut() {
                    a.kind = kind;
                }
            }
        }
        if self.active.is_none()
            && let Some((k, kind)) = wants.first().cloned()
            && let Err(e) = self.start(k.clone(), kind)
        {
            self.fail(&k, e.to_string());
            // Try the next one rather than wait for a tick.
            return self.plan();
        }
        self.publish();
    }

    fn fail(&mut self, k: &JobKey, error: String) {
        let tries = self.failed.get(k).map_or(0, |f| f.tries) + 1;
        let wait = match tries {
            1 => Duration::from_secs(60),
            2 => Duration::from_secs(300),
            _ => Duration::from_secs(1800),
        };
        log::info!("download failed ({error}), trying again in {wait:?}");
        self.failed.insert(
            k.clone(),
            Failure {
                retry_at: Instant::now() + wait,
                error,
                tries,
                due: false,
            },
        );
    }

    // ----- Downloads -----

    /// What a song is downloaded as, for this server: a server set to
    /// convert keeps doing so; otherwise the cache's own setting decides.
    fn format_for(&self, server: &str) -> (StreamFormat, u32) {
        let own = registry()
            .client(server)
            .map(|c| c.stream_setting())
            .unwrap_or((StreamFormat::Raw, 0));
        if own.0 != StreamFormat::Raw {
            own
        } else {
            (self.cfg.format, self.cfg.bitrate)
        }
    }

    fn file_taken(&self, p: &Path, key: &SongKey) -> bool {
        let idx = self.shared.index.read().unwrap();
        idx.iter().any(|(k, s)| k != key && s.file == p)
            || self
                .active
                .iter()
                .chain(self.stopping.values())
                .any(|a| a.dest == p && a.key != JobKey::Song(key.clone()))
    }

    /// Start (or take back) the download for `key`.
    fn start(&mut self, key: JobKey, kind: JobKind) -> Result<(), StartError> {
        if let Some(mut a) = self.stopping.remove(&key) {
            // Told to stop a moment ago and wanted again: let it run on. If
            // it already stopped, its Finished brings it back.
            a.stop.store(false, Ordering::Relaxed);
            a.kind = kind;
            self.active = Some(a);
            return Ok(());
        }
        let a = match &key {
            JobKey::Song(s) => self.prepare_song(s, kind)?,
            JobKey::Episode(id) => self.prepare_episode(*id)?,
        };
        let resume = a.original || matches!(key, JobKey::Episode(_));
        let (url, dest, progress, stop) = (
            a.url.clone(),
            a.dest.clone(),
            a.progress.clone(),
            a.stop.clone(),
        );
        let (tx, http, run) = (self.tx.clone(), self.http.clone(), a.run);
        let k2 = key.clone();
        std::thread::Builder::new()
            .name("patina-download".into())
            .spawn(move || {
                let r = fetch::fetch(&http, &url, &dest, resume, &progress, &stop)
                    .map_err(|e| e.to_string());
                let _ = tx.send(Cmd::Finished {
                    run,
                    key: k2,
                    result: r,
                });
            })
            .map_err(|e| StartError::Local(e.to_string()))?;
        self.active = Some(a);
        self.last_progress = Instant::now();
        Ok(())
    }

    fn prepare_song(&mut self, s: &SongKey, kind: JobKind) -> Result<Active, StartError> {
        self.learn(std::slice::from_ref(s));
        let meta = self.meta.get(s).cloned().unwrap_or(Meta {
            path: None,
            size: None,
            label: s.id.clone(),
        });
        let (format, kbps) = self.format_for(&s.server);
        let server = |e: RemoteError| StartError::Server(e.to_string());
        let client = registry().client(&s.server).map_err(server)?;
        let (url, original) = client.stream_url_as(&s.id, format, kbps).map_err(server)?;
        let ext = if original {
            meta.path
                .as_ref()
                .and_then(|p| p.extension())
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_else(|| "audio".into())
        } else {
            format.as_str().to_string()
        };
        let tier = if self.wanted.contains(s) {
            Tier::Offline
        } else {
            Tier::Cache
        };
        let root = match tier {
            Tier::Cache => &self.roots.cache,
            Tier::Offline => &self.roots.offline,
        };
        let want = store::file_for(root, s, meta.path.as_deref(), &ext);
        let dest = store::free_name(&want, s, |p| self.file_taken(p, s));
        let key = JobKey::Song(s.clone());
        let progress = self
            .parts
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Progress::new()))
            .clone();
        let part = fetch::part_path(&dest);
        let local = |e: std::io::Error| StartError::Local(e.to_string());
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(local)?;
        }
        // The reader opens the .part, so it's there before anyone asks.
        let have = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .and_then(|f| f.metadata())
            .map(|m| m.len())
            .map_err(local)?;
        progress.preset(
            if original { have } else { 0 },
            if original { meta.size } else { None },
        );
        if !original {
            // A conversion can't pick up halfway; start the file over.
            let _ = std::fs::File::create(&part);
        }
        progress.set_state(State::Running);
        let run = self.next_run;
        self.next_run += 1;
        Ok(Active {
            run,
            key,
            kind,
            stop: Arc::new(AtomicBool::new(false)),
            progress,
            dest,
            tier,
            original,
            format: if original {
                "raw".into()
            } else {
                format!("{}:{kbps}", format.as_str())
            },
            url,
            label: meta.label,
        })
    }

    fn prepare_episode(&mut self, id: i64) -> Result<Active, StartError> {
        let ep = self
            .lib
            .episode(id)
            .map_err(|e| StartError::Local(e.to_string()))?
            .ok_or_else(|| StartError::Local("that episode is gone".into()))?;
        let dest = crate::podcast::download_path(&self.roots.podcasts, &ep.podcast_title, &ep);
        let url = match crate::remote::parse_episode_uri(&ep.audio_url) {
            Some(r) => {
                registry()
                    .stream(&r)
                    .map_err(|e| StartError::Server(e.to_string()))?
                    .0
            }
            None => ep.audio_url.clone(),
        };
        let key = JobKey::Episode(id);
        let progress = self
            .parts
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Progress::new()))
            .clone();
        progress.preset(0, ep.size.filter(|s| *s > 0).map(|s| s as u64));
        progress.set_state(State::Running);
        let run = self.next_run;
        self.next_run += 1;
        Ok(Active {
            run,
            key,
            kind: JobKind::Episode,
            stop: Arc::new(AtomicBool::new(false)),
            progress,
            dest,
            tier: Tier::Offline,
            original: false,
            format: String::new(),
            url,
            label: format!("{} · {}", ep.title, ep.podcast_title),
        })
    }

    /// A song is playing (or about to). Hand back its file, or its
    /// download, started now if it wasn't running.
    fn play(&mut self, key: SongKey) -> Result<PlayStart, PlayError> {
        self.playing = Some(key.clone());
        self.play_open = true;
        let jk = JobKey::Song(key.clone());
        self.failed.remove(&jk);
        self.paused.remove(&jk);
        if let Some(s) = self.shared.index.read().unwrap().get(&key)
            && s.file.is_file()
        {
            self.play_open = false;
            return Ok(PlayStart::Stored {
                path: s.file.clone(),
                original: s.original,
            });
        }
        let running = self.active.as_ref().is_some_and(|a| a.key == jk);
        if !running {
            if let Some(a) = self.active.take() {
                a.stop.store(true, Ordering::Relaxed);
                self.stopping.insert(a.key.clone(), a);
            }
            if let Err(e) = self.start(jk.clone(), JobKind::Play) {
                self.plan();
                return Err(match e {
                    StartError::Server(m) => PlayError::Server(m),
                    StartError::Local(m) => PlayError::Local(m),
                });
            }
        }
        let a = self.active.as_mut().expect("just started");
        a.kind = JobKind::Play;
        let start = PlayStart::Growing {
            dest: a.dest.clone(),
            part: fetch::part_path(&a.dest),
            progress: a.progress.clone(),
            // Ranges only make sense on the original file.
            url: a.original.then(|| a.url.clone()),
            original: a.original,
        };
        self.plan();
        Ok(start)
    }

    fn finished(&mut self, run: u64, key: JobKey, result: Result<Outcome, String>) {
        let a = if self.active.as_ref().is_some_and(|a| a.run == run) {
            self.active.take()
        } else if self.stopping.get(&key).is_some_and(|a| a.run == run) {
            self.stopping.remove(&key)
        } else {
            None
        };
        let Some(a) = a else {
            self.plan();
            return;
        };
        match result {
            Ok(Outcome::Done) => {
                self.parts.remove(&key);
                self.failed.remove(&key);
                if matches!(&key, JobKey::Song(s) if Some(s) == self.playing.as_ref()) {
                    self.play_open = false;
                }
                match &key {
                    JobKey::Song(s) => self.record(s, &a),
                    JobKey::Episode(id) => {
                        self.episodes.retain(|e| e != id);
                        match self.lib.set_episode_download(*id, Some(&a.dest)) {
                            Ok(()) => self.emit(CacheEvent::EpisodeDone(*id)),
                            Err(e) => self.emit(CacheEvent::Error(e.to_string())),
                        }
                        self.shared.status.lock().unwrap().episodes.remove(id);
                    }
                }
            }
            Ok(Outcome::Stopped) => {
                // Not wanted at all any more: tidy up its half file. Wanted
                // but paused or pushed back: keep it to resume.
                let wanted = self.wants().iter().any(|(k, _)| *k == key)
                    || self.paused.contains(&key)
                    || matches!(&key, JobKey::Episode(id) if self.episodes.contains(id));
                if !wanted {
                    self.parts.remove(&key);
                    let _ = std::fs::remove_file(fetch::part_path(&a.dest));
                    let root = match a.tier {
                        Tier::Cache => &self.roots.cache,
                        Tier::Offline => &self.roots.offline,
                    };
                    if matches!(key, JobKey::Song(_)) {
                        store::prune_dirs(root, a.dest.parent());
                    }
                }
            }
            Err(e) => {
                if let JobKey::Song(s) = &key {
                    self.emit(CacheEvent::ServerTrouble(s.server.clone()));
                }
                let label = a.label.clone();
                if a.kind != JobKind::Play && a.kind != JobKind::Prefetch {
                    self.emit(CacheEvent::Error(format!(
                        "download of {label} failed: {e}"
                    )));
                }
                self.fail(&key, e);
            }
        }
        self.plan();
        self.emit(CacheEvent::Changed);
    }

    /// A song finished downloading: note it, and make room.
    fn record(&mut self, s: &SongKey, a: &Active) {
        let size = std::fs::metadata(&a.dest).map(|m| m.len()).unwrap_or(0);
        let t = now();
        let row = Stored {
            key: s.clone(),
            // Pinned while it downloaded? Then it belongs offline after all.
            tier: if self.wanted.contains(s) {
                Tier::Offline
            } else {
                a.tier
            },
            file: a.dest.clone(),
            size,
            original: a.original,
            format: a.format.clone(),
            added_at: t,
            last_used: if a.kind == JobKind::Play { t } else { 0 },
        };
        if let Err(e) = store::put(&self.lib.conn, &row) {
            log::warn!("can't note the cached song: {e}");
        }
        let tier = row.tier;
        self.shared.index.write().unwrap().insert(s.clone(), row);
        if tier != a.tier {
            self.move_to(s, tier);
        }
        self.evict();
    }

    // ----- Room -----

    /// Least recently played first, until the cache fits its cap. The song
    /// playing and the ones prefetch is holding stay.
    fn evict(&mut self) {
        let cap = self.cfg.cap();
        let keep: HashSet<&SongKey> = self.window.iter().chain(self.playing.iter()).collect();
        let mut gone = Vec::new();
        {
            let idx = self.shared.index.read().unwrap();
            let mut total: u64 = idx
                .values()
                .filter(|s| s.tier == Tier::Cache)
                .map(|s| s.size)
                .sum();
            if total <= cap {
                return;
            }
            let mut c: Vec<&Stored> = idx
                .values()
                .filter(|s| s.tier == Tier::Cache && !keep.contains(&s.key))
                .collect();
            c.sort_by_key(|s| (s.last_used, s.added_at));
            for s in c {
                if total <= cap {
                    break;
                }
                total = total.saturating_sub(s.size);
                gone.push(s.key.clone());
            }
        }
        for k in gone {
            self.forget(&k);
        }
    }

    /// Delete a kept song, file and row.
    fn forget(&mut self, k: &SongKey) {
        let removed = self.shared.index.write().unwrap().remove(k);
        if let Some(s) = removed {
            let _ = std::fs::remove_file(&s.file);
            let root = match s.tier {
                Tier::Cache => &self.roots.cache,
                Tier::Offline => &self.roots.offline,
            };
            store::prune_dirs(root, s.file.parent());
        }
        let _ = store::remove(&self.lib.conn, k);
    }

    fn clear_cache(&mut self) {
        let keys: Vec<SongKey> = self
            .shared
            .index
            .read()
            .unwrap()
            .values()
            .filter(|s| s.tier == Tier::Cache)
            .map(|s| s.key.clone())
            .collect();
        for k in keys {
            self.forget(&k);
        }
        self.plan();
        self.emit(CacheEvent::Changed);
    }

    /// Move a kept song between the cache and the offline folder.
    fn move_to(&mut self, k: &SongKey, tier: Tier) {
        let Some(s) = self.shared.index.read().unwrap().get(k).cloned() else {
            return;
        };
        let (from_root, to_root) = match tier {
            Tier::Offline => (&self.roots.cache, &self.roots.offline),
            Tier::Cache => (&self.roots.offline, &self.roots.cache),
        };
        let to = match s.file.strip_prefix(from_root) {
            Ok(rel) => to_root.join(rel),
            Err(_) => {
                let ext = s
                    .file
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default();
                store::file_for(to_root, k, None, &ext)
            }
        };
        let to = store::free_name(&to, k, |p| self.file_taken(p, k));
        let moved = if s.file == to {
            Ok(())
        } else {
            store::move_file(&s.file, &to)
        };
        match moved {
            Ok(()) => {
                store::prune_dirs(from_root, s.file.parent());
                let row = Stored {
                    tier,
                    file: to,
                    ..s
                };
                let _ = store::put(&self.lib.conn, &row);
                self.shared.index.write().unwrap().insert(k.clone(), row);
            }
            Err(e) => log::warn!("can't move {}: {e}", s.file.display()),
        }
    }

    // ----- Pins -----

    /// Work out which songs the pins stand for now, and put kept songs in
    /// the folder they belong in.
    fn resolve_pins(&mut self) {
        self.pins = store::pins(&self.lib.conn).unwrap_or_default();
        let mut wanted = Vec::new();
        let mut set = HashSet::new();
        self.pin_songs.clear();
        for p in &self.pins {
            let rows = store::resolve(&self.lib, p).unwrap_or_default();
            let mut keys = Vec::new();
            for t in rows {
                let Some(r) = t.remote() else { continue };
                let k = SongKey::new(r.server, r.id);
                self.meta.entry(k.clone()).or_insert_with(|| Meta {
                    path: Some(t.path.clone()),
                    size: (t.size > 0).then_some(t.size),
                    label: match &t.artist {
                        Some(a) => format!("{} · {a}", t.title),
                        None => t.title.clone(),
                    },
                });
                if set.insert(k.clone()) {
                    wanted.push(k.clone());
                }
                keys.push(k);
            }
            self.pin_songs.insert(p.id, keys);
        }
        self.wanted = wanted;
        *self.shared.wanted.write().unwrap() = set.clone();
        let (promote, demote): (Vec<SongKey>, Vec<SongKey>) = {
            let idx = self.shared.index.read().unwrap();
            (
                self.wanted
                    .iter()
                    .filter(|k| idx.get(*k).is_some_and(|s| s.tier == Tier::Cache))
                    .cloned()
                    .collect(),
                idx.values()
                    .filter(|s| s.tier == Tier::Offline && !set.contains(&s.key))
                    .map(|s| s.key.clone())
                    .collect(),
            )
        };
        for k in promote {
            self.move_to(&k, Tier::Offline);
        }
        for k in demote {
            self.move_to(&k, Tier::Cache);
        }
        self.evict();
        self.publish();
        self.emit(CacheEvent::Changed);
    }

    fn drop_unknown_servers(&mut self) {
        let gone: Vec<SongKey> = self
            .shared
            .index
            .read()
            .unwrap()
            .keys()
            .filter(|k| !self.servers.contains(&k.server))
            .cloned()
            .collect();
        for k in gone {
            self.forget(&k);
        }
        let prefix = crate::remote::SCHEME;
        for p in store::pins(&self.lib.conn).unwrap_or_default() {
            if matches!(p.kind, store::PinKind::Folder | store::PinKind::Song) {
                let k = p.key_str();
                let server = k
                    .strip_prefix(prefix)
                    .map(|r| r.split('/').next().unwrap_or(r));
                if server.is_some_and(|s| !self.servers.contains(s)) {
                    let _ = store::remove_pin(&self.lib.conn, p.id);
                }
            }
        }
    }

    fn rename_server(&mut self, old: &str, new: &str) {
        let _ = store::rename_server(&self.lib.conn, old, new);
        let _ = store::rename_server_pins(&self.lib.conn, old, new);
        {
            let mut idx = self.shared.index.write().unwrap();
            let moved: Vec<SongKey> = idx.keys().filter(|k| k.server == old).cloned().collect();
            for k in moved {
                if let Some(mut s) = idx.remove(&k) {
                    s.key.server = new.to_string();
                    idx.insert(s.key.clone(), s);
                }
            }
        }
        self.servers.remove(old);
        self.servers.insert(new.to_string());
        let fix = |k: &mut SongKey| {
            if k.server == old {
                k.server = new.to_string();
            }
        };
        self.playing.iter_mut().for_each(fix);
        self.upcoming.iter_mut().for_each(fix);
        self.meta.clear();
        self.resolve_pins();
        self.plan();
    }

    // ----- Status -----

    fn publish(&mut self) {
        let mut st = Status {
            cap: self.cfg.cap(),
            paused: self.paused_all,
            metered_hold: self.cfg.pause_on_metered && self.metered,
            ..Default::default()
        };
        {
            let idx = self.shared.index.read().unwrap();
            for s in idx.values() {
                match s.tier {
                    Tier::Cache => {
                        st.cache_bytes += s.size;
                        st.cache_files += 1;
                    }
                    Tier::Offline => {
                        st.offline_bytes += s.size;
                        st.offline_files += 1;
                    }
                }
            }
            for p in &self.pins {
                let keys = self.pin_songs.get(&p.id).map(Vec::as_slice).unwrap_or(&[]);
                let mut row = PinRow {
                    pin: p.clone(),
                    songs: keys.len(),
                    stored: 0,
                    bytes: 0,
                };
                for k in keys {
                    if let Some(s) = idx.get(k).filter(|s| s.tier == Tier::Offline) {
                        row.stored += 1;
                        row.bytes += s.size;
                    }
                }
                st.pins.push(row);
            }
        }
        let label = |k: &JobKey| match k {
            JobKey::Song(s) => self
                .meta
                .get(s)
                .map(|m| m.label.clone())
                .unwrap_or_else(|| s.id.clone()),
            JobKey::Episode(id) => self
                .lib
                .episode(*id)
                .ok()
                .flatten()
                .map(|e| format!("{} · {}", e.title, e.podcast_title))
                .unwrap_or_else(|| format!("episode {id}")),
        };
        if let Some(a) = &self.active {
            st.active = Some(ActiveRow {
                key: a.key.clone(),
                kind: a.kind,
                label: a.label.clone(),
                done: a.progress.done(),
                total: a.progress.total(),
            });
            if let JobKey::Episode(id) = a.key {
                st.episodes.insert(
                    id,
                    EpisodeState::Downloading {
                        done: a.progress.done(),
                        total: a.progress.total(),
                    },
                );
            }
        }
        // What's waiting: everything wanted, plus what's held back.
        let active = self.active.as_ref().map(|a| a.key.clone());
        let mut rows: Vec<QueueRow> = Vec::new();
        let push = |k: JobKey, kind: JobKind, rows: &mut Vec<QueueRow>| {
            if Some(&k) == active.as_ref() || rows.iter().any(|r| r.key == k) {
                return;
            }
            let size = match &k {
                JobKey::Song(s) => Some(self.size_of(s)),
                JobKey::Episode(_) => None,
            };
            rows.push(QueueRow {
                paused: self.paused_all || self.paused.contains(&k),
                error: self.failed.get(&k).map(|f| f.error.clone()),
                label: String::new(),
                key: k,
                kind,
                size,
            });
        };
        if let Some(p) = &self.playing
            && self.play_open
            && self.stored(p).is_none()
        {
            push(JobKey::Song(p.clone()), JobKind::Play, &mut rows);
        }
        if self.cfg.prefetch_on() {
            for s in &self.window {
                if self.stored(s).is_none() {
                    push(JobKey::Song(s.clone()), JobKind::Prefetch, &mut rows);
                }
            }
        }
        for id in &self.episodes {
            push(JobKey::Episode(*id), JobKind::Episode, &mut rows);
        }
        for s in &self.wanted {
            if self.stored(s) != Some(Tier::Offline) {
                push(JobKey::Song(s.clone()), JobKind::Offline, &mut rows);
            }
        }
        st.waiting = rows.len();
        rows.truncate(200);
        for r in &mut rows {
            r.label = label(&r.key);
            if let JobKey::Episode(id) = r.key {
                st.episodes.insert(
                    id,
                    if r.paused {
                        EpisodeState::Paused
                    } else {
                        EpisodeState::Queued
                    },
                );
            }
        }
        // Episodes past the first 200 still show as queued in their view.
        for id in &self.episodes {
            st.episodes.entry(*id).or_insert(EpisodeState::Queued);
        }
        st.queue = rows;
        let changed = {
            let mut cur = self.shared.status.lock().unwrap();
            let changed = *cur != st;
            *cur = st;
            changed
        };
        if changed {
            self.emit(CacheEvent::Changed);
        }
    }
}

/// Remove `.part` files under `root`.
fn remove_parts(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_parts(&p);
            let _ = std::fs::remove_dir(&p);
        } else if p.extension().is_some_and(|x| x == "part") {
            let _ = std::fs::remove_file(&p);
        }
    }
}
