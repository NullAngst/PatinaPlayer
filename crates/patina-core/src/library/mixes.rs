//! What mixes ask of the index: songs for a set of genres, songs by an
//! artist on any source, artists that share genres, and saved mixes.

use rusqlite::{OptionalExtension, params, params_from_iter};

use super::{Library, Result, TrackRow, now_secs};

/// A mix you saved: what it was made from, and what was found similar
/// when it was saved, so it still plays offline.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedMixRow {
    pub id: i64,
    /// "genre", "artist" or "track".
    pub kind: String,
    pub name: String,
    /// What it starts from, as JSON.
    pub seed: String,
    /// Similar artists or songs found for it, as JSON.
    pub similar: String,
    pub created_at: i64,
}

impl Library {
    /// Songs for a genre mix: on albums tagged with every one of `norms`
    /// (any track of the album carrying the tag), or, `by_track`, songs
    /// tagged with every one themselves.
    pub fn genre_mix_tracks(&self, norms: &[String], by_track: bool) -> Result<Vec<TrackRow>> {
        if norms.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; norms.len()].join(", ");
        let n = norms.len();
        let clause = if by_track {
            format!(
                "WHERE id IN (SELECT track_id FROM track_genres WHERE genre_norm IN ({marks}) \
                 GROUP BY track_id HAVING COUNT(DISTINCT genre_norm) = {n})"
            )
        } else {
            format!(
                "WHERE album_key IN (SELECT album_key FROM album_genres WHERE genre_norm IN ({marks}) \
                 GROUP BY album_key HAVING COUNT(DISTINCT genre_norm) = {n})"
            )
        };
        self.tracks_where(&clause, params_from_iter(norms.iter()))
    }

    /// Every song credited to an artist, as the track artist or the album
    /// artist, on every source.
    pub fn tracks_by_artist(&self, name: &str) -> Result<Vec<TrackRow>> {
        self.tracks_where(
            "WHERE artist = ?1 COLLATE NOCASE OR album_artist = ?1 COLLATE NOCASE",
            [name],
        )
    }

    /// Songs with this artist and title, on any source.
    pub fn find_tracks(&self, artist: &str, title: &str) -> Result<Vec<TrackRow>> {
        self.tracks_where(
            "WHERE title = ?2 COLLATE NOCASE \
             AND (artist = ?1 COLLATE NOCASE OR album_artist = ?1 COLLATE NOCASE)",
            [artist, title],
        )
    }

    /// The artist's name as the index spells it, if it has any songs.
    pub fn artist_name(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(artist, album_artist) FROM tracks \
                 WHERE artist = ?1 COLLATE NOCASE OR album_artist = ?1 COLLATE NOCASE LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Artists who share the most genres with `name`, for mixes when
    /// neither a server nor Last.fm knows who's similar.
    pub fn artists_sharing_genres(&self, name: &str, limit: usize) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached(
            "SELECT MIN(t.artist), COUNT(DISTINCT g.genre_norm) AS shared, COUNT(*) AS n \
             FROM tracks t JOIN track_genres g ON g.track_id = t.id \
             WHERE g.genre_norm IN ( \
                 SELECT DISTINCT g2.genre_norm FROM tracks t2 \
                 JOIN track_genres g2 ON g2.track_id = t2.id \
                 WHERE t2.artist = ?1 COLLATE NOCASE OR t2.album_artist = ?1 COLLATE NOCASE) \
             AND t.artist IS NOT NULL AND t.artist <> ?1 COLLATE NOCASE \
             GROUP BY LOWER(t.artist) ORDER BY shared DESC, n DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![name, limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The genres a song carries, normalized.
    pub fn track_genres(&self, track_id: i64) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT genre_norm FROM track_genres WHERE track_id = ?1")?;
        let rows = st.query_map([track_id], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ----- Saved mixes -----

    pub fn saved_mixes(&self) -> Result<Vec<SavedMixRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, kind, name, seed, similar, created_at FROM mixes ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(SavedMixRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                seed: r.get(3)?,
                similar: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn saved_mix(&self, id: i64) -> Result<Option<SavedMixRow>> {
        Ok(self.saved_mixes()?.into_iter().find(|m| m.id == id))
    }

    pub fn save_mix(&self, kind: &str, name: &str, seed: &str, similar: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO mixes (kind, name, seed, similar, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![kind, name, seed, similar, now_secs()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_mix(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE mixes SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    /// Forget a saved mix, and its offline pin with it.
    pub fn delete_mix(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM mixes WHERE id = ?1", [id])?;
        self.conn.execute(
            "DELETE FROM pins WHERE kind = 'mix' AND key = ?1",
            [id.to_string().into_bytes()],
        )?;
        Ok(())
    }
}
