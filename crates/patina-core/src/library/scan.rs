//! Folder scanning. One function, [`scan_tree`], handles everything: a
//! whole root on startup, a single new folder, a changed file, or a path
//! that was deleted. It compares what's on disk under a path with what the
//! index has under that path, reads tags only for new or changed files,
//! and deletes rows for files that are gone.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rusqlite::{Connection, OptionalExtension, params};

use super::tags::{self, TrackMeta, album_key, normalize_genre};
use super::{Result, bytes_path, now_secs, path_bytes, subtree_range};
use crate::track::is_audio_file;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Files that looked like audio but neither lofty nor FFmpeg could read.
    pub unreadable: usize,
}

impl ScanStats {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
    pub fn add(&mut self, o: ScanStats) {
        self.added += o.added;
        self.updated += o.updated;
        self.removed += o.removed;
        self.unreadable += o.unreadable;
    }
}

/// Make the sources table match the configured roots. Roots that were
/// removed from the config take their tracks with them.
pub fn sync_sources(conn: &mut Connection, roots: &[PathBuf]) -> Result<Vec<(i64, PathBuf)>> {
    let tx = conn.transaction()?;
    let existing: Vec<(i64, PathBuf)> = {
        let mut st = tx.prepare("SELECT id, root FROM sources WHERE kind = 'local'")?;
        st.query_map([], |r| Ok((r.get(0)?, bytes_path(r.get(1)?))))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut dropped = false;
    for (id, root) in &existing {
        if !roots.contains(root) {
            tx.execute("DELETE FROM sources WHERE id = ?1", [id])?;
            dropped = true;
        }
    }
    for root in roots {
        tx.execute(
            "INSERT INTO sources (kind, root) VALUES ('local', ?1) ON CONFLICT(root) DO NOTHING",
            [path_bytes(root)],
        )?;
    }
    if dropped {
        // Albums whose tracks all went with the source.
        tx.execute(
            "DELETE FROM albums WHERE key NOT IN (SELECT DISTINCT album_key FROM tracks)",
            [],
        )?;
        tx.execute(
            "DELETE FROM album_genres WHERE album_key NOT IN (SELECT key FROM albums)",
            [],
        )?;
    }
    let out = {
        let mut st =
            tx.prepare("SELECT id, root FROM sources WHERE kind = 'local' ORDER BY root")?;
        st.query_map([], |r| Ok((r.get(0)?, bytes_path(r.get(1)?))))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.commit()?;
    Ok(out)
}

struct OnDisk {
    path: PathBuf,
    mtime: i64,
    size: i64,
}

/// Audio files at or under `path`. Hidden folders and symlinked folders
/// are skipped; the second keeps a loop in the tree from hanging the scan.
fn walk(path: &Path, out: &mut Vec<OnDisk>, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(lmeta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if lmeta.is_dir() {
        let Ok(rd) = std::fs::read_dir(path) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            walk(&p, out, cancel);
        }
    } else if is_audio_file(path) {
        // Follow a symlinked file to its target's size and time.
        if let Ok(m) = std::fs::metadata(path)
            && m.is_file()
        {
            out.push(OnDisk {
                path: path.to_path_buf(),
                mtime: m.mtime(),
                size: m.size() as i64,
            });
        }
    }
}

struct Known {
    mtime: i64,
    size: i64,
    album_key: String,
}

/// Bring the index in line with the disk at or under `under`, which sits
/// inside source `source_id`. `under` may no longer exist, in which case
/// everything indexed below it is removed.
pub fn scan_tree(
    conn: &mut Connection,
    source_id: i64,
    under: &Path,
    progress: &dyn Fn(usize, usize),
    cancel: &AtomicBool,
) -> Result<ScanStats> {
    let mut stats = ScanStats::default();

    let mut on_disk = Vec::new();
    walk(under, &mut on_disk, cancel);
    if cancel.load(Ordering::Relaxed) {
        return Ok(stats);
    }

    let known: HashMap<PathBuf, Known> = {
        let (lo, hi) = subtree_range(under);
        let mut st = conn.prepare_cached(
            "SELECT path, mtime, size, album_key FROM tracks \
             WHERE path = ?1 OR (path >= ?2 AND path < ?3)",
        )?;
        st.query_map(params![path_bytes(under), lo, hi], |r| {
            Ok((
                bytes_path(r.get(0)?),
                Known {
                    mtime: r.get(1)?,
                    size: r.get(2)?,
                    album_key: r.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let seen: HashSet<&Path> = on_disk.iter().map(|f| f.path.as_path()).collect();
    let gone: Vec<&PathBuf> = known
        .keys()
        .filter(|p| !seen.contains(p.as_path()))
        .collect();
    let todo: Vec<&OnDisk> = on_disk
        .iter()
        .filter(|f| match known.get(&f.path) {
            Some(k) => k.mtime != f.mtime || k.size != f.size,
            None => true,
        })
        .collect();

    // Read tags in parallel. Disk and tag parsing are the slow part; the
    // database writes after are quick.
    let total = todo.len();
    let done = AtomicUsize::new(0);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Option<TrackMeta>)>> = Mutex::new(Vec::with_capacity(total));
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 8)
        .min(total.max(1));
    progress(0, total);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= total {
                        return;
                    }
                    let meta = tags::read(&todo[i].path);
                    results.lock().unwrap().push((i, meta));
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        // Report progress from the scoped main thread while workers run.
        loop {
            let d = done.load(Ordering::Relaxed);
            progress(d, total);
            if d >= total || cancel.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return Ok(stats);
    }
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);

    let mut touched: HashSet<String> = HashSet::new();
    let now = now_secs();
    let tx = conn.transaction()?;
    for (i, meta) in results {
        let f = todo[i];
        let Some(meta) = meta else {
            // Unreadable. A file that's still being copied lands here too;
            // the watcher sees the next write and tries again. Keep any
            // older row rather than dropping a track over a bad read.
            stats.unreadable += 1;
            continue;
        };
        let dir = f.path.parent().unwrap_or(Path::new("/"));
        let key = album_key(&meta, dir);
        let filename = f
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let genre_text = (!meta.genres.is_empty()).then(|| meta.genres.join("; "));
        let id: i64 = tx.query_row(
            "INSERT INTO tracks (source_id, path, dir, filename, mtime, size, title, artist, \
               album_artist, album, album_key, track_no, disc_no, year, genre, duration_ms, codec, \
               sample_rate, bits, channels, kbps, lossless, rg_track_gain, rg_track_peak, \
               rg_album_gain, rg_album_peak, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
               ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27) \
             ON CONFLICT(path) DO UPDATE SET source_id = excluded.source_id, dir = excluded.dir, \
               filename = excluded.filename, mtime = excluded.mtime, size = excluded.size, \
               title = excluded.title, artist = excluded.artist, album_artist = excluded.album_artist, \
               album = excluded.album, album_key = excluded.album_key, track_no = excluded.track_no, \
               disc_no = excluded.disc_no, year = excluded.year, genre = excluded.genre, \
               duration_ms = excluded.duration_ms, codec = excluded.codec, \
               sample_rate = excluded.sample_rate, bits = excluded.bits, channels = excluded.channels, \
               kbps = excluded.kbps, lossless = excluded.lossless, \
               rg_track_gain = excluded.rg_track_gain, rg_track_peak = excluded.rg_track_peak, \
               rg_album_gain = excluded.rg_album_gain, rg_album_peak = excluded.rg_album_peak \
             RETURNING id",
            params![
                source_id,
                path_bytes(&f.path),
                path_bytes(dir),
                filename,
                f.mtime,
                f.size,
                meta.title,
                meta.artist,
                meta.album_artist,
                meta.album,
                key,
                meta.track_no,
                meta.disc_no,
                meta.year,
                genre_text,
                meta.duration.map(|d| d.as_millis() as i64),
                meta.codec,
                meta.sample_rate,
                meta.bits,
                meta.channels,
                meta.kbps,
                meta.lossless as i64,
                meta.rg_track_gain,
                meta.rg_track_peak,
                meta.rg_album_gain,
                meta.rg_album_peak,
                now,
            ],
            |r| r.get(0),
        )?;
        tx.execute("DELETE FROM track_genres WHERE track_id = ?1", [id])?;
        for g in &meta.genres {
            tx.execute(
                "INSERT OR IGNORE INTO track_genres (track_id, genre_norm, genre) VALUES (?1, ?2, ?3)",
                params![id, normalize_genre(g), g],
            )?;
        }
        match known.get(&f.path) {
            Some(k) => {
                stats.updated += 1;
                touched.insert(k.album_key.clone());
            }
            None => stats.added += 1,
        }
        touched.insert(key);
    }
    for p in gone {
        tx.execute("DELETE FROM tracks WHERE path = ?1", [path_bytes(p)])?;
        touched.insert(known[p].album_key.clone());
        stats.removed += 1;
    }
    rebuild_albums(&tx, &touched)?;
    tx.execute(
        "UPDATE sources SET last_scan = ?2 WHERE id = ?1",
        params![source_id, now],
    )?;
    tx.commit()?;
    Ok(stats)
}

struct AlbumTrack {
    album: Option<String>,
    album_artist: Option<String>,
    artist: Option<String>,
    year: Option<i32>,
    genre: Option<String>,
    duration_ms: i64,
    added_at: i64,
    play_count: i64,
    dir: PathBuf,
    lossless: bool,
}

fn most_common<'a>(items: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for s in items {
        match counts.iter_mut().find(|(k, _)| *k == s) {
            Some((_, n)) => *n += 1,
            None => counts.push((s, 1)),
        }
    }
    // Ties go to the first seen, which is the first track in order.
    let mut best: Option<(&str, usize)> = None;
    for &(s, n) in &counts {
        if best.is_none_or(|(_, b)| n > b) {
            best = Some((s, n));
        }
    }
    best.map(|(s, _)| s.to_string())
}

/// Recompute album rows for these keys from their tracks.
pub fn rebuild_albums(conn: &Connection, keys: &HashSet<String>) -> Result<()> {
    let mut st = conn.prepare_cached(
        "SELECT album, album_artist, artist, year, genre, COALESCE(duration_ms, 0), added_at, \
         play_count, dir, lossless FROM tracks WHERE album_key = ?1 ORDER BY disc_no, track_no",
    )?;
    for key in keys {
        let tracks: Vec<AlbumTrack> = st
            .query_map([key], |r| {
                Ok(AlbumTrack {
                    album: r.get(0)?,
                    album_artist: r.get(1)?,
                    artist: r.get(2)?,
                    year: r.get(3)?,
                    genre: r.get(4)?,
                    duration_ms: r.get(5)?,
                    added_at: r.get(6)?,
                    play_count: r.get(7)?,
                    dir: bytes_path(r.get(8)?),
                    lossless: r.get::<_, i64>(9)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;

        conn.execute("DELETE FROM album_genres WHERE album_key = ?1", [key])?;
        if tracks.is_empty() {
            conn.execute("DELETE FROM albums WHERE key = ?1", [key])?;
            continue;
        }

        let dir = tags::album_dir(&tracks[0].dir).to_path_buf();
        let name =
            most_common(tracks.iter().filter_map(|t| t.album.as_deref())).unwrap_or_else(|| {
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Unknown album".into())
            });
        let artist = most_common(tracks.iter().filter_map(|t| t.album_artist.as_deref()))
            .or_else(|| {
                let artists: HashSet<String> = tracks
                    .iter()
                    .filter_map(|t| t.artist.as_deref().map(tags::fold))
                    .collect();
                match artists.len() {
                    0 => None,
                    1 => most_common(tracks.iter().filter_map(|t| t.artist.as_deref())),
                    _ => Some("Various Artists".into()),
                }
            })
            .unwrap_or_else(|| "Unknown artist".into());
        let year = tracks.iter().filter_map(|t| t.year).min();
        let first_genres: Vec<&str> = tracks
            .iter()
            .filter_map(|t| t.genre.as_deref())
            .filter_map(|g| g.split("; ").next())
            .collect();
        let genre = most_common(first_genres.into_iter());

        conn.execute(
            "INSERT INTO albums (key, name, artist, year, genre, track_count, duration_ms, \
               added_at, play_count, dir, lossless) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(key) DO UPDATE SET name = excluded.name, artist = excluded.artist, \
               year = excluded.year, genre = excluded.genre, track_count = excluded.track_count, \
               duration_ms = excluded.duration_ms, added_at = excluded.added_at, \
               play_count = excluded.play_count, dir = excluded.dir, lossless = excluded.lossless",
            params![
                key,
                name,
                artist,
                year,
                genre,
                tracks.len() as i64,
                tracks.iter().map(|t| t.duration_ms).sum::<i64>(),
                tracks.iter().map(|t| t.added_at).min().unwrap_or(0),
                tracks.iter().map(|t| t.play_count).sum::<i64>(),
                path_bytes(&dir),
                tracks.iter().all(|t| t.lossless) as i64,
            ],
        )?;
        // Album genres: every genre any track on it carries.
        conn.execute(
            "INSERT OR IGNORE INTO album_genres (album_key, genre_norm) \
             SELECT DISTINCT ?1, g.genre_norm FROM track_genres g \
             JOIN tracks t ON t.id = g.track_id WHERE t.album_key = ?1",
            [key],
        )?;
    }
    Ok(())
}

/// Which configured source a path belongs to.
pub fn source_for(conn: &Connection, path: &Path) -> Result<Option<(i64, PathBuf)>> {
    let mut st = conn.prepare_cached("SELECT id, root FROM sources WHERE kind = 'local'")?;
    let rows: Vec<(i64, PathBuf)> = st
        .query_map([], |r| Ok((r.get(0)?, bytes_path(r.get(1)?))))?
        .collect::<rusqlite::Result<_>>()?;
    // Deepest root wins when roots nest.
    Ok(rows
        .into_iter()
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.as_os_str().len()))
}

/// Does the index know this exact path?
pub fn is_indexed(conn: &Connection, path: &Path) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM tracks WHERE path = ?1",
            [path_bytes(path)],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
