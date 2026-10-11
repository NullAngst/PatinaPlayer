//! Library index tests against a real folder of FFmpeg-made, tagged files.
//! Needs the `ffmpeg` command on PATH.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use patina_core::library::{
    AlbumSort, ArtistMode, DirEntry, LibEvent, Library, LibraryService, SongSort,
};

fn make(path: &Path, secs: u32, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("sine=f=440:r=44100:d={secs}"));
    for (k, v) in tags {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    }
    args.push(path.to_str().unwrap().into());
    let st = Proc::new("ffmpeg")
        .args(&args)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success(), "ffmpeg failed for {}", path.display());
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap().join("Music");
    let r = |p: &str| root.join(p);
    // Proper album with album artist and two genres.
    for n in 1..=2 {
        make(
            &r(&format!("Nirvana/Bleach/0{n} Song.flac")),
            1,
            &[
                ("title", &format!("Bleach Song {n}")),
                ("artist", "Nirvana"),
                ("ALBUMARTIST", "Nirvana"),
                ("album", "Bleach"),
                ("track", &n.to_string()),
                ("date", "1989"),
                ("genre", "Grunge; Blues Rock"),
            ],
        );
    }
    // Same genre, different spelling.
    make(
        &r("Other/Thing/01.flac"),
        1,
        &[
            ("title", "Other Thing"),
            ("artist", "Someone"),
            ("album", "Thing"),
            ("genre", "blues-rock"),
        ],
    );
    // Compilation with no album artist: groups by folder.
    make(
        &r("Comp/01.mp3"),
        1,
        &[("title", "One"), ("artist", "Band A"), ("album", "Comp")],
    );
    make(
        &r("Comp/02.mp3"),
        1,
        &[("title", "Two"), ("artist", "Band B"), ("album", "Comp")],
    );
    // Two discs in disc folders, no album artist.
    make(
        &r("Double/CD1/01.flac"),
        1,
        &[
            ("title", "D1"),
            ("artist", "X"),
            ("album", "Double"),
            ("disc", "1"),
        ],
    );
    make(
        &r("Double/CD2/01.flac"),
        1,
        &[
            ("title", "D2"),
            ("artist", "X"),
            ("album", "Double"),
            ("disc", "2"),
        ],
    );
    // No tags at all.
    make(&r("Loose/raw take.wav"), 1, &[]);
    // Hidden folder is skipped.
    make(&r(".trash/gone.flac"), 1, &[("title", "Hidden")]);
    Fixture { _dir: dir, root }
}

fn scan(lib: &mut Library, root: &Path) -> patina_core::library::scan::ScanStats {
    lib.index_folders(&[root.to_path_buf()]).unwrap()
}

#[test]
fn scan_groups_albums_artists_and_genres() {
    let fx = fixture();
    let mut lib = Library::open_in_memory().unwrap();
    let st = scan(&mut lib, &fx.root);
    assert_eq!(st.added, 8, "{st:?}");
    assert_eq!(lib.track_count().unwrap(), 8);

    let albums = lib.albums(AlbumSort::Name).unwrap();
    let names: Vec<_> = albums
        .iter()
        .map(|a| (a.name.as_str(), a.artist.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Bleach", "Nirvana"),
            ("Comp", "Various Artists"),
            ("Double", "X"),
            ("Loose", "Unknown artist"),
            ("Thing", "Someone"),
        ]
    );
    let bleach = &albums[0];
    assert_eq!(bleach.year, Some(1989));
    assert_eq!(bleach.track_count, 2);

    let double = lib.album_tracks(&albums[2].key).unwrap();
    assert_eq!(
        double.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["D1", "D2"]
    );

    let genres = lib.genres().unwrap();
    let br = genres
        .iter()
        .find(|g| g.norm == "blues-rock")
        .expect("blues-rock");
    assert_eq!(br.tracks, 3, "two spellings, one genre");
    assert_eq!(br.albums, 2);
    assert_eq!(lib.genre_albums("blues-rock").unwrap().len(), 2);

    let artists = lib.artists(ArtistMode::AlbumArtist).unwrap();
    assert!(artists.iter().any(|a| a.name == "Various Artists"));
    let track_artists = lib.artists(ArtistMode::TrackArtist).unwrap();
    assert!(track_artists.iter().any(|a| a.name == "Band A"));
    assert_eq!(
        lib.artist_tracks("Band A", ArtistMode::TrackArtist)
            .unwrap()
            .len(),
        1
    );

    // Folder view.
    let top = lib.folder(&fx.root).unwrap();
    let dirs: Vec<_> = top
        .iter()
        .filter_map(|e| match e {
            DirEntry::Dir { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(dirs, ["Comp", "Double", "Loose", "Nirvana", "Other"]);
    assert_eq!(lib.tracks_under(&fx.root.join("Double")).unwrap().len(), 2);

    // Untagged file shows its file name.
    let loose = lib.folder(&fx.root.join("Loose")).unwrap();
    match &loose[0] {
        DirEntry::Track(t) => assert_eq!(t.title, "raw take"),
        other => panic!("{other:?}"),
    }

    // Search: every word has to match somewhere.
    let res = lib.search("nirv bleach", 50).unwrap();
    assert_eq!(res.albums.len(), 1);
    assert_eq!(res.tracks.len(), 2);
    assert!(
        lib.search("100%", 50).unwrap().tracks.is_empty(),
        "wildcards are escaped"
    );

    assert_eq!(
        lib.songs(SongSort::Title, false).unwrap()[0].title,
        "Bleach Song 1"
    );
}

#[test]
fn rescan_only_touches_changes() {
    let fx = fixture();
    let mut lib = Library::open_in_memory().unwrap();
    scan(&mut lib, &fx.root);
    let again = scan(&mut lib, &fx.root);
    assert!(!again.changed(), "{again:?}");

    // Retag one file: its album moves.
    let f = fx.root.join("Other/Thing/01.flac");
    std::thread::sleep(Duration::from_millis(1100)); // mtime has 1 s granularity
    make(
        &f,
        2,
        &[
            ("title", "Other Thing"),
            ("artist", "Someone"),
            ("album", "Renamed"),
        ],
    );
    let st = scan(&mut lib, &fx.root);
    assert_eq!((st.added, st.updated, st.removed), (0, 1, 0));
    let names: Vec<_> = lib
        .albums(AlbumSort::Name)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names.contains(&"Renamed".to_string()) && !names.contains(&"Thing".to_string()));
    // The old genre left with the old tags.
    assert_eq!(lib.genre_albums("blues-rock").unwrap().len(), 1);

    std::fs::remove_dir_all(fx.root.join("Comp")).unwrap();
    let st = scan(&mut lib, &fx.root);
    assert_eq!(st.removed, 2);
    assert!(
        !lib.albums(AlbumSort::Name)
            .unwrap()
            .iter()
            .any(|a| a.name == "Comp")
    );
}

#[test]
fn playlists_and_play_counts() {
    let fx = fixture();
    let mut lib = Library::open_in_memory().unwrap();
    scan(&mut lib, &fx.root);
    let a = fx.root.join("Nirvana/Bleach/01 Song.flac");
    let b = fx.root.join("Comp/02.mp3");
    let id = lib.create_playlist("Mix").unwrap();
    lib.append_to_playlist(id, &[a.clone(), b.clone(), a.clone()])
        .unwrap();
    let items = lib.playlist_items(id).unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|(_, t)| t.is_some()));

    // Reorder: move the last item to the front.
    let mut uris: Vec<_> = items.into_iter().map(|(u, _)| u).collect();
    let last = uris.pop().unwrap();
    uris.insert(0, last);
    lib.set_playlist_items(id, &uris).unwrap();
    assert_eq!(lib.playlist_items(id).unwrap().len(), 3);

    // A file that leaves the library shows as missing, not vanished.
    std::fs::remove_file(&b).unwrap();
    scan(&mut lib, &fx.root);
    let items = lib.playlist_items(id).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items.iter().filter(|(_, t)| t.is_none()).count(), 1);

    lib.rename_playlist(id, "Renamed").unwrap();
    assert_eq!(lib.playlists().unwrap()[0].name, "Renamed");

    lib.record_play(&a).unwrap();
    lib.record_play(&a).unwrap();
    assert_eq!(lib.track_by_path(&a).unwrap().unwrap().play_count, 2);
    let most = lib.albums(AlbumSort::MostPlayed).unwrap();
    assert_eq!(most[0].name, "Bleach");

    lib.delete_playlist(id).unwrap();
    assert!(lib.playlists().unwrap().is_empty());
}

fn wait_for(svc: &LibraryService, what: &str, mut f: impl FnMut(&LibEvent) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(ev) = svc.events().recv_timeout(Duration::from_millis(200))
            && f(&ev)
        {
            return;
        }
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn watcher_picks_up_new_and_deleted_files() {
    let fx = fixture();
    let dbdir = tempfile::tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let svc = LibraryService::start(&db, vec![fx.root.clone()], true).unwrap();
    wait_for(&svc, "first scan", |e| matches!(e, LibEvent::Ready));
    assert!(svc.status().watching);

    // Write the new file outside the tree, then move it in, the way a
    // download or a file manager copy finishes.
    let tmp = fx.root.parent().unwrap().join("incoming.flac");
    make(&tmp, 1, &[("title", "Fresh"), ("album", "New One")]);
    std::fs::create_dir_all(fx.root.join("New")).unwrap();
    let dest = fx.root.join("New/fresh.flac");
    std::fs::rename(&tmp, &dest).unwrap();
    wait_for(
        &svc,
        "added file",
        |e| matches!(e, LibEvent::Changed(s) if s.added >= 1),
    );
    let lib = Library::open(&db).unwrap();
    assert!(lib.track_by_path(&dest).unwrap().is_some());

    std::fs::remove_dir_all(fx.root.join("New")).unwrap();
    wait_for(
        &svc,
        "removed file",
        |e| matches!(e, LibEvent::Changed(s) if s.removed >= 1),
    );
    assert!(lib.track_by_path(&dest).unwrap().is_none());
}
