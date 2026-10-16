//! MPRIS2: the D-Bus interface desktops use for media keys, the media
//! widget in Plasma's panel, GNOME's notification controls and
//! `playerctl`.
//!
//! The service runs on its own connection to the session bus under
//! `org.mpris.MediaPlayer2.patinaplayer`. Calls from the desktop turn into
//! player commands directly. The frontend calls [`Mpris::update`] from its
//! own loop with the latest snapshot, and that sends the property-change
//! signals the desktop listens for. Raise and Quit need the frontend, so
//! they come back as [`Request`]s.
//!
//! If there's no session bus (a TTY, a minimal system), [`Mpris::start`]
//! returns an error and the app carries on without it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::player::{Command, PlayState, Player, Snapshot};
use crate::queue::Repeat;
use crate::track::{MediaKind, TrackInfo};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.patinaplayer";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// Track ids are object paths; this one means "no track".
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
/// Art handed to the desktop, in pixels.
const ART_SIZE: u32 = 512;

/// Things the desktop asked for that only the frontend can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Bring the window to the front (and out of the tray).
    Raise,
    Quit,
}

/// What the frontend can do, which decides what the desktop is offered.
#[derive(Debug, Clone, Copy)]
pub struct Abilities {
    /// There's a window to raise. False for the TUI.
    pub raise: bool,
}

/// The properties as last published. The D-Bus side reads these; only
/// [`Mpris::update`] writes them.
#[derive(Debug, Clone, Default, PartialEq)]
struct Published {
    status: &'static str,
    loop_status: &'static str,
    shuffle: bool,
    volume: f64,
    meta: Meta,
    can_play: bool,
    can_pause: bool,
    can_seek: bool,
    can_next: bool,
    can_prev: bool,
    /// Not signalled; the desktop asks for it when it wants it.
    position_us: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Meta {
    entry: Option<u64>,
    title: String,
    artists: Vec<String>,
    album: String,
    track_no: Option<i32>,
    length_us: Option<i64>,
    url: Option<String>,
    art: Option<String>,
}

impl Meta {
    fn track_id(&self) -> String {
        match self.entry {
            Some(id) => format!("/org/patinaplayer/track/{id}"),
            None => NO_TRACK.into(),
        }
    }

    fn to_map(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let mut put = |k: &str, v: Value<'_>| {
            if let Ok(v) = OwnedValue::try_from(v) {
                m.insert(k.to_string(), v);
            }
        };
        let id = self.track_id();
        put(
            "mpris:trackid",
            Value::from(
                ObjectPath::try_from(id.as_str())
                    .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked(NO_TRACK)),
            ),
        );
        if self.entry.is_none() {
            return m;
        }
        put("xesam:title", Value::from(self.title.clone()));
        if !self.artists.is_empty() {
            put("xesam:artist", Value::from(self.artists.clone()));
        }
        if !self.album.is_empty() {
            put("xesam:album", Value::from(self.album.clone()));
        }
        if let Some(n) = self.track_no {
            put("xesam:trackNumber", Value::from(n));
        }
        if let Some(l) = self.length_us {
            put("mpris:length", Value::from(l));
        }
        if let Some(u) = &self.url {
            put("xesam:url", Value::from(u.clone()));
        }
        if let Some(a) = &self.art {
            put("mpris:artUrl", Value::from(a.clone()));
        }
        m
    }
}

fn status(s: PlayState) -> &'static str {
    match s {
        PlayState::Playing => "Playing",
        PlayState::Paused => "Paused",
        PlayState::Stopped => "Stopped",
    }
}

fn loop_status(r: Repeat) -> &'static str {
    match r {
        Repeat::Off => "None",
        Repeat::One => "Track",
        Repeat::All => "Playlist",
    }
}

fn repeat_from(s: &str) -> Option<Repeat> {
    match s {
        "None" => Some(Repeat::Off),
        "Track" => Some(Repeat::One),
        "Playlist" => Some(Repeat::All),
        _ => None,
    }
}

/// How many presses of the repeat button go from `from` to `to`.
fn repeat_steps(from: Repeat, to: Repeat) -> usize {
    let mut r = from;
    for n in 0..3 {
        if r == to {
            return n;
        }
        r = r.cycle();
    }
    0
}

fn file_url(p: &Path) -> Option<String> {
    url::Url::from_file_path(p).ok().map(|u| u.to_string())
}

/// Metadata for a track, art left out (it's found separately, since that
/// can be slow).
fn meta_for(entry: u64, t: &TrackInfo, live_title: Option<&str>) -> Meta {
    let mut title = t.display_title();
    let mut artists: Vec<String> = t.artist.iter().cloned().collect();
    // Radio: the stream's "Artist - Title" is the song; the station is
    // the album, so a widget shows both.
    let mut album = t.album.clone().unwrap_or_default();
    if t.kind == MediaKind::Radio {
        album = title.clone();
        match live_title.filter(|s| !s.trim().is_empty()) {
            Some(lt) => match lt.split_once(" - ") {
                Some((a, s)) => {
                    artists = vec![a.trim().to_string()];
                    title = s.trim().to_string();
                }
                None => title = lt.trim().to_string(),
            },
            None => artists.clear(),
        }
    }
    let url = if t.path.as_os_str().is_empty() || crate::remote::is_remote(&t.path) {
        // Server songs: their URL carries a login, so don't hand it out.
        t.url.clone().filter(|_| t.kind != MediaKind::File)
    } else {
        file_url(&t.path)
    };
    Meta {
        entry: Some(entry),
        title,
        artists,
        album,
        track_no: t
            .track_no
            .as_deref()
            .and_then(|n| n.split('/').next())
            .and_then(|n| n.trim().parse().ok()),
        length_us: t.duration.map(|d| d.as_micros() as i64),
        url,
        art: None,
    }
}

struct Shared {
    published: Mutex<Published>,
    /// The latest snapshot, for setters that need the current state.
    snap: Mutex<Snapshot>,
}

struct Root {
    requests: Sender<Request>,
    abilities: Abilities,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        if self.abilities.raise {
            let _ = self.requests.send(Request::Raise);
        }
    }

    fn quit(&self) {
        let _ = self.requests.send(Request::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        self.abilities.raise
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "PatinaPlayer".into()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "patinaplayer".into()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        [
            "audio/flac",
            "audio/mpeg",
            "audio/ogg",
            "audio/opus",
            "audio/mp4",
            "audio/x-wav",
            "audio/x-aiff",
            "audio/x-ape",
            "audio/x-wavpack",
            "audio/x-dsf",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
}

struct PlayerIface {
    player: Player,
    shared: Arc<Shared>,
}

impl PlayerIface {
    fn published(&self) -> Published {
        self.shared.published.lock().unwrap().clone()
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn next(&self) {
        self.player.send(Command::Next);
    }

    fn previous(&self) {
        self.player.send(Command::Back);
    }

    fn pause(&self) {
        self.player.send(Command::Pause);
    }

    fn play_pause(&self) {
        self.player.send(Command::TogglePause);
    }

    fn stop(&self) {
        self.player.send(Command::Stop);
    }

    fn play(&self) {
        self.player.send(Command::Play);
    }

    /// Relative, in microseconds.
    fn seek(&self, offset: i64) {
        if self.published().can_seek {
            self.player.send(Command::SeekBy(offset as f64 / 1e6));
        }
    }

    /// Absolute, and only for the track named (so a late call doesn't seek
    /// the next song).
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let p = self.published();
        if !p.can_seek || track_id.as_str() != p.meta.track_id() || position < 0 {
            return;
        }
        if p.meta.length_us.is_some_and(|l| position > l) {
            return;
        }
        self.player
            .send(Command::Seek(Duration::from_micros(position as u64)));
    }

    fn open_uri(&self, uri: String) -> zbus::fdo::Result<()> {
        let path = url::Url::parse(&uri)
            .ok()
            .filter(|u| u.scheme() == "file")
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| zbus::fdo::Error::NotSupported("only file:// URIs".into()))?;
        self.player.send(Command::SetQueue {
            paths: vec![path],
            start: 0,
        });
        Ok(())
    }

    #[zbus(signal)]
    async fn seeked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        position: i64,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.published().status.into()
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.published().loop_status.into()
    }

    #[zbus(property)]
    fn set_loop_status(&self, value: String) -> zbus::fdo::Result<()> {
        let to = repeat_from(&value)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no loop status {value}")))?;
        let from = self.shared.snap.lock().unwrap().repeat;
        for _ in 0..repeat_steps(from, to) {
            self.player.send(Command::CycleRepeat);
        }
        Ok(())
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn set_rate(&self, _value: f64) {}

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.published().shuffle
    }

    #[zbus(property)]
    fn set_shuffle(&self, value: bool) {
        if self.shared.snap.lock().unwrap().shuffle != value {
            self.player.send(Command::ToggleShuffle);
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.published().meta.to_map()
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.published().volume
    }

    #[zbus(property)]
    fn set_volume(&self, value: f64) {
        if !self.shared.snap.lock().unwrap().volume_locked {
            self.player.send(Command::SetVolume(value.clamp(0.0, 1.0)));
        }
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.published().position_us
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.published().can_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.published().can_prev
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.published().can_play
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.published().can_pause
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.published().can_seek
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

/// The running service. Dropping it leaves the bus.
pub struct Mpris {
    conn: Connection,
    shared: Arc<Shared>,
    /// Art lookups run off the frontend's thread; results come back here
    /// keyed by queue entry.
    art_tx: Sender<(u64, Option<String>)>,
    art_rx: Receiver<(u64, Option<String>)>,
    art_for: Option<u64>,
    art: Option<String>,
    /// Where playback should be by now if nobody seeked, to spot seeks.
    expect: Option<(u64, Duration, std::time::Instant)>,
    /// The playing track, kept through the moments a seek or reopen
    /// leaves the snapshot without one, so the desktop doesn't flicker.
    last_track: Option<(u64, TrackInfo)>,
}

impl Mpris {
    /// Connect to the session bus and take the name. The queue is read
    /// through `player` for the can-go-next/previous flags.
    pub fn start(
        player: Player,
        abilities: Abilities,
    ) -> Result<(Self, Receiver<Request>), String> {
        let conn = Connection::session().map_err(|e| e.to_string())?;
        Self::start_on(conn, BUS_NAME, player, abilities)
    }

    /// [`Mpris::start`] on a given connection and name. Split out for tests.
    pub fn start_on(
        conn: Connection,
        name: &str,
        player: Player,
        abilities: Abilities,
    ) -> Result<(Self, Receiver<Request>), String> {
        let (req_tx, req_rx) = unbounded();
        let shared = Arc::new(Shared {
            published: Mutex::new(Published {
                status: "Stopped",
                loop_status: "None",
                volume: 1.0,
                ..Default::default()
            }),
            snap: Mutex::new(Snapshot::default()),
        });
        let e = |e: zbus::Error| e.to_string();
        conn.object_server()
            .at(
                PATH,
                Root {
                    requests: req_tx,
                    abilities,
                },
            )
            .map_err(e)?;
        conn.object_server()
            .at(
                PATH,
                PlayerIface {
                    player,
                    shared: shared.clone(),
                },
            )
            .map_err(e)?;
        // A second copy of the app gets a unique suffix, as the spec asks,
        // instead of failing.
        if conn.request_name(name).is_err() {
            let alt = format!("{name}.instance{}", std::process::id());
            conn.request_name(alt.as_str()).map_err(e)?;
        }
        let (art_tx, art_rx) = unbounded();
        Ok((
            Self {
                conn,
                shared,
                art_tx,
                art_rx,
                art_for: None,
                art: None,
                expect: None,
                last_track: None,
            },
            req_rx,
        ))
    }

    /// Publish the player's state. Cheap when nothing changed; call it from
    /// the frontend's loop, a few times a second or more.
    pub fn update(&mut self, snap: &Snapshot, queue_len: usize, has_next: bool) {
        let track = match (snap.current, &snap.track) {
            (Some(id), Some(t)) => {
                self.last_track = Some((id, t.clone()));
                Some(t.clone())
            }
            (Some(id), None) => self
                .last_track
                .as_ref()
                .filter(|(lid, _)| *lid == id)
                .map(|(_, t)| t.clone()),
            (None, _) => None,
        };
        // Art for a new track: look for it once, in the background.
        let entry = snap.current.filter(|_| track.is_some());
        if entry != self.art_for {
            self.art_for = entry;
            self.art = None;
            if let (Some(id), Some(t)) = (entry, &track)
                && !t.path.as_os_str().is_empty()
            {
                let tx = self.art_tx.clone();
                let path = t.path.clone();
                std::thread::Builder::new()
                    .name("mpris-art".into())
                    .spawn(move || {
                        let url = crate::art::cached_file(&path, ART_SIZE)
                            .and_then(|p: PathBuf| file_url(&p));
                        let _ = tx.send((id, url));
                    })
                    .ok();
            }
        }
        while let Ok((id, url)) = self.art_rx.try_recv() {
            if Some(id) == self.art_for {
                self.art = url;
            }
        }

        let mut meta = match (entry, &track) {
            (Some(id), Some(t)) => meta_for(id, t, snap.live_title.as_deref()),
            _ => Meta::default(),
        };
        meta.art = self.art.clone();
        let has_track = entry.is_some();
        let next = Published {
            status: status(snap.state),
            loop_status: loop_status(snap.repeat),
            shuffle: snap.shuffle,
            volume: if snap.muted { 0.0 } else { snap.volume },
            meta,
            can_play: queue_len > 0,
            can_pause: has_track,
            can_seek: has_track && !snap.live && snap.duration.is_some(),
            can_next: has_next,
            can_prev: has_track,
            position_us: snap.position.as_micros() as i64,
        };

        *self.shared.snap.lock().unwrap() = snap.clone();
        let prev = std::mem::replace(&mut *self.shared.published.lock().unwrap(), next.clone());

        let mut changed: HashMap<&str, Value<'_>> = HashMap::new();
        if prev.status != next.status {
            changed.insert("PlaybackStatus", Value::from(next.status));
        }
        if prev.loop_status != next.loop_status {
            changed.insert("LoopStatus", Value::from(next.loop_status));
        }
        if prev.shuffle != next.shuffle {
            changed.insert("Shuffle", Value::from(next.shuffle));
        }
        if (prev.volume - next.volume).abs() > 1e-6 {
            changed.insert("Volume", Value::from(next.volume));
        }
        if prev.meta != next.meta {
            let map: HashMap<String, Value<'_>> = next
                .meta
                .to_map()
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect();
            changed.insert("Metadata", Value::from(map));
        }
        for (name, a, b) in [
            ("CanPlay", prev.can_play, next.can_play),
            ("CanPause", prev.can_pause, next.can_pause),
            ("CanSeek", prev.can_seek, next.can_seek),
            ("CanGoNext", prev.can_next, next.can_next),
            ("CanGoPrevious", prev.can_prev, next.can_prev),
        ] {
            if a != b {
                changed.insert(name, Value::from(b));
            }
        }
        if !changed.is_empty() {
            let invalidated: Vec<&str> = Vec::new();
            let _ = self.conn.emit_signal(
                None::<&str>,
                PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(PLAYER_IFACE, changed, invalidated),
            );
        }

        // Seeked: the position moved other than by playing.
        let now = std::time::Instant::now();
        if let Some(id) = entry {
            let jumped = match self.expect {
                Some((eid, pos, at)) if eid == id => {
                    let played = if prev.status == "Playing" {
                        now - at
                    } else {
                        Duration::ZERO
                    };
                    let want = pos + played;
                    let got = snap.position;
                    let off = got.abs_diff(want);
                    off > Duration::from_millis(1500)
                }
                _ => false,
            };
            if jumped {
                let _ = self.conn.emit_signal(
                    None::<&str>,
                    PATH,
                    PLAYER_IFACE,
                    "Seeked",
                    &(next.position_us,),
                );
            }
            self.expect = Some((id, snap.position, now));
        } else {
            self.expect = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_round_trip() {
        for r in [Repeat::Off, Repeat::All, Repeat::One] {
            assert_eq!(repeat_from(loop_status(r)), Some(r));
        }
        assert_eq!(repeat_steps(Repeat::Off, Repeat::Off), 0);
        assert_eq!(repeat_steps(Repeat::Off, Repeat::All), 1);
        assert_eq!(repeat_steps(Repeat::Off, Repeat::One), 2);
        assert_eq!(repeat_steps(Repeat::One, Repeat::Off), 1);
        assert_eq!(repeat_steps(Repeat::One, Repeat::All), 2);
    }

    #[test]
    fn radio_titles_split() {
        let t = TrackInfo {
            kind: MediaKind::Radio,
            title: Some("Radio Paradise".into()),
            url: Some("https://stream.example/rp".into()),
            ..Default::default()
        };
        let m = meta_for(3, &t, Some("Miles Davis - So What"));
        assert_eq!(m.title, "So What");
        assert_eq!(m.artists, vec!["Miles Davis".to_string()]);
        assert_eq!(m.album, "Radio Paradise");
        assert_eq!(m.url.as_deref(), Some("https://stream.example/rp"));
        let m = meta_for(3, &t, None);
        assert_eq!(m.title, "Radio Paradise");
        assert!(m.artists.is_empty());
    }

    #[test]
    fn track_numbers_and_ids() {
        let t = TrackInfo {
            path: "/music/a/01.flac".into(),
            title: Some("One".into()),
            artist: Some("A".into()),
            track_no: Some("4/12".into()),
            duration: Some(Duration::from_secs(61)),
            ..Default::default()
        };
        let m = meta_for(9, &t, None);
        assert_eq!(m.track_no, Some(4));
        assert_eq!(m.length_us, Some(61_000_000));
        assert_eq!(m.track_id(), "/org/patinaplayer/track/9");
        assert_eq!(m.url.as_deref(), Some("file:///music/a/01.flac"));
        let map = m.to_map();
        assert!(map.contains_key("mpris:trackid"));
        assert!(map.contains_key("xesam:artist"));
        assert_eq!(Meta::default().to_map().len(), 1);
    }
}
