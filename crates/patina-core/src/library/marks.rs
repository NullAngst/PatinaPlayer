//! Your marks on the library: ratings of songs and albums, 1 to 5 stars,
//! and favorites (starred folders, albums and artists, in your order).
//!
//! Server songs and albums carry a `dirty` mark while a change hasn't
//! reached their server yet, so a sync never overwrites what you just
//! did with what the server still says, and sends it instead.

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};

use super::{Library, Result, bytes_path, now_secs, path_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FavKind {
    Folder,
    Album,
    Artist,
}

impl FavKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FavKind::Folder => "folder",
            FavKind::Album => "album",
            FavKind::Artist => "artist",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "album" => FavKind::Album,
            "artist" => FavKind::Artist,
            _ => FavKind::Folder,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FavKind::Folder => "Folder",
            FavKind::Album => "Album",
            FavKind::Artist => "Artist",
        }
    }
}

/// One favorite.
#[derive(Debug, Clone, PartialEq)]
pub struct Favorite {
    pub id: i64,
    pub kind: FavKind,
    /// Folder path bytes, album key, or artist name.
    pub key: Vec<u8>,
    pub label: String,
    /// The server whose starred list it's kept in step with.
    pub server: Option<String>,
}

impl Favorite {
    pub fn key_str(&self) -> String {
        String::from_utf8_lossy(&self.key).into_owned()
    }
    pub fn path(&self) -> PathBuf {
        bytes_path(self.key.clone())
    }
}

/// A rating change waiting for a server: what, its id there, the stars.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRating {
    /// Song path, or album key.
    pub key: Vec<u8>,
    pub remote_id: String,
    pub stars: u8,
}

/// A star change waiting for a server.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingStar {
    pub id: i64,
    pub kind: FavKind,
    pub key: Vec<u8>,
    /// For an album, its id on the server.
    pub remote_id: Option<String>,
    /// True to star, false to unstar.
    pub on: bool,
}

impl Library {
    // ----- Ratings -----

    pub fn song_rating(&self, path: &Path) -> Result<u8> {
        Ok(self
            .conn
            .query_row(
                "SELECT stars FROM song_ratings WHERE path = ?1",
                [path_bytes(path)],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            .clamp(0, 5) as u8)
    }

    /// Rate a song, 0 to 5 (0 takes the rating away). `dirty`: a server
    /// song whose server hasn't heard yet.
    pub fn set_song_rating(&self, path: &Path, stars: u8, dirty: bool) -> Result<()> {
        let stars = stars.min(5) as i64;
        if stars == 0 && !dirty {
            self.conn.execute(
                "DELETE FROM song_ratings WHERE path = ?1",
                [path_bytes(path)],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO song_ratings (path, stars, rated_at, dirty) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(path) DO UPDATE SET stars = ?2, rated_at = ?3, dirty = ?4",
                params![path_bytes(path), stars, now_secs(), dirty as i64],
            )?;
        }
        Ok(())
    }

    pub fn album_rating(&self, key: &str) -> Result<u8> {
        Ok(self
            .conn
            .query_row(
                "SELECT stars FROM album_ratings WHERE key = ?1",
                [key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            .clamp(0, 5) as u8)
    }

    pub fn set_album_rating(&self, key: &str, stars: u8, dirty: bool) -> Result<()> {
        let stars = stars.min(5) as i64;
        if stars == 0 && !dirty {
            self.conn
                .execute("DELETE FROM album_ratings WHERE key = ?1", [key])?;
        } else {
            self.conn.execute(
                "INSERT INTO album_ratings (key, stars, rated_at, dirty) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(key) DO UPDATE SET stars = ?2, rated_at = ?3, dirty = ?4",
                params![key, stars, now_secs(), dirty as i64],
            )?;
        }
        Ok(())
    }

    /// A server album's id there, by its key here.
    pub fn remote_album_of(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT remote_album FROM tracks WHERE album_key = ?1 AND remote_album IS NOT NULL \
                 LIMIT 1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Song ratings this server hasn't heard yet.
    pub fn pending_song_ratings(&self, source_id: i64) -> Result<Vec<PendingRating>> {
        let mut st = self.conn.prepare(
            "SELECT r.path, t.remote_id, r.stars FROM song_ratings r \
             JOIN tracks t ON t.path = r.path \
             WHERE r.dirty = 1 AND t.source_id = ?1 AND t.remote_id IS NOT NULL",
        )?;
        let rows = st.query_map([source_id], |r| {
            Ok(PendingRating {
                key: r.get(0)?,
                remote_id: r.get(1)?,
                stars: r.get::<_, i64>(2)?.clamp(0, 5) as u8,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Album ratings this server hasn't heard yet.
    pub fn pending_album_ratings(&self, source_id: i64) -> Result<Vec<PendingRating>> {
        let mut st = self.conn.prepare(
            "SELECT r.key, (SELECT remote_album FROM tracks t WHERE t.album_key = r.key \
                AND t.source_id = ?1 AND t.remote_album IS NOT NULL LIMIT 1) AS rid, r.stars \
             FROM album_ratings r WHERE r.dirty = 1 AND rid IS NOT NULL",
        )?;
        let rows = st.query_map([source_id], |r| {
            Ok(PendingRating {
                key: r.get::<_, String>(0)?.into_bytes(),
                remote_id: r.get(1)?,
                stars: r.get::<_, i64>(2)?.clamp(0, 5) as u8,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The server has a rating now: clear the mark (and the row, for a
    /// rating taken away). Only if nothing changed since it was sent.
    pub fn rating_sent(&self, song: bool, key: &[u8], stars: u8) -> Result<()> {
        let (table, k): (&str, rusqlite::types::Value) = if song {
            ("song_ratings", key.to_vec().into())
        } else {
            (
                "album_ratings",
                String::from_utf8_lossy(key).into_owned().into(),
            )
        };
        let col = if song { "path" } else { "key" };
        if stars == 0 {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE {col} = ?1 AND stars = 0"),
                [k],
            )?;
        } else {
            self.conn.execute(
                &format!("UPDATE {table} SET dirty = 0 WHERE {col} = ?1 AND stars = ?2"),
                params![k, stars as i64],
            )?;
        }
        Ok(())
    }

    /// What a server says a song is rated, unless you changed it here and
    /// the server hasn't heard yet.
    pub(crate) fn server_song_rating(
        tx: &rusqlite::Transaction,
        path: &Path,
        stars: Option<u8>,
    ) -> Result<()> {
        let dirty: Option<i64> = tx
            .query_row(
                "SELECT dirty FROM song_ratings WHERE path = ?1",
                [path_bytes(path)],
                |r| r.get(0),
            )
            .optional()?;
        if dirty == Some(1) {
            return Ok(());
        }
        match stars {
            Some(s) if s > 0 => {
                tx.execute(
                    "INSERT INTO song_ratings (path, stars, rated_at, dirty) VALUES (?1, ?2, ?3, 0) \
                     ON CONFLICT(path) DO UPDATE SET stars = ?2",
                    params![path_bytes(path), s.min(5) as i64, now_secs()],
                )?;
            }
            _ => {
                tx.execute(
                    "DELETE FROM song_ratings WHERE path = ?1",
                    [path_bytes(path)],
                )?;
            }
        }
        Ok(())
    }

    /// What a server says its albums are rated, by its album ids.
    pub fn server_album_ratings(
        &mut self,
        source_id: i64,
        ratings: &[(String, Option<u8>)],
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut changed = false;
        for (rid, stars) in ratings {
            let keys: Vec<String> = {
                let mut st = tx.prepare_cached(
                    "SELECT DISTINCT album_key FROM tracks WHERE source_id = ?1 AND remote_album = ?2",
                )?;
                st.query_map(params![source_id, rid], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for key in keys {
                let have: Option<(i64, i64)> = tx
                    .query_row(
                        "SELECT stars, dirty FROM album_ratings WHERE key = ?1",
                        [&key],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if have.is_some_and(|h| h.1 == 1) {
                    continue;
                }
                let now = have.map(|h| h.0 as u8).unwrap_or(0);
                let want = stars.unwrap_or(0);
                if now == want {
                    continue;
                }
                changed = true;
                if want == 0 {
                    tx.execute("DELETE FROM album_ratings WHERE key = ?1", [&key])?;
                } else {
                    tx.execute(
                        "INSERT INTO album_ratings (key, stars, rated_at, dirty) VALUES (?1, ?2, ?3, 0) \
                         ON CONFLICT(key) DO UPDATE SET stars = ?2",
                        params![key, want as i64, now_secs()],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    // ----- Favorites -----

    pub fn favorites(&self) -> Result<Vec<Favorite>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, kind, key, label, server FROM favorites WHERE removed = 0 ORDER BY pos, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Favorite {
                id: r.get(0)?,
                kind: FavKind::parse(&r.get::<_, String>(1)?),
                key: r.get(2)?,
                label: r.get(3)?,
                server: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn favorite_id(&self, kind: FavKind, key: &[u8]) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM favorites WHERE kind = ?1 AND key = ?2 AND removed = 0",
                params![kind.as_str(), key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Star something. It goes at the end of the list. `server`: the
    /// server it lives on, which should hear about it.
    pub fn add_favorite(
        &self,
        kind: FavKind,
        key: &[u8],
        label: &str,
        server: Option<&str>,
    ) -> Result<i64> {
        let pos: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(pos), 0) + 1 FROM favorites", [], |r| {
                    r.get(0)
                })?;
        self.conn.execute(
            "INSERT INTO favorites (kind, key, label, pos, added_at, server, dirty, removed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0) \
             ON CONFLICT(kind, key) DO UPDATE SET label = ?3, pos = ?4, server = ?6, dirty = ?7, \
               removed = 0",
            params![
                kind.as_str(),
                key,
                label,
                pos,
                now_secs(),
                server,
                server.is_some() as i64
            ],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM favorites WHERE kind = ?1 AND key = ?2",
            params![kind.as_str(), key],
            |r| r.get(0),
        )?)
    }

    /// Unstar. One its server knows about is kept, hidden, until that
    /// server has heard.
    pub fn remove_favorite(&self, id: i64) -> Result<()> {
        let server: Option<Option<String>> = self
            .conn
            .query_row("SELECT server FROM favorites WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        match server {
            Some(Some(_)) => {
                self.conn.execute(
                    "UPDATE favorites SET removed = 1, dirty = 1 WHERE id = ?1",
                    [id],
                )?;
            }
            Some(None) => {
                self.conn
                    .execute("DELETE FROM favorites WHERE id = ?1", [id])?;
            }
            None => {}
        }
        Ok(())
    }

    /// Move a favorite `by` places along the list.
    pub fn move_favorite(&mut self, id: i64, by: i64) -> Result<()> {
        let list = self.favorites()?;
        let Some(i) = list.iter().position(|f| f.id == id) else {
            return Ok(());
        };
        let j = (i as i64 + by).clamp(0, list.len() as i64 - 1) as usize;
        let mut ids: Vec<i64> = list.iter().map(|f| f.id).collect();
        let x = ids.remove(i);
        ids.insert(j, x);
        let tx = self.conn.transaction()?;
        for (p, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE favorites SET pos = ?2 WHERE id = ?1",
                params![id, p as i64 + 1],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stars this server hasn't heard yet.
    pub fn pending_stars(&self, server: &str, source_id: i64) -> Result<Vec<PendingStar>> {
        let mut st = self.conn.prepare(
            "SELECT id, kind, key, removed FROM favorites WHERE dirty = 1 AND server = ?1",
        )?;
        let rows: Vec<(i64, String, Vec<u8>, i64)> = st
            .query_map([server], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        for (id, kind, key, removed) in rows {
            let kind = FavKind::parse(&kind);
            let remote_id = if kind == FavKind::Album {
                self.conn
                    .query_row(
                        "SELECT remote_album FROM tracks WHERE album_key = ?1 AND source_id = ?2 \
                         AND remote_album IS NOT NULL LIMIT 1",
                        params![String::from_utf8_lossy(&key), source_id],
                        |r| r.get(0),
                    )
                    .optional()?
            } else {
                None
            };
            out.push(PendingStar {
                id,
                kind,
                key,
                remote_id,
                on: removed == 0,
            });
        }
        Ok(out)
    }

    /// The server heard: an unstar is gone for good, a star is settled.
    pub fn star_sent(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM favorites WHERE id = ?1 AND removed = 1", [id])?;
        self.conn
            .execute("UPDATE favorites SET dirty = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// What a server has starred: albums (by their keys here, with a
    /// label) and artists. New ones join the list; ones it had and has
    /// let go of leave it. Anything you changed here and the server
    /// hasn't heard yet stays as you left it. True if the list changed.
    pub fn server_starred(
        &mut self,
        server: &str,
        albums: &[(String, String)],
        artists: &[String],
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut changed = false;
        let mut pos: i64 =
            tx.query_row("SELECT COALESCE(MAX(pos), 0) + 1 FROM favorites", [], |r| {
                r.get(0)
            })?;
        let wanted: Vec<(FavKind, Vec<u8>, String)> = albums
            .iter()
            .map(|(k, l)| (FavKind::Album, k.as_bytes().to_vec(), l.clone()))
            .chain(
                artists
                    .iter()
                    .map(|a| (FavKind::Artist, a.as_bytes().to_vec(), a.clone())),
            )
            .collect();
        for (kind, key, label) in &wanted {
            let have: Option<(Option<String>, i64, i64)> = tx
                .query_row(
                    "SELECT server, dirty, removed FROM favorites WHERE kind = ?1 AND key = ?2",
                    params![kind.as_str(), key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            match have {
                None => {
                    tx.execute(
                        "INSERT INTO favorites (kind, key, label, pos, added_at, server) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![kind.as_str(), key, label, pos, now_secs(), server],
                    )?;
                    pos += 1;
                    changed = true;
                }
                // Yours here already: now it's in step with this server.
                Some((None, _, 0)) => {
                    tx.execute(
                        "UPDATE favorites SET server = ?3 WHERE kind = ?1 AND key = ?2",
                        params![kind.as_str(), key, server],
                    )?;
                }
                _ => {}
            }
        }
        // Ones this server had starred and doesn't now.
        let mine: Vec<(i64, String, Vec<u8>)> = {
            let mut st = tx.prepare(
                "SELECT id, kind, key FROM favorites WHERE server = ?1 AND dirty = 0 \
                 AND kind IN ('album', 'artist')",
            )?;
            st.query_map([server], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, kind, key) in mine {
            let kind = FavKind::parse(&kind);
            if !wanted.iter().any(|(k, w, _)| *k == kind && *w == key) {
                tx.execute("DELETE FROM favorites WHERE id = ?1", [id])?;
                changed = true;
            }
        }
        tx.commit()?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_and_favorites() {
        let dir = tempfile::tempdir().unwrap();
        let mut lib = Library::open(&dir.path().join("l.db")).unwrap();
        let p = Path::new("/music/a.flac");
        assert_eq!(lib.song_rating(p).unwrap(), 0);
        lib.set_song_rating(p, 4, false).unwrap();
        assert_eq!(lib.song_rating(p).unwrap(), 4);
        lib.set_song_rating(p, 0, false).unwrap();
        assert_eq!(lib.song_rating(p).unwrap(), 0);
        lib.set_album_rating("k1", 9, false).unwrap();
        assert_eq!(lib.album_rating("k1").unwrap(), 5, "capped");

        let a = lib
            .add_favorite(FavKind::Folder, b"/music/Jazz", "Jazz", None)
            .unwrap();
        let b = lib
            .add_favorite(FavKind::Artist, b"Nirvana", "Nirvana", None)
            .unwrap();
        let c = lib
            .add_favorite(FavKind::Album, b"k1", "Bleach", Some("Home"))
            .unwrap();
        let ids = |l: &Library| {
            l.favorites()
                .unwrap()
                .iter()
                .map(|f| f.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&lib), vec![a, b, c]);
        lib.move_favorite(c, -2).unwrap();
        assert_eq!(ids(&lib), vec![c, a, b]);
        // A local one goes at once; a server one waits for its server.
        lib.remove_favorite(a).unwrap();
        lib.remove_favorite(c).unwrap();
        assert_eq!(ids(&lib), vec![b]);
        let p = lib.pending_stars("Home", 0).unwrap();
        assert_eq!(p.len(), 1);
        assert!(!p[0].on);
        lib.star_sent(c).unwrap();
        assert!(lib.pending_stars("Home", 0).unwrap().is_empty());
        // The server's list: new ones join, and a later sync without
        // them takes them away again. Yours stay.
        assert!(
            lib.server_starred(
                "Home",
                &[("k2".into(), "In Utero".into())],
                &["Nirvana".into()]
            )
            .unwrap()
        );
        let names: Vec<String> = lib
            .favorites()
            .unwrap()
            .into_iter()
            .map(|f| f.label)
            .collect();
        assert_eq!(names, vec!["Nirvana", "In Utero"]);
        assert!(lib.server_starred("Home", &[], &[]).unwrap());
        assert!(
            lib.favorites().unwrap().is_empty(),
            "both were the server's now"
        );
    }
}
