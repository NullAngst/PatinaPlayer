//! Last.fm from the browser's side: what to scrobble and who sends it,
//! linking, love, radio titles, and the Settings rows.
//!
//! For each song: local files are scrobbled by the app. A server song
//! follows that server's choice: the app scrobbles it, the server is told
//! (and passes it on, if it's set up to), or both. When Last.fm isn't
//! linked here the server is always told, since nothing can double up,
//! and that's how server play counts keep going up. Podcasts never
//! scrobble; radio only from song titles, and only if you turn it on.

use std::time::{Duration, Instant};

use super::Browser;
use super::settings::{Control, Key};
use crate::Event;
use crate::config::{self, ScrobbleMode};
use crate::lastfm::{self, Play, split_stream_title};
use crate::scrobble::{LastFmSetup, ScrobbleEvent};
use crate::track::{MediaKind, TrackInfo};

/// A radio title is scrobbled once it's been on this long.
pub const RADIO_AFTER: Duration = Duration::from_secs(60);

pub const BOTH_WARNING: &str = "If this server scrobbles to Last.fm too (Navidrome with its Last.fm agent does), every play will count twice. Only pick both if you know the server doesn't.";

/// The song a station is playing, for scrobbling it.
#[derive(Debug, Clone)]
pub struct RadioSong {
    pub artist: String,
    pub track: String,
    since: Instant,
    started: i64,
    sent: bool,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Open a page in the desktop's browser. Tests set PATINA_NO_BROWSER.
pub fn open_url(url: &str) -> bool {
    if std::env::var_os("PATINA_NO_BROWSER").is_some() {
        return false;
    }
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// The play Last.fm wants for a track, if it has an artist and a title.
pub fn play_of(info: &TrackInfo, started: i64) -> Option<Play> {
    let artist = info.artist.clone().filter(|a| !a.trim().is_empty())?;
    let track = info.title.clone().filter(|t| !t.trim().is_empty())?;
    Some(Play {
        artist,
        track,
        album: info.album.clone().filter(|a| !a.trim().is_empty()),
        album_artist: None,
        track_no: info
            .track_no
            .as_deref()
            .and_then(|t| t.split('/').next())
            .and_then(|n| n.trim().parse().ok()),
        duration: info.duration.map(|d| d.as_secs() as u32),
        timestamp: started,
    })
}

impl Browser {
    pub fn lastfm_user(&self) -> Option<String> {
        self.scrobble
            .as_ref()
            .map(|s| s.status().user)
            .filter(|u| !u.is_empty())
    }

    fn linked(&self) -> bool {
        self.lastfm_user().is_some()
    }

    /// Whether the app sends this song to Last.fm.
    fn app_scrobbles(&self, info: &TrackInfo) -> bool {
        if !self.config.lastfm.scrobble || !self.linked() {
            return false;
        }
        match &info.remote {
            None => true,
            Some(r) => matches!(
                self.config.lastfm.mode(&r.server),
                ScrobbleMode::App | ScrobbleMode::Both
            ),
        }
    }

    /// Whether the song's server is told it played.
    fn server_reports(&self, info: &TrackInfo) -> bool {
        let Some(r) = &info.remote else { return false };
        if r.episode {
            return false;
        }
        let app_live = self.config.lastfm.scrobble && self.linked();
        match self.config.lastfm.mode(&r.server) {
            ScrobbleMode::Server | ScrobbleMode::Both => true,
            ScrobbleMode::App => !app_live,
        }
    }

    fn report_to_server(&self, info: &TrackInfo, at: i64, submission: bool) {
        let Some(r) = info.remote.clone() else { return };
        std::thread::spawn(move || {
            let res = crate::remote::registry()
                .client(&r.server)
                .and_then(|c| c.report_play(&r.id, at, submission));
            if let Err(e) = res {
                log::info!("couldn't report a play to {}: {e}", r.server);
            }
        });
    }

    /// The player's events that matter here.
    /// Public so tests can hand it events the player would send.
    pub fn lastfm_events(&mut self, events: &[Event]) {
        for ev in events {
            match ev {
                Event::TrackChanged(Some(id)) => {
                    let info = self.player.queue().get(*id).map(|e| e.info.clone());
                    self.radio_song = None;
                    match info {
                        Some(i) => self.track_started(&i),
                        None => self.loved_now = None,
                    }
                }
                Event::TrackChanged(None) | Event::Finished => {
                    self.radio_song = None;
                    self.loved_now = None;
                }
                Event::Scrobble { info, started } => {
                    if self.app_scrobbles(info)
                        && let (Some(svc), Some(p)) = (&self.scrobble, play_of(info, *started))
                    {
                        svc.scrobble(p);
                    }
                    if self.server_reports(info) {
                        self.report_to_server(info, *started, true);
                    }
                }
                Event::NowPlaying(title) => self.radio_title(title),
                _ => {}
            }
        }
    }

    fn track_started(&mut self, info: &TrackInfo) {
        if info.kind != MediaKind::File {
            self.loved_now = None;
            return;
        }
        let now = unix_now();
        self.love_by_hand = false;
        if self.app_scrobbles(info)
            && let (Some(svc), Some(p)) = (&self.scrobble, play_of(info, now))
        {
            svc.now_playing(p);
        }
        if self.server_reports(info) {
            self.report_to_server(info, now, false);
        }
        self.loved_now = match (&info.artist, &info.title, &self.lib) {
            (Some(a), Some(t), Some(lib)) => {
                if let Some(svc) = self.scrobble.as_ref().filter(|_| self.linked()) {
                    svc.ask_loved(a, t);
                }
                Some(lib.loved(a, t).unwrap_or(false))
            }
            (Some(_), Some(_), None) => Some(false),
            _ => None,
        };
        self.touch();
    }

    /// A station's title changed: Now Playing for it, and it's scrobbled
    /// once it's been on a minute.
    fn radio_title(&mut self, title: &str) {
        self.radio_song = None;
        if !self.config.lastfm.scrobble_radio || !self.config.lastfm.scrobble {
            return;
        }
        let playing_radio = self
            .player
            .snapshot()
            .track
            .is_some_and(|t| t.kind == MediaKind::Radio);
        if !playing_radio {
            return;
        }
        let Some((artist, track)) = split_stream_title(title) else {
            return;
        };
        if let Some(svc) = self.scrobble.as_ref().filter(|_| self.linked()) {
            svc.now_playing(Play {
                artist: artist.clone(),
                track: track.clone(),
                ..Default::default()
            });
        }
        self.radio_song = Some(RadioSong {
            artist,
            track,
            since: Instant::now(),
            started: unix_now(),
            sent: false,
        });
    }

    /// Scrobble the station's song once it's been on long enough, and
    /// handle what the Last.fm thread reports.
    pub(super) fn poll_lastfm(&mut self) {
        let playing = self.player.snapshot().state == crate::PlayState::Playing;
        if let Some(r) = self.radio_song.as_mut()
            && !r.sent
            && playing
            && r.since.elapsed() >= RADIO_AFTER
        {
            r.sent = true;
            let p = Play {
                artist: r.artist.clone(),
                track: r.track.clone(),
                timestamp: r.started,
                ..Default::default()
            };
            if let Some(svc) = self.scrobble.as_ref().filter(|_| self.linked()) {
                svc.scrobble(p);
            }
        }
        let Some(svc) = &self.scrobble else { return };
        let events: Vec<ScrobbleEvent> = svc.events().try_iter().collect();
        for ev in events {
            match ev {
                ScrobbleEvent::Approve(url) => {
                    let opened = open_url(&url);
                    self.note(if opened {
                        format!("Approve PatinaPlayer on the Last.fm page that just opened. If it didn't: {url}")
                    } else {
                        format!("Approve PatinaPlayer on Last.fm: {url}")
                    });
                }
                ScrobbleEvent::Linked(user) => {
                    let path = self.config_path.clone();
                    let _ = config::set_value(&path, "lastfm", "user", user.as_str());
                    let _ = config::set_value(&path, "lastfm", "scrobble", true);
                    self.config.lastfm.user = user.clone();
                    self.config.lastfm.scrobble = true;
                    self.note(format!("Linked to Last.fm as {user}"));
                }
                ScrobbleEvent::LinkFailed(why) => self.error(format!("Last.fm: {why}")),
                ScrobbleEvent::Unlinked(why) => {
                    let _ = config::set_value(&self.config_path, "lastfm", "user", "");
                    self.config.lastfm.user.clear();
                    if why.is_empty() {
                        self.note("Unlinked from Last.fm");
                    } else {
                        self.error(why);
                    }
                }
                // An answer that crossed your own click is out of date.
                ScrobbleEvent::Loved { .. } if self.love_by_hand => {}
                ScrobbleEvent::Loved { artist, track, on } => {
                    if let Some(lib) = &self.lib {
                        let _ = lib.set_loved(&artist, &track, on);
                    }
                    self.refresh_section(super::Section::Playlists);
                    let cur = self.player.snapshot().track;
                    if cur.is_some_and(|t| {
                        t.artist
                            .as_deref()
                            .is_some_and(|a| a.eq_ignore_ascii_case(&artist))
                            && t.title
                                .as_deref()
                                .is_some_and(|x| x.eq_ignore_ascii_case(&track))
                    }) {
                        self.loved_now = Some(on);
                    }
                }
                ScrobbleEvent::Error(e) => self.error(e),
            }
            self.refresh_section(super::Section::Settings);
            self.touch();
        }
    }

    // ----- Love -----

    /// Love the song playing, or stop loving it.
    pub fn toggle_love(&mut self) {
        let Some(info) = self.player.snapshot().track else {
            return self.note("Nothing's playing to love");
        };
        if info.kind != MediaKind::File {
            return self.note("Only songs can be loved");
        }
        let (Some(artist), Some(title)) = (info.artist.clone(), info.title.clone()) else {
            return self.note("This song has no artist or title to love it by");
        };
        let on = !self.loved_now.unwrap_or(false);
        if self.set_love(&artist, &title, info.remote.clone(), on) {
            self.note(if on { "Loved" } else { "Not loved any more" });
        }
    }

    /// Heart or unheart a song anywhere it shows: the library (and so
    /// Hearted songs), Last.fm when linked, the server's star when that's
    /// on, and the bottom bar's heart when it's the song playing. False if
    /// the library wouldn't take it.
    pub(super) fn set_love(
        &mut self,
        artist: &str,
        title: &str,
        remote: Option<crate::remote::RemoteRef>,
        on: bool,
    ) -> bool {
        if let Some(lib) = &self.lib
            && let Err(e) = lib.set_loved(artist, title, on)
        {
            self.error(e.to_string());
            return false;
        }
        let playing = self.player.snapshot().track.is_some_and(|t| {
            t.artist
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(artist))
                && t.title
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(title))
        });
        if playing {
            self.loved_now = Some(on);
            self.love_by_hand = true;
        }
        if let Some(svc) = self.scrobble.as_ref().filter(|_| self.linked()) {
            svc.love(artist, title, on);
        }
        if self.config.lastfm.love_stars
            && let Some(r) = remote
        {
            std::thread::spawn(move || {
                let res = crate::remote::registry()
                    .client(&r.server)
                    .and_then(|c| c.star(&r.id, on));
                if let Err(e) = res {
                    log::info!("couldn't star on {}: {e}", r.server);
                }
            });
        }
        self.refresh_section(super::Section::Playlists);
        self.touch();
        true
    }

    /// In Hearted songs: removing a song unhearts it. Its order is the
    /// order you hearted in, so there's nothing to move.
    pub(super) fn edit_hearted(&mut self, remove: bool) {
        if !remove {
            return self.note("Hearted songs is in the order you hearted them");
        }
        let Some(super::Item::Track(t)) = self.selected_item() else {
            return;
        };
        let (Some(artist), title) = (t.artist.clone(), t.title.clone()) else {
            return self.note("This song has no artist to unheart it by");
        };
        let sel = self.level().map(|l| l.selected).unwrap_or(0);
        if self.set_love(&artist, &title, t.remote(), false) {
            self.note(format!("Unhearted {title}"));
            if let Some(l) = self.level_mut() {
                l.selected = l.snap(sel, -1);
            }
        }
    }

    // ----- Settings -----

    pub fn lastfm_link(&mut self) {
        let Some(svc) = &self.scrobble else {
            return self.note("Last.fm needs the library (for the scrobble queue)");
        };
        if self.linked() {
            svc.unlink();
        } else {
            svc.link();
            self.note("Asking Last.fm for a link…");
        }
    }

    fn lastfm_setup(&self) -> LastFmSetup {
        let c = &self.config.lastfm;
        LastFmSetup {
            key: lastfm::effective_key(&c.api_key),
            secret: lastfm::effective_secret(&c.api_key, &c.api_secret),
            user: c.user.clone(),
        }
    }

    pub(super) fn change_lastfm_setting(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        match (key, new) {
            (Key::LastFmKey, Control::Text { value, .. }) => {
                w(config::set_value(
                    &path,
                    "lastfm",
                    "api_key",
                    value.as_str(),
                ))?;
                self.config.lastfm.api_key = value.trim().to_string();
                if let Some(m) = &self.mixes {
                    m.set_lastfm_key(&lastfm::effective_key(value));
                }
            }
            (Key::LastFmSecret, Control::Text { value, .. }) => {
                w(config::set_value(
                    &path,
                    "lastfm",
                    "api_secret",
                    value.as_str(),
                ))?;
                self.config.lastfm.api_secret = value.trim().to_string();
            }
            (Key::Scrobble, Control::Toggle(on)) => {
                w(config::set_value(&path, "lastfm", "scrobble", *on))?;
                self.config.lastfm.scrobble = *on;
            }
            (Key::ScrobbleRadio, Control::Toggle(on)) => {
                w(config::set_value(&path, "lastfm", "scrobble_radio", *on))?;
                self.config.lastfm.scrobble_radio = *on;
            }
            (Key::LoveStars, Control::Toggle(on)) => {
                w(config::set_value(&path, "lastfm", "love_stars", *on))?;
                self.config.lastfm.love_stars = *on;
            }
            (Key::ServerScrobble(name), Control::Choice { options, index }) => {
                let m: ScrobbleMode = options[*index].0.parse().unwrap_or_default();
                w(config::set_scrobble_mode(&path, name, m))?;
                self.config.lastfm.servers.insert(name.clone(), m);
            }
            _ => return Ok(()),
        }
        if matches!(key, Key::LastFmKey | Key::LastFmSecret)
            && let Some(svc) = &self.scrobble
        {
            svc.setup(self.lastfm_setup());
        }
        Ok(())
    }
}
