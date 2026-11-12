//! Listening history: every play, in order, newest first. Play counts say
//! how often; this says when.

use rusqlite::params;

use super::{Library, Result, TrackRow};

/// How many plays the Recently played view shows. Older ones stay in the
/// database, so the history isn't lost, they just aren't listed.
pub const SHOWN: usize = 500;

/// One play.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayRow {
    pub id: i64,
    /// When it counted as played, Unix seconds.
    pub played_at: i64,
    pub track: TrackRow,
}

impl Library {
    /// Note a play. Called from [`Library::record_play`], once a song has
    /// counted.
    pub(crate) fn log_play(&self, uri: &[u8], at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plays (song, played_at) VALUES (?1, ?2)",
            params![uri, at],
        )?;
        Ok(())
    }

    /// The latest plays, newest first. Plays of songs no longer in the
    /// library are left out of the list but kept, so they come back if
    /// the song does.
    pub fn recent_plays(&self, limit: usize) -> Result<Vec<PlayRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, song, played_at FROM plays ORDER BY played_at DESC, id DESC LIMIT ?1",
        )?;
        let rows: Vec<(i64, Vec<u8>, i64)> = st
            .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut cache: std::collections::HashMap<Vec<u8>, Option<TrackRow>> =
            std::collections::HashMap::new();
        let mut out = Vec::with_capacity(rows.len());
        for (id, uri, played_at) in rows {
            let track = match cache.get(&uri) {
                Some(t) => t.clone(),
                None => {
                    let t = self.track_by_uri(&uri)?;
                    cache.insert(uri, t.clone());
                    t
                }
            };
            if let Some(track) = track {
                out.push(PlayRow {
                    id,
                    played_at,
                    track,
                });
            }
        }
        Ok(out)
    }

    /// How many plays the history holds.
    pub fn play_log_len(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0))?)
    }

    /// Forget the listening history. Play counts stay, since the stats and
    /// the Most played sorts are built from them.
    pub fn clear_history(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM plays", [])?)
    }

    /// Take one play out of the history.
    pub fn forget_play(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM plays WHERE id = ?1", [id])?;
        Ok(())
    }
}

/// "Today", "Yesterday", "Monday", "Sat 3 Oct", or "12 Oct 2025" for a
/// play time, against `now`, in local time.
pub fn day_label(at: i64, now: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let (Some(then), Some(today)) = (
        Local.timestamp_opt(at, 0).single(),
        Local.timestamp_opt(now, 0).single(),
    ) else {
        return "Earlier".into();
    };
    let days = (today.date_naive() - then.date_naive()).num_days();
    match days {
        i64::MIN..=0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => then.format("%A").to_string(),
        _ if then.year() == today.year() => then.format("%a %-d %b").to_string(),
        _ => then.format("%-d %b %Y").to_string(),
    }
}

/// "14:05" for a play time, in local time.
pub fn time_label(at: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(at, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}
