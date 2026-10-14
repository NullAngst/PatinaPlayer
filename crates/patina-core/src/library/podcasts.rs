//! Podcast rows in the library database: what the feeds said, and where
//! you are in each episode.

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{OptionalExtension, Row, params};

use super::{Library, Result, bytes_path, now_secs, path_bytes};
use crate::track::{MediaKind, TrackInfo};

#[derive(Debug, Clone, PartialEq)]
pub struct PodcastRow {
    pub id: i64,
    pub feed_url: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub episodes: i64,
    pub unplayed: i64,
    pub last_fetch: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeRow {
    pub id: i64,
    pub podcast_id: i64,
    pub podcast_title: String,
    pub author: Option<String>,
    pub title: String,
    /// Unix seconds.
    pub published: Option<i64>,
    pub duration: Option<Duration>,
    pub audio_url: String,
    pub mime: Option<String>,
    pub size: Option<i64>,
    pub description: Option<String>,
    pub position: Duration,
    pub played: bool,
    /// Local copy, if downloaded and still on disk.
    pub download: Option<PathBuf>,
}

impl EpisodeRow {
    const COLS: &'static str = "e.id, e.podcast_id, COALESCE(p.title, p.feed_url), p.author, e.title, \
        e.published, e.duration_ms, e.audio_url, e.mime, e.size, e.description, e.position_ms, \
        e.played, e.download_path";

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let download: Option<Vec<u8>> = r.get(13)?;
        Ok(Self {
            id: r.get(0)?,
            podcast_id: r.get(1)?,
            podcast_title: r.get(2)?,
            author: r.get(3)?,
            title: r.get(4)?,
            published: r.get(5)?,
            duration: r
                .get::<_, Option<i64>>(6)?
                .map(|ms| Duration::from_millis(ms.max(0) as u64)),
            audio_url: r.get(7)?,
            mime: r.get(8)?,
            size: r.get(9)?,
            description: r.get(10)?,
            position: Duration::from_millis(r.get::<_, i64>(11)?.max(0) as u64),
            played: r.get::<_, i64>(12)? != 0,
            download: download.map(bytes_path).filter(|p| p.is_file()),
        })
    }

    /// Queue entry for this episode: the local copy if there is one,
    /// otherwise streamed from the feed's URL.
    pub fn to_info(&self) -> TrackInfo {
        // An episode on a music server streams from there, unless it's
        // been downloaded.
        let remote =
            crate::remote::parse_episode_uri(&self.audio_url).filter(|_| self.download.is_none());
        TrackInfo {
            path: self.download.clone().unwrap_or_default(),
            kind: MediaKind::Podcast,
            url: remote.is_none().then(|| self.audio_url.clone()),
            remote,
            episode: Some(self.id),
            title: Some(self.title.clone()),
            artist: self
                .author
                .clone()
                .or_else(|| Some(self.podcast_title.clone())),
            album: Some(self.podcast_title.clone()),
            duration: self.duration,
            ..Default::default()
        }
    }

    /// Where to start when played: the saved spot, unless it's finished or
    /// so close to the start it isn't worth resuming.
    pub fn resume_at(&self) -> Duration {
        if self.played || self.position < Duration::from_secs(10) {
            Duration::ZERO
        } else {
            // Back up a few seconds so you catch the sentence again.
            self.position.saturating_sub(Duration::from_secs(5))
        }
    }
}

/// What a feed fetch produced, ready to store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedData {
    pub title: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub link: Option<String>,
    pub episodes: Vec<EpisodeData>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeData {
    pub guid: String,
    pub title: String,
    pub published: Option<i64>,
    pub duration: Option<Duration>,
    pub audio_url: String,
    pub mime: Option<String>,
    pub size: Option<i64>,
    pub description: Option<String>,
}

impl Library {
    /// Make the podcasts table match the configured feeds. Feeds you
    /// removed take their episodes and listening state with them.
    pub fn sync_podcasts(&self, feeds: &[String]) -> Result<()> {
        let existing: Vec<(i64, String)> = {
            let mut st = self.conn.prepare("SELECT id, feed_url FROM podcasts")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, url) in existing {
            // Server podcasts come and go with their server, not the config.
            if !feeds.contains(&url) && !url.starts_with(crate::remote::SCHEME) {
                self.conn
                    .execute("DELETE FROM podcasts WHERE id = ?1", [id])?;
            }
        }
        for f in feeds {
            self.conn.execute(
                "INSERT INTO podcasts (feed_url) VALUES (?1) ON CONFLICT(feed_url) DO NOTHING",
                [f],
            )?;
        }
        Ok(())
    }

    pub fn podcasts(&self) -> Result<Vec<PodcastRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT p.id, p.feed_url, COALESCE(p.title, p.feed_url), p.author, p.description, \
               (SELECT COUNT(*) FROM episodes e WHERE e.podcast_id = p.id), \
               (SELECT COUNT(*) FROM episodes e WHERE e.podcast_id = p.id AND e.played = 0), \
               p.last_fetch, p.last_error \
             FROM podcasts p ORDER BY COALESCE(p.title, p.feed_url) COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok(PodcastRow {
                id: r.get(0)?,
                feed_url: r.get(1)?,
                title: r.get(2)?,
                author: r.get(3)?,
                description: r.get(4)?,
                episodes: r.get(5)?,
                unplayed: r.get(6)?,
                last_fetch: r.get(7)?,
                last_error: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Episodes, newest first.
    pub fn episodes(&self, podcast_id: i64) -> Result<Vec<EpisodeRow>> {
        let sql = format!(
            "SELECT {} FROM episodes e JOIN podcasts p ON p.id = e.podcast_id \
             WHERE e.podcast_id = ?1 ORDER BY e.published IS NULL, e.published DESC, e.id DESC",
            EpisodeRow::COLS
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map([podcast_id], EpisodeRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every episode with a local copy, newest first.
    pub fn downloaded_episodes(&self) -> Result<Vec<EpisodeRow>> {
        let sql = format!(
            "SELECT {} FROM episodes e JOIN podcasts p ON p.id = e.podcast_id \
             WHERE e.download_path IS NOT NULL ORDER BY e.published DESC",
            EpisodeRow::COLS
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map([], EpisodeRow::from_row)?;
        // Files deleted behind our back come back with no download; skip them.
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|e| e.download.is_some())
            .collect())
    }

    pub fn episode(&self, id: i64) -> Result<Option<EpisodeRow>> {
        let sql = format!(
            "SELECT {} FROM episodes e JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = ?1",
            EpisodeRow::COLS
        );
        Ok(self
            .conn
            .query_row(&sql, [id], EpisodeRow::from_row)
            .optional()?)
    }

    /// Store a fetched feed. New episodes are added; known ones get their
    /// details refreshed but keep your position, played flag and download.
    pub fn store_feed(&mut self, feed_url: &str, data: &FeedData) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO podcasts (feed_url) VALUES (?1) ON CONFLICT(feed_url) DO NOTHING",
            [feed_url],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM podcasts WHERE feed_url = ?1",
            [feed_url],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE podcasts SET title = COALESCE(?2, title), author = ?3, description = ?4, \
             image_url = ?5, link = ?6, last_fetch = ?7, last_error = NULL WHERE id = ?1",
            params![
                id,
                data.title,
                data.author,
                data.description,
                data.image_url,
                data.link,
                now_secs()
            ],
        )?;
        for e in &data.episodes {
            tx.execute(
                "INSERT INTO episodes (podcast_id, guid, title, published, duration_ms, audio_url, \
                   mime, size, description) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(podcast_id, guid) DO UPDATE SET title = excluded.title, \
                   published = excluded.published, duration_ms = COALESCE(excluded.duration_ms, duration_ms), \
                   audio_url = excluded.audio_url, mime = excluded.mime, size = excluded.size, \
                   description = excluded.description",
                params![
                    id,
                    e.guid,
                    e.title,
                    e.published,
                    e.duration.map(|d| d.as_millis() as i64),
                    e.audio_url,
                    e.mime,
                    e.size,
                    e.description
                ],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn set_feed_error(&self, feed_url: &str, err: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE podcasts SET last_error = ?2, last_fetch = ?3 WHERE feed_url = ?1",
            params![feed_url, err, now_secs()],
        )?;
        Ok(())
    }

    /// Save where you are. `finished` marks it played and rewinds it, so
    /// playing it again starts at the top.
    pub fn set_episode_progress(&self, id: i64, pos: Duration, finished: bool) -> Result<()> {
        if finished {
            self.conn.execute(
                "UPDATE episodes SET position_ms = 0, played = 1 WHERE id = ?1",
                [id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE episodes SET position_ms = ?2 WHERE id = ?1",
                params![id, pos.as_millis() as i64],
            )?;
        }
        Ok(())
    }

    pub fn set_episode_played(&self, id: i64, played: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET played = ?2, position_ms = 0 WHERE id = ?1",
            params![id, played as i64],
        )?;
        Ok(())
    }

    pub fn set_episode_download(&self, id: i64, path: Option<&std::path::Path>) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET download_path = ?2 WHERE id = ?1",
            params![id, path.map(path_bytes)],
        )?;
        Ok(())
    }
}
