//! Music servers: Subsonic (Navidrome, Gonic and the rest), Ampache, and
//! DLNA media servers on the local network.
//!
//! Each server is a source like a local folder. Its songs are synced into
//! the same SQLite index (see [`sync`]), so every view, search and the
//! offline browsing work the same for both. Server songs live in the index
//! under made-up paths, `server://<name>/<path on the server>`, which keeps
//! the folder views working and gives every song one key. The server's own
//! song id sits beside it; that's what playing, playlists and covers use.
//!
//! [`Client`] is what a server can do. [`subsonic::Subsonic`] and
//! [`ampache::Ampache`] log in; [`dlna::Dlna`] is found on the network and
//! needs no login. [`service::RemoteService`] keeps
//! them connected and synced in the background, and [`registry`] lets the
//! player and the art loader reach a connected server from any thread.

pub mod ampache;
pub mod dlna;
mod http;
pub mod service;
pub mod subsonic;
pub mod sync;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use crate::library::Library;

pub use service::{RemoteEvent, RemoteService, ServerState, ServerStatus};

/// What every server path starts with.
pub const SCHEME: &str = "server://";

/// The client name sent to servers, and shown as "changed by" in their
/// play queue.
pub const CLIENT: &str = "PatinaPlayer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ServerKind {
    #[default]
    Subsonic,
    Ampache,
    /// A UPnP/DLNA media server. No login, no playlists, no play queue.
    Dlna,
}

impl ServerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServerKind::Subsonic => "subsonic",
            ServerKind::Ampache => "ampache",
            ServerKind::Dlna => "dlna",
        }
    }

    /// Whether it takes a user name and password.
    pub fn logs_in(self) -> bool {
        self != ServerKind::Dlna
    }

    /// Whether it can convert songs as it streams them.
    pub fn transcodes(self) -> bool {
        self != ServerKind::Dlna
    }
}

impl std::str::FromStr for ServerKind {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "subsonic" => Ok(ServerKind::Subsonic),
            "ampache" => Ok(ServerKind::Ampache),
            "dlna" | "upnp" => Ok(ServerKind::Dlna),
            other => Err(format!(
                "unknown server kind \"{other}\" (subsonic, ampache, dlna)"
            )),
        }
    }
}

/// What the server sends when a song plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum StreamFormat {
    /// The original file, untouched. The default, so the sound chain
    /// works on real data and bit-perfect can apply.
    #[default]
    Raw,
    Opus,
    Mp3,
}

impl StreamFormat {
    pub const ALL: [StreamFormat; 3] = [StreamFormat::Raw, StreamFormat::Opus, StreamFormat::Mp3];
    pub fn as_str(self) -> &'static str {
        match self {
            StreamFormat::Raw => "raw",
            StreamFormat::Opus => "opus",
            StreamFormat::Mp3 => "mp3",
        }
    }
}

impl std::str::FromStr for StreamFormat {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        StreamFormat::ALL
            .into_iter()
            .find(|f| f.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("unknown stream format \"{s}\" (raw, opus, mp3)"))
    }
}

/// One `[[server]]` in the config. The password is in the keyring.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerConfig {
    /// Shown in the sidebar, and part of every song's path. Unique.
    pub name: String,
    /// For DLNA, the device description URL.
    pub url: String,
    pub user: String,
    pub kind: ServerKind,
    pub stream: StreamFormat,
    /// For Opus and MP3, in kbps.
    pub bitrate: u32,
    /// A DLNA server's unique id, to find it again when its address
    /// changes. Empty for the rest.
    pub udn: String,
}

impl ServerConfig {
    /// Keyring key: the address without a trailing slash.
    pub fn secret_key(&self) -> String {
        normalize_url(&self.url)
    }
}

/// "music.example.com" becomes "https://music.example.com", and a trailing
/// slash or a pasted "/rest" or "/app" goes.
pub fn normalize_url(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if !u.contains("://") {
        u = format!("https://{u}");
    }
    for tail in ["/rest", "/app", "/server/json.server.php", "/server"] {
        if let Some(stripped) = u.strip_suffix(tail) {
            u = stripped.to_string();
        }
    }
    u.trim_end_matches('/').to_string()
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum RemoteError {
    #[error("can't reach the server: {0}")]
    Http(String),
    #[error("wrong user name or password")]
    Auth,
    #[error("the server said: {message} (error {code})")]
    Server { code: i64, message: String },
    #[error("the server sent something unexpected: {0}")]
    Parse(String),
    #[error("this server doesn't support {0}")]
    Unsupported(&'static str),
    #[error("not connected to {0}")]
    NotConnected(String),
}

pub type Result<T> = std::result::Result<T, RemoteError>;

/// What a server said about itself when it connected.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServerInfo {
    pub kind: ServerKind,
    /// "Navidrome 0.58.0", "gonic 0.17.0", "Ampache 6.6.0", "Subsonic".
    pub product: String,
    pub open_subsonic: bool,
    pub api_version: String,
    /// The server keeps a play queue we can save and load.
    pub play_queue: bool,
}

/// An album as the server lists it: enough to tell whether it changed
/// since the last sync.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteAlbum {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub song_count: u32,
    pub duration_s: u64,
    pub year: Option<i32>,
    /// When the server last changed it, if it says.
    pub changed: Option<String>,
}

impl RemoteAlbum {
    /// Changes when anything sync cares about changes.
    pub fn signature(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{}",
            self.name,
            self.artist,
            self.song_count,
            self.duration_s,
            self.year,
            self.changed.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteSong {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub album_id: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub duration: Option<Duration>,
    /// File extension: "flac", "mp3".
    pub suffix: String,
    pub sample_rate: Option<u32>,
    pub bits: Option<u32>,
    pub channels: Option<u16>,
    pub kbps: Option<u32>,
    pub size: Option<i64>,
    /// Where the file sits on the server, relative, if it says.
    pub path: Option<String>,
    pub cover: Option<String>,
    pub rg_track_gain: Option<f64>,
    pub rg_track_peak: Option<f64>,
    pub rg_album_gain: Option<f64>,
    pub rg_album_peak: Option<f64>,
    pub play_count: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemotePlaylist {
    pub id: String,
    pub name: String,
    pub song_count: u32,
    /// Someone else's playlist we can only read.
    pub readonly: bool,
    pub changed: Option<String>,
}

impl RemotePlaylist {
    pub fn signature(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.name,
            self.song_count,
            self.changed.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteStation {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteEpisode {
    pub id: String,
    /// What to stream. Only episodes the server has downloaded have one.
    pub stream_id: Option<String>,
    pub title: String,
    pub published: Option<i64>,
    pub duration: Option<Duration>,
    pub description: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemotePodcast {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub feed_url: Option<String>,
    pub episodes: Vec<RemoteEpisode>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedQueue {
    pub ids: Vec<String>,
    pub current: Option<String>,
    pub position: Duration,
    pub changed_by: Option<String>,
    pub changed: Option<String>,
}

/// Everything PatinaPlayer asks of a server. Blocking: callers run these
/// off the UI thread.
pub trait Client: Send + Sync {
    fn info(&self) -> &ServerInfo;
    /// A cheap call to check the server still answers. An `Http` error
    /// means it doesn't; anything else means it answered.
    fn ping(&self) -> Result<()>;
    /// Every album, paged through behind the scenes.
    fn albums(&self) -> Result<Vec<RemoteAlbum>>;
    fn album_songs(&self, album_id: &str) -> Result<Vec<RemoteSong>>;
    /// A URL to play `song_id` from, and whether it's the original file.
    /// Uses the server's own streaming setting.
    fn stream_url(&self, song_id: &str) -> Result<(String, bool)> {
        let (f, kbps) = self.stream_setting();
        self.stream_url_as(song_id, f, kbps)
    }
    /// The same, in a format of your choosing. A server that can't convert
    /// sends the original.
    fn stream_url_as(
        &self,
        song_id: &str,
        format: StreamFormat,
        kbps: u32,
    ) -> Result<(String, bool)>;
    /// What this server is set to stream: the original, or a conversion.
    fn stream_setting(&self) -> (StreamFormat, u32);
    /// A podcast episode's stream.
    fn episode_url(&self, stream_id: &str) -> Result<String>;
    fn cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>>;
    fn playlists(&self) -> Result<Vec<RemotePlaylist>>;
    fn playlist_songs(&self, id: &str) -> Result<Vec<RemoteSong>>;
    fn create_playlist(&self, name: &str, songs: &[String]) -> Result<String>;
    /// Set a playlist's songs to exactly these, in this order.
    fn replace_playlist(&self, id: &str, songs: &[String]) -> Result<()>;
    fn rename_playlist(&self, id: &str, name: &str) -> Result<()>;
    fn delete_playlist(&self, id: &str) -> Result<()>;
    fn radio(&self) -> Result<Vec<RemoteStation>>;
    fn podcasts(&self) -> Result<Vec<RemotePodcast>>;
    fn save_queue(&self, ids: &[String], current: Option<&str>, pos: Duration) -> Result<()>;
    fn load_queue(&self) -> Result<Option<SavedQueue>>;
    /// Where a podcast episode was left, for other clients.
    fn bookmark(&self, id: &str, pos: Duration) -> Result<()>;

    /// Songs like this one, by the server's ids, from its similarity data
    /// (which servers mostly get from Last.fm). Empty when it has none.
    fn similar_songs(&self, _song_id: &str, _count: usize) -> Result<Vec<String>> {
        Err(RemoteError::Unsupported("similar songs"))
    }

    /// Artists like this one, by name. Empty when it has none.
    fn similar_artists(&self, _artist: &str, _count: usize) -> Result<Vec<String>> {
        Err(RemoteError::Unsupported("similar artists"))
    }

    /// Tell the server a song was played (`submission`), or is playing
    /// now. `at` is when it started, in Unix seconds. Servers set up to
    /// pass plays on to Last.fm do that from here.
    fn report_play(&self, _id: &str, _at: i64, _submission: bool) -> Result<()> {
        Err(RemoteError::Unsupported("play reporting"))
    }

    /// Star or unstar a song (a favourite on the server).
    fn star(&self, _id: &str, _on: bool) -> Result<()> {
        Err(RemoteError::Unsupported("stars"))
    }
}

/// Ids and names out of the similarity answers.
#[derive(Debug, Default, serde::Deserialize)]
struct Named {
    #[serde(default, deserialize_with = "http::opt_id")]
    id: Option<String>,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Songs {
    #[serde(default, deserialize_with = "http::list")]
    song: Vec<Named>,
}

/// Connect to a server of a known kind.
pub fn connect(cfg: &ServerConfig, secret: &str) -> Result<Arc<dyn Client>> {
    let url = normalize_url(&cfg.url);
    Ok(match cfg.kind {
        ServerKind::Subsonic => Arc::new(subsonic::Subsonic::connect(
            &url,
            &cfg.user,
            secret,
            cfg.stream,
            cfg.bitrate,
        )?),
        ServerKind::Ampache => Arc::new(ampache::Ampache::connect(
            &url,
            &cfg.user,
            secret,
            cfg.stream,
            cfg.bitrate,
        )?),
        ServerKind::Dlna => Arc::new(dlna::Dlna::connect(cfg.url.trim(), &cfg.udn)?),
    })
}

/// Find out what kind of server is at `url` by logging in. Ampache first,
/// since Ampache speaks Subsonic too but its own API carries more.
pub fn detect(url: &str, user: &str, secret: &str) -> Result<(ServerKind, ServerInfo)> {
    let url = normalize_url(url);
    let amp = ampache::Ampache::connect(&url, user, secret, StreamFormat::Raw, 0);
    match amp {
        Ok(a) => return Ok((ServerKind::Ampache, a.info().clone())),
        // It is Ampache, and it said no.
        Err(e @ (RemoteError::Auth | RemoteError::Server { .. })) => {
            if ampache::Ampache::is_there(&url) {
                return Err(e);
            }
        }
        Err(_) => {}
    }
    let s = subsonic::Subsonic::connect(&url, user, secret, StreamFormat::Raw, 0)?;
    Ok((ServerKind::Subsonic, s.info().clone()))
}

// ----- Paths -----

/// The root path of a server's songs in the index.
pub fn root(server: &str) -> PathBuf {
    PathBuf::from(format!("{SCHEME}{server}"))
}

/// True for a server song's path (or a server folder).
pub fn is_remote(path: &Path) -> bool {
    path.as_os_str()
        .to_str()
        .is_some_and(|s| s.starts_with(SCHEME))
}

/// Which server a server path belongs to.
pub fn server_of(path: &Path) -> Option<&str> {
    let s = path.as_os_str().to_str()?.strip_prefix(SCHEME)?;
    Some(s.split('/').next().unwrap_or(s))
}

/// Points at one song (or podcast episode) on one server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RemoteRef {
    pub server: String,
    pub id: String,
    /// Episodes stream from a different id than their own.
    pub episode: bool,
}

/// Podcast feeds and episodes from a server are stored as
/// `server://<name>#<id>` in place of a feed or audio URL.
pub fn episode_uri(server: &str, id: &str) -> String {
    format!("{SCHEME}{server}#{id}")
}

pub fn parse_episode_uri(uri: &str) -> Option<RemoteRef> {
    let rest = uri.strip_prefix(SCHEME)?;
    let (server, id) = rest.split_once('#')?;
    Some(RemoteRef {
        server: server.into(),
        id: id.into(),
        episode: true,
    })
}

// ----- Registry -----

/// Connected servers, reachable from any thread: the player resolves
/// stream URLs here, and the art loader fetches covers.
pub struct Registry {
    servers: RwLock<HashMap<String, Arc<Entry>>>,
}

struct Entry {
    client: RwLock<Option<Arc<dyn Client>>>,
    /// Its own connection to the index, for looking up cover ids by path.
    db: Mutex<Option<Library>>,
}

pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Registry {
        servers: RwLock::new(HashMap::new()),
    })
}

impl Registry {
    fn entry(&self, server: &str) -> Arc<Entry> {
        if let Some(e) = self.servers.read().unwrap().get(server) {
            return e.clone();
        }
        self.servers
            .write()
            .unwrap()
            .entry(server.to_string())
            .or_insert_with(|| {
                Arc::new(Entry {
                    client: RwLock::new(None),
                    db: Mutex::new(None),
                })
            })
            .clone()
    }

    /// A server's index connection, so covers can be found by path even
    /// while it's offline.
    pub fn set_db(&self, server: &str, db: Option<Library>) {
        *self.entry(server).db.lock().unwrap() = db;
    }

    pub fn set_client(&self, server: &str, client: Option<Arc<dyn Client>>) {
        *self.entry(server).client.write().unwrap() = client;
    }

    /// Servers that are connected right now.
    pub fn connected(&self) -> Vec<String> {
        self.servers
            .read()
            .unwrap()
            .iter()
            .filter(|(_, e)| e.client.read().unwrap().is_some())
            .map(|(n, _)| n.clone())
            .collect()
    }

    pub fn remove(&self, server: &str) {
        self.servers.write().unwrap().remove(server);
    }

    pub fn client(&self, server: &str) -> Result<Arc<dyn Client>> {
        self.servers
            .read()
            .unwrap()
            .get(server)
            .and_then(|e| e.client.read().unwrap().clone())
            .ok_or_else(|| RemoteError::NotConnected(server.into()))
    }

    /// What to hand FFmpeg for a server song or episode, and whether it's
    /// the original file.
    pub fn stream(&self, r: &RemoteRef) -> Result<(String, bool)> {
        let c = self.client(&r.server)?;
        if r.episode {
            Ok((c.episode_url(&r.id)?, false))
        } else {
            c.stream_url(&r.id)
        }
    }

    /// The cover id for a server song or folder.
    pub fn cover_id(&self, path: &Path) -> Option<(String, String)> {
        let server = server_of(path)?.to_string();
        let e = self.servers.read().unwrap().get(&server)?.clone();
        let db = e.db.lock().unwrap();
        let id = db.as_ref()?.remote_cover_for(path).ok().flatten()?;
        Some((server, id))
    }
}
