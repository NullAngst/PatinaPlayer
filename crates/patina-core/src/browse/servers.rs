//! The Browser's side of music servers: adding and removing them, their
//! playlists, and keeping the play queue saved on the server.

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(super) use crate::remote::service::Job;

use super::{Browser, Section};
use crate::config;
use crate::library::PlaylistRow;
use crate::remote::dlna::Found;
use crate::remote::{self, RemoteEvent, ServerConfig, ServerKind, ServerState, StreamFormat};
use crate::secrets::Stored;
use crate::{Command, PlayState, TrackInfo};

/// How long the queue has to sit still before it's saved to the server.
const QUEUE_SETTLE: Duration = Duration::from_secs(3);
/// While playing, the position is saved this often.
const QUEUE_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct QueueSync {
    version: u64,
    dirty_at: Option<Instant>,
    sent_at: Option<Instant>,
    was_playing: bool,
    bookmark_at: Option<Instant>,
}

/// How long "Find DLNA servers" listens for answers.
const DLNA_LISTEN: Duration = Duration::from_secs(3);

/// A "Find DLNA servers" in progress, and what it found.
#[derive(Default)]
pub(super) struct DlnaSearch {
    rx: Option<crossbeam_channel::Receiver<Vec<Found>>>,
    found: Option<Vec<Found>>,
}

/// The queue as one server can keep it: (server, song ids, current id).
type ServerQueue = (String, Vec<String>, Option<String>);

impl Browser {
    /// Handle what the server threads reported. True when the index
    /// changed and the views should be rebuilt.
    pub(super) fn poll_servers(&mut self) -> bool {
        let Some(remote) = &self.remote else {
            return false;
        };
        let events: Vec<RemoteEvent> = remote.events().try_iter().collect();
        let mut status = false;
        for ev in events {
            match ev {
                RemoteEvent::Changed => {
                    self.servers_changed.get_or_insert_with(Instant::now);
                }
                RemoteEvent::Status => status = true,
                RemoteEvent::Error(e) => self.error(e),
                RemoteEvent::Done(m) => self.note(m),
                RemoteEvent::AddFailed(e) => self.error(e),
                RemoteEvent::Added {
                    cfg,
                    info,
                    stored,
                    replaces,
                } => self.server_added(cfg, &info.product, stored, replaces),
                RemoteEvent::QueueLoaded { server, queue } => {
                    self.queue_loaded(&server, queue);
                }
            }
        }
        if status {
            self.servers = self
                .remote
                .as_ref()
                .map(|r| r.statuses())
                .unwrap_or_default();
            // A server that went away or came back changes what greys out.
            self.avail_dirty = true;
            // Stations and status lines show in Radio and Settings.
            self.refresh_section(Section::Radio);
            self.refresh_section(Section::Settings);
            self.rebuild_sidebar();
        }
        self.dlna_tick();
        self.queue_tick();
        // A big sync reports after every few albums. Redraw at most twice
        // a second, so the views don't rebuild hundreds of times.
        match self.servers_changed {
            Some(t) if t.elapsed() >= Duration::from_millis(500) => {
                self.servers_changed = None;
                true
            }
            _ => false,
        }
    }

    // ----- DLNA -----

    /// Look for DLNA servers on the network, in the background. What
    /// answers comes back through [`take_dlna_found`](Self::take_dlna_found).
    pub fn find_dlna(&mut self) {
        if self.dlna.rx.is_some() {
            return;
        }
        if self.remote.is_none() {
            return self.error("Servers need the library");
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let probe = self.dlna_probe;
        std::thread::spawn(move || {
            let found = match probe {
                Some(addr) => remote::dlna::discover_at(addr, DLNA_LISTEN),
                None => remote::dlna::discover(DLNA_LISTEN),
            };
            let _ = tx.send(found);
        });
        self.dlna.rx = Some(rx);
        self.note("Looking for DLNA servers on your network…");
    }

    /// Whether a search is still listening.
    pub fn finding_dlna(&self) -> bool {
        self.dlna.rx.is_some()
    }

    fn dlna_tick(&mut self) {
        let Some(rx) = &self.dlna.rx else { return };
        let Ok(found) = rx.try_recv() else { return };
        self.dlna.rx = None;
        let known: Vec<&str> = self
            .config
            .servers
            .iter()
            .filter(|s| s.kind == ServerKind::Dlna)
            .map(|s| s.udn.as_str())
            .collect();
        let total = found.len();
        let new: Vec<Found> = found
            .into_iter()
            .filter(|f| !known.contains(&f.udn.as_str()))
            .collect();
        if new.is_empty() {
            self.note(if total == 0 {
                "No DLNA servers answered. Check the server is on and on the same network."
                    .to_string()
            } else {
                format!(
                    "Found {total} DLNA server{}, already added",
                    if total == 1 { "" } else { "s" }
                )
            });
        } else {
            self.dlna.found = Some(new);
        }
        self.touch();
    }

    /// The servers the last search found that aren't added yet, once. The
    /// apps show these as a list to pick from.
    pub fn take_dlna_found(&mut self) -> Option<Vec<Found>> {
        self.dlna.found.take()
    }

    /// Add a DLNA server that a search found, named after itself.
    pub fn add_dlna_server(&mut self, f: &Found) {
        if !f.udn.is_empty()
            && self
                .config
                .servers
                .iter()
                .any(|s| s.kind == ServerKind::Dlna && s.udn == f.udn)
        {
            return self.note(format!("{} is already added", f.name));
        }
        let Some(remote) = &self.remote else {
            return self.error("Servers need the library");
        };
        let base: String = f
            .name
            .chars()
            .map(|c| if c == '/' || c == '#' { '-' } else { c })
            .collect::<String>()
            .trim()
            .to_string();
        let base = if base.is_empty() {
            "DLNA".to_string()
        } else {
            base
        };
        let mut name = base.clone();
        let mut n = 2;
        while self.config.servers.iter().any(|s| s.name == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        remote.add(
            ServerConfig {
                name,
                url: f.location.clone(),
                kind: ServerKind::Dlna,
                bitrate: 192,
                udn: f.udn.clone(),
                ..Default::default()
            },
            String::new(),
            None,
        );
        self.note(format!("Connecting to {}…", f.name));
    }

    // ----- Adding, editing and removing servers -----

    /// Check a name for a new server (or a renamed one).
    fn check_server_name(&self, name: &str, except: Option<&str>) -> Result<(), String> {
        if name.is_empty() {
            return Err("A server needs a name".into());
        }
        if name.contains('/') || name.contains('#') {
            return Err("A server name can't have / or # in it".into());
        }
        if self
            .config
            .servers
            .iter()
            .any(|s| s.name == name && Some(s.name.as_str()) != except)
        {
            return Err(format!("There's already a server called \"{name}\""));
        }
        Ok(())
    }

    /// Log in to a new server. It's saved once the login works; the
    /// answer comes back through [`poll`](Self::poll). An empty user means
    /// `secret` is an API key.
    pub fn add_server(&mut self, name: &str, url: &str, user: &str, secret: &str) {
        let name = name.trim();
        if let Err(e) = self.check_server_name(name, None) {
            return self.error(e);
        }
        if url.trim().is_empty() || secret.is_empty() {
            return self.error("A server needs its address and a password (or an API key)");
        }
        let Some(remote) = &self.remote else {
            return self.error("Servers need the library");
        };
        remote.add(
            ServerConfig {
                name: name.into(),
                url: url.trim().into(),
                user: user.trim().into(),
                kind: Default::default(),
                stream: StreamFormat::Raw,
                bitrate: 192,
                udn: String::new(),
            },
            secret.to_string(),
            None,
        );
        self.note(format!("Logging in to {}…", url.trim()));
    }

    /// Change a server's name, address, user or password. With no
    /// password given, the saved one is used.
    pub fn edit_server(
        &mut self,
        old: &str,
        name: &str,
        url: &str,
        user: &str,
        secret: Option<&str>,
    ) {
        let name = name.trim();
        if let Err(e) = self.check_server_name(name, Some(old)) {
            return self.error(e);
        }
        let Some(cur) = self.config.servers.iter().find(|s| s.name == old).cloned() else {
            return;
        };
        let Some(remote) = &self.remote else { return };
        if !cur.kind.logs_in() {
            // Nothing to change but the name; the address comes from the
            // network.
            remote.add(
                ServerConfig {
                    name: name.into(),
                    ..cur.clone()
                },
                String::new(),
                Some(old.to_string()),
            );
            return self.note(format!("Renaming {old}…"));
        }
        let secret = match secret.filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => match remote.secrets().get(&cur.secret_key(), &cur.user) {
                Ok(Some(s)) => s,
                _ => return self.error("Type the password too; there's none saved for it"),
            },
        };
        remote.add(
            ServerConfig {
                name: name.into(),
                url: url.trim().into(),
                user: user.trim().into(),
                ..cur
            },
            secret,
            Some(old.to_string()),
        );
        self.note(format!("Logging in to {}…", url.trim()));
    }

    fn server_added(
        &mut self,
        cfg: ServerConfig,
        product: &str,
        stored: Stored,
        replaces: Option<String>,
    ) {
        let mut list = self.config.servers.clone();
        let old = replaces
            .as_ref()
            .and_then(|o| list.iter().position(|s| &s.name == o));
        match old {
            Some(i) => {
                let prev = list[i].clone();
                list[i] = cfg.clone();
                // A password for an address and user nothing uses any more.
                if prev.kind.logs_in()
                    && (prev.secret_key(), &prev.user) != (cfg.secret_key(), &cfg.user)
                    && let Some(r) = &self.remote
                {
                    let _ = r.secrets().delete(&prev.secret_key(), &prev.user);
                }
            }
            None => list.push(cfg.clone()),
        }
        if let Err(e) = config::set_servers(&self.config_path, &list) {
            return self.error(format!("couldn't save the server: {e}"));
        }
        self.config.servers = list;
        if let Some(c) = &self.cache {
            // Kept songs and pins follow a renamed server, before the old
            // name is forgotten.
            if let Some(old) = replaces.as_ref().filter(|o| **o != cfg.name) {
                c.rename_server(old, &cfg.name);
            }
            c.set_servers(self.config.servers.iter().map(|s| s.name.clone()).collect());
        }
        if let Some(r) = &self.remote {
            r.set_servers(self.config.servers.clone());
        }
        let product = if product.is_empty() {
            "the server"
        } else {
            product
        };
        self.note(format!(
            "Connected to {} ({product}). Syncing its library now.",
            cfg.name
        ));
        if let Stored::File(p) = stored {
            self.note(format!(
                "No keyring answered, so the password is in {} (only you can read it)",
                p.display()
            ));
        }
        self.rebuild_sidebar();
        self.refresh_section(Section::Settings);
    }

    /// Forget a server: its songs leave the index, its password the keyring.
    pub fn remove_server(&mut self, name: &str) {
        let Some(cur) = self.config.servers.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        let list: Vec<ServerConfig> = self
            .config
            .servers
            .iter()
            .filter(|s| s.name != name)
            .cloned()
            .collect();
        if let Err(e) = config::set_servers(&self.config_path, &list) {
            return self.error(format!("couldn't save: {e}"));
        }
        self.config.servers = list;
        if let Some(c) = &self.cache {
            // Its kept songs go with it.
            c.set_servers(self.config.servers.iter().map(|s| s.name.clone()).collect());
        }
        if let Some(r) = &self.remote {
            r.set_servers(self.config.servers.clone());
            let shared = self
                .config
                .servers
                .iter()
                .any(|s| s.secret_key() == cur.secret_key() && s.user == cur.user);
            if cur.kind.logs_in()
                && !shared
                && let Err(e) = r.secrets().delete(&cur.secret_key(), &cur.user)
            {
                self.error(format!("couldn't remove the saved password: {e}"));
            }
        }
        self.servers.retain(|s| s.name != name);
        self.note(format!("Removed {name}"));
        self.rebuild_sidebar();
        self.refresh_section(Section::Settings);
    }

    /// Read a server's library again now (or every server's).
    pub fn sync_server(&mut self, name: Option<&str>) {
        if let Some(r) = &self.remote {
            r.sync(name);
            self.note(match name {
                Some(n) => format!("Syncing {n}"),
                None => "Syncing every server".into(),
            });
        }
    }

    /// The server config by name.
    pub fn server(&self, name: &str) -> Option<&ServerConfig> {
        self.config.servers.iter().find(|s| s.name == name)
    }

    /// Servers that are connected now.
    pub fn connected_servers(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|s| matches!(s.state, ServerState::Ready | ServerState::Syncing { .. }))
            .map(|s| s.name.clone())
            .collect()
    }

    // ----- Server playlists -----

    pub(super) fn playlist_row(&self, id: i64) -> Option<PlaylistRow> {
        self.lib
            .as_ref()?
            .playlists()
            .ok()?
            .into_iter()
            .find(|p| p.id == id)
    }

    /// Song ids on `server` for these paths, and how many weren't from it.
    fn server_ids(&self, server: &str, paths: &[PathBuf]) -> (Vec<String>, usize) {
        let Some(lib) = &self.lib else {
            return (Vec::new(), paths.len());
        };
        let mut ids = Vec::new();
        let mut other = 0;
        for p in paths {
            match lib
                .track_by_path(p)
                .ok()
                .flatten()
                .and_then(|t| t.remote())
                .filter(|r| r.server == server)
            {
                Some(r) => ids.push(r.id),
                None => other += 1,
            }
        }
        (ids, other)
    }

    /// For a server playlist: refuse if it isn't ours to change, else
    /// send the job. True when the local change should go ahead.
    pub(super) fn server_playlist_job(&mut self, id: i64, job: impl FnOnce(String) -> Job) -> bool {
        let Some(p) = self.playlist_row(id) else {
            return true;
        };
        let (Some(server), Some(rid)) = (p.server.clone(), p.remote_id.clone()) else {
            return true;
        };
        if p.readonly {
            self.error(format!(
                "\"{}\" belongs to someone else on {server}",
                p.name
            ));
            return false;
        }
        if let Some(r) = &self.remote {
            r.job(&server, job(rid));
        }
        true
    }

    /// After a local edit of a server playlist, send its new song list.
    pub(super) fn push_server_playlist(&mut self, id: i64, uris: &[Vec<u8>]) {
        let Some(p) = self.playlist_row(id) else {
            return;
        };
        let (Some(server), Some(rid)) = (p.server, p.remote_id) else {
            return;
        };
        let songs: Vec<String> = uris
            .iter()
            .filter_map(|u| {
                std::str::from_utf8(u)
                    .ok()
                    .and_then(remote::parse_episode_uri)
            })
            .filter(|r| r.server == server)
            .map(|r| r.id)
            .collect();
        if let Some(r) = &self.remote {
            r.job(&server, Job::ReplacePlaylist { id: rid, songs });
        }
    }

    pub(super) fn add_to_server_playlist(&mut self, p: &PlaylistRow, paths: &[PathBuf]) {
        let Some(server) = p.server.clone() else {
            return;
        };
        if p.readonly {
            return self.error(format!(
                "\"{}\" belongs to someone else on {server}",
                p.name
            ));
        }
        let (ids, other) = self.server_ids(&server, paths);
        if ids.is_empty() {
            return self.note(format!("Only songs from {server} can go in \"{}\"", p.name));
        }
        let uris: Vec<Vec<u8>> = ids
            .iter()
            .map(|i| crate::library::song_uri(&server, i))
            .collect();
        let Some(lib) = self.lib.as_mut() else { return };
        let added = match lib.append_uris(p.id, &uris) {
            Ok(n) => n,
            Err(e) => return self.error(e.to_string()),
        };
        let all: Vec<Vec<u8>> = lib
            .playlist_items(p.id)
            .map(|v| v.into_iter().map(|(u, _)| u).collect())
            .unwrap_or_default();
        let mut msg = super::added_note(added, ids.len(), &p.name);
        if other > 0 {
            msg.push_str(&format!(", and {other} not from {server} stayed out"));
        }
        self.note(msg);
        self.push_server_playlist(p.id, &all);
        self.refresh_section(Section::Playlists);
    }

    /// A new playlist on a server, from these songs. Songs from elsewhere
    /// are left out.
    pub fn create_server_playlist(&mut self, server: &str, name: &str, paths: &[PathBuf]) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let (songs, other) = self.server_ids(server, paths);
        let Some(r) = &self.remote else { return };
        r.job(
            server,
            Job::CreatePlaylist {
                name: name.into(),
                songs,
            },
        );
        if other > 0 {
            self.note(format!(
                "{other} not from {server} stayed out of \"{name}\""
            ));
        }
    }

    // ----- The play queue on the server -----

    /// Servers that keep a play queue, connected now.
    pub fn queue_servers(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|s| {
                matches!(s.state, ServerState::Ready | ServerState::Syncing { .. })
                    && s.info.as_ref().is_some_and(|i| i.play_queue)
            })
            .map(|s| s.name.clone())
            .collect()
    }

    /// Replace the queue with the one saved on `server`.
    pub fn load_server_queue(&mut self, server: &str) {
        if let Some(r) = &self.remote {
            r.job(server, Job::LoadQueue);
            self.note(format!("Asking {server} for its queue"));
        }
    }

    fn queue_loaded(&mut self, server: &str, q: crate::remote::SavedQueue) {
        let Some(lib) = &self.lib else { return };
        let rows = lib.tracks_by_remote(server, &q.ids).unwrap_or_default();
        let mut tracks: Vec<TrackInfo> = Vec::new();
        let mut start = 0;
        let mut missing = 0;
        for (id, row) in q.ids.iter().zip(rows) {
            match row {
                Some(t) => {
                    if q.current.as_deref() == Some(id.as_str()) {
                        start = tracks.len();
                    }
                    tracks.push(t.to_info());
                }
                None => missing += 1,
            }
        }
        if tracks.is_empty() {
            return self.note(format!("None of {server}'s queue is synced here yet"));
        }
        let n = tracks.len();
        self.player.send(Command::SetQueueTracks {
            tracks,
            start,
            pos: q.position,
        });
        let by = q
            .changed_by
            .filter(|c| !c.is_empty())
            .map(|c| format!(", last saved by {c}"))
            .unwrap_or_default();
        let mut msg = format!("Loaded {} from {server}{by}", super::count(n));
        if missing > 0 {
            msg.push_str(&format!(" ({missing} not synced yet)"));
        }
        self.note(msg);
    }

    /// The queue as one server can keep it, if every entry is a song from
    /// that server and it keeps queues.
    fn server_queue(&self) -> Option<ServerQueue> {
        let q = self.player.queue();
        let mut server: Option<String> = None;
        let mut ids = Vec::new();
        for id in q.play_order() {
            let r = q.get(*id)?.info.remote.clone()?;
            if r.episode || server.as_ref().is_some_and(|s| *s != r.server) {
                return None;
            }
            server = Some(r.server);
            ids.push(r.id);
        }
        let server = server?;
        if !self.queue_servers().contains(&server) {
            return None;
        }
        let snap = self.player.snapshot();
        let current = snap
            .current
            .and_then(|c| q.get(c))
            .and_then(|e| e.info.remote.as_ref())
            .map(|r| r.id.clone());
        Some((server, ids, current))
    }

    /// Save the queue to its server when it changes, a few seconds after
    /// it settles, and every half minute while playing so the position is
    /// close. Server podcast positions go as bookmarks the same way.
    fn queue_tick(&mut self) {
        if self.remote.is_none() {
            return;
        }
        let version = self.player.queue().version;
        let snap = self.player.snapshot();
        let playing = snap.state == PlayState::Playing;
        let qs = &mut self.queue_sync;
        if version != qs.version {
            qs.version = version;
            qs.dirty_at.get_or_insert_with(Instant::now);
        }
        if playing != qs.was_playing {
            qs.was_playing = playing;
            qs.dirty_at.get_or_insert_with(Instant::now);
        }
        if playing && qs.sent_at.is_none_or(|t| t.elapsed() >= QUEUE_EVERY) {
            qs.dirty_at.get_or_insert_with(Instant::now);
        }
        // Server podcast episodes: their spot goes up as a bookmark.
        if let Some(r) = snap.track.as_ref().and_then(|t| t.remote.as_ref())
            && r.episode
            && playing
            && qs.bookmark_at.is_none_or(|t| t.elapsed() >= QUEUE_EVERY)
        {
            qs.bookmark_at = Some(Instant::now());
            if let Some(remote) = &self.remote {
                remote.job(
                    &r.server,
                    Job::Bookmark {
                        id: r.id.clone(),
                        pos: snap.position,
                    },
                );
            }
        }
        let due = self
            .queue_sync
            .dirty_at
            .is_some_and(|t| t.elapsed() >= QUEUE_SETTLE);
        if !due {
            return;
        }
        self.queue_sync.dirty_at = None;
        self.queue_sync.sent_at = Some(Instant::now());
        if let Some((server, ids, current)) = self.server_queue()
            && let Some(remote) = &self.remote
        {
            remote.job(
                &server,
                Job::SaveQueue {
                    ids,
                    current,
                    pos: snap.position,
                },
            );
        }
    }

    /// Save the queue to its server now, waiting for the answer. Both
    /// frontends call this on the way out.
    pub fn before_quit(&mut self) {
        if let Some((server, ids, current)) = self.server_queue()
            && let Ok(c) = remote::registry().client(&server)
        {
            let pos = self.player.snapshot().position;
            if let Err(e) = c.save_queue(&ids, current.as_deref(), pos) {
                log::warn!("couldn't save the queue to {server}: {e}");
            }
        }
    }
}
