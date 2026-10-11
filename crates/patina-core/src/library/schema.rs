//! Database layout and migrations. Bump `VERSION` and add a step to
//! `migrate` for every schema change; never edit an old step.

use rusqlite::Connection;

pub const VERSION: i64 = 2;

/// Paths and dirs are stored as BLOBs of their raw bytes, since Linux
/// filenames don't have to be UTF-8 and a lossy copy can't be opened.
const V1: &str = r#"
CREATE TABLE sources (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,          -- 'local' now, 'subsonic' later
    root        BLOB NOT NULL UNIQUE,
    last_scan   INTEGER
);

CREATE TABLE tracks (
    id            INTEGER PRIMARY KEY,
    source_id     INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    path          BLOB NOT NULL UNIQUE,
    dir           BLOB NOT NULL,
    filename      TEXT NOT NULL,
    mtime         INTEGER NOT NULL,
    size          INTEGER NOT NULL,
    title         TEXT,
    artist        TEXT,
    album_artist  TEXT,
    album         TEXT,
    album_key     TEXT NOT NULL,
    track_no      INTEGER,
    disc_no       INTEGER,
    year          INTEGER,
    genre         TEXT,
    duration_ms   INTEGER,
    codec         TEXT,
    sample_rate   INTEGER,
    bits          INTEGER,
    channels      INTEGER,
    kbps          INTEGER,
    lossless      INTEGER NOT NULL DEFAULT 0,
    rg_track_gain REAL,
    rg_track_peak REAL,
    rg_album_gain REAL,
    rg_album_peak REAL,
    added_at      INTEGER NOT NULL,
    play_count    INTEGER NOT NULL DEFAULT 0,
    last_played   INTEGER
);
CREATE INDEX tracks_dir ON tracks(dir);
CREATE INDEX tracks_album ON tracks(album_key);
CREATE INDEX tracks_artist ON tracks(artist COLLATE NOCASE);
CREATE INDEX tracks_source ON tracks(source_id);

-- One row per genre per track. A track tagged "Blues Rock; Post-Grunge"
-- gets two. genre_norm is the matching form: lowercase, hyphenated.
CREATE TABLE track_genres (
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    genre_norm  TEXT NOT NULL,
    genre       TEXT NOT NULL,
    PRIMARY KEY (track_id, genre_norm)
);
CREATE INDEX track_genres_norm ON track_genres(genre_norm);

-- Derived from tracks after every scan. Never edited directly.
CREATE TABLE albums (
    key           TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    artist        TEXT NOT NULL,
    year          INTEGER,
    genre         TEXT,
    track_count   INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,
    added_at      INTEGER NOT NULL,
    play_count    INTEGER NOT NULL,
    dir           BLOB NOT NULL,
    lossless      INTEGER NOT NULL
);
CREATE INDEX albums_artist ON albums(artist COLLATE NOCASE);

CREATE TABLE album_genres (
    album_key   TEXT NOT NULL,
    genre_norm  TEXT NOT NULL,
    PRIMARY KEY (album_key, genre_norm)
);
CREATE INDEX album_genres_norm ON album_genres(genre_norm);

CREATE TABLE playlists (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- Items point at tracks by URI rather than row id, so a playlist survives
-- a rescan that re-creates rows. Local files are "file:" plus the path
-- bytes; Subsonic songs get their own scheme later.
CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    pos         INTEGER NOT NULL,
    uri         BLOB NOT NULL,
    PRIMARY KEY (playlist_id, pos)
);
"#;

/// Podcasts. Feed URLs live in the config; this caches what the feeds
/// said, plus your listening state, which the feeds can't know.
const V2: &str = r#"
CREATE TABLE podcasts (
    id           INTEGER PRIMARY KEY,
    feed_url     TEXT NOT NULL UNIQUE,
    title        TEXT,
    author       TEXT,
    description  TEXT,
    image_url    TEXT,
    link         TEXT,
    last_fetch   INTEGER,
    last_error   TEXT
);

CREATE TABLE episodes (
    id            INTEGER PRIMARY KEY,
    podcast_id    INTEGER NOT NULL REFERENCES podcasts(id) ON DELETE CASCADE,
    guid          TEXT NOT NULL,
    title         TEXT NOT NULL,
    published     INTEGER,
    duration_ms   INTEGER,
    audio_url     TEXT NOT NULL,
    mime          TEXT,
    size          INTEGER,
    description   TEXT,
    position_ms   INTEGER NOT NULL DEFAULT 0,
    played        INTEGER NOT NULL DEFAULT 0,
    download_path BLOB,
    UNIQUE (podcast_id, guid)
);
CREATE INDEX episodes_podcast ON episodes(podcast_id, published);
"#;

pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(())
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current >= VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    if current < 1 {
        tx.execute_batch(V1)?;
    }
    if current < 2 {
        tx.execute_batch(V2)?;
    }
    tx.pragma_update(None, "user_version", VERSION)?;
    tx.commit()
}
