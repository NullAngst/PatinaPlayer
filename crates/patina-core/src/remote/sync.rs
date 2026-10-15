//! Copying a server's library into the local index.
//!
//! The first sync reads every album. Later ones list the albums again
//! (cheap: a few hundred per request), compare each with what it looked
//! like last time, and only fetch the songs of albums that are new or
//! changed. Albums gone from the server take their songs with them.
//!
//! Songs keep their row across syncs, matched by the server's id, so play
//! counts and playlist entries survive a retag on the server.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{OptionalExtension, params};

use super::http::{codec_name, lossless_suffix};
use super::{Client, RemoteError, RemoteSong, ServerKind};
use crate::library::podcasts::{EpisodeData, FeedData};
use crate::library::scan::rebuild_albums;
use crate::library::tags::normalize_genre;
use crate::library::{LibError, Library, now_secs, path_bytes, song_uri};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("{0}")]
    Remote(#[from] RemoteError),
    #[error("{0}")]
    Library(#[from] LibError),
    #[error("library database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Albums whose songs were fetched.
    pub albums: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

impl SyncStats {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// Albums per database transaction.
const BATCH: usize = 20;

/// The source row for a server, made if it's new.
pub fn ensure_source(lib: &Library, server: &str, kind: ServerKind) -> Result<i64> {
    let root = super::root(server);
    lib.conn.execute(
        "INSERT INTO sources (kind, root) VALUES (?1, ?2) ON CONFLICT(root) DO NOTHING",
        params![kind.as_str(), path_bytes(&root)],
    )?;
    Ok(lib.source_id(&root)?.expect("just inserted"))
}

/// Forget a server: its songs, playlists and podcasts.
pub fn drop_source(lib: &mut Library, server: &str) -> Result<()> {
    let root = super::root(server);
    let tx = lib.conn.transaction()?;
    tx.execute("DELETE FROM sources WHERE root = ?1", [path_bytes(&root)])?;
    tx.execute(
        "DELETE FROM albums WHERE key NOT IN (SELECT DISTINCT album_key FROM tracks)",
        [],
    )?;
    tx.execute(
        "DELETE FROM album_genres WHERE album_key NOT IN (SELECT key FROM albums)",
        [],
    )?;
    tx.execute(
        "DELETE FROM podcasts WHERE substr(feed_url, 1, ?2) = ?1",
        params![
            format!("{}#", root.display()),
            format!("{}#", root.display()).len() as i64
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// A file or folder name from a tag: no slashes, never empty.
fn clean(s: &str) -> String {
    let s = s.trim().replace('/', "\u{2215}");
    if s.is_empty() || s == "." || s == ".." {
        "_".into()
    } else {
        s
    }
}

/// Where a song shows in the folder view: its path on the server when the
/// server says, else artist / album / track.
fn song_path(root: &Path, s: &RemoteSong) -> PathBuf {
    let rel: Vec<String> = match &s.path {
        Some(p) => p
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    let rel = if rel.is_empty() {
        let artist = s
            .album_artist
            .as_deref()
            .or(s.artist.as_deref())
            .unwrap_or("Unknown artist");
        let album = s.album.as_deref().unwrap_or("Unknown album");
        let num = s.track_no.map(|n| format!("{n:02} - ")).unwrap_or_default();
        let ext = if s.suffix.is_empty() {
            "audio"
        } else {
            &s.suffix
        };
        vec![
            clean(artist),
            clean(album),
            format!("{num}{}.{ext}", clean(&s.title)),
        ]
    } else {
        rel
    };
    let mut p = root.to_path_buf();
    for c in rel {
        p.push(c);
    }
    p
}

/// `name.flac` becomes `name [id].flac`, for two songs claiming one path.
fn with_id(path: &Path, id: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(e) => format!("{stem} [{}].{}", clean(id), e.to_string_lossy()),
        None => format!("{stem} [{}]", clean(id)),
    };
    path.with_file_name(name)
}

fn album_key(source_id: i64, s: &RemoteSong, dir: &Path) -> String {
    if s.album_id.is_empty() {
        format!("srv{source_id}\u{1f}dir\u{1f}{}", dir.display())
    } else {
        format!("srv{source_id}\u{1f}{}", s.album_id)
    }
}

/// Write one album's songs. Returns the album keys touched.
fn write_album(
    tx: &rusqlite::Transaction,
    source_id: i64,
    root: &Path,
    album_id: &str,
    songs: &[RemoteSong],
    stats: &mut SyncStats,
) -> Result<HashSet<String>> {
    let mut touched = HashSet::new();
    let now = now_secs();
    // What the index has for this album now.
    let before: Vec<(i64, String, String)> = {
        let mut st = tx.prepare_cached(
            "SELECT id, remote_id, album_key FROM tracks WHERE source_id = ?1 AND remote_album = ?2",
        )?;
        st.query_map(params![source_id, album_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let keep: HashSet<&str> = songs.iter().map(|s| s.id.as_str()).collect();
    for (id, rid, key) in &before {
        if !keep.contains(rid.as_str()) {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
            touched.insert(key.clone());
            stats.removed += 1;
        }
    }
    for s in songs {
        let mut path = song_path(root, s);
        // Two songs on one path (a fake path built from tags can do that):
        // the second gets its id in the name.
        let owner: Option<Option<String>> = tx
            .query_row(
                "SELECT remote_id FROM tracks WHERE path = ?1",
                [path_bytes(&path)],
                |r| r.get(0),
            )
            .optional()?;
        if matches!(&owner, Some(o) if o.as_deref() != Some(s.id.as_str())) {
            path = with_id(&path, &s.id);
        }
        let dir = path.parent().unwrap_or(root).to_path_buf();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = album_key(source_id, s, &dir);
        let genre_text = (!s.genres.is_empty()).then(|| s.genres.join("; "));
        let existing: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, album_key FROM tracks WHERE source_id = ?1 AND remote_id = ?2",
                params![source_id, s.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let codec = codec_name(&s.suffix);
        let lossless = lossless_suffix(&s.suffix);
        let duration = s.duration.map(|d| d.as_millis() as i64);
        let track_id = match existing {
            Some((id, old_key)) => {
                tx.execute(
                    "UPDATE tracks SET path = ?2, dir = ?3, filename = ?4, size = ?5, title = ?6, \
                       artist = ?7, album_artist = ?8, album = ?9, album_key = ?10, track_no = ?11, \
                       disc_no = ?12, year = ?13, genre = ?14, duration_ms = ?15, codec = ?16, \
                       sample_rate = ?17, bits = ?18, channels = ?19, kbps = ?20, lossless = ?21, \
                       rg_track_gain = ?22, rg_track_peak = ?23, rg_album_gain = ?24, \
                       rg_album_peak = ?25, remote_album = ?26, cover_id = ?27, \
                       play_count = MAX(play_count, ?28) \
                     WHERE id = ?1",
                    params![
                        id,
                        path_bytes(&path),
                        path_bytes(&dir),
                        filename,
                        s.size.unwrap_or(0),
                        s.title,
                        s.artist,
                        s.album_artist,
                        s.album,
                        key,
                        s.track_no,
                        s.disc_no,
                        s.year,
                        genre_text,
                        duration,
                        codec,
                        s.sample_rate,
                        s.bits,
                        s.channels,
                        s.kbps,
                        lossless as i64,
                        s.rg_track_gain,
                        s.rg_track_peak,
                        s.rg_album_gain,
                        s.rg_album_peak,
                        album_id,
                        s.cover,
                        s.play_count,
                    ],
                )?;
                touched.insert(old_key);
                stats.updated += 1;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO tracks (source_id, path, dir, filename, mtime, size, title, artist, \
                       album_artist, album, album_key, track_no, disc_no, year, genre, duration_ms, \
                       codec, sample_rate, bits, channels, kbps, lossless, rg_track_gain, \
                       rg_track_peak, rg_album_gain, rg_album_peak, added_at, play_count, \
                       remote_id, remote_album, cover_id) \
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                       ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
                    params![
                        source_id,
                        path_bytes(&path),
                        path_bytes(&dir),
                        filename,
                        s.size.unwrap_or(0),
                        s.title,
                        s.artist,
                        s.album_artist,
                        s.album,
                        key,
                        s.track_no,
                        s.disc_no,
                        s.year,
                        genre_text,
                        duration,
                        codec,
                        s.sample_rate,
                        s.bits,
                        s.channels,
                        s.kbps,
                        lossless as i64,
                        s.rg_track_gain,
                        s.rg_track_peak,
                        s.rg_album_gain,
                        s.rg_album_peak,
                        now,
                        s.play_count,
                        s.id,
                        album_id,
                        s.cover,
                    ],
                )?;
                stats.added += 1;
                tx.last_insert_rowid()
            }
        };
        Library::server_song_rating(tx, &path, s.rating)?;
        tx.execute("DELETE FROM track_genres WHERE track_id = ?1", [track_id])?;
        for g in &s.genres {
            tx.execute(
                "INSERT OR IGNORE INTO track_genres (track_id, genre_norm, genre) VALUES (?1, ?2, ?3)",
                params![track_id, normalize_genre(g), g],
            )?;
        }
        touched.insert(key);
    }
    Ok(touched)
}

/// Bring the index up to date with the server's albums and songs.
/// `progress` hears (albums fetched, albums to fetch). `full` reads every
/// album again, which catches tag edits that left an album's song count,
/// length and dates alone; otherwise only changed albums are read.
pub fn sync_library(
    lib: &mut Library,
    server: &str,
    kind: ServerKind,
    client: &dyn Client,
    full: bool,
    progress: &dyn Fn(usize, usize),
    cancel: &AtomicBool,
) -> Result<SyncStats> {
    let source_id = ensure_source(lib, server, kind)?;
    let root = super::root(server);
    let albums = client.albums()?;
    let known: HashMap<String, String> = {
        let mut st = lib
            .conn
            .prepare("SELECT album_id, sig FROM remote_albums WHERE source_id = ?1")?;
        st.query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let listed: HashSet<&str> = albums.iter().map(|a| a.id.as_str()).collect();
    let todo: Vec<_> = albums
        .iter()
        .filter(|a| full || known.get(&a.id) != Some(&a.signature()))
        .collect();
    let mut stats = SyncStats::default();

    // Albums that left the server.
    let gone: Vec<&String> = known
        .keys()
        .filter(|k| !listed.contains(k.as_str()))
        .collect();
    if !gone.is_empty() {
        let tx = lib.conn.transaction()?;
        let mut touched = HashSet::new();
        for id in gone {
            let keys: Vec<String> = {
                let mut st = tx.prepare_cached(
                    "SELECT DISTINCT album_key FROM tracks WHERE source_id = ?1 AND remote_album = ?2",
                )?;
                st.query_map(params![source_id, id], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            touched.extend(keys);
            stats.removed += tx.execute(
                "DELETE FROM tracks WHERE source_id = ?1 AND remote_album = ?2",
                params![source_id, id],
            )?;
            tx.execute(
                "DELETE FROM remote_albums WHERE source_id = ?1 AND album_id = ?2",
                params![source_id, id],
            )?;
        }
        rebuild_albums(&tx, &touched)?;
        tx.commit()?;
    }

    let total = todo.len();
    progress(0, total);
    for (n, batch) in todo.chunks(BATCH).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(SyncError::Cancelled);
        }
        // Fetch first, then write, so the database isn't held while the
        // network is slow.
        let mut fetched = Vec::with_capacity(batch.len());
        for a in batch {
            if cancel.load(Ordering::Relaxed) {
                return Err(SyncError::Cancelled);
            }
            fetched.push((*a, client.album_songs(&a.id)?));
        }
        let tx = lib.conn.transaction()?;
        let mut touched = HashSet::new();
        for (a, songs) in &fetched {
            touched.extend(write_album(
                &tx, source_id, &root, &a.id, songs, &mut stats,
            )?);
            tx.execute(
                "INSERT INTO remote_albums (source_id, album_id, sig) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(source_id, album_id) DO UPDATE SET sig = excluded.sig",
                params![source_id, a.id, a.signature()],
            )?;
            stats.albums += 1;
        }
        rebuild_albums(&tx, &touched)?;
        tx.commit()?;
        progress(((n + 1) * BATCH).min(total), total);
    }
    // Album ratings ride along with the album list, so they're read on
    // every sync, not just for albums that changed.
    let ratings: Vec<(String, Option<u8>)> =
        albums.iter().map(|a| (a.id.clone(), a.rating)).collect();
    if lib.server_album_ratings(source_id, &ratings)? {
        stats.updated += 1;
    }
    lib.conn.execute(
        "UPDATE sources SET last_scan = ?2 WHERE id = ?1",
        params![source_id, now_secs()],
    )?;
    Ok(stats)
}

/// Send the ratings and stars you set here that this server hasn't heard
/// yet. One that fails stays waiting for the next try. True if anything
/// was sent.
pub fn push_marks(
    lib: &Library,
    server: &str,
    kind: ServerKind,
    client: &dyn Client,
) -> Result<bool> {
    let source_id = ensure_source(lib, server, kind)?;
    let mut sent = false;
    let mut first_err = None;
    let mut note = |r: std::result::Result<(), RemoteError>| match r {
        Ok(()) => true,
        Err(RemoteError::Unsupported(_)) => false,
        Err(e) => {
            if first_err.is_none() {
                first_err = Some(e);
            }
            false
        }
    };
    for p in lib.pending_song_ratings(source_id)? {
        if note(client.set_rating(super::Rated::Song, &p.remote_id, p.stars)) {
            lib.rating_sent(true, &p.key, p.stars)?;
            sent = true;
        }
    }
    for p in lib.pending_album_ratings(source_id)? {
        if note(client.set_rating(super::Rated::Album, &p.remote_id, p.stars)) {
            lib.rating_sent(false, &p.key, p.stars)?;
            sent = true;
        }
    }
    for p in lib.pending_stars(server, source_id)? {
        use crate::library::marks::FavKind;
        let r = match p.kind {
            FavKind::Album => match &p.remote_id {
                Some(id) => client.star_album(id, p.on),
                // The album's gone from the server: nothing to tell it.
                None => Ok(()),
            },
            FavKind::Artist => client.star_artist(&String::from_utf8_lossy(&p.key), p.on),
            FavKind::Folder => Ok(()),
        };
        if note(r) {
            lib.star_sent(p.id)?;
            sent = true;
        }
    }
    if let Some(e) = first_err {
        return Err(e.into());
    }
    Ok(sent)
}

/// Read what the server has starred into the favorites. True if the list
/// changed.
pub fn pull_starred(
    lib: &mut Library,
    server: &str,
    kind: ServerKind,
    client: &dyn Client,
) -> Result<bool> {
    let source_id = ensure_source(lib, server, kind)?;
    let starred = match client.starred() {
        Ok(s) => s,
        Err(RemoteError::Unsupported(_)) => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let mut albums = Vec::new();
    for id in &starred.albums {
        let row: Option<(String, String)> = lib
            .conn
            .query_row(
                "SELECT t.album_key, COALESCE(a.name, t.album, '') FROM tracks t                  LEFT JOIN albums a ON a.key = t.album_key                  WHERE t.source_id = ?1 AND t.remote_album = ?2 LIMIT 1",
                params![source_id, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some(r) = row {
            albums.push(r);
        }
    }
    Ok(lib.server_starred(server, &albums, &starred.artists)?)
}

/// Copy the server's playlists. True if anything changed.
pub fn sync_playlists(
    lib: &mut Library,
    server: &str,
    kind: ServerKind,
    client: &dyn Client,
) -> Result<bool> {
    let source_id = ensure_source(lib, server, kind)?;
    let lists = client.playlists()?;
    let known: HashMap<String, (i64, Option<String>)> = {
        let mut st = lib
            .conn
            .prepare("SELECT remote_id, id, remote_sig FROM playlists WHERE source_id = ?1")?;
        st.query_map([source_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut changed = false;
    let listed: HashSet<&str> = lists.iter().map(|p| p.id.as_str()).collect();
    for (rid, (id, _)) in &known {
        if !listed.contains(rid.as_str()) {
            lib.conn
                .execute("DELETE FROM playlists WHERE id = ?1", [id])?;
            changed = true;
        }
    }
    for p in &lists {
        let sig = p.signature();
        let local = known.get(&p.id);
        if local.is_some_and(|(_, s)| s.as_deref() == Some(sig.as_str())) {
            continue;
        }
        let songs = client.playlist_songs(&p.id)?;
        let uris: Vec<Vec<u8>> = songs.iter().map(|s| song_uri(server, &s.id)).collect();
        let now = now_secs();
        let id = match local {
            Some((id, _)) => {
                lib.conn.execute(
                    "UPDATE playlists SET name = ?2, remote_sig = ?3, readonly = ?4, updated_at = ?5 \
                     WHERE id = ?1",
                    params![id, p.name, sig, p.readonly as i64, now],
                )?;
                *id
            }
            None => {
                lib.conn.execute(
                    "INSERT INTO playlists (name, created_at, updated_at, source_id, remote_id, \
                       remote_sig, readonly) VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6)",
                    params![p.name, now, source_id, p.id, sig, p.readonly as i64],
                )?;
                lib.conn.last_insert_rowid()
            }
        };
        lib.set_playlist_items(id, &uris)?;
        changed = true;
    }
    Ok(changed)
}

/// Copy the server's podcasts and the episodes it has on hand. Their
/// listening state stays local, like any other podcast.
pub fn sync_podcasts(lib: &mut Library, server: &str, client: &dyn Client) -> Result<bool> {
    let pods = client.podcasts()?;
    let prefix = format!("{}#", super::root(server).display());
    let mut keep: Vec<String> = Vec::new();
    for p in &pods {
        let feed = super::episode_uri(server, &p.id);
        let episodes: Vec<EpisodeData> = p
            .episodes
            .iter()
            .filter_map(|e| {
                Some(EpisodeData {
                    guid: e.id.clone(),
                    title: e.title.clone(),
                    published: e.published,
                    duration: e.duration,
                    audio_url: super::episode_uri(server, e.stream_id.as_deref()?),
                    mime: None,
                    size: e.size,
                    description: e.description.clone(),
                })
            })
            .collect();
        let guids: Vec<String> = episodes.iter().map(|e| e.guid.clone()).collect();
        let id = lib.store_feed(
            &feed,
            &FeedData {
                title: Some(p.title.clone()),
                author: p.author.clone(),
                description: p.description.clone(),
                image_url: None,
                link: p.feed_url.clone(),
                episodes,
            },
        )?;
        // Episodes the server dropped go too, unless you downloaded them.
        let existing: Vec<(i64, String)> = {
            let mut st = lib.conn.prepare(
                "SELECT id, guid FROM episodes WHERE podcast_id = ?1 AND download_path IS NULL",
            )?;
            st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (eid, guid) in existing {
            if !guids.contains(&guid) {
                lib.conn
                    .execute("DELETE FROM episodes WHERE id = ?1", [eid])?;
            }
        }
        keep.push(feed);
    }
    let stale: Vec<(i64, String)> = {
        let mut st = lib
            .conn
            .prepare("SELECT id, feed_url FROM podcasts WHERE substr(feed_url, 1, ?2) = ?1")?;
        st.query_map(params![prefix, prefix.len() as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    for (id, url) in stale {
        if !keep.contains(&url) {
            lib.conn
                .execute("DELETE FROM podcasts WHERE id = ?1", [id])?;
        }
    }
    Ok(!pods.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_the_server_or_from_tags() {
        let root = super::super::root("Home");
        let mut s = RemoteSong {
            id: "1".into(),
            title: "Blew / Live".into(),
            album: Some("Bleach".into()),
            artist: Some("Nirvana".into()),
            track_no: Some(1),
            suffix: "flac".into(),
            ..Default::default()
        };
        assert_eq!(
            song_path(&root, &s),
            PathBuf::from("server://Home/Nirvana/Bleach/01 - Blew \u{2215} Live.flac")
        );
        s.path = Some("/music/../Nirvana/Bleach/01 Blew.flac".into());
        assert_eq!(
            song_path(&root, &s),
            PathBuf::from("server://Home/music/Nirvana/Bleach/01 Blew.flac")
        );
        assert_eq!(
            with_id(Path::new("server://Home/a/b.flac"), "x9"),
            PathBuf::from("server://Home/a/b [x9].flac")
        );
    }
}
