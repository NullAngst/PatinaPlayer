//! What can play while a server is unreachable: its songs that are kept
//! on disk (cached or pinned), and anything local or on another server.
//! Rows that can't play grey out instead of vanishing, so the library
//! still looks like your library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cache::{SongKey, Tier};
use crate::library::{ArtistMode, Library, TrackRow};
use crate::remote;
use crate::track::TrackInfo;

use super::Item;

/// What the views grey out. Empty (everything plays) while every server
/// is reachable.
#[derive(Debug, Default, Clone)]
pub struct Availability {
    /// Servers that can't be reached right now.
    offline: HashSet<String>,
    kept: HashSet<SongKey>,
    dead_albums: HashSet<String>,
    /// Album artists, lowercased, with nothing that can play.
    dead_album_artists: HashSet<String>,
    dead_track_artists: HashSet<String>,
    dead_genres: HashSet<String>,
    dead_playlists: HashSet<i64>,
    /// Folders on an unreachable server that hold a kept song.
    kept_dirs: HashSet<PathBuf>,
}

impl Availability {
    /// Work it out for these unreachable servers and these kept songs.
    pub fn compute(lib: &Library, offline: HashSet<String>, kept: &HashMap<SongKey, Tier>) -> Self {
        let mut a = Self {
            offline,
            ..Default::default()
        };
        if a.offline.is_empty() {
            return a;
        }
        a.kept = kept
            .keys()
            .filter(|k| a.offline.contains(&k.server))
            .cloned()
            .collect();
        if let Err(e) = a.fill(lib) {
            log::warn!("can't work out what plays offline: {e}");
        }
        a
    }

    pub fn any_offline(&self) -> bool {
        !self.offline.is_empty()
    }

    pub fn server_offline(&self, name: &str) -> bool {
        self.offline.contains(name)
    }

    fn fill(&mut self, lib: &Library) -> rusqlite::Result<()> {
        let c = &lib.conn;
        c.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS avail_off (id INTEGER PRIMARY KEY);
             CREATE TEMP TABLE IF NOT EXISTS avail_kept (
                 source_id INTEGER NOT NULL, remote_id TEXT NOT NULL,
                 PRIMARY KEY (source_id, remote_id));
             DELETE FROM avail_off; DELETE FROM avail_kept;",
        )?;
        let mut source_of: HashMap<String, i64> = HashMap::new();
        for name in &self.offline {
            if let Ok(Some(id)) = lib.source_id(&remote::root(name)) {
                source_of.insert(name.clone(), id);
                c.execute("INSERT OR IGNORE INTO avail_off (id) VALUES (?1)", [id])?;
            }
        }
        {
            let mut ins = c.prepare(
                "INSERT OR IGNORE INTO avail_kept (source_id, remote_id) VALUES (?1, ?2)",
            )?;
            for k in &self.kept {
                if let Some(id) = source_of.get(&k.server) {
                    ins.execute(rusqlite::params![id, k.id])?;
                }
            }
        }
        // A track plays if it's local, on a reachable server, or kept.
        const PLAYS: &str = "(t.source_id NOT IN (SELECT id FROM avail_off) OR EXISTS \
            (SELECT 1 FROM avail_kept k WHERE k.source_id = t.source_id \
             AND k.remote_id = t.remote_id))";
        let strings = |sql: &str| -> rusqlite::Result<HashSet<String>> {
            let mut st = c.prepare(sql)?;
            let v = st
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<String>>>()?;
            Ok(v)
        };
        self.dead_albums = strings(&format!(
            "SELECT t.album_key FROM tracks t WHERE t.album_key IN \
             (SELECT album_key FROM tracks WHERE source_id IN (SELECT id FROM avail_off)) \
             GROUP BY t.album_key HAVING SUM({PLAYS}) = 0"
        ))?;
        self.dead_track_artists = strings(&format!(
            "SELECT LOWER(t.artist) FROM tracks t WHERE t.artist IS NOT NULL AND \
             LOWER(t.artist) IN (SELECT LOWER(artist) FROM tracks \
               WHERE source_id IN (SELECT id FROM avail_off) AND artist IS NOT NULL) \
             GROUP BY LOWER(t.artist) HAVING SUM({PLAYS}) = 0"
        ))?;
        self.dead_genres = strings(&format!(
            "SELECT g.genre_norm FROM track_genres g JOIN tracks t ON t.id = g.track_id \
             WHERE g.genre_norm IN (SELECT genre_norm FROM track_genres x JOIN tracks y \
               ON y.id = x.track_id WHERE y.source_id IN (SELECT id FROM avail_off)) \
             GROUP BY g.genre_norm HAVING SUM({PLAYS}) = 0"
        ))?;
        // Album artists come from the albums table: dead when every album
        // under the name is.
        {
            let mut st = c.prepare("SELECT key, artist FROM albums")?;
            let mut alive: HashSet<String> = HashSet::new();
            let mut seen: HashSet<String> = HashSet::new();
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (key, artist) = r?;
                let a = artist.to_lowercase();
                if !self.dead_albums.contains(&key) {
                    alive.insert(a.clone());
                }
                seen.insert(a);
            }
            self.dead_album_artists = seen.difference(&alive).cloned().collect();
        }
        // Playlists: dead when no item can play. Items are file: URIs
        // (local, always fine) or server://name#id.
        {
            let mut st = c.prepare("SELECT playlist_id, uri FROM playlist_items")?;
            let mut alive: HashSet<i64> = HashSet::new();
            let mut seen: HashSet<i64> = HashSet::new();
            for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
                let (id, uri) = r?;
                seen.insert(id);
                let plays = match std::str::from_utf8(&uri)
                    .ok()
                    .and_then(remote::parse_episode_uri)
                {
                    Some(r) => {
                        !self.offline.contains(&r.server)
                            || self.kept.contains(&SongKey::new(r.server, r.id))
                    }
                    None => true,
                };
                if plays {
                    alive.insert(id);
                }
            }
            self.dead_playlists = seen.difference(&alive).cloned().collect();
        }
        // Folders holding kept songs, all the way up to the server's root.
        let mut by_server: HashMap<&str, Vec<String>> = HashMap::new();
        for k in &self.kept {
            by_server.entry(&k.server).or_default().push(k.id.clone());
        }
        for (server, ids) in by_server {
            let root = remote::root(server);
            for t in lib
                .tracks_by_remote(server, &ids)
                .unwrap_or_default()
                .into_iter()
                .flatten()
            {
                let mut d = t.path.parent().map(Path::to_path_buf);
                while let Some(p) = d {
                    let top = p == root;
                    if !self.kept_dirs.insert(p.clone()) || top {
                        break;
                    }
                    d = p.parent().map(Path::to_path_buf);
                }
            }
        }
        Ok(())
    }

    fn track_plays(&self, t: &TrackRow) -> bool {
        match t.remote() {
            Some(r) if self.offline.contains(&r.server) => {
                self.kept.contains(&SongKey::new(r.server, r.id))
            }
            _ => true,
        }
    }

    /// Whether a queue entry can play right now.
    pub fn info_plays(&self, t: &TrackInfo) -> bool {
        match &t.remote {
            Some(r) if !r.episode && self.offline.contains(&r.server) => self
                .kept
                .contains(&SongKey::new(r.server.clone(), r.id.clone())),
            _ => true,
        }
    }

    /// Whether a row has anything that can play right now.
    pub fn plays(&self, item: &Item, mode: ArtistMode) -> bool {
        if self.offline.is_empty() {
            return true;
        }
        match item {
            Item::Track(t) => self.track_plays(t),
            Item::Album(a) => !self.dead_albums.contains(&a.key),
            Item::Artist(a) => {
                let n = a.name.to_lowercase();
                match mode {
                    ArtistMode::AlbumArtist => !self.dead_album_artists.contains(&n),
                    ArtistMode::TrackArtist => !self.dead_track_artists.contains(&n),
                }
            }
            Item::Genre(g) => !self.dead_genres.contains(&g.norm),
            Item::Playlist(p) => !self.dead_playlists.contains(&p.id),
            Item::Dir { path, .. } | Item::Root { path, .. } => match remote::server_of(path) {
                Some(s) if self.offline.contains(s) => self.kept_dirs.contains(path),
                _ => true,
            },
            _ => true,
        }
    }
}
