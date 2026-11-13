//! Time the library's views on a made-up library as big as a large real
//! one (85,000 songs by default), to find what's slow on first open.
//! `cargo run --release --example bigbench -- [songs]`
//!
//! The database is written straight with SQL rather than scanned, so it
//! takes seconds instead of the hours a real scan of that many files would.

use std::path::{Path, PathBuf};
use std::time::Instant;

use patina_core::library::{AlbumSort, ArtistMode, Library, SongSort};

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("{label:<44} {:>9.2?}", t.elapsed());
    r
}

fn fill(db: &Path, root: &Path, songs: usize) {
    drop(Library::open(db).expect("open db"));
    let mut c = rusqlite::Connection::open(db).unwrap();
    let tx = c.transaction().unwrap();
    tx.execute(
        "INSERT INTO sources (id, kind, root, last_scan) VALUES (1, 'local', ?1, 0)",
        [root.as_os_str().as_encoded_bytes()],
    )
    .unwrap();
    let per_album = 10;
    let albums_per_artist = 5;
    let albums = songs.div_ceil(per_album);
    let genres = [
        "Rock", "Pop", "Jazz", "Blues", "Folk", "Metal", "Punk", "Soul", "Funk", "Disco", "House",
        "Techno", "Ambient", "Grunge", "Indie", "Country", "Reggae", "Ska", "Swing", "Gospel",
    ];
    {
        let mut tr = tx
            .prepare(
                "INSERT INTO tracks (id, source_id, path, dir, filename, mtime, size, title, artist, \
                 album_artist, album, album_key, track_no, year, genre, duration_ms, codec, \
                 sample_rate, bits, channels, lossless, added_at, play_count) \
                 VALUES (?1, 1, ?2, ?3, ?4, 0, 1000, ?5, ?6, ?6, ?7, ?8, ?9, ?10, ?11, 200000, \
                 'flac', 44100, 16, 2, 1, ?12, ?13)",
            )
            .unwrap();
        let mut tg = tx
            .prepare("INSERT INTO track_genres (track_id, genre_norm, genre) VALUES (?1, ?2, ?3)")
            .unwrap();
        let mut al = tx
            .prepare(
                "INSERT INTO albums (key, name, artist, year, genre, track_count, duration_ms, \
                 added_at, play_count, dir, lossless) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 1)",
            )
            .unwrap();
        let mut ag = tx
            .prepare("INSERT INTO album_genres (album_key, genre_norm) VALUES (?1, ?2)")
            .unwrap();
        let mut id = 0i64;
        for a in 0..albums {
            let artist = format!("Artist {:04}", a / albums_per_artist);
            let album = format!("Album {a:05}");
            let key = format!("{}\u{1f}{}", artist.to_lowercase(), album.to_lowercase());
            let dir = root.join(&artist).join(&album);
            let genre = genres[a % genres.len()];
            let year = 1960 + (a % 60) as i64;
            let n = per_album.min(songs - a * per_album);
            for t in 0..n {
                id += 1;
                let file = format!("{:02} Song {id}.flac", t + 1);
                let path = dir.join(&file);
                tr.execute(rusqlite::params![
                    id,
                    path.as_os_str().as_encoded_bytes(),
                    dir.as_os_str().as_encoded_bytes(),
                    file,
                    format!("Song {id}"),
                    artist,
                    album,
                    key,
                    t as i64 + 1,
                    year,
                    genre,
                    id,
                    id % 7,
                ])
                .unwrap();
                tg.execute(rusqlite::params![id, genre.to_lowercase(), genre])
                    .unwrap();
            }
            al.execute(rusqlite::params![
                key,
                album,
                artist,
                year,
                genre,
                n as i64,
                n as i64 * 200000,
                a as i64,
                dir.as_os_str().as_encoded_bytes(),
            ])
            .unwrap();
            ag.execute(rusqlite::params![key, genre.to_lowercase()])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    c.execute_batch("ANALYZE").ok();
}

fn main() {
    let songs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(85_000);
    // BENCH_DB keeps the database there, for timing the window on it.
    let keep = std::env::var_os("BENCH_DB").map(std::path::PathBuf::from);
    let db = keep.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("patina-bench-{}.db", std::process::id()))
    });
    if keep.is_some() {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", db.display()));
        }
    }
    let root = PathBuf::from("/music");
    time(&format!("fill {songs} songs"), || fill(&db, &root, songs));

    let lib = time("open", || Library::open(&db).unwrap());
    time("Folders (sources)", || lib.sources().unwrap().len());
    let n = time("Folder: the root", || lib.folder(&root).unwrap().len());
    println!("  {n} entries");
    time("Folder: one artist", || {
        lib.folder(&root.join("Artist 0100")).unwrap().len()
    });
    time("Folder: one album", || {
        lib.folder(&root.join("Artist 0100/Album 00500"))
            .unwrap()
            .len()
    });
    let n = time("Artists (album artist)", || {
        lib.artists(ArtistMode::AlbumArtist).unwrap().len()
    });
    println!("  {n} artists");
    time("Artists (track artist)", || {
        lib.artists(ArtistMode::TrackArtist).unwrap().len()
    });
    for s in [
        AlbumSort::Name,
        AlbumSort::Artist,
        AlbumSort::Year,
        AlbumSort::RecentlyAdded,
    ] {
        time(&format!("Albums by {}", s.label()), || {
            lib.albums(s).unwrap().len()
        });
    }
    time("Songs by title", || {
        lib.songs(SongSort::Title, false).unwrap().len()
    });
    time("Songs by title, again", || {
        lib.songs(SongSort::Title, false).unwrap().len()
    });
    let n = time("Genres", || lib.genres().unwrap().len());
    println!("  {n} genres");
    time("Most listened (top 5s)", || {
        (
            lib.top_tracks(5).unwrap().len(),
            lib.top_albums(5).unwrap().len(),
            lib.top_artists(ArtistMode::AlbumArtist, 5).unwrap().len(),
        )
    });
    time("Search \"song 4\"", || {
        lib.search("song 4", 200).unwrap().tracks.len()
    });
    time("Favorites", || lib.favorites().unwrap().len());
    time("Playlists", || lib.playlists().unwrap().len());
    time("Car: albums tab", || {
        patina_core::browse::auto::children(&lib, "albums").len()
    });
    time("Car: artists tab", || {
        patina_core::browse::auto::children(&lib, "artists").len()
    });
    time("Car: folders tab", || {
        patina_core::browse::auto::children(&lib, "folders").len()
    });
    drop(lib);
    if keep.is_some() {
        return;
    }
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", db.display()));
    }
}
