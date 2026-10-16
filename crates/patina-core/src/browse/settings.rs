//! The Settings view as data: every option in config.toml, with the kind
//! of control it needs and how a change applies. Both frontends draw these
//! rows and send changes back through [`super::Browser::change_setting`],
//! which writes the file (comments kept) and applies the change live where
//! it can.

use std::path::PathBuf;

use crate::audio::output::exclusive::HwDevice;
use crate::audio::output::{Backend, BitPerfect, OutDevice, RATES, SampleFormat};
use crate::config::{Config, Theme};
use crate::dsp::resample::Quality;
use crate::remote::{ServerStatus, StreamFormat};

/// Which option a row is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A library folder; its row removes it.
    Folder(PathBuf),
    AddFolder,
    Watch,
    Theme,
    StartVolume,
    FadeMs,
    Backend,
    Device,
    BufferMs,
    /// Output sample rate: match the source, or a fixed rate.
    OutRate,
    Resampler,
    /// Sample format for ALSA.
    OutFormat,
    Dither,
    Crossfade,
    BitPerfect,
    BitPerfectDevice,
    PodcastSpeed,
    PodcastTrim,
    /// A music server, by name: sync, edit, remove.
    Server(String),
    /// What a server streams: the original file, or a transcode.
    ServerStream(String),
    AddServer,
    /// Look for DLNA servers on the network.
    FindDlna,
    /// The cache's size cap.
    CacheSize,
    /// Prefetch by size or by song count.
    Prefetch,
    /// How far ahead prefetch goes, in the unit `Prefetch` picks.
    PrefetchAmount,
    PauseMetered,
    /// What songs are kept as: the original, or a conversion.
    CacheFormat,
    /// Genre mixes match album tags or each song's tags.
    GenreMatch,
    /// Ask LRCLIB for lyrics.
    LyricsOnline,
    LastFmKey,
    /// Link to Last.fm, or unlink.
    LastFmLink,
    LastFmSecret,
    Scrobble,
    ScrobbleRadio,
    /// Love stars the song on its server too.
    LoveStars,
    /// Who scrobbles a server's songs.
    ServerScrobble(String),
    LevelMode,
    /// Target loudness, LUFS.
    LevelTarget,
    /// Gain for songs not measured yet.
    LevelFallback,
    ClipGuard,
    /// The live normalizer for music not measured yet.
    LiveMusic,
    /// The live normalizer for radio and podcasts.
    LiveRadio,
    /// Measure every untagged song now, or stop.
    MeasureLibrary,
    /// The true-peak limiter, and its ceiling.
    Limiter,
    LimiterCeiling,
    // Channel tools.
    ChannelOn,
    ChannelMono,
    ChannelWidth,
    ChannelCrossfeed,
    ChannelBalance,
    // The desktop app's tray icon.
    Tray,
    CloseToTray,
    // The EQ view's rows.
    EqOn,
    EqMode,
    /// Graphic band count.
    EqBands,
    EqPreset,
    EqPreamp,
    EqAutoPreamp,
    /// Give the output device its own EQ, or take it away.
    EqDevice,
    EqSavePreset,
    EqImport,
    EqDeletePreset,
    /// A graphic band's gain.
    EqGraphic(usize),
    /// A parametric band, as text.
    EqBand(usize),
    EqAddBand,
    // The Amp view's rows.
    AmpOn,
    AmpPreset,
    AmpOversample,
    /// Add a device to the rack.
    AmpAdd,
    AmpSavePreset,
    AmpDeletePreset,
    /// Give the output device its own stack, or take it away.
    AmpDevice,
    AmpUnitOn(crate::dsp::amp::Kind),
    /// A device's knob, by its place in [`crate::dsp::amp::Kind::params`].
    AmpParam(crate::dsp::amp::Kind, usize),
    AmpTrimIn(crate::dsp::amp::Kind),
    AmpTrimOut(crate::dsp::amp::Kind),
    AmpMoveUp(crate::dsp::amp::Kind),
    AmpMoveDown(crate::dsp::amp::Kind),
    AmpRemove(crate::dsp::amp::Kind),
    /// Read-only facts: where files are.
    Info(&'static str),
}

impl Key {
    /// A row of the Amp view.
    pub fn is_amp(&self) -> bool {
        matches!(
            self,
            Key::AmpOn
                | Key::AmpPreset
                | Key::AmpOversample
                | Key::AmpAdd
                | Key::AmpSavePreset
                | Key::AmpDeletePreset
                | Key::AmpDevice
                | Key::AmpUnitOn(_)
                | Key::AmpParam(..)
                | Key::AmpTrimIn(_)
                | Key::AmpTrimOut(_)
                | Key::AmpMoveUp(_)
                | Key::AmpMoveDown(_)
                | Key::AmpRemove(_)
        )
    }

    /// A row of the EQ view rather than Settings.
    pub fn is_eq(&self) -> bool {
        matches!(
            self,
            Key::EqOn
                | Key::EqMode
                | Key::EqBands
                | Key::EqPreset
                | Key::EqPreamp
                | Key::EqAutoPreamp
                | Key::EqDevice
                | Key::EqSavePreset
                | Key::EqImport
                | Key::EqDeletePreset
                | Key::EqGraphic(_)
                | Key::EqBand(_)
                | Key::EqAddBand
        )
    }
}

/// How a row is edited.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Toggle(bool),
    /// Pick one. `options` are (value written to the file, label shown).
    Choice {
        options: Vec<(String, String)>,
        index: usize,
    },
    /// A whole number shown as `value / scale` with a unit: podcast speed
    /// is stored as tenths and shown as "1.2x".
    Number {
        value: i64,
        min: i64,
        max: i64,
        step: i64,
        scale: i64,
        unit: &'static str,
    },
    /// Free text, like a device name. Empty means the default.
    Text {
        value: String,
        empty: &'static str,
    },
    /// A library folder, removable.
    Folder,
    /// A button.
    Action(&'static str),
    /// A music server: its status line. Sync, Edit and Remove go with it.
    Server(String),
    Info(String),
}

impl Control {
    /// The value as the row shows it.
    pub fn display(&self) -> String {
        match self {
            Control::Toggle(true) => "on".into(),
            Control::Toggle(false) => "off".into(),
            Control::Choice { options, index } => {
                options.get(*index).map(|o| o.1.clone()).unwrap_or_default()
            }
            Control::Number {
                value, scale, unit, ..
            } => {
                if *scale == 1 {
                    format!("{value}{unit}")
                } else {
                    format!("{:.1}{unit}", *value as f64 / *scale as f64)
                }
            }
            Control::Text { value, empty } => {
                if value.is_empty() {
                    (*empty).to_string()
                } else {
                    value.clone()
                }
            }
            Control::Folder => String::new(),
            Control::Action(a) => (*a).to_string(),
            Control::Server(s) => s.clone(),
            Control::Info(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: Key,
    pub label: String,
    /// One line on what it does, for a tooltip or the TUI's hint line.
    pub help: &'static str,
    pub control: Control,
    /// Greyed out, and why: leveling while bit-perfect is on.
    pub disabled: Option<&'static str>,
}

/// A change to a setting, as a frontend expresses it.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Flip a toggle.
    Toggle,
    /// Next or previous choice, or a number up or down by its step.
    Step(i32),
    /// Pick a choice by index.
    Choose(usize),
    /// Typed text, or a typed number.
    Set(String),
}

/// What Settings shows besides the config: where things live, and the
/// sound cards there are to pick from.
#[derive(Debug, Clone, Default)]
pub struct SettingsInfo {
    pub config_path: PathBuf,
    pub db_path: PathBuf,
    pub offline_dir: PathBuf,
    /// Where cached songs go, and pinned ones.
    pub cache_dir: PathBuf,
    pub pinned_dir: PathBuf,
    pub no_library: bool,
    /// Options a command-line flag set for this run, and the flag, so the
    /// row can say why what's playing differs from what it shows.
    pub overrides: Vec<(Key, String)>,
}

/// The warning shown before bit-perfect is turned on.
pub const BIT_PERFECT_WARNING: &str = "Bit-perfect takes exclusive control of this device. \
Other apps won't be able to play sound through it until PatinaPlayer stops or you turn this off.";

/// Why leveling greys out.
pub const LEVELING_BIT_PERFECT: &str =
    "Bit-perfect is on, and it skips leveling. Set bit-perfect to Off or Auto to level.";

/// Stream choices for a server: (value, label). The value is
/// "format" or "format:kbps".
pub const STREAM_CHOICES: [(&str, &str); 6] = [
    ("raw", "Original file"),
    ("opus:96", "Opus 96 kbps"),
    ("opus:128", "Opus 128 kbps"),
    ("opus:192", "Opus 192 kbps"),
    ("mp3:192", "MP3 192 kbps"),
    ("mp3:320", "MP3 320 kbps"),
];

/// Read a stream choice back.
pub fn parse_stream(v: &str) -> (StreamFormat, Option<u32>) {
    let (f, k) = v.split_once(':').unwrap_or((v, ""));
    (f.parse().unwrap_or_default(), k.parse().ok())
}

/// What the threads report that Settings shows.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// A whole-library loudness run is going; None without the library.
    pub measuring: Option<bool>,
    /// Last.fm's status; None without the library.
    pub lastfm: Option<crate::scrobble::ScrobbleStatus>,
    /// Devices the shared output can play to, as last looked up.
    pub outputs: Vec<OutDevice>,
}

/// Device choices: the system default, then each device there is, then
/// the configured one if it isn't there now.
pub fn device_options(outputs: &[OutDevice], cur: Option<&str>) -> (Vec<(String, String)>, usize) {
    let mut opts: Vec<(String, String)> = vec![(String::new(), "System default".into())];
    opts.extend(outputs.iter().map(|d| (d.id.clone(), d.label.clone())));
    if let Some(cur) = cur
        && !outputs.iter().any(|d| d.id == cur)
    {
        opts.push((cur.to_string(), format!("{cur} (not connected)")));
    }
    let index = cur
        .and_then(|c| opts.iter().position(|o| o.0 == c))
        .unwrap_or(0);
    (opts, index)
}

/// "44.1 kHz", "48 kHz".
pub fn khz(rate: u32) -> String {
    format!("{} kHz", khz_num(rate))
}

/// "44.1→48k", for a resampling badge.
pub fn resample_badge((from, to): (u32, u32)) -> String {
    format!("{}→{}k", khz_num(from), khz_num(to))
}

fn khz_num(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        (rate / 1000).to_string()
    } else {
        format!("{:.1}", rate as f64 / 1000.0)
    }
}

/// Rows, grouped under headers, for the Settings view.
pub fn rows(
    cfg: &Config,
    info: &SettingsInfo,
    podcast_tempo: f64,
    podcast_trim: bool,
    devices: &[HwDevice],
    servers: &[ServerStatus],
    live: Live,
) -> Vec<(Option<&'static str>, Setting)> {
    let Live {
        measuring,
        lastfm,
        outputs,
    } = live;
    let lastfm = lastfm.as_ref();
    let mut v: Vec<(Option<&'static str>, Setting)> = Vec::new();
    let mut push = |group: &'static str, s: Setting| v.push((Some(group), s));

    // Library
    if !info.no_library {
        for f in &cfg.library.folders {
            push(
                "Library",
                Setting {
                    disabled: None,
                    key: Key::Folder(f.clone()),
                    label: f.display().to_string(),
                    help: "A library folder. Removing it takes its tracks out of the index; the files stay where they are.",
                    control: Control::Folder,
                },
            );
        }
        push(
            "Library",
            Setting {
                disabled: None,
                key: Key::AddFolder,
                label: "Add a music folder".into(),
                help: "Index another folder. The scan starts right away.",
                control: Control::Action("Add folder"),
            },
        );
        push(
            "Library",
            Setting {
                disabled: None,
                key: Key::Watch,
                label: "Watch for changes".into(),
                help: "Pick up added, changed and removed files while the player runs.",
                control: Control::Toggle(cfg.library.watch),
            },
        );
    }

    // Servers
    if !info.no_library {
        for sc in &cfg.servers {
            let status = servers
                .iter()
                .find(|s| s.name == sc.name)
                .map(ServerStatus::summary)
                .unwrap_or_else(|| "starting".into());
            if !sc.kind.logs_in() {
                let host = url::Url::parse(&sc.url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_else(|| sc.url.clone());
                push(
                    "Servers",
                    Setting {
                        disabled: None,
                        key: Key::Server(sc.name.clone()),
                        label: sc.name.clone(),
                        help: "A DLNA server on your network. Sync reads its library again now. Edit renames it.",
                        control: Control::Server(format!("{host} · {status}")),
                    },
                );
                continue;
            }
            let who = if sc.user.is_empty() {
                "API key".to_string()
            } else {
                sc.user.clone()
            };
            push(
                "Servers",
                Setting {
                    disabled: None,
                    key: Key::Server(sc.name.clone()),
                    label: sc.name.clone(),
                    help: "Sync reads the server's library again now. Edit changes the address, user or password.",
                    control: Control::Server(format!("{} · {who} · {status}", sc.url)),
                },
            );
            let current = match sc.stream {
                StreamFormat::Raw => "raw".to_string(),
                f => format!("{}:{}", f.as_str(), sc.bitrate),
            };
            let mut options: Vec<(String, String)> = STREAM_CHOICES
                .iter()
                .map(|(v, l)| (v.to_string(), l.to_string()))
                .collect();
            if !options.iter().any(|o| o.0 == current) {
                options.push((
                    current.clone(),
                    format!("{} {} kbps", sc.stream.as_str(), sc.bitrate),
                ));
            }
            let index = options.iter().position(|o| o.0 == current).unwrap_or(0);
            let mode = cfg.lastfm.mode(&sc.name);
            push(
                "Servers",
                Setting {
                    disabled: None,
                    key: Key::ServerScrobble(sc.name.clone()),
                    label: format!("{}: scrobbles", sc.name),
                    help: "Who tells Last.fm about this server's songs. Pick the server if it scrobbles for you (Navidrome's Last.fm agent); both counts every play twice if it does.",
                    control: Control::Choice {
                        options: crate::config::ScrobbleMode::ALL
                            .iter()
                            .map(|m| (m.as_str().to_string(), m.label().to_string()))
                            .collect(),
                        index: crate::config::ScrobbleMode::ALL
                            .iter()
                            .position(|m| *m == mode)
                            .unwrap_or(0),
                    },
                },
            );
            push(
                "Servers",
                Setting {
                    disabled: None,
                    key: Key::ServerStream(sc.name.clone()),
                    label: format!("{}: streaming", sc.name),
                    help: "The original file keeps the sound untouched (and lets bit-perfect work). A transcode saves data on a slow line.",
                    control: Control::Choice { options, index },
                },
            );
        }
        push(
            "Servers",
            Setting {
                disabled: None,
                key: Key::AddServer,
                label: "Add a music server".into(),
                help: "Navidrome, Gonic, Airsonic and other Subsonic servers, or Ampache. The password goes in your keyring.",
                control: Control::Action("Add server"),
            },
        );
        push(
            "Servers",
            Setting {
                disabled: None,
                key: Key::FindDlna,
                label: "Find DLNA servers".into(),
                help: "MiniDLNA, Gerbera, Jellyfin, Plex, Serviio or a NAS on your network. No login needed.",
                control: Control::Action("Find servers"),
            },
        );
    }

    // Cache and offline
    if !info.no_library {
        let c = &cfg.cache;
        push(
            "Cache and offline",
            Setting {
                disabled: None,
                key: Key::CacheSize,
                label: "Cache size".into(),
                help: "Room for recently played and prefetched songs from your servers. 0 turns it off; pinned songs don't count.",
                control: Control::Number {
                    value: ((c.size_mb * 10 + 512) / 1024) as i64,
                    min: 0,
                    max: (crate::config::MAX_CACHE_MB / 1024 * 10) as i64,
                    step: 10,
                    scale: 10,
                    unit: " GB",
                },
            },
        );
        push(
            "Cache and offline",
            Setting {
                disabled: None,
                key: Key::Prefetch,
                label: "Prefetch".into(),
                help: "Download songs coming up in the queue, nearest first. By size suits a mix of hi-res and MP3.",
                control: Control::Choice {
                    options: vec![
                        ("size".into(), "By size".into()),
                        ("tracks".into(), "By number of songs".into()),
                    ],
                    index: usize::from(c.prefetch == crate::cache::PrefetchMode::Tracks),
                },
            },
        );
        push(
            "Cache and offline",
            Setting {
                disabled: None,
                key: Key::PrefetchAmount,
                label: "Prefetch ahead".into(),
                help: "How far ahead to download. 0 turns prefetch off. It comes out of the cache size.",
                control: match c.prefetch {
                    crate::cache::PrefetchMode::Size => Control::Number {
                        value: c.prefetch_mb as i64,
                        min: 0,
                        max: crate::config::MAX_CACHE_MB as i64,
                        step: 256,
                        scale: 1,
                        unit: " MB",
                    },
                    crate::cache::PrefetchMode::Tracks => Control::Number {
                        value: i64::from(c.prefetch_tracks),
                        min: 0,
                        max: crate::cache::LOOKAHEAD as i64,
                        step: 5,
                        scale: 1,
                        unit: " songs",
                    },
                },
            },
        );
        push(
            "Cache and offline",
            Setting {
                disabled: None,
                key: Key::PauseMetered,
                label: "Hold on metered connections".into(),
                help: "Hold prefetch and offline downloads while NetworkManager says the connection is metered (a phone hotspot).",
                control: Control::Toggle(c.pause_on_metered),
            },
        );
        let current = c.format_label();
        let mut options: Vec<(String, String)> = STREAM_CHOICES
            .iter()
            .map(|(v, l)| (v.to_string(), l.to_string()))
            .collect();
        if !options.iter().any(|o| o.0 == current) {
            options.push((
                current.clone(),
                format!("{} {} kbps", c.format.as_str(), c.bitrate),
            ));
        }
        let index = options.iter().position(|o| o.0 == current).unwrap_or(0);
        push(
            "Cache and offline",
            Setting {
                disabled: None,
                key: Key::CacheFormat,
                label: "Keep songs as".into(),
                help: "The original file, or a conversion the server makes to save space. Converted songs can't play bit-perfect.",
                control: Control::Choice { options, index },
            },
        );
    }

    // Mixes
    if !info.no_library {
        push(
            "Mixes",
            Setting {
                disabled: None,
                key: Key::GenreMatch,
                label: "Genre mixes match".into(),
                help: "Album tags keeps half-tagged albums out; each song's tags follows the songs themselves.",
                control: Control::Choice {
                    options: vec![
                        ("album".into(), "Album tags".into()),
                        ("track".into(), "Each song's tags".into()),
                    ],
                    index: usize::from(cfg.mixes.by_track),
                },
            },
        );
    }

    // Last.fm
    if let Some(st) = lastfm {
        let lf = &cfg.lastfm;
        let (label, action) = if !st.user.is_empty() {
            (format!("Linked as {}", st.user), "Unlink")
        } else if st.linking {
            (
                "Waiting for you to approve it on Last.fm".to_string(),
                "Cancel",
            )
        } else {
            ("Not linked".to_string(), "Link")
        };
        let mut label = label;
        if st.waiting > 0 {
            label.push_str(&format!(
                " · {} scrobble{} waiting",
                st.waiting,
                if st.waiting == 1 { "" } else { "s" }
            ));
        }
        push(
            "Last.fm",
            Setting {
                disabled: None,
                key: Key::LastFmLink,
                label,
                help: "Opens Last.fm's page to approve PatinaPlayer, and finishes on its own once you do. The session key goes in your keyring.",
                control: Control::Action(action),
            },
        );
        let has_builtin = crate::lastfm::builtin().0.is_some();
        push(
            "Last.fm",
            Setting {
                disabled: None,
                key: Key::LastFmKey,
                label: "API key".into(),
                help: "Your own, from last.fm/api/account/create (free). Mixes use it to ask who's similar, and scrobbling needs it with its secret.",
                control: Control::Text {
                    value: lf.api_key.clone(),
                    empty: if has_builtin { "built in" } else { "not set" },
                },
            },
        );
        push(
            "Last.fm",
            Setting {
                disabled: None,
                key: Key::LastFmSecret,
                label: "API secret".into(),
                help: "The shared secret that came with your API key. Scrobbling signs its calls with it.",
                control: Control::Text {
                    value: if lf.api_secret.is_empty() {
                        String::new()
                    } else {
                        "••••••••".into()
                    },
                    empty: if has_builtin && lf.api_key.is_empty() {
                        "built in"
                    } else {
                        "not set"
                    },
                },
            },
        );
        push(
            "Last.fm",
            Setting {
                disabled: None,
                key: Key::Scrobble,
                label: "Scrobble".into(),
                help: "Send plays to Last.fm: half a song or four minutes, whichever is first, for songs over 30 seconds. Podcasts never.",
                control: Control::Toggle(lf.scrobble),
            },
        );
        push(
            "Last.fm",
            Setting {
                disabled: None,
                key: Key::ScrobbleRadio,
                label: "Scrobble radio".into(),
                help: "From the station's song titles (\"Artist - Title\"), once a song has been on a minute. Station metadata is often junk.",
                control: Control::Toggle(lf.scrobble_radio),
            },
        );
        push(
            "Last.fm",
            Setting {
                disabled: None,
                key: Key::LoveStars,
                label: "Love also stars it".into(),
                help: "Loving a song from a server (l, or the heart) also stars it there.",
                control: Control::Toggle(lf.love_stars),
            },
        );
    }

    // Appearance
    push(
        "Appearance",
        Setting {
            disabled: None,
            key: Key::Theme,
            label: "Theme".into(),
            help: "Runout, plain Light or Dark, OLED true black (saves power on OLED screens), or the pastel kawaii pair.",
            control: Control::Choice {
                options: Theme::ALL
                    .iter()
                    .map(|t| (t.as_str().to_string(), t.label().to_string()))
                    .collect(),
                index: Theme::ALL
                    .iter()
                    .position(|t| *t == cfg.appearance.theme)
                    .unwrap_or(0),
            },
        },
    );

    // Desktop
    push(
        "Desktop",
        Setting {
            disabled: None,
            key: Key::Tray,
            label: "Tray icon".into(),
            help: "An icon in the system tray: click it to show or hide the window, middle-click to play or pause, right-click for a menu. Desktop app only.",
            control: Control::Toggle(cfg.desktop.tray),
        },
    );
    push(
        "Desktop",
        Setting {
            disabled: (!cfg.desktop.tray).then_some("Needs the tray icon"),
            key: Key::CloseToTray,
            label: "Close to tray".into(),
            help: "Closing the window hides it to the tray and keeps playing. Off, closing quits. Ctrl+Q always quits.",
            control: Control::Toggle(cfg.desktop.close_to_tray),
        },
    );

    // Playback
    push(
        "Playback",
        Setting {
            disabled: None,
            key: Key::StartVolume,
            label: "Starting volume".into(),
            help: "Volume the player starts at.",
            control: Control::Number {
                value: cfg.playback.volume as i64,
                min: 0,
                max: 100,
                step: 5,
                scale: 1,
                unit: "%",
            },
        },
    );
    push(
        "Playback",
        Setting {
            disabled: None,
            key: Key::FadeMs,
            label: "Fades".into(),
            help: "Fade on pause, resume, seek and stop. 0 turns it off.",
            control: Control::Number {
                value: cfg.playback.fade_ms as i64,
                min: 0,
                max: 2000,
                step: 50,
                scale: 1,
                unit: " ms",
            },
        },
    );

    // Leveling
    let lv = &cfg.leveling;
    let off = (cfg.output.bit_perfect == BitPerfect::On).then_some(LEVELING_BIT_PERFECT);
    let mut lpush = |s: Setting| push("Leveling", Setting { disabled: off, ..s });
    lpush(Setting {
        key: Key::LevelMode,
        label: "Leveling".into(),
        help: "Evens out loudness between songs. Smart keeps an album's own dynamics while it plays in order and levels song by song on shuffle and in mixes.",
        control: Control::Choice {
            options: crate::level::LevelMode::ALL
                .iter()
                .map(|m| (m.as_str().to_string(), m.label().to_string()))
                .collect(),
            index: crate::level::LevelMode::ALL
                .iter()
                .position(|m| *m == lv.mode)
                .unwrap_or(3),
        },
        disabled: None,
    });
    lpush(Setting {
        key: Key::LevelTarget,
        label: "Target loudness".into(),
        help: "-18 LUFS is the ReplayGain reference. Higher is louder, with less room before peaks have to be held back.",
        control: Control::Number {
            value: lv.target.round() as i64,
            min: crate::level::TARGET_MIN as i64,
            max: crate::level::TARGET_MAX as i64,
            step: 1,
            scale: 1,
            unit: " LUFS",
        },
        disabled: None,
    });
    lpush(Setting {
        key: Key::LevelFallback,
        label: "Gain before measuring".into(),
        help: "For a song with no ReplayGain that hasn't been measured yet. It gets measured as it downloads or comes up in the queue.",
        control: Control::Number {
            value: lv.fallback.round() as i64,
            min: crate::level::FALLBACK_MIN as i64,
            max: crate::level::FALLBACK_MAX as i64,
            step: 1,
            scale: 1,
            unit: " dB",
        },
        disabled: None,
    });
    lpush(Setting {
        key: Key::ClipGuard,
        label: "Keep peaks from clipping".into(),
        help: "Holds the gain down so a song's loudest peak stays under full scale.",
        control: Control::Toggle(lv.clip_guard),
        disabled: None,
    });
    lpush(Setting {
        key: Key::LiveMusic,
        label: "Live normalizer: music".into(),
        help: "Follows songs not measured yet as they play, instead of the fixed gain. Audibly worse than a measured gain.",
        control: Control::Toggle(lv.live_music),
        disabled: None,
    });
    lpush(Setting {
        key: Key::LiveRadio,
        label: "Live normalizer: radio".into(),
        help: "Radio and podcasts can't be measured ahead, so this rides their level as they play.",
        control: Control::Toggle(lv.live_streams),
        disabled: None,
    });
    if let Some(running) = measuring {
        // Measuring only fills in the index, so it stays open either way.
        push(
            "Leveling",
            Setting {
                key: Key::MeasureLibrary,
                label: "Measure the library".into(),
                help: "Measure every local song without ReplayGain tags now, in the background, instead of as each comes up.",
                control: Control::Action(if running {
                    "Stop measuring"
                } else {
                    "Measure now"
                }),
                disabled: None,
            },
        );
    }

    // Limiter
    push(
        "Limiter",
        Setting {
            key: Key::Limiter,
            label: "True-peak limiter".into(),
            help: "Last in the chain, so EQ boosts and leveling never clip. Below the ceiling it leaves the sound alone, bit for bit.",
            control: Control::Toggle(cfg.limiter.on),
            disabled: None,
        },
    );
    push(
        "Limiter",
        Setting {
            key: Key::LimiterCeiling,
            label: "Ceiling".into(),
            help: "How high peaks may go, between samples included. -1 dBTP leaves room for DACs that clip a little early.",
            control: Control::Number {
                value: (cfg.limiter.ceiling_db * 10.0).round() as i64,
                min: -60,
                max: 0,
                step: 1,
                scale: 10,
                unit: " dBTP",
            },
            disabled: None,
        },
    );

    // Channel tools
    let chc = &cfg.channel;
    let ch_off = (!chc.on).then_some("Channel tools are off");
    push(
        "Channel tools",
        Setting {
            key: Key::ChannelOn,
            label: "Channel tools".into(),
            help: "Mono, width, crossfeed and balance, after the EQ. Off bypasses all four at once, for a quick A/B.",
            control: Control::Toggle(chc.on),
            disabled: None,
        },
    );
    push(
        "Channel tools",
        Setting {
            key: Key::ChannelMono,
            label: "Mono".into(),
            help: "Both speakers play the same thing. For one earbud, or a speaker in another room.",
            control: Control::Toggle(chc.mono),
            disabled: ch_off,
        },
    );
    push(
        "Channel tools",
        Setting {
            key: Key::ChannelWidth,
            label: "Stereo width".into(),
            help: "0% is mono, 100% untouched, 200% twice as wide. Wider makes the sides louder.",
            control: Control::Number {
                value: chc.width as i64,
                min: 0,
                max: 200,
                step: 10,
                scale: 1,
                unit: "%",
            },
            disabled: ch_off,
        },
    );
    push(
        "Channel tools",
        Setting {
            key: Key::ChannelCrossfeed,
            label: "Crossfeed".into(),
            help: "For headphones: a little of each side's bass reaches the other ear, the way speakers sound. Takes the edge off hard-panned old stereo.",
            control: Control::Choice {
                options: crate::dsp::channel::Crossfeed::ALL
                    .iter()
                    .map(|c| (c.as_str().to_string(), c.label().to_string()))
                    .collect(),
                index: crate::dsp::channel::Crossfeed::ALL
                    .iter()
                    .position(|c| *c == chc.crossfeed)
                    .unwrap_or(0),
            },
            disabled: ch_off,
        },
    );
    push(
        "Channel tools",
        Setting {
            key: Key::ChannelBalance,
            label: "Balance".into(),
            help: "Negative leans left, positive right. The far side is turned down; nothing is turned up.",
            control: Control::Number {
                value: chc.balance as i64,
                min: -100,
                max: 100,
                step: 5,
                scale: 1,
                unit: "",
            },
            disabled: ch_off,
        },
    );

    // Output
    #[cfg(not(target_os = "android"))]
    let backends = [
        (Backend::Auto, "auto", "PipeWire, else ALSA"),
        (Backend::PipeWire, "pipewire", "PipeWire"),
        (Backend::Alsa, "alsa", "ALSA"),
        (Backend::Null, "null", "None (silent)"),
    ];
    // Android has one way out.
    #[cfg(target_os = "android")]
    let backends = [
        (Backend::Auto, "auto", "Android (AAudio)"),
        (Backend::Null, "null", "None (silent)"),
    ];
    push(
        "Output",
        Setting {
            disabled: None,
            key: Key::Backend,
            label: "Output".into(),
            help: "Where sound goes when it isn't playing bit-perfect.",
            control: Control::Choice {
                options: backends
                    .iter()
                    .map(|(_, v, l)| (v.to_string(), l.to_string()))
                    .collect(),
                index: backends
                    .iter()
                    .position(|(b, _, _)| *b == cfg.output.backend)
                    .unwrap_or(0),
            },
        },
    );
    let (options, index) = device_options(&outputs, cfg.output.device.as_deref());
    push(
        "Output",
        Setting {
            disabled: (cfg.output.backend == Backend::Null)
                .then_some("Output is set to none: there's no device to pick."),
            key: Key::Device,
            label: "Device".into(),
            help: "Where the sound goes. Also on the bottom bar: click the output there.",
            control: Control::Choice { options, index },
        },
    );
    push(
        "Output",
        Setting {
            disabled: None,
            key: Key::BufferMs,
            label: "Buffer".into(),
            help: "Output latency. Raise it if you hear dropouts.",
            control: Control::Number {
                value: cfg.output.buffer_ms as i64,
                min: 50,
                max: 1000,
                step: 50,
                scale: 1,
                unit: " ms",
            },
        },
    );

    let mut rates: Vec<(String, String)> = vec![("match".into(), "Match the source".into())];
    rates.extend(RATES.iter().map(|r| (r.to_string(), khz(*r))));
    if cfg.output.rate != 0 && !RATES.contains(&cfg.output.rate) {
        rates.push((cfg.output.rate.to_string(), khz(cfg.output.rate)));
    }
    let index = rates
        .iter()
        .position(|(v, _)| v.parse::<u32>().unwrap_or(0) == cfg.output.rate)
        .unwrap_or(0);
    push(
        "Output",
        Setting {
            disabled: None,
            key: Key::OutRate,
            label: "Sample rate".into(),
            help: "Match plays each album at its own rate. A fixed rate resamples everything to it. Bit-perfect ignores this.",
            control: Control::Choice {
                options: rates,
                index,
            },
        },
    );
    push(
        "Output",
        Setting {
            disabled: None,
            key: Key::Resampler,
            label: "Resampler".into(),
            help: "Fast skips the anti-aliasing filter. Balanced is a short sinc filter. Best is a long one, run by FFT.",
            control: Control::Choice {
                options: Quality::ALL
                    .iter()
                    .map(|q| (q.as_str().to_string(), q.label().to_string()))
                    .collect(),
                index: Quality::ALL
                    .iter()
                    .position(|q| *q == cfg.output.resampler)
                    .unwrap_or(1),
            },
        },
    );
    push(
        "Output",
        Setting {
            disabled: (cfg.output.backend == Backend::PipeWire)
                .then_some("PipeWire takes float and picks the device's format itself."),
            key: Key::OutFormat,
            label: "Sample format".into(),
            help: "What ALSA is sent. Auto takes the best the device accepts. PipeWire always takes float.",
            control: Control::Choice {
                options: SampleFormat::ALL
                    .iter()
                    .map(|f| (f.as_str().to_string(), f.label().to_string()))
                    .collect(),
                index: SampleFormat::ALL
                    .iter()
                    .position(|f| *f == cfg.output.format)
                    .unwrap_or(0),
            },
        },
    );
    push(
        "Output",
        Setting {
            disabled: None,
            key: Key::Dither,
            label: "Dither".into(),
            help: "TPDF dither when ALSA gets 16 or 24-bit. Trades rounding distortion for a hiss far below hearing.",
            control: Control::Toggle(cfg.output.dither),
        },
    );
    push(
        "Output",
        Setting {
            disabled: None,
            key: Key::Crossfade,
            label: "Crossfade".into(),
            help: "Fade each song into the next. Never between songs that follow on from each other on an album.",
            control: Control::Number {
                value: (cfg.output.crossfade_ms / 100) as i64,
                min: 0,
                max: 120,
                step: 5,
                scale: 10,
                unit: " s",
            },
        },
    );

    // Bit-perfect
    let modes = [
        (BitPerfect::Off, "Off"),
        (BitPerfect::On, "On, for local files"),
        (
            BitPerfect::Auto,
            "Auto, whenever nothing changes the sound (leveling off)",
        ),
    ];
    push(
        "Bit-perfect",
        Setting {
            disabled: None,
            key: Key::BitPerfect,
            label: "Bit-perfect".into(),
            help: if cfg!(target_os = "android") {
                "Local files go straight to a USB DAC, untouched. Needs Android 14 or later, and a DAC Android offers bit-perfect for."
            } else {
                "Local files go straight to the DAC, untouched. Takes the card for itself while playing."
            },
            control: Control::Choice {
                options: modes
                    .iter()
                    .map(|(m, l)| (m.as_str().to_string(), l.to_string()))
                    .collect(),
                index: modes
                    .iter()
                    .position(|(m, _)| *m == cfg.output.bit_perfect)
                    .unwrap_or(0),
            },
        },
    );
    let first = if cfg!(target_os = "android") {
        "First USB DAC"
    } else {
        "First sound card"
    };
    let mut opts: Vec<(String, String)> = vec![(String::new(), first.into())];
    opts.extend(devices.iter().map(|d| (d.id.clone(), d.name.clone())));
    if let Some(cur) = &cfg.output.bit_perfect_device
        && !devices.iter().any(|d| &d.id == cur)
    {
        opts.push((cur.clone(), format!("{cur} (not connected)")));
    }
    let index = cfg
        .output
        .bit_perfect_device
        .as_ref()
        .and_then(|cur| opts.iter().position(|o| &o.0 == cur))
        .unwrap_or(0);
    push(
        "Bit-perfect",
        Setting {
            disabled: None,
            key: Key::BitPerfectDevice,
            label: "Bit-perfect device".into(),
            help: "The sound card bit-perfect playback takes over.",
            control: Control::Choice {
                options: opts,
                index,
            },
        },
    );

    // Lyrics
    push(
        "Lyrics",
        Setting {
            disabled: None,
            key: Key::LyricsOnline,
            label: "Look lyrics up online".into(),
            help: "Ask LRCLIB (lrclib.net) when the song, its folder and its server have none. Only ever when you press Lyrics.",
            control: Control::Toggle(cfg.lyrics.online),
        },
    );

    // Podcasts
    push(
        "Podcasts",
        Setting {
            disabled: None,
            key: Key::PodcastSpeed,
            label: "Podcast speed".into(),
            help: "Speed with pitch kept. Podcasts only; music and radio never change speed.",
            control: Control::Number {
                value: (podcast_tempo * 10.0).round() as i64,
                min: 5,
                max: 30,
                step: 1,
                scale: 10,
                unit: "x",
            },
        },
    );
    push(
        "Podcasts",
        Setting {
            disabled: None,
            key: Key::PodcastTrim,
            label: "Trim silences".into(),
            help: "Cut pauses over 0.7 s down to a quarter second. Podcasts only.",
            control: Control::Toggle(podcast_trim),
        },
    );

    // Files
    let file = |label: &'static str, p: &PathBuf| Setting {
        disabled: None,
        key: Key::Info(label),
        label: label.into(),
        help: "",
        control: Control::Info(p.display().to_string()),
    };
    push("Files", file("Config file", &info.config_path));
    if !info.no_library {
        push("Files", file("Library index", &info.db_path));
        push("Files", file("Downloaded episodes", &info.offline_dir));
        push("Files", file("Song cache", &info.cache_dir));
        push("Files", file("Pinned songs", &info.pinned_dir));
    }
    // A flag on the command line wins over the file for this run.
    for (key, flag) in &info.overrides {
        for (_, s) in v.iter_mut().filter(|(_, s)| &s.key == key) {
            s.label = format!("{}  ({flag} this run)", s.label);
        }
    }
    v
}

/// The new value for a numeric or choice control after `change`, or None
/// if it doesn't apply.
pub fn stepped(control: &Control, change: &Change) -> Option<Control> {
    match (control, change) {
        (Control::Toggle(b), Change::Toggle | Change::Step(_)) => Some(Control::Toggle(!b)),
        (Control::Choice { options, index }, Change::Step(d)) => {
            let n = options.len() as i64;
            if n == 0 {
                return None;
            }
            let i = (*index as i64 + *d as i64).rem_euclid(n) as usize;
            Some(Control::Choice {
                options: options.clone(),
                index: i,
            })
        }
        (Control::Choice { options, .. }, Change::Choose(i)) if *i < options.len() => {
            Some(Control::Choice {
                options: options.clone(),
                index: *i,
            })
        }
        (
            Control::Number {
                value,
                min,
                max,
                step,
                scale,
                unit,
            },
            change,
        ) => {
            let v = match change {
                Change::Step(d) => value + step * *d as i64,
                Change::Set(t) => {
                    let t = t.trim().trim_end_matches(unit.trim()).trim();
                    let f: f64 = t.parse().ok()?;
                    (f * *scale as f64).round() as i64
                }
                _ => return None,
            };
            Some(Control::Number {
                value: v.clamp(*min, *max),
                min: *min,
                max: *max,
                step: *step,
                scale: *scale,
                unit,
            })
        }
        (Control::Text { empty, .. }, Change::Set(t)) => Some(Control::Text {
            value: t.trim().to_string(),
            empty,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(value: i64) -> Control {
        Control::Number {
            value,
            min: 5,
            max: 30,
            step: 1,
            scale: 10,
            unit: "x",
        }
    }

    #[test]
    fn command_line_flags_are_named_on_their_rows() {
        let info = SettingsInfo {
            overrides: vec![(Key::Backend, "--backend null".into())],
            ..Default::default()
        };
        let v = rows(
            &Config::default(),
            &info,
            1.0,
            false,
            &[],
            &[],
            Live::default(),
        );
        let label = |k: Key| {
            v.iter()
                .find(|(_, s)| s.key == k)
                .map(|(_, s)| s.label.clone())
                .unwrap()
        };
        assert!(label(Key::Backend).ends_with("(--backend null this run)"));
        assert!(!label(Key::BufferMs).contains("this run"));
    }

    #[test]
    fn numbers_step_clamp_and_parse() {
        assert_eq!(stepped(&num(10), &Change::Step(1)), Some(num(11)));
        assert_eq!(stepped(&num(30), &Change::Step(1)), Some(num(30)));
        assert_eq!(
            stepped(&num(10), &Change::Set("1.5x".into())),
            Some(num(15))
        );
        assert_eq!(stepped(&num(10), &Change::Set("nope".into())), None);
        assert_eq!(num(12).display(), "1.2x");
    }

    #[test]
    fn choices_wrap() {
        let c = Control::Choice {
            options: vec![("a".into(), "A".into()), ("b".into(), "B".into())],
            index: 1,
        };
        assert_eq!(stepped(&c, &Change::Step(1)).unwrap().display(), "A");
        assert_eq!(stepped(&c, &Change::Step(-1)).unwrap().display(), "A");
    }
}
