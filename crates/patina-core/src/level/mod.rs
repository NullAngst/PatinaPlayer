//! Volume leveling: ReplayGain 2 done properly, with a fallback for songs
//! that have no tags.
//!
//! Where a song's gain comes from, best first: the server's replayGain
//! field, ReplayGain tags in the file, then our own EBU R128 measurement.
//! Measurements are stored in the database by song, so each song is
//! measured once. A song that has none of these plays at the fallback gain
//! (or under the live normalizer, if that's on) and is right the next time.
//!
//! The gain is worked out when the decoder opens a track and applied to
//! the decoded samples before they go into the ring, so it switches at
//! exactly the sample where one track becomes the next. That's what keeps
//! gapless albums gapless.

pub mod live;
pub mod lookup;
pub mod measure;
pub mod service;

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::audio::AtomicF64;
use crate::track::TrackInfo;

pub use live::LiveNorm;
pub use service::{LevelEvent, LevelService};

/// The ReplayGain 2 reference loudness. Every gain in tags, on servers and
/// in our own table is relative to this.
pub const REFERENCE_LUFS: f64 = -18.0;
/// Target loudness range the setting allows.
pub const TARGET_MIN: f64 = -23.0;
pub const TARGET_MAX: f64 = -10.0;
/// Gain for a song nothing is known about yet.
pub const DEFAULT_FALLBACK_DB: f64 = -6.0;
/// The fallback setting's range.
pub const FALLBACK_MIN: f64 = -12.0;
pub const FALLBACK_MAX: f64 = 0.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LevelMode {
    /// No gain at all, not even for radio.
    Off,
    /// Every song on its own.
    Track,
    /// The whole album shares one gain, so quiet intros stay quiet.
    Album,
    /// Album gain while an album plays in order, track gain on shuffle,
    /// in a mix, or in a playlist that jumps between albums.
    #[default]
    Smart,
}

impl LevelMode {
    pub const ALL: [LevelMode; 4] = [
        LevelMode::Off,
        LevelMode::Track,
        LevelMode::Album,
        LevelMode::Smart,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LevelMode::Off => "off",
            LevelMode::Track => "track",
            LevelMode::Album => "album",
            LevelMode::Smart => "smart",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LevelMode::Off => "Off",
            LevelMode::Track => "Track",
            LevelMode::Album => "Album",
            LevelMode::Smart => "Smart (album in order, else track)",
        }
    }

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => LevelMode::Off,
            1 => LevelMode::Track,
            2 => LevelMode::Album,
            _ => LevelMode::Smart,
        }
    }
}

impl std::str::FromStr for LevelMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        LevelMode::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("unknown leveling mode \"{s}\" (off, track, album, smart)"))
    }
}

/// The `[leveling]` section of config.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelConfig {
    pub mode: LevelMode,
    /// LUFS, -23 to -10.
    pub target: f64,
    /// dB, for songs nothing is known about yet.
    pub fallback: f64,
    /// Cap the gain so the song's peak can't go over full scale.
    pub clip_guard: bool,
    /// The slow live normalizer for music that isn't measured yet.
    pub live_music: bool,
    /// The live normalizer for internet radio and podcasts.
    pub live_streams: bool,
}

impl Default for LevelConfig {
    fn default() -> Self {
        Self {
            mode: LevelMode::Smart,
            target: REFERENCE_LUFS,
            fallback: DEFAULT_FALLBACK_DB,
            clip_guard: true,
            live_music: false,
            live_streams: true,
        }
    }
}

/// The leveling settings as the decoder thread reads them. Atomics only,
/// like everything else in [`crate::audio::Shared`].
#[derive(Debug)]
pub struct LevelShared {
    mode: AtomicU8,
    target: AtomicF64,
    fallback: AtomicF64,
    clip_guard: AtomicBool,
    live_music: AtomicBool,
    live_streams: AtomicBool,
    /// Bumped on every change, so the decoder works the playing song's
    /// gain out again.
    pub generation: AtomicU64,
    /// A mix is playing: Smart treats it like shuffle.
    pub mixing: AtomicBool,
    /// The live normalizer's gain right now, in dB, for the bottom bar.
    pub live_db: AtomicF64,
}

impl Default for LevelShared {
    fn default() -> Self {
        let s = Self {
            mode: AtomicU8::new(0),
            target: AtomicF64::new(0.0),
            fallback: AtomicF64::new(0.0),
            clip_guard: AtomicBool::new(true),
            live_music: AtomicBool::new(false),
            live_streams: AtomicBool::new(true),
            generation: AtomicU64::new(0),
            mixing: AtomicBool::new(false),
            live_db: AtomicF64::new(0.0),
        };
        s.set(&LevelConfig::default());
        s
    }
}

impl LevelShared {
    pub fn set(&self, c: &LevelConfig) {
        self.mode.store(c.mode.to_u8(), Ordering::Relaxed);
        self.target.store(c.target.clamp(TARGET_MIN, TARGET_MAX));
        self.fallback
            .store(c.fallback.clamp(FALLBACK_MIN, FALLBACK_MAX));
        self.clip_guard.store(c.clip_guard, Ordering::Relaxed);
        self.live_music.store(c.live_music, Ordering::Relaxed);
        self.live_streams.store(c.live_streams, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn get(&self) -> LevelConfig {
        LevelConfig {
            mode: LevelMode::from_u8(self.mode.load(Ordering::Relaxed)),
            target: self.target.load(),
            fallback: self.fallback.load(),
            clip_guard: self.clip_guard.load(Ordering::Relaxed),
            live_music: self.live_music.load(Ordering::Relaxed),
            live_streams: self.live_streams.load(Ordering::Relaxed),
        }
    }

    pub fn mode(&self) -> LevelMode {
        LevelMode::from_u8(self.mode.load(Ordering::Relaxed))
    }
}

/// What's known about a song's loudness, as ReplayGain values (dB relative
/// to -18 LUFS, peaks as linear sample values where 1.0 is full scale).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gains {
    pub track_gain: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_gain: Option<f64>,
    pub album_peak: Option<f64>,
    /// Where the track gain came from.
    pub track_from: Option<Origin>,
    /// Where the album gain came from.
    pub album_from: Option<Origin>,
}

/// Where a gain came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The server's replayGain field.
    Server,
    /// ReplayGain tags in the file.
    Tags,
    /// Our own EBU R128 measurement.
    Measured,
}

/// Which gain is playing, for the bottom bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// Leveling is off.
    Off,
    /// Off because the track plays bit-perfect.
    BitPerfect,
    Track(Origin),
    Album(Origin),
    /// Nothing known yet: the fallback gain.
    Fallback,
    /// The live normalizer, which starts at the fallback gain.
    Live,
}

impl Source {
    /// A word or two for after the gain: "-4.2 dB album".
    pub fn label(self) -> &'static str {
        match self {
            Source::Off => "off",
            Source::BitPerfect => "off (bit-perfect)",
            Source::Track(_) => "track",
            Source::Album(_) => "album",
            Source::Fallback => "not measured yet",
            Source::Live => "live",
        }
    }

    /// Where it came from, for a tooltip.
    pub fn detail(self) -> &'static str {
        match self {
            Source::Off => "Leveling is off",
            Source::BitPerfect => "Bit-perfect playback skips leveling",
            Source::Track(Origin::Server) | Source::Album(Origin::Server) => {
                "ReplayGain from the server"
            }
            Source::Track(Origin::Tags) | Source::Album(Origin::Tags) => {
                "ReplayGain tags in the file"
            }
            Source::Track(Origin::Measured) | Source::Album(Origin::Measured) => {
                "Measured here (EBU R128)"
            }
            Source::Fallback => "Not measured yet: the fallback gain, right on the next play",
            Source::Live => "The live normalizer, following the stream as it plays",
        }
    }
}

/// The gain a track plays at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// dB, after the target and the clip guard.
    pub gain_db: f64,
    pub source: Source,
    /// Run the live normalizer, starting from `gain_db`.
    pub live: bool,
}

impl Level {
    pub const OFF: Level = Level {
        gain_db: 0.0,
        source: Source::Off,
        live: false,
    };

    /// Linear gain to multiply samples by.
    pub fn linear(&self) -> f64 {
        db_to_lin(self.gain_db)
    }
}

pub fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

pub fn lin_to_db(lin: f64) -> f64 {
    20.0 * lin.max(1e-12).log10()
}

/// Everything that decides a track's gain.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    /// The song is a live stream or a podcast episode.
    pub stream: bool,
    /// Smart mode: the album is playing in order.
    pub album_in_order: bool,
    /// Plays through the bit-perfect output.
    pub bit_perfect: bool,
}

/// Work out the gain for a track. Pure, so every rule is testable.
pub fn resolve(cfg: &LevelConfig, gains: &Gains, ctx: Context) -> Level {
    if cfg.mode == LevelMode::Off {
        return Level::OFF;
    }
    if ctx.bit_perfect {
        return Level {
            source: Source::BitPerfect,
            ..Level::OFF
        };
    }
    let fallback = cfg.fallback.clamp(FALLBACK_MIN, FALLBACK_MAX);
    if ctx.stream {
        return Level {
            gain_db: fallback,
            source: if cfg.live_streams {
                Source::Live
            } else {
                Source::Fallback
            },
            live: cfg.live_streams,
        };
    }
    let album = match cfg.mode {
        LevelMode::Album => true,
        LevelMode::Smart => ctx.album_in_order,
        _ => false,
    };
    let track = gains.track_gain.map(|g| {
        (
            g,
            gains.track_peak,
            Source::Track(gains.track_from.unwrap_or(Origin::Tags)),
        )
    });
    let album_g = gains.album_gain.map(|g| {
        (
            g,
            gains.album_peak,
            Source::Album(gains.album_from.unwrap_or(Origin::Tags)),
        )
    });
    // Album mode on a song with no album gain falls back to its track
    // gain, and the other way round: a known gain beats the fallback.
    let pick = if album {
        album_g.or(track)
    } else {
        track.or(album_g)
    };
    let Some((g, peak, source)) = pick else {
        return Level {
            gain_db: fallback,
            source: if cfg.live_music {
                Source::Live
            } else {
                Source::Fallback
            },
            live: cfg.live_music,
        };
    };
    let target = cfg.target.clamp(TARGET_MIN, TARGET_MAX);
    let mut gain = g + (target - REFERENCE_LUFS);
    if cfg.clip_guard
        && let Some(p) = peak.filter(|p| *p > 0.0)
    {
        gain = gain.min(-lin_to_db(p));
    }
    Level {
        gain_db: gain,
        source,
        live: false,
    }
}

/// One song's part in an album's loudness.
#[derive(Debug, Clone, Copy)]
pub struct Part {
    pub gain: f64,
    pub peak: Option<f64>,
    pub seconds: f64,
}

/// An album gain from its songs' track gains: their loudness averaged by
/// energy, weighted by length. That's the gated R128 album figure without
/// the gating across songs, which moves it a few tenths of a dB at most on
/// real albums. None unless every song is known.
pub fn album_from_parts(parts: &[Option<Part>]) -> Option<(f64, Option<f64>)> {
    if parts.is_empty() || parts.iter().any(Option::is_none) {
        return None;
    }
    let parts: Vec<Part> = parts.iter().flatten().copied().collect();
    let total: f64 = parts.iter().map(|p| p.seconds.max(1.0)).sum();
    let energy: f64 = parts
        .iter()
        .map(|p| p.seconds.max(1.0) * 10f64.powf((REFERENCE_LUFS - p.gain) / 10.0))
        .sum();
    let lufs = 10.0 * (energy / total).log10();
    let peak = parts
        .iter()
        .map(|p| p.peak)
        .try_fold(0.0f64, |m, p| p.map(|p| m.max(p)));
    Some((REFERENCE_LUFS - lufs, peak))
}

/// The leading number of a track number tag: "3/12" is 3.
fn track_number(t: &TrackInfo) -> Option<u32> {
    let s = t.track_no.as_deref()?.trim();
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s[..end].parse().ok()
}

fn same_album(a: &TrackInfo, b: &TrackInfo) -> bool {
    match (&a.album, &b.album) {
        (Some(x), Some(y)) => !x.trim().is_empty() && x.trim().eq_ignore_ascii_case(y.trim()),
        _ => false,
    }
}

/// Whether `b` is the next song on `a`'s album: same album, and the track
/// number one up (or not known). Crossfade stays out of such joins, and a
/// rate change there is resampled rather than left as a gap.
pub fn follows_on(a: &TrackInfo, b: &TrackInfo) -> bool {
    same_album(a, b)
        && match (track_number(a), track_number(b)) {
            (Some(x), Some(y)) => y == x + 1,
            _ => true,
        }
}

/// Smart mode's question: is this song part of an album playing in order?
/// It is when a neighbour in the queue is the same album and the track
/// numbers run on (or the numbers are missing). The first song of a
/// second disc counts through the song after it. Shuffle and mixes say no
/// before this is asked.
pub fn album_in_order(prev: Option<&TrackInfo>, cur: &TrackInfo, next: Option<&TrackInfo>) -> bool {
    let follows = |a: &TrackInfo, b: &TrackInfo| match (track_number(a), track_number(b)) {
        (Some(x), Some(y)) => y == x + 1,
        _ => true,
    };
    prev.is_some_and(|p| same_album(p, cur) && follows(p, cur))
        || next.is_some_and(|x| same_album(cur, x) && follows(cur, x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LevelConfig {
        LevelConfig::default()
    }

    fn ctx() -> Context {
        Context {
            stream: false,
            album_in_order: false,
            bit_perfect: false,
        }
    }

    fn tagged() -> Gains {
        Gains {
            track_gain: Some(-4.0),
            track_peak: Some(0.5),
            album_gain: Some(-6.0),
            album_peak: Some(0.9),
            track_from: Some(Origin::Tags),
            album_from: Some(Origin::Tags),
        }
    }

    #[test]
    fn smart_picks_album_only_in_order() {
        let l = resolve(&cfg(), &tagged(), ctx());
        assert_eq!(l.source, Source::Track(Origin::Tags));
        assert!((l.gain_db + 4.0).abs() < 1e-9);
        let l = resolve(
            &cfg(),
            &tagged(),
            Context {
                album_in_order: true,
                ..ctx()
            },
        );
        assert_eq!(l.source, Source::Album(Origin::Tags));
        assert!((l.gain_db + 6.0).abs() < 1e-9);
    }

    #[test]
    fn target_shifts_and_peak_caps() {
        let mut c = cfg();
        c.target = -10.0;
        // +8 dB louder target: -4 + 8 = +4, but a 0.5 peak allows +6.02,
        // so it stands. Album: -6 + 8 = +2, peak 0.9 allows 0.92: capped.
        let l = resolve(&c, &tagged(), ctx());
        assert!((l.gain_db - 4.0).abs() < 1e-9);
        c.mode = LevelMode::Album;
        let l = resolve(&c, &tagged(), ctx());
        assert!((l.gain_db - (-lin_to_db(0.9))).abs() < 1e-9, "{l:?}");
        c.clip_guard = false;
        let l = resolve(&c, &tagged(), ctx());
        assert!((l.gain_db - 2.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_songs_get_the_fallback_or_live() {
        let l = resolve(&cfg(), &Gains::default(), ctx());
        assert_eq!(l.source, Source::Fallback);
        assert_eq!(l.gain_db, -6.0);
        assert!(!l.live);
        let mut c = cfg();
        c.live_music = true;
        let l = resolve(&c, &Gains::default(), ctx());
        assert!(l.live);
        // Radio: live by default.
        let l = resolve(
            &cfg(),
            &Gains::default(),
            Context {
                stream: true,
                ..ctx()
            },
        );
        assert_eq!(l.source, Source::Live);
    }

    #[test]
    fn off_and_bit_perfect_do_nothing() {
        let mut c = cfg();
        c.mode = LevelMode::Off;
        assert_eq!(resolve(&c, &tagged(), ctx()), Level::OFF);
        let l = resolve(
            &cfg(),
            &tagged(),
            Context {
                bit_perfect: true,
                ..ctx()
            },
        );
        assert_eq!(l.gain_db, 0.0);
        assert_eq!(l.source, Source::BitPerfect);
    }

    #[test]
    fn album_mode_without_album_gain_uses_track() {
        let mut c = cfg();
        c.mode = LevelMode::Album;
        let g = Gains {
            album_gain: None,
            ..tagged()
        };
        assert_eq!(resolve(&c, &g, ctx()).source, Source::Track(Origin::Tags));
    }

    #[test]
    fn album_gain_from_parts() {
        // Two equally long songs at the same loudness: the album is that.
        let p = |g| {
            Some(Part {
                gain: g,
                peak: Some(0.5),
                seconds: 200.0,
            })
        };
        let (g, peak) = album_from_parts(&[p(-5.0), p(-5.0)]).unwrap();
        assert!((g + 5.0).abs() < 1e-9);
        assert_eq!(peak, Some(0.5));
        // A loud song and a quiet one: the loud one dominates by energy.
        let (g, _) = album_from_parts(&[p(-10.0), p(0.0)]).unwrap();
        assert!(g < -5.0 && g > -10.0, "{g}");
        // One song unknown: no album gain.
        assert!(album_from_parts(&[p(-5.0), None]).is_none());
    }

    #[test]
    fn in_order_needs_a_running_neighbour() {
        let t = |album: &str, n: &str| TrackInfo {
            album: Some(album.into()),
            track_no: Some(n.into()),
            ..Default::default()
        };
        let (a1, a2, a3) = (t("A", "1/9"), t("A", "2/9"), t("A", "3"));
        let b5 = t("B", "5");
        assert!(album_in_order(Some(&a1), &a2, Some(&a3)));
        assert!(album_in_order(None, &a1, Some(&a2)));
        assert!(!album_in_order(Some(&b5), &a2, None));
        // Same album, but jumping around: a playlist, not the album.
        assert!(!album_in_order(Some(&a3), &a1, Some(&a3)));
    }

    #[test]
    fn mode_parses() {
        assert_eq!("Album".parse::<LevelMode>().unwrap(), LevelMode::Album);
        assert!("loud".parse::<LevelMode>().is_err());
    }
}
