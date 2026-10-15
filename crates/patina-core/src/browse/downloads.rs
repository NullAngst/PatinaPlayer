//! The browser's side of the cache: feeding prefetch from the queue,
//! pinning for offline, the Downloads view's buttons, the cache's
//! settings, and greying out what can't play while a server is away.

use std::collections::HashSet;

use super::{Browser, Item, JobRow, ListAction, Section, Source};
use crate::browse::settings::{Control, Key};
use crate::cache::{CacheEvent, LOOKAHEAD, PrefetchMode, SongKey};
use crate::config;
use crate::remote::ServerState;
use crate::track::TrackInfo;

impl Browser {
    /// Hand prefetch the songs coming up, if they changed.
    pub(super) fn feed_prefetch(&mut self) {
        let Some(c) = &self.cache else { return };
        // The current song leads the list: a queue that was just set
        // hasn't opened it yet, and prefetch fetching it first means the
        // player picks up that download instead of pausing another.
        let keys: Vec<SongKey> = {
            let q = self.player.queue();
            q.current()
                .into_iter()
                .chain(q.ahead(LOOKAHEAD))
                .filter_map(|id| q.get(id))
                .filter_map(|e| e.info.remote.as_ref().and_then(SongKey::of))
                .collect()
        };
        if keys != self.sent_upcoming {
            c.upcoming(keys.clone());
            self.sent_upcoming = keys;
        }
    }

    pub(super) fn poll_cache(&mut self) {
        let Some(c) = &self.cache else { return };
        let events: Vec<CacheEvent> = c.events().try_iter().collect();
        if events.is_empty() {
            return;
        }
        let mut changed = false;
        let mut episodes = false;
        for ev in events {
            match ev {
                CacheEvent::Changed => changed = true,
                CacheEvent::EpisodeDone(id) => {
                    episodes = true;
                    let title = self
                        .lib
                        .as_ref()
                        .and_then(|l| l.episode(id).ok().flatten())
                        .map(|e| e.title)
                        .unwrap_or_default();
                    self.note(format!("Downloaded \"{title}\""));
                }
                CacheEvent::Error(e) => self.error(e),
                CacheEvent::ServerTrouble(s) => {
                    if let Some(r) = &self.remote {
                        r.check(&s);
                    }
                }
            }
        }
        let c = self.cache.as_ref().expect("checked above");
        let st = c.status();
        // What's kept changed: songs may play offline that didn't.
        if st.cache_files != self.cache_status.cache_files
            || st.offline_files != self.cache_status.offline_files
        {
            self.avail_dirty = true;
        }
        self.cache_status = st;
        if changed || episodes {
            self.refresh_section(Section::Downloads);
            self.touch();
        }
        if episodes {
            self.refresh_section(Section::Podcasts);
            if let Some(Source::Podcast { .. }) = self
                .stacks
                .get(&Section::Podcasts)
                .and_then(|s| s.last())
                .map(|l| &l.source)
            {
                self.refresh_current();
            }
        }
    }

    // ----- Offline mode -----

    /// Work out again what greys out.
    pub(super) fn update_availability(&mut self) {
        let offline: HashSet<String> = self
            .servers
            .iter()
            .filter(|s| {
                matches!(
                    s.state,
                    ServerState::Offline(_) | ServerState::NoPassword | ServerState::BadLogin
                )
            })
            .map(|s| s.name.clone())
            .collect();
        let kept = self.cache.as_ref().map(|c| c.stored()).unwrap_or_default();
        let before = self.avail.any_offline();
        self.avail = match &self.lib {
            Some(lib) => super::offline::Availability::compute(lib, offline, &kept),
            None => Default::default(),
        };
        if before || self.avail.any_offline() {
            self.touch();
        }
    }

    /// Whether a row has anything that can play right now. False only
    /// while its server is unreachable and none of it is kept here.
    pub fn plays(&self, item: &Item) -> bool {
        self.avail.plays(item, self.prefs.artist_mode)
    }

    /// Queue entries that can play right now.
    pub(super) fn playable(&self, tracks: Vec<TrackInfo>) -> Vec<TrackInfo> {
        if !self.avail.any_offline() {
            return tracks;
        }
        tracks
            .into_iter()
            .filter(|t| self.avail.info_plays(t))
            .collect()
    }

    // ----- Pins -----

    /// Whether "Keep offline" makes sense for a row: it has (or may have)
    /// server songs in it. Artists and playlists can mix local and server
    /// songs, so they always offer it.
    pub fn can_pin(&self, item: &Item) -> bool {
        if self.cache.is_none() {
            return false;
        }
        match item {
            Item::Track(t) => t.remote_id.is_some(),
            Item::Album(a) => a.server().is_some(),
            Item::Dir { path, .. } | Item::Root { path, .. } => crate::remote::is_remote(path),
            Item::Artist(_) | Item::Playlist(_) | Item::Mix(_) => true,
            _ => false,
        }
    }

    /// The pin for an item, if it's pinned.
    pub fn pinned(&self, item: &Item) -> Option<i64> {
        let want = item.pin(self.prefs.artist_mode)?;
        self.cache_status
            .pins
            .iter()
            .find(|p| p.pin.kind == want.kind && p.pin.key == want.key)
            .map(|p| p.pin.id)
    }

    /// Pin an item for offline, or unpin it if it's pinned.
    pub fn toggle_pin(&mut self, item: &Item) {
        let Some(cache) = &self.cache else {
            return self.error("Offline needs the library");
        };
        let Some(pin) = item.pin(self.prefs.artist_mode) else {
            return self.note("That can't be kept offline");
        };
        if let Some(id) = self.pinned(item) {
            cache.unpin(id);
            self.cache_status.pins.retain(|p| p.pin.id != id);
            self.note(format!(
                "Unpinned {}. Its songs stay in the cache until it needs the room.",
                pin.name
            ));
        } else {
            let has_server_songs = match (&self.lib, item) {
                (_, Item::Pinned(_)) => true,
                (Some(lib), _) => super::tracks_of(lib, item, &self.prefs)
                    .iter()
                    .any(|t| t.remote_id.is_some()),
                (None, _) => false,
            };
            if !has_server_songs {
                return self.note("Those are local files, which play without a server anyway");
            }
            let name = pin.name.clone();
            cache.pin(pin);
            self.note(format!("Keeping {name} offline"));
        }
        self.refresh_section(Section::Downloads);
        self.touch();
    }

    pub fn toggle_pin_selected(&mut self) {
        if let Some(item) = self.selected_item() {
            self.toggle_pin(&item);
        }
    }

    /// The open view as an item, for pinning the whole thing: an album,
    /// an artist, a playlist or a folder.
    pub fn view_item(&self) -> Option<Item> {
        let lib = self.lib.as_ref()?;
        match &self.level()?.source {
            Source::Album { key } => lib.album(key).ok().flatten().map(Item::Album),
            Source::ArtistAlbums(name) => Some(Item::Artist(crate::library::ArtistRow {
                name: name.clone(),
                albums: 0,
                tracks: 0,
                plays: 0,
            })),
            Source::Playlist { id, .. } => lib
                .playlists()
                .ok()?
                .into_iter()
                .find(|p| p.id == *id)
                .map(Item::Playlist),
            Source::Folder(path) => Some(Item::Dir {
                path: path.clone(),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
            }),
            _ => None,
        }
    }

    /// Whether the open view can be pinned as a whole, and whether it is.
    pub fn view_pin_state(&self) -> Option<bool> {
        self.cache.as_ref()?;
        let item = self.view_item()?;
        let any_remote = match (&self.lib, &self.level()?.source) {
            (_, Source::Folder(p)) if crate::remote::is_remote(p) => true,
            (Some(lib), Source::Folder(p)) => lib
                .tracks_under(p)
                .map(|v| v.iter().any(|t| t.remote_id.is_some()))
                .unwrap_or(false),
            _ => {
                self.level()?
                    .items
                    .iter()
                    .any(|i| matches!(i, Item::Track(t) if t.remote_id.is_some()))
                    || matches!(&item, Item::Artist(_))
            }
        };
        any_remote.then(|| self.pinned(&item).is_some())
    }

    pub fn toggle_pin_view(&mut self) {
        if let Some(item) = self.view_item() {
            self.toggle_pin(&item);
        }
    }

    // ----- Downloads view -----

    pub fn list_action(&mut self, a: ListAction) {
        let Some(c) = &self.cache else { return };
        match a {
            ListAction::PauseDownloads => {
                c.pause(None, true);
                self.cache_status.paused = true;
                self.note("Downloads paused. The song playing still loads.");
            }
            ListAction::ResumeDownloads => {
                c.pause(None, false);
                self.cache_status.paused = false;
                self.note("Downloads resumed");
            }
            ListAction::ClearCache => {
                c.clear_cache();
                self.note("Cache cleared. Pinned songs stay.");
            }
        }
        self.refresh_section(Section::Downloads);
    }

    /// Pause one download, or let it go again.
    pub fn toggle_job_pause(&mut self, j: &JobRow) {
        let Some(c) = &self.cache else { return };
        if j.kind == crate::cache::JobKind::Play {
            return self.note("The song playing always loads");
        }
        c.pause(Some(j.key.clone()), !j.paused);
        self.note(if j.paused { "Resumed" } else { "Paused" });
    }

    /// Stop a download for good: an episode or a song coming up. A pinned
    /// song stays wanted while it's pinned, so that's unpinning.
    pub fn cancel_job(&mut self, j: &JobRow) {
        use crate::cache::{JobKey, JobKind};
        let Some(c) = &self.cache else { return };
        match (&j.key, j.kind) {
            (JobKey::Episode(id), _) => {
                c.cancel_episode(*id);
                self.note("Download cancelled");
            }
            (_, JobKind::Prefetch) => {
                c.cancel(j.key.clone());
                self.note("Skipped. It streams when it comes up.");
            }
            (_, JobKind::Play) => self.note("The song playing always loads"),
            (_, JobKind::Offline) => {
                self.note("That song is pinned. Unpin what it belongs to, or pause it.")
            }
            (_, JobKind::Episode) => {}
        }
    }

    pub fn unpin(&mut self, id: i64) {
        if let Some(c) = &self.cache {
            c.unpin(id);
            self.cache_status.pins.retain(|p| p.pin.id != id);
            self.note("Unpinned. Its songs stay in the cache until it needs the room.");
            self.refresh_section(Section::Downloads);
        }
    }

    // ----- Settings -----

    pub(super) fn change_cache_setting(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        let mut c = self.config.cache.clone();
        match (key, new) {
            (Key::CacheSize, Control::Number { value, scale, .. }) => {
                let mb = (*value as u64 * 1024).div_ceil(*scale as u64);
                if mb > 0 && c.prefetch == PrefetchMode::Size && mb < c.prefetch_mb {
                    self.error(format!(
                        "The cache can't be smaller than prefetch ({} MB). Lower prefetch first.",
                        c.prefetch_mb
                    ));
                    return Ok(());
                }
                w(config::set_value(&path, "cache", "size_mb", mb as i64))?;
                c.size_mb = mb;
            }
            (Key::Prefetch, Control::Choice { options, index }) => {
                let v = &options[*index].0;
                c.prefetch = v.parse().unwrap_or_default();
                if c.prefetch == PrefetchMode::Size && c.size_mb > 0 && c.prefetch_mb > c.size_mb {
                    c.prefetch_mb = c.size_mb;
                    w(config::set_value(
                        &path,
                        "cache",
                        "prefetch_mb",
                        c.prefetch_mb as i64,
                    ))?;
                }
                w(config::set_value(&path, "cache", "prefetch", v.as_str()))?;
            }
            (Key::PrefetchAmount, Control::Number { value, .. }) => match c.prefetch {
                PrefetchMode::Size => {
                    let mb = *value as u64;
                    if c.size_mb > 0 && mb > c.size_mb {
                        self.error(format!(
                            "Prefetch can't take more than the cache holds ({} MB). Make the cache bigger first.",
                            c.size_mb
                        ));
                        return Ok(());
                    }
                    w(config::set_value(&path, "cache", "prefetch_mb", mb as i64))?;
                    c.prefetch_mb = mb;
                }
                PrefetchMode::Tracks => {
                    w(config::set_value(&path, "cache", "prefetch_tracks", *value))?;
                    c.prefetch_tracks = *value as u32;
                }
            },
            (Key::PauseMetered, Control::Toggle(on)) => {
                w(config::set_value(&path, "cache", "pause_on_metered", *on))?;
                c.pause_on_metered = *on;
            }
            (Key::CacheFormat, Control::Choice { options, index }) => {
                let (f, kbps) = super::settings::parse_stream(&options[*index].0);
                w(config::set_value(&path, "cache", "format", f.as_str()))?;
                c.format = f;
                if let Some(k) = kbps {
                    w(config::set_value(&path, "cache", "bitrate", i64::from(k)))?;
                    c.bitrate = k;
                }
            }
            _ => return Ok(()),
        }
        self.config.cache = c.clone();
        if let Some(cache) = &self.cache {
            cache.set_config(c);
        }
        self.refresh_section(Section::Downloads);
        Ok(())
    }
}
