//! Where kept songs live on disk and in the index, and what you pinned.
//!
//! Files sit under a readable path made from the song's place on the
//! server (`tracks/Home/Nirvana/Bleach/01 Blew.flac`), so the offline
//! folder makes sense in a file manager too. The index maps each server
//! song to its file, so a later re-sync that renames the song doesn't
//! lose it.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::{SongKey, Tier};
use crate::library::{ArtistMode, LibError, Library, TrackRow};

#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub key: SongKey,
    pub tier: Tier,
    pub file: PathBuf,
    pub size: u64,
    /// The file as the server has it, not converted.
    pub original: bool,
    /// "raw", or what it was converted to, like "opus:192".
    pub format: String,
    pub added_at: i64,
    pub last_used: i64,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

fn bytes_path(b: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(b))
}

pub fn load(conn: &Connection) -> rusqlite::Result<Vec<Stored>> {
    let mut st = conn.prepare(
        "SELECT server, song_id, tier, file, size, original, format, added_at, last_used \
         FROM cached",
    )?;
    st.query_map([], |r| {
        let tier: String = r.get(2)?;
        Ok(Stored {
            key: SongKey::new(r.get::<_, String>(0)?, r.get::<_, String>(1)?),
            tier: if tier == "offline" {
                Tier::Offline
            } else {
                Tier::Cache
            },
            file: bytes_path(r.get(3)?),
            size: r.get::<_, i64>(4)?.max(0) as u64,
            original: r.get(5)?,
            format: r.get(6)?,
            added_at: r.get(7)?,
            last_used: r.get(8)?,
        })
    })?
    .collect()
}

pub fn put(conn: &Connection, s: &Stored) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO cached \
         (server, song_id, tier, file, size, original, format, added_at, last_used) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            s.key.server,
            s.key.id,
            s.tier.as_str(),
            path_bytes(&s.file),
            s.size as i64,
            s.original,
            s.format,
            s.added_at,
            s.last_used
        ],
    )?;
    Ok(())
}

pub fn touch(conn: &Connection, k: &SongKey, at: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE cached SET last_used = ?3 WHERE server = ?1 AND song_id = ?2",
        params![k.server, k.id, at],
    )?;
    Ok(())
}

pub fn remove(conn: &Connection, k: &SongKey) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM cached WHERE server = ?1 AND song_id = ?2",
        params![k.server, k.id],
    )?;
    Ok(())
}

pub fn rename_server(conn: &Connection, old: &str, new: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE cached SET server = ?2 WHERE server = ?1",
        params![old, new],
    )?;
    Ok(())
}

/// A file name that's safe on any filesystem a music folder might sit on
/// (FAT on a USB stick included): no control characters, no `:*?"<>|\`,
/// no trailing dots or spaces, nothing longer than 180 bytes.
fn clean(part: &str) -> String {
    let mut s: String = part
        .chars()
        .map(|c| {
            if c.is_control() || "\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.len() > 180 {
        let mut cut = 180;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    if s.is_empty() || s == ".." {
        "_".into()
    } else {
        s
    }
}

/// Where a server song goes under `root`: its path on the server, cleaned
/// up, with the extension of what's actually stored.
pub fn file_for(root: &Path, key: &SongKey, index_path: Option<&Path>, ext: &str) -> PathBuf {
    let mut out = root.join(clean(&key.server));
    let rel = index_path
        .and_then(|p| p.to_str())
        .and_then(|p| p.strip_prefix(crate::remote::SCHEME))
        .and_then(|p| p.split_once('/'))
        .map(|(_, rest)| rest.to_string());
    match rel {
        Some(rel) if !rel.is_empty() => {
            let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
            for (i, p) in parts.iter().enumerate() {
                if i + 1 == parts.len() {
                    let stem = Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string());
                    out.push(format!("{}.{ext}", clean(&stem)));
                } else {
                    out.push(clean(p));
                }
            }
        }
        _ => out.push(format!("{}.{ext}", id_name(&key.id))),
    }
    out
}

/// A short stable name for a song id, for songs the index has no path
/// for and for telling two songs with the same name apart.
pub fn id_name(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(id.as_bytes());
    hex::encode(&h[..8])
}

/// `file`, unless something else already has that name: then the same
/// name with the song's id in it.
pub fn free_name(file: &Path, key: &SongKey, taken: impl Fn(&Path) -> bool) -> PathBuf {
    if !taken(file) {
        return file.to_path_buf();
    }
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = file
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.with_file_name(format!("{stem} [{}].{ext}", id_name(&key.id)))
}

/// Move a file, across filesystems if it has to.
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(18) => {
            // EXDEV: ~/.cache on another disk than ~/.local/share.
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

/// Take out empty folders left behind under `root`, up from `dir`.
pub fn prune_dirs(root: &Path, dir: Option<&Path>) {
    let mut d = dir.map(Path::to_path_buf);
    while let Some(p) = d {
        if p == root || !p.starts_with(root) || std::fs::remove_dir(&p).is_err() {
            break;
        }
        d = p.parent().map(Path::to_path_buf);
    }
}

// ----- Pins -----

/// What a pin stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinKind {
    Album,
    /// By album artist.
    Artist,
    /// By the artist on each track.
    TrackArtist,
    Playlist,
    Folder,
    Song,
}

impl PinKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PinKind::Album => "album",
            PinKind::Artist => "artist",
            PinKind::TrackArtist => "track-artist",
            PinKind::Playlist => "playlist",
            PinKind::Folder => "folder",
            PinKind::Song => "song",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PinKind::Album => "Album",
            PinKind::Artist | PinKind::TrackArtist => "Artist",
            PinKind::Playlist => "Playlist",
            PinKind::Folder => "Folder",
            PinKind::Song => "Song",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "album" => PinKind::Album,
            "artist" => PinKind::Artist,
            "track-artist" => PinKind::TrackArtist,
            "playlist" => PinKind::Playlist,
            "folder" => PinKind::Folder,
            "song" => PinKind::Song,
            _ => return None,
        })
    }
}

/// Something pinned for offline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pin {
    pub id: i64,
    pub kind: PinKind,
    /// Album key, artist name, playlist id, folder or song path, as bytes.
    pub key: Vec<u8>,
    pub name: String,
    pub added_at: i64,
}

impl Pin {
    pub fn new(kind: PinKind, key: impl Into<Vec<u8>>, name: impl Into<String>) -> Self {
        Self {
            id: 0,
            kind,
            key: key.into(),
            name: name.into(),
            added_at: now(),
        }
    }

    pub fn key_str(&self) -> String {
        String::from_utf8_lossy(&self.key).into_owned()
    }
}

pub fn pins(conn: &Connection) -> rusqlite::Result<Vec<Pin>> {
    let mut st = conn.prepare("SELECT id, kind, key, name, added_at FROM pins ORDER BY id")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (id, kind, key, name, added_at) = r?;
        if let Some(kind) = PinKind::parse(&kind) {
            out.push(Pin {
                id,
                kind,
                key,
                name,
                added_at,
            });
        }
    }
    Ok(out)
}

/// Pin it. False if it was already pinned.
pub fn add_pin(conn: &Connection, p: &Pin) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO pins (kind, key, name, added_at) VALUES (?1, ?2, ?3, ?4)",
        params![p.kind.as_str(), p.key, p.name, p.added_at],
    )?;
    Ok(n > 0)
}

pub fn remove_pin(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM pins WHERE id = ?1", [id])?;
    Ok(())
}

/// Folder and song pins carry paths with the server's name in them.
pub fn rename_server_pins(conn: &Connection, old: &str, new: &str) -> rusqlite::Result<()> {
    let from = format!("{}{old}", crate::remote::SCHEME);
    let to = format!("{}{new}", crate::remote::SCHEME);
    for p in pins(conn)? {
        if !matches!(p.kind, PinKind::Folder | PinKind::Song) {
            continue;
        }
        let k = p.key_str();
        let moved = if k == from {
            Some(to.clone())
        } else {
            k.strip_prefix(&format!("{from}/"))
                .map(|rest| format!("{to}/{rest}"))
        };
        if let Some(m) = moved {
            conn.execute(
                "UPDATE OR IGNORE pins SET key = ?2 WHERE id = ?1",
                params![p.id, m.into_bytes()],
            )?;
        }
    }
    Ok(())
}

/// The songs a pin stands for, right now, in play order.
pub fn resolve(lib: &Library, p: &Pin) -> Result<Vec<TrackRow>, LibError> {
    let key = p.key_str();
    match p.kind {
        PinKind::Album => lib.album_tracks(&key),
        PinKind::Artist => lib.artist_tracks(&key, ArtistMode::AlbumArtist),
        PinKind::TrackArtist => lib.artist_tracks(&key, ArtistMode::TrackArtist),
        PinKind::Playlist => match key.parse::<i64>() {
            Ok(id) => Ok(lib
                .playlist_items(id)?
                .into_iter()
                .filter_map(|(_, t)| t)
                .collect()),
            Err(_) => Ok(Vec::new()),
        },
        PinKind::Folder => lib.tracks_under(&bytes_path(p.key.clone())),
        PinKind::Song => Ok(lib
            .track_by_path(&bytes_path(p.key.clone()))?
            .into_iter()
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_file_names() {
        let k = SongKey::new("Home", "11");
        let root = Path::new("/c/tracks");
        assert_eq!(
            file_for(
                root,
                &k,
                Some(Path::new("server://Home/Nirvana/Bleach/01 Blew.flac")),
                "flac"
            ),
            PathBuf::from("/c/tracks/Home/Nirvana/Bleach/01 Blew.flac")
        );
        // Converted: the extension follows what's stored.
        assert_eq!(
            file_for(
                root,
                &k,
                Some(Path::new("server://Home/A/B: C?/01 x.flac")),
                "opus"
            ),
            PathBuf::from("/c/tracks/Home/A/B_ C_/01 x.opus")
        );
        // No path in the index: the id makes the name.
        let f = file_for(root, &k, None, "mp3");
        assert!(f.starts_with("/c/tracks/Home"), "{f:?}");
        assert!(f.to_string_lossy().ends_with(".mp3"));
    }

    #[test]
    fn taken_names_get_the_id() {
        let k = SongKey::new("Home", "11");
        let f = Path::new("/x/Intro.flac");
        assert_eq!(free_name(f, &k, |_| false), f);
        let other = free_name(f, &k, |p| p == f);
        assert_ne!(other, f);
        assert!(other.to_string_lossy().ends_with("].flac"), "{other:?}");
    }
}
