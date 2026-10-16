//! Server songs kept on disk, in three tiers:
//!
//! - Prefetch: songs coming up in the queue, downloaded ahead, nearest
//!   first, up to a size or a track count you set. Once played they're
//!   ordinary cache.
//! - Cache: recently played songs, so a replay doesn't hit the server.
//!   Capped in size; the least recently played go first.
//! - Offline: albums, artists, playlists, folders and songs you pinned.
//!   Never evicted. Unpinned songs drop back into the cache.
//!
//! Every server song plays through here. One already on disk opens as a
//! local file. One that isn't plays from its download while the download
//! runs, so playback starts as soon as the first bytes arrive, and the
//! song ends up in the cache for next time.
//!
//! The cache lives in `~/.cache/patinaplayer/tracks` and pinned songs in
//! `~/.local/share/patinaplayer/offline/tracks`, since cleaners like
//! BleachBit wipe `~/.cache` and nobody should lose a pinned library to
//! that. Downloaded podcast episodes go through the same download queue,
//! into `offline/podcasts`.

pub mod fetch;
mod metered;
pub mod reader;
mod service;
pub mod store;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

pub use service::{
    ActiveRow, CacheEvent, CacheService, EpisodeState, JobKey, JobKind, LOOKAHEAD, PinRow,
    QueueRow, Roots, Status,
};
pub use store::{Pin, PinKind};

use crate::remote::{RemoteRef, StreamFormat};

/// How prefetch measures how far ahead to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefetchMode {
    /// Up to so many megabytes. The default, since twenty hi-res FLACs and
    /// twenty MP3s are very different amounts of disk.
    #[default]
    Size,
    Tracks,
}

impl PrefetchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PrefetchMode::Size => "size",
            PrefetchMode::Tracks => "tracks",
        }
    }
}

impl std::str::FromStr for PrefetchMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "size" => Ok(PrefetchMode::Size),
            "tracks" | "count" => Ok(PrefetchMode::Tracks),
            o => Err(format!("unknown prefetch mode \"{o}\" (size, tracks)")),
        }
    }
}

/// `[cache]` in the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    /// The cap on cached and prefetched songs, in MB. 0 turns the cache
    /// off: songs stream and aren't kept. Pinned songs don't count.
    pub size_mb: u64,
    pub prefetch: PrefetchMode,
    /// How far ahead, by size. 0 turns prefetch off.
    pub prefetch_mb: u64,
    /// How far ahead, by tracks. 0 turns prefetch off.
    pub prefetch_tracks: u32,
    /// Hold prefetch and offline downloads while NetworkManager says the
    /// connection is metered.
    pub pause_on_metered: bool,
    /// What songs are stored as: the original file, or a conversion the
    /// server makes, to save space.
    pub format: StreamFormat,
    /// For Opus and MP3, in kbps.
    pub bitrate: u32,
}

impl CacheConfig {
    /// The cap in bytes.
    pub fn cap(&self) -> u64 {
        self.size_mb * MB
    }

    /// Prefetch's share, by size, never past the cap.
    pub fn prefetch_bytes(&self) -> u64 {
        self.prefetch_mb.min(self.size_mb) * MB
    }

    pub fn prefetch_on(&self) -> bool {
        self.size_mb > 0
            && match self.prefetch {
                PrefetchMode::Size => self.prefetch_mb > 0,
                PrefetchMode::Tracks => self.prefetch_tracks > 0,
            }
    }

    /// "raw", or "opus:192".
    pub fn format_label(&self) -> String {
        match self.format {
            StreamFormat::Raw => "raw".into(),
            f => format!("{}:{}", f.as_str(), self.bitrate),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            size_mb: 10 * 1024,
            prefetch: PrefetchMode::Size,
            prefetch_mb: 1024,
            prefetch_tracks: 20,
            pause_on_metered: true,
            format: StreamFormat::Raw,
            bitrate: 192,
        }
    }
}

pub const MB: u64 = 1024 * 1024;

/// One song on one server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SongKey {
    pub server: String,
    pub id: String,
}

impl SongKey {
    pub fn new(server: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            id: id.into(),
        }
    }

    pub fn of(r: &RemoteRef) -> Option<Self> {
        (!r.episode).then(|| Self::new(r.server.clone(), r.id.clone()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    Cache,
    Offline,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Cache => "cache",
            Tier::Offline => "offline",
        }
    }
}

/// `~/.cache/patinaplayer/tracks`
pub fn cache_root() -> PathBuf {
    crate::config::cache_dir().join("tracks")
}

/// `~/.local/share/patinaplayer/offline/tracks`
pub fn offline_root() -> PathBuf {
    crate::config::offline_dir().join("tracks")
}

/// How the player should open a server song.
pub enum Opening {
    /// It's on disk.
    File { path: PathBuf, original: bool },
    /// It's downloading: read it as it comes.
    Growing {
        reader: Box<reader::PartReader>,
        /// A file name, for FFmpeg's format guess.
        name: String,
        original: bool,
    },
}

/// What the player thread reaches the cache through.
struct Handle {
    shared: Arc<service::Shared>,
}

static HANDLE: RwLock<Option<Handle>> = RwLock::new(None);

/// How the decoder opens a server song. None means "stream it straight
/// from the server" (there's no cache running, or it's turned off and the
/// song isn't pinned).
pub fn open(r: &RemoteRef, abort: Option<Arc<AtomicBool>>) -> Option<Result<Opening, String>> {
    open_for(r, None, abort)
}

/// [`open`], knowing the song's length, so a song read as it downloads
/// can wait for seconds of it when playback catches up.
pub fn open_for(
    r: &RemoteRef,
    duration: Option<std::time::Duration>,
    abort: Option<Arc<AtomicBool>>,
) -> Option<Result<Opening, String>> {
    let key = SongKey::of(r)?;
    let shared = HANDLE.read().unwrap().as_ref()?.shared.clone();
    service::open(&shared, key, duration, abort)
}

/// Songs on disk right now, for telling what can play while a server is
/// unreachable.
pub fn stored_keys() -> HashMap<SongKey, Tier> {
    HANDLE
        .read()
        .unwrap()
        .as_ref()
        .map(|h| {
            h.shared
                .index
                .read()
                .unwrap()
                .iter()
                .map(|(k, s)| (k.clone(), s.tier))
                .collect()
        })
        .unwrap_or_default()
}

/// Where a kept song's file is, if it's on disk.
pub fn stored_file(key: &SongKey) -> Option<PathBuf> {
    let h = HANDLE.read().unwrap();
    let idx = h.as_ref()?.shared.index.read().unwrap();
    idx.get(key).map(|s| s.file.clone()).filter(|f| f.is_file())
}

/// Every kept song and its file, for measuring what lands in the cache.
pub fn stored_files() -> Vec<(SongKey, PathBuf)> {
    HANDLE
        .read()
        .unwrap()
        .as_ref()
        .map(|h| {
            h.shared
                .index
                .read()
                .unwrap()
                .iter()
                .map(|(k, s)| (k.clone(), s.file.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn set_handle(shared: Option<Arc<service::Shared>>) {
    *HANDLE.write().unwrap() = shared.map(|shared| Handle { shared });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_stays_under_the_cap() {
        let mut c = CacheConfig {
            size_mb: 512,
            prefetch_mb: 1024,
            ..Default::default()
        };
        assert_eq!(c.prefetch_bytes(), 512 * MB);
        assert!(c.prefetch_on());
        c.size_mb = 0;
        assert!(!c.prefetch_on(), "no cache, no prefetch");
        assert_eq!(c.format_label(), "raw");
        c.format = StreamFormat::Opus;
        assert_eq!(c.format_label(), "opus:192");
    }
}
