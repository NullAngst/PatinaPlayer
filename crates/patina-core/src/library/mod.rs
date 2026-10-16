//! The library index: one SQLite database holding every source, so
//! browsing is instant and works offline.
//!
//! [`Library`] is a connection with the queries the views need. Each
//! thread opens its own; SQLite in WAL mode lets readers run while the
//! scanner writes. [`service::LibraryService`] owns scanning and watching.

pub mod loudness;
pub mod marks;
pub mod mixes;
pub mod podcasts;
pub mod scan;
mod schema;
pub mod scrobbles;
pub mod service;
pub mod tags;

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::track::{TrackInfo, natural_cmp};

pub use scrobbles::{HEARTED, HEARTED_NAME};
pub use service::{LibEvent, LibraryService, ScanStatus};

#[derive(Debug, thiserror::Error)]
pub enum LibError {
    #[error("library database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, LibError>;

pub(crate) fn path_bytes(p: &Path) -> &[u8] {
    p.as_os_str().as_bytes()
}

pub(crate) fn bytes_path(b: Vec<u8>) -> PathBuf {
    PathBuf::from(std::ffi::OsString::from_vec(b))
}

/// Byte range [lo, hi) holding every path strictly under `dir`.
/// '/' + 1 is '0', so "dir/" .. "dir0" covers exactly the subtree.
pub(crate) fn subtree_range(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut lo = path_bytes(dir).to_vec();
    if lo.last() != Some(&b'/') {
        lo.push(b'/');
    }
    let mut hi = lo.clone();
    *hi.last_mut().unwrap() = b'0';
    (lo, hi)
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// URI a playlist stores for a local file.
pub fn file_uri(path: &Path) -> Vec<u8> {
    let mut v = b"file:".to_vec();
    v.extend_from_slice(path_bytes(path));
    v
}

/// URI a playlist stores for a server song.
pub fn song_uri(server: &str, id: &str) -> Vec<u8> {
    crate::remote::episode_uri(server, id).into_bytes()
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub album_key: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration: Option<Duration>,
    pub codec: String,
    pub sample_rate: u32,
    pub bits: Option<u32>,
    pub channels: u16,
    pub kbps: Option<u32>,
    pub lossless: bool,
    pub added_at: i64,
    pub play_count: i64,
    /// The server's id for a server song.
    pub remote_id: Option<String>,
    /// File size in bytes, as the folder or the server reported it.
    pub size: u64,
    /// Your rating, 1 to 5; 0 is none.
    pub rating: u8,
}

impl TrackRow {
    const COLS: &'static str = "id, path, title, filename, artist, album_artist, album, album_key, \
        track_no, disc_no, year, genre, duration_ms, codec, sample_rate, bits, channels, kbps, \
        lossless, added_at, play_count, remote_id, size, \
        (SELECT stars FROM song_ratings WHERE song_ratings.path = tracks.path)";

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let title: Option<String> = r.get(2)?;
        let filename: String = r.get(3)?;
        Ok(Self {
            id: r.get(0)?,
            path: bytes_path(r.get(1)?),
            title: title.unwrap_or_else(|| {
                Path::new(&filename)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(filename)
            }),
            artist: r.get(4)?,
            album_artist: r.get(5)?,
            album: r.get(6)?,
            album_key: r.get(7)?,
            track_no: r.get(8)?,
            disc_no: r.get(9)?,
            year: r.get(10)?,
            genre: r.get(11)?,
            duration: r
                .get::<_, Option<i64>>(12)?
                .map(|ms| Duration::from_millis(ms.max(0) as u64)),
            codec: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
            sample_rate: r.get::<_, Option<u32>>(14)?.unwrap_or(0),
            bits: r.get(15)?,
            channels: r.get::<_, Option<u16>>(16)?.unwrap_or(0),
            kbps: r.get(17)?,
            lossless: r.get::<_, i64>(18)? != 0,
            added_at: r.get(19)?,
            play_count: r.get(20)?,
            remote_id: r.get(21)?,
            size: r.get::<_, Option<i64>>(22)?.unwrap_or(0).max(0) as u64,
            rating: r.get::<_, Option<i64>>(23)?.unwrap_or(0).clamp(0, 5) as u8,
        })
    }

    /// Which server and song this is, for a server song.
    pub fn remote(&self) -> Option<crate::remote::RemoteRef> {
        Some(crate::remote::RemoteRef {
            server: crate::remote::server_of(&self.path)?.to_string(),
            id: self.remote_id.clone()?,
            episode: false,
        })
    }

    /// What a playlist stores for this track.
    pub fn uri(&self) -> Vec<u8> {
        match self.remote() {
            Some(r) => song_uri(&r.server, &r.id),
            None => file_uri(&self.path),
        }
    }

    /// What the player needs to queue this track without probing it.
    pub fn to_info(&self) -> TrackInfo {
        TrackInfo {
            path: self.path.clone(),
            remote: self.remote(),
            title: Some(self.title.clone()),
            artist: self.artist.clone(),
            album: self.album.clone(),
            track_no: self.track_no.map(|n| n.to_string()),
            duration: self.duration,
            codec: self.codec.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits: self.bits,
            kbps: self.kbps,
            lossless: self.lossless,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumRow {
    pub key: String,
    pub name: String,
    pub artist: String,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub track_count: i64,
    pub duration: Duration,
    pub added_at: i64,
    pub play_count: i64,
    pub dir: PathBuf,
    pub lossless: bool,
    /// First track on the album, which is where its cover art is looked up.
    pub cover_track: Option<PathBuf>,
    /// Your rating, 1 to 5; 0 is none.
    pub rating: u8,
}

impl AlbumRow {
    /// The server it's on, for a server album.
    pub fn server(&self) -> Option<&str> {
        crate::remote::server_of(&self.dir)
    }

    /// "Nirvana", or "Nirvana · Home" for an album on a server, so it can
    /// be told from a local copy.
    pub fn artist_line(&self) -> String {
        match self.server() {
            Some(s) => format!("{} · {s}", self.artist),
            None => self.artist.clone(),
        }
    }
    const COLS: &'static str = "key, name, artist, year, genre, track_count, duration_ms, added_at, \
        play_count, dir, lossless, \
        (SELECT path FROM tracks t WHERE t.album_key = albums.key ORDER BY disc_no, track_no LIMIT 1), \
        (SELECT stars FROM album_ratings WHERE album_ratings.key = albums.key)";

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            key: r.get(0)?,
            name: r.get(1)?,
            artist: r.get(2)?,
            year: r.get(3)?,
            genre: r.get(4)?,
            track_count: r.get(5)?,
            duration: Duration::from_millis(r.get::<_, i64>(6)?.max(0) as u64),
            added_at: r.get(7)?,
            play_count: r.get(8)?,
            dir: bytes_path(r.get(9)?),
            lossless: r.get::<_, i64>(10)? != 0,
            cover_track: r.get::<_, Option<Vec<u8>>>(11)?.map(bytes_path),
            rating: r.get::<_, Option<i64>>(12)?.unwrap_or(0).clamp(0, 5) as u8,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistRow {
    pub name: String,
    pub albums: i64,
    pub tracks: i64,
    /// Plays across all their tracks.
    pub plays: i64,
}

impl ArtistRow {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            name: r.get(0)?,
            albums: r.get(1)?,
            tracks: r.get(2)?,
            plays: r.get(3)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenreRow {
    /// Display name, the most common spelling in your tags.
    pub name: String,
    pub norm: String,
    pub albums: i64,
    pub tracks: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistRow {
    pub id: i64,
    pub name: String,
    pub items: i64,
    pub updated_at: i64,
    /// The server it lives on, for a server playlist.
    pub server: Option<String>,
    pub remote_id: Option<String>,
    /// Someone else's playlist on the server, or a smart list.
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceRow {
    pub id: i64,
    pub root: PathBuf,
    pub tracks: i64,
    pub last_scan: Option<i64>,
}

/// One entry in the Folders view.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // short-lived, built once per folder listing
pub enum DirEntry {
    Dir { path: PathBuf, name: String },
    Track(TrackRow),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtistMode {
    /// Album artist, falling back to the track artist. What most people
    /// mean by "the artist", and it keeps compilations from flooding the list.
    #[default]
    AlbumArtist,
    TrackArtist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Name,
    Artist,
    Year,
    RecentlyAdded,
    MostPlayed,
    /// Your rating, highest first.
    Rating,
    Random,
}

impl AlbumSort {
    pub const ALL: [AlbumSort; 7] = [
        AlbumSort::Name,
        AlbumSort::Artist,
        AlbumSort::Year,
        AlbumSort::RecentlyAdded,
        AlbumSort::MostPlayed,
        AlbumSort::Rating,
        AlbumSort::Random,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AlbumSort::Name => "name",
            AlbumSort::Artist => "artist",
            AlbumSort::Year => "year",
            AlbumSort::RecentlyAdded => "recently added",
            AlbumSort::MostPlayed => "most played",
            AlbumSort::Rating => "rating",
            AlbumSort::Random => "random",
        }
    }
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    fn order_by(self) -> &'static str {
        match self {
            AlbumSort::Name => "name COLLATE NOCASE, artist COLLATE NOCASE",
            AlbumSort::Artist => "artist COLLATE NOCASE, year, name COLLATE NOCASE",
            AlbumSort::Year => {
                "year IS NULL, year DESC, artist COLLATE NOCASE, name COLLATE NOCASE"
            }
            AlbumSort::RecentlyAdded => "added_at DESC, name COLLATE NOCASE",
            AlbumSort::MostPlayed => "play_count DESC, name COLLATE NOCASE",
            AlbumSort::Rating => {
                "COALESCE((SELECT stars FROM album_ratings WHERE album_ratings.key = albums.key), 0) DESC, \
                 artist COLLATE NOCASE, name COLLATE NOCASE"
            }
            AlbumSort::Random => "random()",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SongSort {
    #[default]
    Title,
    Artist,
    Album,
    Year,
    Duration,
    RecentlyAdded,
    MostPlayed,
    /// Your rating, highest first.
    Rating,
}

impl SongSort {
    pub const ALL: [SongSort; 8] = [
        SongSort::Title,
        SongSort::Artist,
        SongSort::Album,
        SongSort::Year,
        SongSort::Duration,
        SongSort::RecentlyAdded,
        SongSort::MostPlayed,
        SongSort::Rating,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SongSort::Title => "title",
            SongSort::Artist => "artist",
            SongSort::Album => "album",
            SongSort::Year => "year",
            SongSort::Duration => "length",
            SongSort::RecentlyAdded => "recently added",
            SongSort::MostPlayed => "most played",
            SongSort::Rating => "rating",
        }
    }
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    fn order_by(self, desc: bool) -> String {
        let d = if desc { " DESC" } else { "" };
        let title = "COALESCE(title, filename) COLLATE NOCASE";
        match self {
            SongSort::Title => format!("{title}{d}"),
            SongSort::Artist => format!(
                "artist IS NULL, artist COLLATE NOCASE{d}, album COLLATE NOCASE, disc_no, track_no"
            ),
            SongSort::Album => format!("album IS NULL, album COLLATE NOCASE{d}, disc_no, track_no"),
            SongSort::Year => {
                format!("year IS NULL, year{d}, album COLLATE NOCASE, disc_no, track_no")
            }
            SongSort::Duration => format!("duration_ms{d}, {title}"),
            SongSort::RecentlyAdded => {
                format!("added_at{}, {title}", if desc { "" } else { " DESC" })
            }
            SongSort::MostPlayed => {
                format!("play_count{}, {title}", if desc { "" } else { " DESC" })
            }
            SongSort::Rating => format!(
                "COALESCE((SELECT stars FROM song_ratings WHERE song_ratings.path = tracks.path), 0){}, \
                 artist COLLATE NOCASE, album COLLATE NOCASE, disc_no, track_no",
                if desc { "" } else { " DESC" }
            ),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchResults {
    pub artists: Vec<ArtistRow>,
    pub albums: Vec<AlbumRow>,
    pub tracks: Vec<TrackRow>,
}

pub struct Library {
    pub(crate) conn: Connection,
}

/// Sort tracks the way an album plays: disc, track number, then file name.
pub fn album_order(a: &TrackRow, b: &TrackRow) -> std::cmp::Ordering {
    let fname = |t: &TrackRow| {
        t.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    a.disc_no
        .unwrap_or(1)
        .cmp(&b.disc_no.unwrap_or(1))
        .then(a.path.parent().cmp(&b.path.parent()))
        .then(
            a.track_no
                .unwrap_or(u32::MAX)
                .cmp(&b.track_no.unwrap_or(u32::MAX)),
        )
        .then_with(|| natural_cmp(&fname(a), &fname(b)))
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// Point the index at these roots and scan them once, on this thread.
    /// The service does the same thing in the background; this is for
    /// tools and tests that want the result right away.
    pub fn index_folders(&mut self, roots: &[PathBuf]) -> Result<scan::ScanStats> {
        let sources = scan::sync_sources(&mut self.conn, roots)?;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut total = scan::ScanStats::default();
        for (id, root) in sources {
            if root.is_dir() {
                total.add(scan::scan_tree(
                    &mut self.conn,
                    id,
                    &root,
                    &|_, _| {},
                    &cancel,
                )?);
            }
        }
        Ok(total)
    }

    fn tracks_where(&self, clause: &str, args: impl rusqlite::Params) -> Result<Vec<TrackRow>> {
        let sql = format!("SELECT {} FROM tracks {clause}", TrackRow::COLS);
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map(args, TrackRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn track_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?)
    }

    pub fn sources(&self) -> Result<Vec<SourceRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT s.id, s.root, s.last_scan, (SELECT COUNT(*) FROM tracks t WHERE t.source_id = s.id) \
             FROM sources s ORDER BY s.root",
        )?;
        let rows = st.query_map([], |r| {
            Ok(SourceRow {
                id: r.get(0)?,
                root: bytes_path(r.get(1)?),
                last_scan: r.get(2)?,
                tracks: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn track_by_path(&self, path: &Path) -> Result<Option<TrackRow>> {
        Ok(self
            .tracks_where("WHERE path = ?1", [path_bytes(path)])?
            .into_iter()
            .next())
    }

    /// The track a playlist URI points at, if it's in the index.
    pub fn track_by_uri(&self, uri: &[u8]) -> Result<Option<TrackRow>> {
        if let Some(p) = uri.strip_prefix(b"file:") {
            return self.track_by_path(&bytes_path(p.to_vec()));
        }
        let Some(r) = std::str::from_utf8(uri)
            .ok()
            .and_then(crate::remote::parse_episode_uri)
        else {
            return Ok(None);
        };
        Ok(self
            .tracks_by_remote(&r.server, std::slice::from_ref(&r.id))?
            .pop()
            .flatten())
    }

    /// What a playlist should store for the track at `path`.
    pub fn uri_for_path(&self, path: &Path) -> Result<Vec<u8>> {
        if crate::remote::is_remote(path)
            && let Some(t) = self.track_by_path(path)?
        {
            return Ok(t.uri());
        }
        Ok(file_uri(path))
    }

    /// Server songs by id, in the order asked, None for any the index
    /// doesn't have.
    pub fn tracks_by_remote(&self, server: &str, ids: &[String]) -> Result<Vec<Option<TrackRow>>> {
        let root = crate::remote::root(server);
        let Some(src) = self.source_id(&root)? else {
            return Ok(vec![None; ids.len()]);
        };
        let sql = format!(
            "SELECT {} FROM tracks WHERE source_id = ?1 AND remote_id = ?2",
            TrackRow::COLS
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        ids.iter()
            .map(|id| {
                Ok(st
                    .query_row(params![src, id], TrackRow::from_row)
                    .optional()?)
            })
            .collect()
    }

    /// The source row for a root, local or server.
    pub fn source_id(&self, root: &Path) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM sources WHERE root = ?1",
                [path_bytes(root)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The server's cover id for a server song, or for the first song
    /// under a server folder.
    pub fn remote_cover_for(&self, path: &Path) -> Result<Option<String>> {
        let direct: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT cover_id FROM tracks WHERE path = ?1",
                [path_bytes(path)],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(c) = direct {
            return Ok(c);
        }
        let (lo, hi) = subtree_range(path);
        Ok(self
            .conn
            .query_row(
                "SELECT cover_id FROM tracks WHERE (dir = ?1 OR (dir >= ?2 AND dir < ?3)) \
                 AND cover_id IS NOT NULL ORDER BY dir, disc_no, track_no LIMIT 1",
                params![path_bytes(path), lo, hi],
                |r| r.get(0),
            )
            .optional()?)
    }

    // ----- Folders -----

    /// Subfolders that contain music, and the tracks directly in `dir`.
    pub fn folder(&self, dir: &Path) -> Result<Vec<DirEntry>> {
        let (lo, hi) = subtree_range(dir);
        let mut st = self
            .conn
            .prepare_cached("SELECT DISTINCT dir FROM tracks WHERE dir >= ?1 AND dir < ?2")?;
        let dirs: Vec<Vec<u8>> = st
            .query_map(params![lo, hi], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut children: Vec<String> = Vec::new();
        for d in dirs {
            let rest = &d[lo.len()..];
            let first = rest.split(|&b| b == b'/').next().unwrap_or(rest);
            let name = String::from_utf8_lossy(first).into_owned();
            if !name.is_empty() && !children.contains(&name) {
                children.push(name);
            }
        }
        children.sort_by(|a, b| natural_cmp(a, b));
        let mut out: Vec<DirEntry> = children
            .into_iter()
            .map(|name| DirEntry::Dir {
                path: dir.join(&name),
                name,
            })
            .collect();
        let mut tracks = self.tracks_where("WHERE dir = ?1", [path_bytes(dir)])?;
        tracks.sort_by(|a, b| {
            natural_cmp(
                &a.path.file_name().unwrap_or_default().to_string_lossy(),
                &b.path.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        out.extend(tracks.into_iter().map(DirEntry::Track));
        Ok(out)
    }

    /// Every track under `dir`, in folder order. What "play this folder" plays.
    pub fn tracks_under(&self, dir: &Path) -> Result<Vec<TrackRow>> {
        let (lo, hi) = subtree_range(dir);
        let mut t = self.tracks_where(
            "WHERE dir = ?1 OR (dir >= ?2 AND dir < ?3)",
            params![path_bytes(dir), lo, hi],
        )?;
        t.sort_by(|a, b| natural_cmp(&a.path.to_string_lossy(), &b.path.to_string_lossy()));
        Ok(t)
    }

    // ----- Artists -----

    pub fn artists(&self, mode: ArtistMode) -> Result<Vec<ArtistRow>> {
        let sql = match mode {
            ArtistMode::AlbumArtist => {
                "SELECT artist, COUNT(*), SUM(track_count), SUM(play_count) FROM albums \
                 GROUP BY artist COLLATE NOCASE ORDER BY artist COLLATE NOCASE"
            }
            ArtistMode::TrackArtist => {
                "SELECT artist, COUNT(DISTINCT album_key), COUNT(*), SUM(play_count) FROM tracks \
                 WHERE artist IS NOT NULL GROUP BY artist COLLATE NOCASE ORDER BY artist COLLATE NOCASE"
            }
        };
        let mut st = self.conn.prepare_cached(sql)?;
        let rows = st.query_map([], ArtistRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn artist_albums(&self, name: &str, mode: ArtistMode) -> Result<Vec<AlbumRow>> {
        let sql = match mode {
            ArtistMode::AlbumArtist => format!(
                "SELECT {} FROM albums WHERE artist = ?1 COLLATE NOCASE \
                 ORDER BY year IS NULL, year, name COLLATE NOCASE",
                AlbumRow::COLS
            ),
            ArtistMode::TrackArtist => format!(
                "SELECT {} FROM albums WHERE key IN \
                 (SELECT album_key FROM tracks WHERE artist = ?1 COLLATE NOCASE) \
                 ORDER BY year IS NULL, year, name COLLATE NOCASE",
                AlbumRow::COLS
            ),
        };
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map([name], AlbumRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// An artist's tracks, album by album. In track-artist mode, only the
    /// tracks they're credited on.
    pub fn artist_tracks(&self, name: &str, mode: ArtistMode) -> Result<Vec<TrackRow>> {
        let mut out = Vec::new();
        for album in self.artist_albums(name, mode)? {
            let mut t = self.album_tracks(&album.key)?;
            if mode == ArtistMode::TrackArtist {
                t.retain(|t| {
                    t.artist
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(name))
                });
            }
            out.extend(t);
        }
        Ok(out)
    }

    // ----- Albums -----

    pub fn albums(&self, sort: AlbumSort) -> Result<Vec<AlbumRow>> {
        let sql = format!(
            "SELECT {} FROM albums ORDER BY {}",
            AlbumRow::COLS,
            sort.order_by()
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([], AlbumRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn album(&self, key: &str) -> Result<Option<AlbumRow>> {
        let sql = format!("SELECT {} FROM albums WHERE key = ?1", AlbumRow::COLS);
        Ok(self
            .conn
            .query_row(&sql, [key], AlbumRow::from_row)
            .optional()?)
    }

    pub fn album_tracks(&self, key: &str) -> Result<Vec<TrackRow>> {
        let mut t = self.tracks_where("WHERE album_key = ?1", [key])?;
        t.sort_by(album_order);
        Ok(t)
    }

    // ----- Songs -----

    pub fn songs(&self, sort: SongSort, desc: bool) -> Result<Vec<TrackRow>> {
        self.tracks_where(&format!("ORDER BY {}", sort.order_by(desc)), [])
    }

    // ----- Genres -----

    pub fn genres(&self) -> Result<Vec<GenreRow>> {
        // Display name = the most common spelling of each normalized genre.
        let mut st = self.conn.prepare_cached(
            "SELECT g.genre_norm, \
               (SELECT genre FROM track_genres x WHERE x.genre_norm = g.genre_norm \
                  GROUP BY genre ORDER BY COUNT(*) DESC, genre LIMIT 1), \
               (SELECT COUNT(*) FROM album_genres a WHERE a.genre_norm = g.genre_norm), \
               COUNT(*) \
             FROM track_genres g GROUP BY g.genre_norm",
        )?;
        let mut rows: Vec<GenreRow> = st
            .query_map([], |r| {
                Ok(GenreRow {
                    norm: r.get(0)?,
                    name: r.get(1)?,
                    albums: r.get(2)?,
                    tracks: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        rows.sort_by_key(|a| a.name.to_lowercase());
        Ok(rows)
    }

    /// Albums tagged with a genre (by album genre: any of its tracks).
    pub fn genre_albums(&self, norm: &str) -> Result<Vec<AlbumRow>> {
        let sql = format!(
            "SELECT {} FROM albums WHERE key IN (SELECT album_key FROM album_genres WHERE genre_norm = ?1) \
             ORDER BY artist COLLATE NOCASE, year, name COLLATE NOCASE",
            AlbumRow::COLS
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map([norm], AlbumRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Tracks whose own genre tag includes this genre.
    pub fn genre_tracks(&self, norm: &str) -> Result<Vec<TrackRow>> {
        let mut t = self.tracks_where(
            "WHERE id IN (SELECT track_id FROM track_genres WHERE genre_norm = ?1)",
            [norm],
        )?;
        t.sort_by(|a, b| {
            a.artist
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b.artist.as_deref().unwrap_or("").to_lowercase())
                .then(a.album_key.cmp(&b.album_key))
                .then(album_order(a, b))
        });
        Ok(t)
    }

    // ----- Search -----

    /// Type-ahead search over the local index. Every word has to match
    /// somewhere, so "nirv bleach" finds the album.
    pub fn search(&self, query: &str, limit: usize) -> Result<SearchResults> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| {
                let esc = w
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                format!("%{esc}%")
            })
            .collect();
        if words.is_empty() {
            return Ok(SearchResults::default());
        }
        let build = |fields: &[&str]| {
            words
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let any: Vec<String> = fields
                        .iter()
                        .map(|f| format!("{f} LIKE ?{} ESCAPE '\\'", i + 1))
                        .collect();
                    format!("({})", any.join(" OR "))
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        let args = rusqlite::params_from_iter(words.iter());

        let artist_sql = format!(
            "SELECT artist, COUNT(*), SUM(track_count), SUM(play_count) FROM albums WHERE {} \
             GROUP BY artist COLLATE NOCASE ORDER BY artist COLLATE NOCASE LIMIT {limit}",
            build(&["artist"])
        );
        let mut st = self.conn.prepare(&artist_sql)?;
        let artists = st
            .query_map(args, ArtistRow::from_row)?
            .collect::<rusqlite::Result<_>>()?;

        let album_sql = format!(
            "SELECT {} FROM albums WHERE {} ORDER BY name COLLATE NOCASE LIMIT {limit}",
            AlbumRow::COLS,
            build(&["name", "artist"])
        );
        let mut st = self.conn.prepare(&album_sql)?;
        let albums = st
            .query_map(rusqlite::params_from_iter(words.iter()), AlbumRow::from_row)?
            .collect::<rusqlite::Result<_>>()?;

        let tracks = self.tracks_where(
            &format!(
                "WHERE {} ORDER BY COALESCE(title, filename) COLLATE NOCASE LIMIT {limit}",
                build(&["title", "filename", "artist", "album"])
            ),
            rusqlite::params_from_iter(words.iter()),
        )?;
        Ok(SearchResults {
            artists,
            albums,
            tracks,
        })
    }

    // ----- Play counts -----

    pub fn record_play(&self, path: &Path) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE path = ?1",
            params![path_bytes(path), now_secs()],
        )?;
        if n > 0 {
            self.conn.execute(
                "UPDATE albums SET play_count = play_count + 1 \
                 WHERE key = (SELECT album_key FROM tracks WHERE path = ?1)",
                [path_bytes(path)],
            )?;
        }
        Ok(())
    }

    // ----- Listening stats -----

    /// The most played tracks, most first. Tracks never played are left
    /// out, so a fresh library gives an empty list.
    pub fn top_tracks(&self, limit: usize) -> Result<Vec<TrackRow>> {
        self.tracks_where(
            &format!(
                "WHERE play_count > 0 \
                 ORDER BY play_count DESC, last_played DESC, COALESCE(title, filename) LIMIT {limit}"
            ),
            [],
        )
    }

    /// Albums by the plays of all their songs added up.
    pub fn top_albums(&self, limit: usize) -> Result<Vec<AlbumRow>> {
        let sql = format!(
            "SELECT {} FROM albums WHERE play_count > 0 \
             ORDER BY play_count DESC, name COLLATE NOCASE LIMIT {limit}",
            AlbumRow::COLS
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map([], AlbumRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Artists by the plays of all their songs added up, counted the same
    /// way the Artists view groups them.
    pub fn top_artists(&self, mode: ArtistMode, limit: usize) -> Result<Vec<ArtistRow>> {
        let sql = match mode {
            ArtistMode::AlbumArtist => format!(
                "SELECT artist, COUNT(*), SUM(track_count), SUM(play_count) FROM albums \
                 GROUP BY artist COLLATE NOCASE HAVING SUM(play_count) > 0 \
                 ORDER BY SUM(play_count) DESC, artist COLLATE NOCASE LIMIT {limit}"
            ),
            ArtistMode::TrackArtist => format!(
                "SELECT artist, COUNT(DISTINCT album_key), COUNT(*), SUM(play_count) FROM tracks \
                 WHERE artist IS NOT NULL GROUP BY artist COLLATE NOCASE HAVING SUM(play_count) > 0 \
                 ORDER BY SUM(play_count) DESC, artist COLLATE NOCASE LIMIT {limit}"
            ),
        };
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map([], ArtistRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ----- Playlists -----

    pub fn playlists(&self) -> Result<Vec<PlaylistRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT p.id, p.name, (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id), \
             p.updated_at, s.root, p.remote_id, p.readonly \
             FROM playlists p LEFT JOIN sources s ON s.id = p.source_id \
             ORDER BY p.name COLLATE NOCASE, s.root",
        )?;
        let rows = st.query_map([], |r| {
            let root: Option<Vec<u8>> = r.get(4)?;
            Ok(PlaylistRow {
                id: r.get(0)?,
                name: r.get(1)?,
                items: r.get(2)?,
                updated_at: r.get(3)?,
                server: root
                    .and_then(|b| crate::remote::server_of(&bytes_path(b)).map(str::to_string)),
                remote_id: r.get(5)?,
                readonly: r.get::<_, i64>(6)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One playlist by id, Hearted songs included.
    pub fn playlist(&self, id: i64) -> Result<Option<PlaylistRow>> {
        if id == scrobbles::HEARTED {
            return self.hearted_row().map(Some);
        }
        Ok(self.playlists()?.into_iter().find(|p| p.id == id))
    }

    pub fn create_playlist(&self, name: &str) -> Result<i64> {
        let now = now_secs();
        self.conn.execute(
            "INSERT INTO playlists (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
            params![name, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE playlists SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now_secs()],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Items in order. Entries whose file is no longer in the library come
    /// back as None, so the view can show them as missing instead of
    /// silently dropping them.
    pub fn playlist_items(&self, id: i64) -> Result<Vec<(Vec<u8>, Option<TrackRow>)>> {
        if id == scrobbles::HEARTED {
            return Ok(self
                .hearted_tracks()?
                .into_iter()
                .map(|t| (t.uri(), Some(t)))
                .collect());
        }
        let mut st = self
            .conn
            .prepare_cached("SELECT uri FROM playlist_items WHERE playlist_id = ?1 ORDER BY pos")?;
        let uris: Vec<Vec<u8>> = st
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::with_capacity(uris.len());
        for uri in uris {
            let track = self.track_by_uri(&uri)?;
            out.push((uri, track));
        }
        Ok(out)
    }

    /// Add files to the end of a playlist, leaving out any already in it
    /// (or listed twice). Returns how many went in.
    pub fn append_to_playlist(&mut self, id: i64, paths: &[PathBuf]) -> Result<usize> {
        let uris: Vec<Vec<u8>> = paths
            .iter()
            .map(|p| self.uri_for_path(p))
            .collect::<Result<_>>()?;
        self.append_uris(id, &uris)
    }

    /// [`Self::append_to_playlist`] for URIs already worked out.
    pub fn append_uris(&mut self, id: i64, uris: &[Vec<u8>]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut have: std::collections::HashSet<Vec<u8>> = {
            let mut st = tx.prepare("SELECT uri FROM playlist_items WHERE playlist_id = ?1")?;
            st.query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut pos: i64 = tx.query_row(
            "SELECT COALESCE(MAX(pos) + 1, 0) FROM playlist_items WHERE playlist_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let mut added = 0;
        for uri in uris {
            if !have.insert(uri.clone()) {
                continue;
            }
            tx.execute(
                "INSERT INTO playlist_items (playlist_id, pos, uri) VALUES (?1, ?2, ?3)",
                params![id, pos, uri],
            )?;
            pos += 1;
            added += 1;
        }
        tx.execute(
            "UPDATE playlists SET updated_at = ?2 WHERE id = ?1",
            params![id, now_secs()],
        )?;
        tx.commit()?;
        Ok(added)
    }

    /// Replace a playlist's items wholesale. Used for remove and reorder,
    /// which keeps positions dense without any renumbering tricks.
    pub fn set_playlist_items(&mut self, id: i64, uris: &[Vec<u8>]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
        for (i, u) in uris.iter().enumerate() {
            tx.execute(
                "INSERT INTO playlist_items (playlist_id, pos, uri) VALUES (?1, ?2, ?3)",
                params![id, i as i64, u],
            )?;
        }
        tx.execute(
            "UPDATE playlists SET updated_at = ?2 WHERE id = ?1",
            params![id, now_secs()],
        )?;
        tx.commit()?;
        Ok(())
    }
}
