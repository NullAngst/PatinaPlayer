//! What leveling asks of the index: the ReplayGain values a song carries
//! (from the server or its tags), our own measurements, and the album
//! gain worked out from its songs when no tag gives one.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{OptionalExtension, params};

use super::{Library, Result, bytes_path, file_uri, now_secs, path_bytes, song_uri};
use crate::level::{Gains, Origin, Part, album_from_parts};
use crate::track::TrackInfo;

/// A row of the loudness table.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudRow {
    pub song: Vec<u8>,
    pub lufs: Option<f64>,
    pub track_gain: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_gain: Option<f64>,
    pub album_peak: Option<f64>,
    pub seconds: Option<f64>,
    /// "measured", "tags" or "failed".
    pub source: String,
    pub size: Option<u64>,
}

impl LoudRow {
    fn origin(&self) -> Origin {
        if self.source == "tags" {
            Origin::Tags
        } else {
            Origin::Measured
        }
    }
}

/// The key a song's loudness is kept under.
pub fn song_key(info: &TrackInfo) -> Option<Vec<u8>> {
    if let Some(r) = &info.remote {
        return (!r.episode).then(|| song_uri(&r.server, &r.id));
    }
    (!info.path.as_os_str().is_empty() && info.url.is_none()).then(|| file_uri(&info.path))
}

/// The ReplayGain columns of a track row.
struct RgRow {
    song: Vec<u8>,
    remote: bool,
    seconds: Option<f64>,
    track_gain: Option<f64>,
    track_peak: Option<f64>,
    album_gain: Option<f64>,
    album_peak: Option<f64>,
    album_key: String,
}

const RG_COLS: &str = "path, remote_id, duration_ms, rg_track_gain, rg_track_peak, \
    rg_album_gain, rg_album_peak, album_key";

fn rg_row(r: &rusqlite::Row) -> rusqlite::Result<RgRow> {
    let path = bytes_path(r.get(0)?);
    let remote_id: Option<String> = r.get(1)?;
    let song = match (&remote_id, crate::remote::server_of(&path)) {
        (Some(id), Some(server)) => song_uri(server, id),
        _ => file_uri(&path),
    };
    Ok(RgRow {
        song,
        remote: remote_id.is_some(),
        seconds: r.get::<_, Option<i64>>(2)?.map(|ms| ms as f64 / 1000.0),
        track_gain: r.get(3)?,
        track_peak: r.get(4)?,
        album_gain: r.get(5)?,
        album_peak: r.get(6)?,
        album_key: r.get(7)?,
    })
}

/// Albums bigger than this are folders of loose songs, not albums.
const MAX_ALBUM: usize = 200;

impl Library {
    pub fn loudness(&self, song: &[u8]) -> Result<Option<LoudRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT song, lufs, track_gain, track_peak, album_gain, album_peak, seconds, \
                 source, size FROM loudness WHERE song = ?1",
                [song],
                |r| {
                    Ok(LoudRow {
                        song: r.get(0)?,
                        lufs: r.get(1)?,
                        track_gain: r.get(2)?,
                        track_peak: r.get(3)?,
                        album_gain: r.get(4)?,
                        album_peak: r.get(5)?,
                        seconds: r.get(6)?,
                        source: r.get(7)?,
                        size: r.get::<_, Option<i64>>(8)?.map(|s| s as u64),
                    })
                },
            )
            .optional()?)
    }

    pub fn put_loudness(&self, row: &LoudRow) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO loudness (song, lufs, track_gain, track_peak, album_gain, \
             album_peak, seconds, source, size, measured_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                row.song,
                row.lufs,
                row.track_gain,
                row.track_peak,
                row.album_gain,
                row.album_peak,
                row.seconds,
                row.source,
                row.size.map(|s| s as i64),
                now_secs(),
            ],
        )?;
        Ok(())
    }

    /// Every song with a loudness row, failed ones included: what the
    /// measuring thread doesn't need to look at again.
    pub fn loudness_keys(&self) -> Result<HashSet<Vec<u8>>> {
        let mut st = self.conn.prepare("SELECT song FROM loudness")?;
        let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Server songs whose server sent ReplayGain values.
    pub fn server_gain_keys(&self) -> Result<HashSet<Vec<u8>>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {RG_COLS} FROM tracks WHERE remote_id IS NOT NULL AND rg_track_gain IS NOT NULL"
        ))?;
        let rows = st.query_map([], rg_row)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|r| r.song)
            .collect())
    }

    /// Local songs with no ReplayGain tags and no row in the loudness
    /// table, for measuring the whole library.
    pub fn unmeasured_files(&self) -> Result<Vec<std::path::PathBuf>> {
        let known = self.loudness_keys()?;
        let mut st = self.conn.prepare(
            "SELECT path FROM tracks WHERE remote_id IS NULL AND rg_track_gain IS NULL \
             ORDER BY album_key, disc_no, track_no",
        )?;
        let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for p in rows {
            let p = bytes_path(p?);
            if !known.contains(&file_uri(&p)) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// The track gain one song carries, and how long it is: the server's
    /// value or its tags (as indexed), else the loudness table.
    fn part(&self, r: &RgRow) -> Result<Option<Part>> {
        if let Some(g) = r.track_gain {
            return Ok(Some(Part {
                gain: g,
                peak: r.track_peak,
                seconds: r.seconds.unwrap_or(0.0),
            }));
        }
        Ok(self.loudness(&r.song)?.and_then(|l| {
            Some(Part {
                gain: l.track_gain?,
                peak: l.track_peak,
                seconds: l.seconds.or(r.seconds).unwrap_or(0.0),
            })
        }))
    }

    /// Everything known about a song's loudness, best source first.
    pub fn gains_for(&self, info: &TrackInfo) -> Result<Gains> {
        let Some(song) = song_key(info) else {
            return Ok(Gains::default());
        };
        let row = self
            .conn
            .query_row(
                &format!("SELECT {RG_COLS} FROM tracks WHERE path = ?1"),
                [path_bytes(&info.path)],
                rg_row,
            )
            .optional()?;
        let indexed = if row.as_ref().is_some_and(|r| r.remote) {
            Origin::Server
        } else {
            Origin::Tags
        };
        let mut g = Gains::default();
        if let Some(r) = &row {
            if let Some(t) = r.track_gain {
                g.track_gain = Some(t);
                g.track_peak = r.track_peak;
                g.track_from = Some(indexed);
            }
            if let Some(a) = r.album_gain {
                g.album_gain = Some(a);
                g.album_peak = r.album_peak;
                g.album_from = Some(indexed);
            }
        }
        // A measurement, or tags read from a cached copy, fill in what the
        // index didn't have. A local file that changed size since is
        // stale and doesn't count.
        let stale = |l: &LoudRow| {
            info.remote.is_none() && l.size.is_some() && file_size(&info.path) != l.size
        };
        if let Some(l) = self.loudness(&song)?.filter(|l| !stale(l)) {
            if g.track_gain.is_none() && l.track_gain.is_some() {
                g.track_gain = l.track_gain;
                g.track_peak = l.track_peak;
                g.track_from = Some(l.origin());
            }
            if g.album_gain.is_none() && l.album_gain.is_some() {
                g.album_gain = l.album_gain;
                g.album_peak = l.album_peak;
                g.album_from = Some(Origin::Tags);
            }
        }
        // No album gain anywhere: work it out from the album's songs, if
        // every one of them is known.
        if g.album_gain.is_none()
            && let Some(r) = &row
            && info.album.as_deref().is_some_and(|a| !a.trim().is_empty())
        {
            let mut st = self.conn.prepare_cached(&format!(
                "SELECT {RG_COLS} FROM tracks WHERE album_key = ?1 LIMIT {}",
                MAX_ALBUM + 1
            ))?;
            let rows = st
                .query_map([&r.album_key], rg_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len() <= MAX_ALBUM {
                let parts = rows
                    .iter()
                    .map(|t| self.part(t))
                    .collect::<Result<Vec<_>>>()?;
                if let Some((a, peak)) = album_from_parts(&parts) {
                    g.album_gain = Some(a);
                    g.album_peak = peak;
                    g.album_from = Some(Origin::Measured);
                }
            }
        }
        Ok(g)
    }

    /// Whether a song still needs measuring: no gain from the index or
    /// its tags, and no row (or a stale one) in the loudness table.
    pub fn needs_measuring(&self, info: &TrackInfo) -> Result<bool> {
        let Some(song) = song_key(info) else {
            return Ok(false);
        };
        let indexed: Option<Option<f64>> = self
            .conn
            .query_row(
                "SELECT rg_track_gain FROM tracks WHERE path = ?1",
                [path_bytes(&info.path)],
                |r| r.get(0),
            )
            .optional()?;
        if indexed.flatten().is_some() {
            return Ok(false);
        }
        match self.loudness(&song)? {
            None => Ok(true),
            Some(l) => {
                Ok(info.remote.is_none() && l.size.is_some() && file_size(&info.path) != l.size)
            }
        }
    }
}

fn file_size(p: &Path) -> Option<u64> {
    std::fs::metadata(p).map(|m| m.len()).ok()
}
