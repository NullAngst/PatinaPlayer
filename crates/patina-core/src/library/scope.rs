//! Which source the library browses: everything, the local folders, or one
//! server. Browsing reads through two temporary views, `s_tracks` and
//! `s_albums`, which hold only the albums in scope. With every source in
//! scope, an album more than one source has shows once, from the source
//! first in the source order.
//!
//! The scope is set for the whole process (Android Auto has a connection
//! of its own and follows it), or for one connection, which tests use.
//! Each connection fills its own list of albums in scope, again whenever
//! the scope or the library changes.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;

/// What to browse.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Pick {
    /// Every source together.
    #[default]
    All,
    /// The local folders.
    Local,
    /// One server, by its name.
    Server(String),
}

impl Pick {
    /// As the config keeps it: "all", "local" or "server:Name".
    pub fn to_config(&self) -> String {
        match self {
            Pick::All => "all".into(),
            Pick::Local => "local".into(),
            Pick::Server(n) => format!("server:{n}"),
        }
    }

    pub fn from_config(s: &str) -> Self {
        let s = s.trim();
        if let Some(n) = s.strip_prefix("server:") {
            Pick::Server(n.to_string())
        } else if s.eq_ignore_ascii_case("local") {
            Pick::Local
        } else {
            Pick::All
        }
    }

    /// For the picker and the status bar.
    pub fn label(&self) -> String {
        match self {
            Pick::All => "All sources".into(),
            Pick::Local => "Local".into(),
            Pick::Server(n) => n.clone(),
        }
    }
}

/// The scope and the order sources win in when they share an album.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub pick: Pick,
    /// "local", a kind of server ("plex", "subsonic"...) or a server's
    /// name, first wins. Sources not listed come after, oldest first.
    pub order: Vec<String>,
    /// Servers that can't be reached now. They lose to every other
    /// source, so a shared album shows from one that can play it.
    pub away: Vec<String>,
}

/// The order a new config starts with: your own files, then Plex.
pub fn default_order() -> Vec<String> {
    vec!["local".into(), "plex".into()]
}

/// Read a comma list, as typed into Settings.
pub fn parse_order(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

static GLOBAL: RwLock<Option<Scope>> = RwLock::new(None);
static GLOBAL_GEN: AtomicU64 = AtomicU64::new(1);

/// Set the scope every connection follows, unless it has its own.
pub fn set_global(scope: Scope) {
    let mut g = GLOBAL.write().unwrap_or_else(|e| e.into_inner());
    if g.as_ref() != Some(&scope) {
        *g = Some(scope);
        GLOBAL_GEN.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn global() -> Scope {
    GLOBAL
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

pub(crate) fn global_gen() -> u64 {
    GLOBAL_GEN.load(Ordering::Acquire)
}

/// Where a source sorts: by `order`, and after everything else if it's
/// away.
fn rank(scope: &Scope, id: i64, kind: &str, name: Option<&str>) -> i64 {
    let hit = scope.order.iter().position(|o| {
        o.eq_ignore_ascii_case(kind) || name.is_some_and(|n| o.eq_ignore_ascii_case(n))
    });
    let r = match hit {
        Some(i) => i as i64,
        None => 1000 + id,
    };
    if name.is_some_and(|n| scope.away.iter().any(|a| a == n)) {
        r + 1_000_000
    } else {
        r
    }
}

/// The SQL that names an album's source: "local", or "srv" and the
/// server's source id, the way server album keys start.
const ALBUM_SRC: &str = "CASE WHEN key LIKE 'srv%' \
     THEN substr(key, 1, instr(key, char(31)) - 1) ELSE 'local' END";

/// Make the temporary tables and views this connection browses through.
pub(crate) fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS scope_keys (key TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS scope_rank (src TEXT PRIMARY KEY, rank INTEGER NOT NULL);
         CREATE TEMP VIEW IF NOT EXISTS s_albums AS
             SELECT * FROM main.albums WHERE key IN (SELECT key FROM temp.scope_keys);
         CREATE TEMP VIEW IF NOT EXISTS s_tracks AS
             SELECT * FROM main.tracks WHERE album_key IN (SELECT key FROM temp.scope_keys);",
    )
}

/// Fill the list of albums in scope. Returns false when everything is in
/// scope and nothing is hidden, so the plain tables can be read instead.
pub(crate) fn fill(conn: &Connection, scope: &Scope) -> rusqlite::Result<bool> {
    // Only temporary tables change, so no transaction: a browse can run
    // inside someone else's.
    let tx = conn;
    tx.execute("DELETE FROM temp.scope_keys", [])?;
    let filtered = match &scope.pick {
        Pick::Local => {
            tx.execute(
                "INSERT INTO temp.scope_keys SELECT key FROM main.albums WHERE key NOT LIKE 'srv%'",
                [],
            )?;
            true
        }
        Pick::Server(name) => {
            let root = crate::remote::root(name);
            let root = crate::library::path_bytes(&root);
            let id: Option<i64> = tx
                .query_row("SELECT id FROM main.sources WHERE root = ?1", [root], |r| {
                    r.get(0)
                })
                .ok();
            if let Some(id) = id {
                let prefix = format!("srv{id}\u{1f}");
                tx.execute(
                    "INSERT INTO temp.scope_keys SELECT key FROM main.albums \
                     WHERE substr(key, 1, length(?1)) = ?1",
                    [prefix],
                )?;
            }
            true
        }
        Pick::All => {
            // Rank the sources, then keep, of each artist and album name,
            // only the copies from the best-ranked source that has it. Two
            // albums of one source never hide each other.
            tx.execute("DELETE FROM temp.scope_rank", [])?;
            let mut local_rank = rank(scope, 0, "local", None);
            let mut groups = 0;
            {
                let mut st = tx.prepare("SELECT id, kind, root FROM main.sources")?;
                let rows = st.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                })?;
                let mut ins = tx.prepare(
                    "INSERT OR REPLACE INTO temp.scope_rank (src, rank) VALUES (?1, ?2)",
                )?;
                let mut any_local = false;
                for row in rows {
                    let (id, kind, root) = row?;
                    if kind == "local" {
                        any_local = true;
                        local_rank = local_rank.min(rank(scope, id, "local", None));
                        continue;
                    }
                    let root = crate::library::bytes_path(root);
                    let name = crate::remote::server_of(&root);
                    ins.execute(rusqlite::params![
                        format!("srv{id}"),
                        rank(scope, id, &kind, name)
                    ])?;
                    groups += 1;
                }
                if any_local {
                    groups += 1;
                }
                ins.execute(rusqlite::params!["local", local_rank])?;
            }
            if groups < 2 {
                // One source: nothing to hide.
                return Ok(false);
            }
            let sql = format!(
                "INSERT INTO temp.scope_keys
                 SELECT key FROM (
                     SELECT a.key, r.rank,
                            MIN(r.rank) OVER (PARTITION BY lower(a.artist), lower(a.name)) AS best
                     FROM (SELECT key, artist, name, {ALBUM_SRC} AS src FROM main.albums) a
                     LEFT JOIN temp.scope_rank r ON r.src = a.src)
                 WHERE rank IS NULL OR rank = best"
            );
            tx.execute(&sql, [])?;
            true
        }
    };
    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{AlbumSort, ArtistMode, Library, SongSort};
    use std::collections::HashSet;
    use std::path::Path;

    /// A song: its source, path, artist, album and key.
    fn song(lib: &Library, src: i64, path: &str, artist: &str, album: &str, key: &str) {
        let p = Path::new(path);
        lib.conn
            .execute(
                "INSERT INTO tracks (source_id, path, dir, filename, mtime, size, title, artist, \
                 album_artist, album, album_key, track_no, duration_ms, added_at) \
                 VALUES (?1, ?2, ?3, ?4, 0, 1, ?4, ?5, ?5, ?6, ?7, 1, 1000, 0)",
                rusqlite::params![
                    src,
                    path.as_bytes(),
                    p.parent().unwrap().as_os_str().as_encoded_bytes(),
                    p.file_name().unwrap().to_string_lossy(),
                    artist,
                    album,
                    key
                ],
            )
            .unwrap();
    }

    fn library() -> Library {
        let lib = Library::open_in_memory().unwrap();
        for (id, kind, root) in [
            (1, "local", "/music"),
            (2, "plex", "server://Home"),
            (3, "subsonic", "server://Nav"),
        ] {
            lib.conn
                .execute(
                    "INSERT INTO sources (id, kind, root) VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, kind, root.as_bytes()],
                )
                .unwrap();
        }
        // Bleach is everywhere; Nevermind on Plex and Navidrome; In Utero
        // only on Navidrome.
        song(
            &lib,
            1,
            "/music/Nirvana/Bleach/01 Blew.flac",
            "Nirvana",
            "Bleach",
            "aa\u{1f}nirvana\u{1f}bleach",
        );
        song(
            &lib,
            2,
            "server://Home/a1/1",
            "Nirvana",
            "Bleach",
            "srv2\u{1f}a1",
        );
        song(
            &lib,
            2,
            "server://Home/a2/1",
            "Nirvana",
            "Nevermind",
            "srv2\u{1f}a2",
        );
        song(
            &lib,
            3,
            "server://Nav/b1/1",
            "Nirvana",
            "Bleach",
            "srv3\u{1f}b1",
        );
        song(
            &lib,
            3,
            "server://Nav/b2/1",
            "Nirvana",
            "Nevermind",
            "srv3\u{1f}b2",
        );
        song(
            &lib,
            3,
            "server://Nav/b3/1",
            "Nirvana",
            "In Utero",
            "srv3\u{1f}b3",
        );
        let keys: HashSet<String> = [
            "aa\u{1f}nirvana\u{1f}bleach",
            "srv2\u{1f}a1",
            "srv2\u{1f}a2",
            "srv3\u{1f}b1",
            "srv3\u{1f}b2",
            "srv3\u{1f}b3",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        crate::library::scan::rebuild_albums(&lib.conn, &keys).unwrap();
        lib
    }

    fn keys(lib: &Library) -> Vec<String> {
        let mut k: Vec<String> = lib
            .albums(AlbumSort::Name)
            .unwrap()
            .into_iter()
            .map(|a| a.key)
            .collect();
        k.sort();
        k
    }

    fn scope(pick: Pick, order: &[&str]) -> Scope {
        Scope {
            pick,
            order: order.iter().map(|s| s.to_string()).collect(),
            away: Vec::new(),
        }
    }

    #[test]
    fn unscoped_sees_everything() {
        let lib = library();
        assert_eq!(keys(&lib).len(), 6);
        assert_eq!(lib.songs(SongSort::Title, false).unwrap().len(), 6);
    }

    #[test]
    fn all_sources_show_each_album_once() {
        let lib = library();
        lib.set_scope(scope(Pick::All, &["local", "plex"]));
        // Bleach from the folders, Nevermind from Plex, In Utero from the
        // only source that has it.
        assert_eq!(
            keys(&lib),
            [
                "aa\u{1f}nirvana\u{1f}bleach",
                "srv2\u{1f}a2",
                "srv3\u{1f}b3"
            ]
        );
        assert_eq!(lib.songs(SongSort::Title, false).unwrap().len(), 3);
        // Another order, another winner: a server named first beats a kind.
        lib.set_scope(scope(Pick::All, &["Nav", "local"]));
        assert_eq!(keys(&lib), ["srv3\u{1f}b1", "srv3\u{1f}b2", "srv3\u{1f}b3"]);
        let artists = lib.artists(ArtistMode::AlbumArtist).unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].albums, 3);
        // The winner unreachable: the next source that has it shows.
        let mut s = scope(Pick::All, &["Nav", "local"]);
        s.away = vec!["Nav".into()];
        lib.set_scope(s);
        assert_eq!(
            keys(&lib),
            [
                "aa\u{1f}nirvana\u{1f}bleach",
                "srv2\u{1f}a2",
                "srv3\u{1f}b3"
            ]
        );
    }

    #[test]
    fn one_source() {
        let lib = library();
        lib.set_scope(scope(Pick::Local, &[]));
        assert_eq!(keys(&lib), ["aa\u{1f}nirvana\u{1f}bleach"]);
        assert_eq!(lib.browse_roots().unwrap().len(), 1);
        lib.set_scope(scope(Pick::Server("Home".into()), &[]));
        assert_eq!(keys(&lib), ["srv2\u{1f}a1", "srv2\u{1f}a2"]);
        let found = lib.search("nirvana", 50).unwrap();
        assert_eq!(found.albums.len(), 2);
        assert_eq!(found.tracks.len(), 2);
        // Smart playlists see only the source too.
        let q = crate::library::smart::parse("artist ~ nirvana").unwrap();
        assert_eq!(lib.query_tracks(&q, 0).unwrap().len(), 2);
        assert_eq!(lib.query_count(&q).unwrap(), 2);
        // A server not synced yet shows nothing, rather than everything.
        lib.set_scope(scope(Pick::Server("Gone".into()), &[]));
        assert!(keys(&lib).is_empty());
        // Lookups the queue and playlists make aren't scoped.
        assert!(
            lib.track_by_path(Path::new("/music/Nirvana/Bleach/01 Blew.flac"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn follows_changes_to_the_library() {
        let lib = library();
        lib.set_scope(scope(Pick::Server("Home".into()), &[]));
        assert_eq!(keys(&lib).len(), 2);
        song(
            &lib,
            2,
            "server://Home/a3/1",
            "Hole",
            "Live Through This",
            "srv2\u{1f}a3",
        );
        let k: HashSet<String> = ["srv2\u{1f}a3".to_string()].into_iter().collect();
        crate::library::scan::rebuild_albums(&lib.conn, &k).unwrap();
        assert_eq!(keys(&lib).len(), 3);
    }

    #[test]
    fn config_values() {
        for p in [Pick::All, Pick::Local, Pick::Server("Home Plex".into())] {
            assert_eq!(Pick::from_config(&p.to_config()), p);
        }
        assert_eq!(parse_order(" local, plex ,, Nav"), ["local", "plex", "Nav"]);
    }
}
