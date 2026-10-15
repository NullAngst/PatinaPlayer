//! The measuring thread. Songs with no ReplayGain from their server or
//! their tags get measured here, once, and the result goes in the index.
//!
//! What it measures, in order: the song playing and the next few in the
//! queue (so they're right before they start), then server songs as they
//! land in the cache, then, if you ask for it, every untagged song in the
//! library. A server song is measured from its cached file, so nothing is
//! downloaded twice. A cached file is checked for ReplayGain tags first,
//! since the server may not have passed them on.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};

use super::measure::{MeasureError, measure_file};
use crate::cache::{self, SongKey};
use crate::library::loudness::{LoudRow, song_key};
use crate::library::{Library, tags};
use crate::track::{MediaKind, TrackInfo};

/// Songs ahead in the queue worth measuring before they play.
pub const AHEAD: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum LevelEvent {
    /// A song was measured (or its tags read).
    Measured(Vec<u8>),
    /// A whole-library run finished.
    LibraryDone { measured: usize, failed: usize },
}

/// What the thread is up to, for Settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelStatus {
    /// Songs waiting.
    pub queued: usize,
    /// A whole-library run is going.
    pub library_run: bool,
    pub measured: usize,
}

enum Cmd {
    Ahead(Vec<TrackInfo>),
    CacheChanged,
    Library,
    StopLibrary,
}

#[derive(Debug, Clone)]
enum Where {
    File(PathBuf),
    Cached(SongKey),
}

#[derive(Debug, Clone)]
struct Job {
    song: Vec<u8>,
    at: Where,
}

pub struct LevelService {
    tx: Option<Sender<Cmd>>,
    events: Receiver<LevelEvent>,
    status: Arc<Mutex<LevelStatus>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl LevelService {
    pub fn start(db: &Path) -> crate::library::Result<Self> {
        let lib = Library::open(db)?;
        let (tx, rx) = unbounded();
        let (etx, events) = unbounded();
        let status = Arc::new(Mutex::new(LevelStatus::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut w = Worker {
            lib,
            rx,
            events: etx,
            status: status.clone(),
            stop: stop.clone(),
            ahead: VecDeque::new(),
            cached: VecDeque::new(),
            library: VecDeque::new(),
            known: HashSet::new(),
            run: None,
        };
        let join = std::thread::Builder::new()
            .name("patina-level".into())
            .spawn(move || w.run())
            .expect("spawn leveling thread");
        Ok(Self {
            tx: Some(tx),
            events,
            status,
            stop,
            join: Some(join),
        })
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(c);
        }
    }

    /// The song playing and what comes next, nearest first.
    pub fn ahead(&self, songs: Vec<TrackInfo>) {
        self.send(Cmd::Ahead(songs));
    }

    /// What's in the cache changed: measure what landed.
    pub fn cache_changed(&self) {
        self.send(Cmd::CacheChanged);
    }

    /// Measure every untagged song in the library, a few a second.
    pub fn measure_library(&self) {
        self.status.lock().unwrap().library_run = true;
        self.send(Cmd::Library);
    }

    pub fn stop_library(&self) {
        self.send(Cmd::StopLibrary);
    }

    pub fn events(&self) -> &Receiver<LevelEvent> {
        &self.events
    }

    pub fn status(&self) -> LevelStatus {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for LevelService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct Worker {
    lib: Library,
    rx: Receiver<Cmd>,
    events: Sender<LevelEvent>,
    status: Arc<Mutex<LevelStatus>>,
    stop: Arc<AtomicBool>,
    ahead: VecDeque<Job>,
    cached: VecDeque<Job>,
    library: VecDeque<Job>,
    /// Songs already in the loudness table or with a server gain.
    known: HashSet<Vec<u8>>,
    /// A whole-library run: (measured, failed).
    run: Option<(usize, usize)>,
}

impl Worker {
    fn run(&mut self) {
        self.known = self.lib.loudness_keys().unwrap_or_default();
        if let Ok(k) = self.lib.server_gain_keys() {
            self.known.extend(k);
        }
        loop {
            // Waiting on work: block. Working: just look for news.
            let idle = self.ahead.is_empty() && self.cached.is_empty() && self.library.is_empty();
            let cmd = if idle {
                match self.rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            } else {
                match self.rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                    Err(_) => return,
                }
            };
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            if let Some(c) = cmd {
                self.handle(c);
                continue;
            }
            let (job, from_library) = match self.ahead.pop_front() {
                Some(j) => (j, false),
                None => match self.cached.pop_front() {
                    Some(j) => (j, false),
                    None => match self.library.pop_front() {
                        Some(j) => (j, true),
                        None => continue,
                    },
                },
            };
            self.publish();
            let ok = self.measure(&job);
            if from_library {
                if let Some(r) = self.run.as_mut() {
                    if ok {
                        r.0 += 1;
                    } else {
                        r.1 += 1;
                    }
                }
                if self.library.is_empty() {
                    self.end_library_run();
                } else {
                    // A library run is background work: leave the
                    // machine some room.
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            self.publish();
        }
    }

    fn publish(&self) {
        let mut s = self.status.lock().unwrap();
        s.queued = self.ahead.len() + self.cached.len() + self.library.len();
        s.library_run = self.run.is_some();
    }

    fn end_library_run(&mut self) {
        if let Some((measured, failed)) = self.run.take() {
            let _ = self
                .events
                .send(LevelEvent::LibraryDone { measured, failed });
        }
        self.publish();
    }

    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Ahead(songs) => {
                self.ahead.clear();
                for info in songs.iter().take(AHEAD + 1) {
                    if info.kind != MediaKind::File || info.is_live() {
                        continue;
                    }
                    let Some(song) = song_key(info) else { continue };
                    if !self.lib.needs_measuring(info).unwrap_or(false) {
                        continue;
                    }
                    let at = match &info.remote {
                        Some(r) => match SongKey::of(r) {
                            Some(k) => Where::Cached(k),
                            None => continue,
                        },
                        None => Where::File(info.path.clone()),
                    };
                    self.ahead.push_back(Job { song, at });
                }
            }
            Cmd::CacheChanged => {
                let queued: HashSet<Vec<u8>> = self.cached.iter().map(|j| j.song.clone()).collect();
                if let Ok(k) = self.lib.server_gain_keys() {
                    self.known.extend(k);
                }
                for (key, _) in cache::stored_files() {
                    let song = crate::library::song_uri(&key.server, &key.id);
                    if self.known.contains(&song) || queued.contains(&song) {
                        continue;
                    }
                    self.cached.push_back(Job {
                        song,
                        at: Where::Cached(key),
                    });
                }
            }
            Cmd::Library => {
                self.library = self
                    .lib
                    .unmeasured_files()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| Job {
                        song: crate::library::file_uri(&p),
                        at: Where::File(p),
                    })
                    .collect();
                self.run = Some((0, 0));
                if self.library.is_empty() {
                    self.end_library_run();
                }
            }
            Cmd::StopLibrary => {
                self.library.clear();
                self.end_library_run();
            }
        }
        self.publish();
    }

    /// Measure one song and store what came of it. False if it failed.
    fn measure(&mut self, job: &Job) -> bool {
        let (file, local) = match &job.at {
            Where::File(p) => (p.clone(), true),
            Where::Cached(k) => match cache::stored_file(k) {
                Some(f) => (f, false),
                // Not downloaded yet: it comes back when it lands.
                None => return true,
            },
        };
        let size = std::fs::metadata(&file).map(|m| m.len()).ok();
        let mut row = LoudRow {
            song: job.song.clone(),
            lufs: None,
            track_gain: None,
            track_peak: None,
            album_gain: None,
            album_peak: None,
            seconds: None,
            source: "failed".into(),
            size: if local { size } else { None },
        };
        // Tags first: a cached server file may carry them even when the
        // server didn't say, and a loose local file isn't indexed.
        let meta = tags::read(&file);
        let ok = match meta.as_ref().and_then(|m| m.rg_track_gain.map(|g| (g, m))) {
            Some((g, m)) => {
                row.track_gain = Some(g);
                row.track_peak = m.rg_track_peak;
                row.album_gain = m.rg_album_gain;
                row.album_peak = m.rg_album_peak;
                row.seconds = m.duration.map(|d| d.as_secs_f64());
                row.source = "tags".into();
                true
            }
            None => match measure_file(&file, &self.stop) {
                Ok(m) => {
                    row.lufs = Some(m.lufs);
                    row.track_gain = Some(m.gain());
                    row.track_peak = Some(m.peak);
                    row.seconds = Some(m.seconds);
                    row.source = "measured".into();
                    log::info!(
                        "measured {}: {:.1} LUFS, peak {:.2}",
                        file.display(),
                        m.lufs,
                        m.peak
                    );
                    true
                }
                Err(MeasureError::Stopped) => return false,
                Err(e) => {
                    log::info!("can't measure {}: {e}", file.display());
                    false
                }
            },
        };
        if let Err(e) = self.lib.put_loudness(&row) {
            log::warn!("couldn't store loudness: {e}");
            return false;
        }
        self.known.insert(job.song.clone());
        self.status.lock().unwrap().measured += 1;
        let _ = self.events.send(LevelEvent::Measured(job.song.clone()));
        ok
    }
}
