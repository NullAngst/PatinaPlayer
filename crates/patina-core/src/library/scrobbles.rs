//! The scrobble queue and loved songs. Scrobbles wait here until Last.fm
//! takes them, so a play made offline still counts, at the time it
//! happened.

use rusqlite::{OptionalExtension, params};

use super::{Library, Result, now_secs};
use crate::lastfm::Play;

/// A queued scrobble and its row id.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub id: i64,
    pub play: Play,
    pub tries: i64,
}

impl Library {
    pub fn queue_scrobble(&self, p: &Play) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO scrobbles (artist, track, album, album_artist, track_no, duration, played_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                p.artist,
                p.track,
                p.album,
                p.album_artist,
                p.track_no,
                p.duration,
                p.timestamp
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The oldest `n` waiting, oldest first.
    pub fn queued_scrobbles(&self, n: usize) -> Result<Vec<Queued>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, artist, track, album, album_artist, track_no, duration, played_at, tries \
             FROM scrobbles ORDER BY played_at, id LIMIT ?1",
        )?;
        let rows = st.query_map([n as i64], |r| {
            Ok(Queued {
                id: r.get(0)?,
                play: Play {
                    artist: r.get(1)?,
                    track: r.get(2)?,
                    album: r.get(3)?,
                    album_artist: r.get(4)?,
                    track_no: r.get(5)?,
                    duration: r.get(6)?,
                    timestamp: r.get(7)?,
                },
                tries: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn scrobbles_waiting(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM scrobbles", [], |r| r.get(0))?)
    }

    pub fn drop_scrobbles(&self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM scrobbles WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn bump_scrobble_tries(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.conn
                .execute("UPDATE scrobbles SET tries = tries + 1 WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    // ----- Loves -----

    pub fn loved(&self, artist: &str, track: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM loves WHERE artist = ?1 AND track = ?2",
                params![artist, track],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn set_loved(&self, artist: &str, track: &str, on: bool) -> Result<()> {
        if on {
            self.conn.execute(
                "INSERT OR IGNORE INTO loves (artist, track, loved_at) VALUES (?1, ?2, ?3)",
                params![artist, track, now_secs()],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM loves WHERE artist = ?1 AND track = ?2",
                params![artist, track],
            )?;
        }
        Ok(())
    }
}
