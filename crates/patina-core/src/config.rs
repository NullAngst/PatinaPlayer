//! Settings file and the standard places PatinaPlayer keeps things.
//!
//! The config is plain TOML at `~/.config/patinaplayer/config.toml`. It is
//! read with serde and edited with toml_edit, so a change made by the app
//! (adding a library folder, say) keeps your comments and layout.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::audio::output::{Backend, BitPerfect, OutputConfig};
use crate::cache::{CacheConfig, PrefetchMode};
use crate::remote::{ServerConfig, ServerKind, StreamFormat};

/// The biggest cache Settings offers: a terabyte.
pub const MAX_CACHE_MB: u64 = 1024 * 1024;

pub const APP_DIR: &str = "patinaplayer";

/// `~/.config/patinaplayer`
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| home().join(".config"))
        .join(APP_DIR)
}

/// `~/.local/share/patinaplayer`, where the library index and pinned
/// offline files live. Kept out of `~/.cache` on purpose, since cleaners
/// like BleachBit wipe that.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home().join(".local/share"))
        .join(APP_DIR)
}

/// `~/.cache/patinaplayer`
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home().join(".cache"))
        .join(APP_DIR)
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn library_db_path() -> PathBuf {
    data_dir().join("library.db")
}

/// Pinned and downloaded files. Kept out of ~/.cache so cleaners leave it.
pub fn offline_dir() -> PathBuf {
    data_dir().join("offline")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Expand a leading `~/` to the home directory.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Write a path back with `~/` when it lives under home, so the file stays
/// readable and portable between accounts.
fn contract_tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        Ok(_) => "~".into(),
        Err(_) => p.display().to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub library: LibraryConfig,
    pub output: OutputConfig,
    pub playback: PlaybackConfig,
    /// Your own radio stations, in the order you listed them.
    pub stations: Vec<Station>,
    pub podcasts: PodcastConfig,
    pub appearance: AppearanceConfig,
    /// Music servers, in the order they show. Passwords are in the keyring.
    pub servers: Vec<ServerConfig>,
    /// The track cache, prefetch and offline downloads.
    pub cache: CacheConfig,
    pub mixes: MixConfig,
    pub lastfm: LastFmConfig,
    pub leveling: crate::level::LevelConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MixConfig {
    /// Genre mixes match each song's own genre tags instead of its
    /// album's.
    pub by_track: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LastFmConfig {
    /// For finding similar artists and songs when a server can't. Free
    /// from last.fm/api/account/create.
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppearanceConfig {
    pub theme: Theme,
}

/// How both apps look. The TUI follows it as well as the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Theme {
    /// Dark verdigris and copper, Runout's palette.
    #[default]
    Runout,
    /// Pastel pink and cream, rounded, with Japanese labels.
    KawaiiLight,
    /// The same, on dreamy lavender.
    KawaiiDark,
    /// Plain light: white and grey, one blue accent.
    Light,
    /// Plain dark: charcoal grey, the same blue.
    Dark,
    /// True black, so an OLED screen switches those pixels off. Saves
    /// battery on phones and stops grey glow in a dark room.
    Oled,
}

impl Theme {
    pub const ALL: [Theme; 6] = [
        Theme::Runout,
        Theme::Light,
        Theme::Dark,
        Theme::Oled,
        Theme::KawaiiLight,
        Theme::KawaiiDark,
    ];

    /// As written in config.toml.
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Runout => "runout",
            Theme::KawaiiLight => "kawaii-light",
            Theme::KawaiiDark => "kawaii-dark",
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::Oled => "oled",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::Runout => "Runout",
            Theme::KawaiiLight => "Kawaii Light",
            Theme::KawaiiDark => "Kawaii Dark",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
            Theme::Oled => "OLED (true black)",
        }
    }

    /// The GUI's palette number.
    pub fn index(self) -> i32 {
        match self {
            Theme::Runout => 0,
            Theme::KawaiiLight => 1,
            Theme::KawaiiDark => 2,
            Theme::Light => 3,
            Theme::Dark => 4,
            Theme::Oled => 5,
        }
    }

    pub fn is_kawaii(self) -> bool {
        matches!(self, Theme::KawaiiLight | Theme::KawaiiDark)
    }

    /// Dark text on a pale ground.
    pub fn is_light(self) -> bool {
        matches!(self, Theme::KawaiiLight | Theme::Light)
    }
}

impl std::str::FromStr for Theme {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Theme::ALL
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| {
                format!(
                    "unknown theme \"{s}\" (runout, light, dark, oled, kawaii-light, kawaii-dark)"
                )
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PodcastConfig {
    pub feeds: Vec<String>,
    /// Speed with pitch kept, 0.5 to 3.
    pub speed: f64,
    pub trim_silence: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibraryConfig {
    /// Local folder roots. Each one shows as its own source.
    pub folders: Vec<PathBuf>,
    /// Watch the folders for changes while running.
    pub watch: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackConfig {
    /// Starting volume, 0 to 100.
    pub volume: u8,
    pub fade_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library: LibraryConfig {
                folders: Vec::new(),
                watch: true,
            },
            output: OutputConfig::default(),
            playback: PlaybackConfig {
                volume: 80,
                fade_ms: 100,
            },
            stations: Vec::new(),
            podcasts: PodcastConfig {
                feeds: Vec::new(),
                speed: 1.0,
                trim_silence: false,
            },
            appearance: AppearanceConfig::default(),
            servers: Vec::new(),
            cache: CacheConfig::default(),
            mixes: MixConfig::default(),
            lastfm: LastFmConfig::default(),
            leveling: crate::level::LevelConfig::default(),
        }
    }
}

// Raw serde shapes. Everything optional, so a half-written file still
// loads and falls back to defaults for what it leaves out.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    library: Option<RawLibrary>,
    output: Option<RawOutput>,
    playback: Option<RawPlayback>,
    radio: Option<RawRadio>,
    podcasts: Option<RawPodcasts>,
    appearance: Option<RawAppearance>,
    server: Option<Vec<RawServer>>,
    cache: Option<RawCache>,
    mixes: Option<RawMixes>,
    lastfm: Option<RawLastFm>,
    leveling: Option<RawLeveling>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLeveling {
    mode: Option<String>,
    target_lufs: Option<f64>,
    fallback_db: Option<f64>,
    clip_guard: Option<bool>,
    live_music: Option<bool>,
    live_radio: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMixes {
    genre_match: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLastFm {
    api_key: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawCache {
    size_mb: Option<u64>,
    prefetch: Option<String>,
    prefetch_mb: Option<u64>,
    prefetch_tracks: Option<u32>,
    pause_on_metered: Option<bool>,
    format: Option<String>,
    bitrate: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    name: String,
    url: String,
    #[serde(default)]
    user: String,
    kind: Option<String>,
    stream: Option<String>,
    bitrate: Option<u32>,
    udn: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawAppearance {
    theme: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRadio {
    station: Option<Vec<RawStation>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStation {
    name: String,
    url: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPodcasts {
    feeds: Option<Vec<String>>,
    speed: Option<f64>,
    trim_silence: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLibrary {
    folders: Option<Vec<String>>,
    watch: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    backend: Option<String>,
    device: Option<String>,
    buffer_ms: Option<u32>,
    bit_perfect: Option<String>,
    bit_perfect_device: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPlayback {
    volume: Option<u8>,
    fade_ms: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("can't read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {msg}")]
    Parse { path: PathBuf, msg: String },
}

impl Config {
    /// Parse config text. `path` is only used in error messages.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            msg: e.to_string(),
        })?;
        let d = Config::default();
        let lib = raw.library.unwrap_or_default();
        let out = raw.output.unwrap_or_default();
        let pb = raw.playback.unwrap_or_default();
        let radio = raw.radio.unwrap_or_default();
        let pods = raw.podcasts.unwrap_or_default();
        let look = raw.appearance.unwrap_or_default();
        let rc = raw.cache.unwrap_or_default();
        let parse_err = |msg: String| ConfigError::Parse {
            path: path.to_path_buf(),
            msg,
        };
        let mut servers: Vec<ServerConfig> = Vec::new();
        for s in raw.server.unwrap_or_default() {
            let name = s.name.trim().to_string();
            if name.is_empty() || name.contains('/') || name.contains('#') {
                return Err(parse_err(format!(
                    "server name \"{}\": needs to be set, without / or #",
                    s.name
                )));
            }
            if servers.iter().any(|x| x.name == name) {
                return Err(parse_err(format!("two servers are called \"{name}\"")));
            }
            let kind = match s.kind {
                Some(k) => k
                    .parse::<ServerKind>()
                    .map_err(|m| parse_err(format!("server.kind: {m}")))?,
                None => ServerKind::Subsonic,
            };
            servers.push(ServerConfig {
                name,
                // A DLNA address is the exact description URL.
                url: if kind == ServerKind::Dlna {
                    s.url.trim().to_string()
                } else {
                    crate::remote::normalize_url(&s.url)
                },
                user: s.user.trim().to_string(),
                kind,
                stream: match s.stream {
                    Some(f) => f
                        .parse::<StreamFormat>()
                        .map_err(|m| parse_err(format!("server.stream: {m}")))?,
                    None => StreamFormat::Raw,
                },
                bitrate: s.bitrate.unwrap_or(192).clamp(32, 512),
                udn: s.udn.unwrap_or_default().trim().to_string(),
            });
        }
        let dc = CacheConfig::default();
        let cache = CacheConfig {
            size_mb: rc.size_mb.unwrap_or(dc.size_mb).min(MAX_CACHE_MB),
            prefetch: match rc.prefetch {
                Some(m) => m
                    .parse::<PrefetchMode>()
                    .map_err(|m| parse_err(format!("cache.prefetch: {m}")))?,
                None => dc.prefetch,
            },
            prefetch_mb: rc.prefetch_mb.unwrap_or(dc.prefetch_mb).min(MAX_CACHE_MB),
            prefetch_tracks: rc
                .prefetch_tracks
                .unwrap_or(dc.prefetch_tracks)
                .min(crate::cache::LOOKAHEAD as u32),
            pause_on_metered: rc.pause_on_metered.unwrap_or(dc.pause_on_metered),
            format: match rc.format {
                Some(f) => f
                    .parse::<StreamFormat>()
                    .map_err(|m| parse_err(format!("cache.format: {m}")))?,
                None => dc.format,
            },
            bitrate: rc.bitrate.unwrap_or(dc.bitrate).clamp(32, 512),
        };
        let mixes = MixConfig {
            by_track: match raw
                .mixes
                .and_then(|m| m.genre_match)
                .as_deref()
                .map(|s| s.trim().to_ascii_lowercase())
                .as_deref()
            {
                None | Some("album") => false,
                Some("track") => true,
                Some(o) => {
                    return Err(parse_err(format!(
                        "mixes.genre_match: unknown \"{o}\" (album, track)"
                    )));
                }
            },
        };
        let lastfm = LastFmConfig {
            api_key: raw
                .lastfm
                .and_then(|l| l.api_key)
                .unwrap_or_default()
                .trim()
                .to_string(),
        };
        let rl = raw.leveling.unwrap_or_default();
        let dl = crate::level::LevelConfig::default();
        let leveling = crate::level::LevelConfig {
            mode: match rl.mode {
                Some(m) => m
                    .parse()
                    .map_err(|m| parse_err(format!("leveling.mode: {m}")))?,
                None => dl.mode,
            },
            target: rl
                .target_lufs
                .unwrap_or(dl.target)
                .clamp(crate::level::TARGET_MIN, crate::level::TARGET_MAX),
            fallback: rl
                .fallback_db
                .unwrap_or(dl.fallback)
                .clamp(crate::level::FALLBACK_MIN, crate::level::FALLBACK_MAX),
            clip_guard: rl.clip_guard.unwrap_or(dl.clip_guard),
            live_music: rl.live_music.unwrap_or(dl.live_music),
            live_streams: rl.live_radio.unwrap_or(dl.live_streams),
        };
        let theme = match look.theme {
            Some(t) => t.parse::<Theme>().map_err(|msg| ConfigError::Parse {
                path: path.to_path_buf(),
                msg: format!("appearance.theme: {msg}"),
            })?,
            None => Theme::default(),
        };
        let backend = match out.backend {
            Some(b) => b.parse::<Backend>().map_err(|msg| ConfigError::Parse {
                path: path.to_path_buf(),
                msg: format!("output.backend: {msg}"),
            })?,
            None => d.output.backend,
        };
        let bit_perfect = match out.bit_perfect {
            Some(b) => b.parse::<BitPerfect>().map_err(|msg| ConfigError::Parse {
                path: path.to_path_buf(),
                msg: format!("output.bit_perfect: {msg}"),
            })?,
            None => d.output.bit_perfect,
        };
        Ok(Self {
            library: LibraryConfig {
                folders: lib
                    .folders
                    .unwrap_or_default()
                    .iter()
                    .map(|f| expand_tilde(f))
                    .collect(),
                watch: lib.watch.unwrap_or(d.library.watch),
            },
            output: OutputConfig {
                backend,
                device: out.device.filter(|d| !d.is_empty()),
                buffer_ms: out.buffer_ms.unwrap_or(d.output.buffer_ms).clamp(50, 1000),
                bit_perfect,
                bit_perfect_device: out.bit_perfect_device.filter(|d| !d.trim().is_empty()),
            },
            playback: PlaybackConfig {
                volume: pb.volume.unwrap_or(d.playback.volume).min(100),
                fade_ms: pb.fade_ms.unwrap_or(d.playback.fade_ms).min(2000),
            },
            stations: radio
                .station
                .unwrap_or_default()
                .into_iter()
                .filter(|s| !s.url.trim().is_empty())
                .map(|s| Station {
                    name: s.name,
                    url: s.url.trim().to_string(),
                })
                .collect(),
            podcasts: PodcastConfig {
                feeds: pods
                    .feeds
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect(),
                speed: pods.speed.unwrap_or(1.0).clamp(0.5, 3.0),
                trim_silence: pods.trim_silence.unwrap_or(false),
            },
            appearance: AppearanceConfig { theme },
            servers,
            cache,
            mixes,
            lastfm,
            leveling,
        })
    }

    /// Load the config, writing a commented default first if there is none.
    pub fn load_or_create() -> Result<Self, ConfigError> {
        let path = config_path();
        if !path.exists() {
            let text = default_text();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, &text).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
            return Self::parse(&text, &path);
        }
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text, path)
    }
}

/// The file written on first run. Your music folder goes in if the
/// system knows where it is.
pub fn default_text() -> String {
    let folder = dirs::audio_dir()
        .filter(|d| d.is_dir())
        .map(|d| format!("\"{}\"", contract_tilde(&d)))
        .unwrap_or_default();
    format!(
        r#"# PatinaPlayer settings. Edit freely; comments are kept when the app
# changes something here itself.

[library]
# Local folders to index. Each one shows up as its own source.
folders = [{folder}]
# Pick up added, changed and removed files while the player runs.
watch = true

[output]
# auto tries PipeWire, then ALSA. Also: pipewire, alsa, null.
backend = "auto"
# ALSA device ("default", "hw:1,0") or PipeWire node name. Empty is the
# system default.
device = ""
# Output buffer in milliseconds, 50 to 1000.
buffer_ms = 200
# Bit-perfect playback of local files: off, on, or auto (whenever no EQ or
# leveling is changing the sound). It takes the sound card for itself while
# it plays, so other apps can't use it until you pause or turn this off.
bit_perfect = "off"
# Card for bit-perfect playback, like "hw:CARD=DAC,DEV=0". Empty is the
# first sound card. Settings lists the ones you have.
bit_perfect_device = ""

[appearance]
# How the app looks: runout (dark verdigris and copper), light or dark
# (plain grey with a blue accent), oled (true black, for OLED screens),
# kawaii-light (pastel pink and cream) or kawaii-dark (pastel on lavender).
# The TUI follows it too.
theme = "runout"

[playback]
# Starting volume, 0 to 100.
volume = 80
# Fade on pause, resume, seek and stop, in milliseconds. 0 turns it off.
fade_ms = 100

[leveling]
# Evens out loudness between songs. off, track (every song on its own),
# album (an album keeps its own dynamics), or smart: album while an album
# plays in order, track on shuffle, in a mix or in a mixed playlist.
mode = "smart"
# Loudness to aim for, -23 to -10 LUFS. -18 is the ReplayGain reference;
# higher is louder (and leaves less room before peaks need taming).
target_lufs = -18
# Gain for a song not measured yet, -12 to 0 dB. It's measured as it
# downloads or as it comes up in the queue, and right from then on.
fallback_db = -6
# Keep the gain low enough that the song's loudest peak can't clip.
clip_guard = true
# The live normalizer: follows the sound as it plays. Worse than a
# measured gain, so for music it's only for songs not measured yet, and
# off by default. For radio and podcasts it's all there is.
live_music = false
live_radio = true

[podcasts]
# Feed URLs. The Podcasts view adds to this list for you.
feeds = []
# Speed with pitch kept, 0.5 to 3. Applies to podcasts only.
speed = 1.0
# Cut long silences down. Applies to podcasts only.
trim_silence = false

[cache]
# Room for songs from your servers: recently played ones, so a replay
# doesn't hit the server, and prefetched ones. In MB; 0 turns it off, and
# songs then stream without being kept. Songs you pin for offline live
# apart from this and don't count.
size_mb = 10240
# Download ahead in the queue, by "size" (prefetch_mb) or by "tracks"
# (prefetch_tracks). 0 turns prefetch off. It counts against size_mb.
prefetch = "size"
prefetch_mb = 1024
prefetch_tracks = 20
# Hold prefetch and offline downloads on a metered connection (a phone
# hotspot, as NetworkManager sees it).
pause_on_metered = true
# Keep songs as the original file (raw), or as opus or mp3 the server
# converts to, at bitrate kbps, to save space. Converted songs can't play
# bit-perfect.
format = "raw"
bitrate = 192

[mixes]
# Genre mixes take songs from albums tagged with every genre you pick
# ("album"), or only songs tagged with them themselves ("track").
genre_match = "album"

[lastfm]
# An API key from last.fm/api/account/create (free). Artist and track
# mixes ask Last.fm who's similar when your server doesn't know, and it's
# the only way besides your genre tags for local folders. Optional.
api_key = ""

# Music servers: Subsonic (Navidrome, Gonic and the like) or Ampache. Add
# them in Settings, which checks the login and keeps the password in your
# keyring, never in this file.
#
# [[server]]
# name = "Home"                      # shown in the sidebar
# url = "https://music.example.com"
# user = "me"
# kind = "subsonic"                  # subsonic or ampache; Settings finds out
# stream = "raw"                     # raw (the original file), opus or mp3
# bitrate = 192                      # kbps, for opus and mp3

# Your own radio stations, in the order they show in the Radio view.
# The Radio view can add and edit these too.
#
# [[radio.station]]
# name = "Some Station"
# url = "https://example.com/listen.pls"
"#
    )
}

/// Load the config file as an editable document, let `f` change it, and
/// write it back if `f` says it changed something. Comments and layout
/// outside what `f` touches stay as they were.
fn edit(
    path: &Path,
    f: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<bool, String>,
) -> Result<bool, ConfigError> {
    let text = if path.exists() {
        std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?
    } else {
        default_text()
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| ConfigError::Parse {
                path: path.to_path_buf(),
                msg: e.to_string(),
            })?;
    let changed = f(&mut doc).map_err(|msg| ConfigError::Parse {
        path: path.to_path_buf(),
        msg,
    })?;
    if changed {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, doc.to_string()).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(changed)
}

fn table<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    name: &str,
) -> Result<&'a mut toml_edit::Table, String> {
    let t = doc.entry(name).or_insert(toml_edit::table());
    // An implicit table (only sub-tables, like [[radio.station]]) is fine.
    t.as_table_mut()
        .ok_or_else(|| format!("[{name}] is not a table"))
}

/// Replace the station list with `stations`, in order.
pub fn set_stations(path: &Path, stations: &[Station]) -> Result<bool, ConfigError> {
    edit(path, |doc| {
        let radio = table(doc, "radio")?;
        radio.set_implicit(true);
        let mut arr = toml_edit::ArrayOfTables::new();
        for s in stations {
            let mut t = toml_edit::Table::new();
            t.insert("name", toml_edit::value(s.name.clone()));
            t.insert("url", toml_edit::value(s.url.clone()));
            arr.push(t);
        }
        if arr.is_empty() {
            radio.remove("station");
        } else {
            radio.insert("station", toml_edit::Item::ArrayOfTables(arr));
        }
        Ok(true)
    })
}

/// Replace the server list with `servers`, in order.
pub fn set_servers(path: &Path, servers: &[ServerConfig]) -> Result<bool, ConfigError> {
    edit(path, |doc| {
        let mut arr = toml_edit::ArrayOfTables::new();
        for s in servers {
            let mut t = toml_edit::Table::new();
            t.insert("name", toml_edit::value(s.name.clone()));
            t.insert("url", toml_edit::value(s.url.clone()));
            t.insert("kind", toml_edit::value(s.kind.as_str()));
            if s.kind.logs_in() {
                t.insert("user", toml_edit::value(s.user.clone()));
            }
            if s.kind.transcodes() {
                t.insert("stream", toml_edit::value(s.stream.as_str()));
                t.insert("bitrate", toml_edit::value(i64::from(s.bitrate)));
            }
            if !s.udn.is_empty() {
                t.insert("udn", toml_edit::value(s.udn.clone()));
            }
            arr.push(t);
        }
        if arr.is_empty() {
            doc.remove("server");
        } else {
            doc.insert("server", toml_edit::Item::ArrayOfTables(arr));
        }
        Ok(true)
    })
}

/// Add or remove a podcast feed URL.
pub fn edit_podcast_feed(path: &Path, url: &str, add: bool) -> Result<bool, ConfigError> {
    edit(path, |doc| {
        let pods = table(doc, "podcasts")?;
        let arr = pods
            .entry("feeds")
            .or_insert(toml_edit::value(toml_edit::Array::new()))
            .as_array_mut()
            .ok_or("podcasts.feeds is not a list")?;
        let has = arr.iter().any(|v| v.as_str() == Some(url));
        if add && !has {
            arr.push(url);
            Ok(true)
        } else if !add && has {
            arr.retain(|v| v.as_str() != Some(url));
            Ok(true)
        } else {
            Ok(false)
        }
    })
}

/// Save podcast speed and silence trimming.
pub fn set_podcast_playback(path: &Path, speed: f64, trim: bool) -> Result<bool, ConfigError> {
    edit(path, |doc| {
        let pods = table(doc, "podcasts")?;
        let speed = (speed * 100.0).round() / 100.0;
        pods.insert("speed", toml_edit::value(speed));
        pods.insert("trim_silence", toml_edit::value(trim));
        Ok(true)
    })
}

/// Set one value, like `output.buffer_ms = 300`, keeping the rest of the
/// file as it was.
pub fn set_value(
    path: &Path,
    section: &str,
    key: &str,
    value: impl Into<toml_edit::Value>,
) -> Result<bool, ConfigError> {
    let value = value.into();
    edit(path, |doc| {
        let t = table(doc, section)?;
        let bare = |v: &toml_edit::Value| {
            let mut v = v.clone();
            v.decor_mut().clear();
            v.to_string()
        };
        let same = t
            .get(key)
            .and_then(|i| i.as_value())
            .is_some_and(|v| bare(v) == bare(&value));
        if same {
            return Ok(false);
        }
        // Keep a trailing comment on the line if there was one.
        let suffix = t
            .get(key)
            .and_then(|i| i.as_value())
            .and_then(|v| v.decor().suffix().cloned());
        let mut v = value;
        if let Some(sfx) = suffix {
            v.decor_mut().set_suffix(sfx);
        }
        t.insert(key, toml_edit::Item::Value(v));
        Ok(true)
    })
}

/// Add or remove a library folder in the config file, keeping the rest of
/// the file as you wrote it. Returns false if nothing needed changing.
pub fn edit_library_folder(path: &Path, folder: &Path, add: bool) -> Result<bool, ConfigError> {
    edit(path, |doc| edit_folders(doc, folder, add))
}

fn edit_folders(
    doc: &mut toml_edit::DocumentMut,
    folder: &Path,
    add: bool,
) -> Result<bool, String> {
    let lib = table(doc, "library")?;
    let arr = lib
        .entry("folders")
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
        .ok_or("library.folders is not a list")?;
    let same = |v: &toml_edit::Value| v.as_str().is_some_and(|s| expand_tilde(s) == folder);
    if add {
        if arr.iter().any(same) {
            return Ok(false);
        }
        arr.push(contract_tilde(folder));
        Ok(true)
    } else {
        let before = arr.len();
        arr.retain(|v| !same(v));
        Ok(arr.len() != before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_text_parses() {
        let c = Config::parse(&default_text(), Path::new("x")).unwrap();
        assert_eq!(c.playback.volume, 80);
        assert!(c.library.watch);
        assert_eq!(c.appearance.theme, Theme::Runout);
        assert_eq!(
            c.cache,
            CacheConfig::default(),
            "the file says the defaults"
        );
    }

    #[test]
    fn cache_section() {
        let c = Config::parse(
            "[cache]\nsize_mb = 2048\nprefetch = \"tracks\"\nprefetch_tracks = 50\n\
             format = \"opus\"\nbitrate = 128\npause_on_metered = false\n",
            Path::new("x"),
        )
        .unwrap();
        assert_eq!(c.cache.size_mb, 2048);
        assert_eq!(c.cache.prefetch, PrefetchMode::Tracks);
        assert_eq!(c.cache.prefetch_tracks, 50);
        assert_eq!(c.cache.format, StreamFormat::Opus);
        assert_eq!(c.cache.bitrate, 128);
        assert!(!c.cache.pause_on_metered);
        assert!(Config::parse("[cache]\nprefetch = \"soon\"\n", Path::new("x")).is_err());
        assert!(Config::parse("[cache]\nsize = 3\n", Path::new("x")).is_err());
    }

    #[test]
    fn servers_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "# mine\n[playback]\nvolume = 50\n").unwrap();
        let s = ServerConfig {
            name: "Home".into(),
            url: "https://music.example.com".into(),
            user: "zack".into(),
            kind: ServerKind::Ampache,
            stream: StreamFormat::Opus,
            bitrate: 128,
            udn: String::new(),
        };
        let dlna = ServerConfig {
            name: "NAS".into(),
            url: "http://192.168.1.5:8200/rootDesc.xml".into(),
            kind: ServerKind::Dlna,
            bitrate: 192,
            udn: "uuid:4d696e69-444c-164e-9d41-363da121361f".into(),
            ..Default::default()
        };
        set_servers(&p, &[s.clone(), dlna.clone()]).unwrap();
        let c = Config::load_from(&p).unwrap();
        assert_eq!(c.servers, vec![s, dlna]);
        assert_eq!(c.playback.volume, 50);
        assert!(std::fs::read_to_string(&p).unwrap().contains("# mine"));
        set_servers(&p, &[]).unwrap();
        assert!(Config::load_from(&p).unwrap().servers.is_empty());
        assert!(
            Config::parse("[[server]]\nname = \"a/b\"\nurl = \"x\"\n", &p).is_err(),
            "no slash in a name"
        );
    }

    #[test]
    fn themes_parse() {
        let c = Config::parse("[appearance]\ntheme = \"kawaii-dark\"\n", Path::new("x")).unwrap();
        assert_eq!(c.appearance.theme, Theme::KawaiiDark);
        let c = Config::parse("[appearance]\ntheme = \"OLED\"\n", Path::new("x")).unwrap();
        assert_eq!(c.appearance.theme, Theme::Oled);
        assert!(Config::parse("[appearance]\ntheme = \"neon\"\n", Path::new("x")).is_err());
        for t in Theme::ALL {
            assert_eq!(t.as_str().parse::<Theme>(), Ok(t));
        }
    }

    #[test]
    fn partial_file_falls_back() {
        let c = Config::parse("[output]\nbuffer_ms = 5000\n", Path::new("x")).unwrap();
        assert_eq!(c.output.buffer_ms, 1000, "clamped");
        assert_eq!(c.playback.fade_ms, 100);
    }

    #[test]
    fn typo_is_an_error() {
        assert!(Config::parse("[output]\nbufer_ms = 5\n", Path::new("x")).is_err());
        assert!(Config::parse("[output]\nbackend = \"oss\"\n", Path::new("x")).is_err());
    }

    #[test]
    fn tilde_expands() {
        let c = Config::parse("[library]\nfolders = [\"~/Music\"]\n", Path::new("x")).unwrap();
        assert_eq!(c.library.folders[0], home().join("Music"));
    }

    #[test]
    fn stations_and_feeds_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, default_text()).unwrap();
        let st = vec![
            Station {
                name: "One".into(),
                url: "http://a/1.pls".into(),
            },
            Station {
                name: "Two".into(),
                url: "http://b/2".into(),
            },
        ];
        set_stations(&p, &st).unwrap();
        assert!(edit_podcast_feed(&p, "https://x/feed.xml", true).unwrap());
        assert!(!edit_podcast_feed(&p, "https://x/feed.xml", true).unwrap());
        set_podcast_playback(&p, 1.25, true).unwrap();
        let c = Config::load_from(&p).unwrap();
        assert_eq!(c.stations, st);
        assert_eq!(c.podcasts.feeds, vec!["https://x/feed.xml"]);
        assert_eq!(c.podcasts.speed, 1.25);
        assert!(c.podcasts.trim_silence);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# Starting volume"), "comments survive");
        // Removing every station leaves a valid file.
        set_stations(&p, &[]).unwrap();
        assert!(Config::load_from(&p).unwrap().stations.is_empty());
    }

    #[test]
    fn set_value_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "# top\n[output]\nbuffer_ms = 200 # latency\n").unwrap();
        assert!(set_value(&p, "output", "buffer_ms", 300i64).unwrap());
        assert!(!set_value(&p, "output", "buffer_ms", 300i64).unwrap());
        assert!(set_value(&p, "output", "bit_perfect", "on").unwrap());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("# top") && text.contains("# latency"),
            "{text}"
        );
        let c = Config::load_from(&p).unwrap();
        assert_eq!(c.output.buffer_ms, 300);
        assert_eq!(c.output.bit_perfect, BitPerfect::On);
    }

    #[test]
    fn add_folder_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "# mine\n[library]\nfolders = [] # roots\n").unwrap();
        assert!(edit_library_folder(&p, Path::new("/srv/music"), true).unwrap());
        assert!(!edit_library_folder(&p, Path::new("/srv/music"), true).unwrap());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("# mine") && text.contains("# roots"),
            "{text}"
        );
        assert!(text.contains("/srv/music"));
        assert!(edit_library_folder(&p, Path::new("/srv/music"), false).unwrap());
        let c = Config::load_from(&p).unwrap();
        assert!(c.library.folders.is_empty());
    }
}
