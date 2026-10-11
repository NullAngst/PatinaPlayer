//! Index folders into a throwaway database and report how long it took.
//! `cargo run --release --example scan -- ~/Music`

use std::path::PathBuf;
use std::time::Instant;

use patina_core::library::{AlbumSort, Library, SongSort};

fn main() {
    let roots: Vec<PathBuf> = std::env::args()
        .skip(1)
        .map(|a| std::fs::canonicalize(&a).unwrap_or_else(|_| PathBuf::from(a)))
        .collect();
    if roots.is_empty() {
        eprintln!("usage: scan DIR...");
        std::process::exit(2);
    }
    let db = std::env::temp_dir().join(format!("patina-scan-{}.db", std::process::id()));
    let mut lib = Library::open(&db).expect("open db");
    let t = Instant::now();
    let st = lib.index_folders(&roots).expect("scan");
    println!("first scan: {st:?} in {:.2?}", t.elapsed());
    let t = Instant::now();
    let st = lib.index_folders(&roots).expect("scan");
    println!("rescan, nothing changed: {st:?} in {:.2?}", t.elapsed());
    let t = Instant::now();
    let n = lib.songs(SongSort::Title, false).unwrap().len();
    println!("Songs view query: {n} rows in {:.2?}", t.elapsed());
    let t = Instant::now();
    let n = lib.albums(AlbumSort::Name).unwrap().len();
    println!("Albums view query: {n} rows in {:.2?}", t.elapsed());
    let t = Instant::now();
    let r = lib.search("artist 4", 200).unwrap();
    println!("search: {} tracks in {:.2?}", r.tracks.len(), t.elapsed());
    let _ = std::fs::remove_file(&db);
}
