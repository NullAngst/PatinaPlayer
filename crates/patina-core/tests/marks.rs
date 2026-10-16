//! Ratings, favorites and lyrics end to end: through the shared Browser on
//! a real indexed folder, and through the stand-in Subsonic and Ampache
//! servers for what syncs both ways. Needs the `ffmpeg` command on PATH.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use common::{Ampache, Flavor, Subsonic};
use patina_core::browse::marks::{LyricsStatus, fav_key};
use patina_core::browse::{Browser, Item, Section, SettingsInfo, SideEntry, Source, Startup};
use patina_core::config::Config;
use patina_core::library::marks::FavKind;
use patina_core::library::{AlbumSort, Library, SongSort};
use patina_core::lyrics::Origin;
use patina_core::remote::{self, Rated, ServerConfig, ServerKind, StreamFormat, sync};
use patina_core::{Backend, Command, OutputConfig, Player, PlayerConfig, TrackInfo};

fn make(path: &Path, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cmd = Proc::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
    ])
    .arg("sine=f=440:r=44100:d=3");
    for (k, v) in tags {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    let st = cmd.arg(path).status().expect("ffmpeg must be installed");
    assert!(st.success());
}

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = base.join("Music");
    for n in 1..=2 {
        make(
            &root.join(format!("Nirvana/Bleach/0{n}.flac")),
            &[
                ("title", &format!("Bleach Song {n}")),
                ("artist", "Nirvana"),
                ("album_artist", "Nirvana"),
                ("album", "Bleach"),
                ("track", &n.to_string()),
            ],
        );
    }
    make(
        &root.join("Other/Thing/01.flac"),
        &[
            ("title", "Other Thing"),
            ("artist", "Someone"),
            ("album", "Thing"),
            (
                "LYRICS",
                "[00:00.00]first line\n[00:01.50]second line\n[00:02.50]last",
            ),
        ],
    );
    let config = base.join("config.toml");
    std::fs::write(&config, "# test config\n").unwrap();
    Fx {
        _dir: dir,
        root,
        config,
    }
}

fn browser(fx: &Fx) -> Browser {
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_folders(std::slice::from_ref(&fx.root)).unwrap();
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    Browser::new(Startup {
        player,
        lib: Some(lib),
        service: None,
        pod: None,
        remote: None,
        cache: None,
        mixes: None,
        level: None,
        scrobble: None,
        config: Config {
            library: patina_core::config::LibraryConfig {
                folders: vec![fx.root.clone()],
                watch: false,
            },
            ..Config::default()
        },
        settings_info: SettingsInfo {
            config_path: fx.config.clone(),
            ..Default::default()
        },
    })
}

fn find(b: &Browser, f: impl Fn(&Item) -> bool) -> Item {
    b.level()
        .unwrap()
        .items
        .iter()
        .find(|i| f(i))
        .cloned()
        .expect("item not found")
}

#[test]
fn songs_and_albums_take_ratings_and_sort_by_them() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    let other = find(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Other Thing"),
    );
    b.rate_item(&other, 4);
    let song2 = find(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Bleach Song 2"),
    );
    b.rate_item(&song2, 2);
    // The rows show it at once.
    let other = find(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Other Thing"),
    );
    assert_eq!(Browser::item_rating(&other), Some(4));
    let lib = b.lib.as_ref().unwrap();
    let titles: Vec<String> = lib
        .songs(SongSort::Rating, false)
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(
        titles,
        vec!["Other Thing", "Bleach Song 2", "Bleach Song 1"]
    );
    // Albums: rate Bleach and it leads.
    b.switch(Section::Albums);
    let bleach = find(&b, |i| matches!(i, Item::Album(a) if a.name == "Bleach"));
    b.rate_item(&bleach, 5);
    let names: Vec<String> = b
        .lib
        .as_ref()
        .unwrap()
        .albums(AlbumSort::Rating)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec!["Bleach", "Thing"]);
    // Inside the album, the header has it; 0 takes it away.
    b.activate();
    assert_eq!(b.level_rating(), Some(5));
    b.rate_level(0);
    assert_eq!(b.level_rating(), Some(0));
    // Rated by a song playing too.
    b.player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo {
            path: fx.root.join("Nirvana/Bleach/01.flac"),
            ..Default::default()
        }],
        start: 0,
        pos: Duration::ZERO,
    });
    let end = Instant::now() + Duration::from_secs(10);
    while b.playing_rating().is_none() {
        b.poll();
        assert!(Instant::now() < end, "never saw the song playing");
        std::thread::sleep(Duration::from_millis(20));
    }
    b.rate_playing(3);
    assert_eq!(
        b.lib
            .as_ref()
            .unwrap()
            .song_rating(&fx.root.join("Nirvana/Bleach/01.flac"))
            .unwrap(),
        3
    );
    assert_eq!(b.playing_rating(), Some(3));
}

#[test]
fn favorites_star_reorder_open_and_unstar() {
    let fx = fixture();
    let mut b = browser(&fx);
    // An artist, an album and a folder.
    b.switch(Section::Artists);
    let nirvana = find(&b, |i| matches!(i, Item::Artist(a) if a.name == "Nirvana"));
    b.toggle_favorite(&b.fav_target(&nirvana).unwrap());
    b.switch(Section::Albums);
    let thing = find(&b, |i| matches!(i, Item::Album(a) if a.name == "Thing"));
    let t = b.fav_target(&thing).unwrap();
    b.toggle_favorite(&t);
    assert!(b.is_favorite(&t));
    assert_eq!(fav_key(&thing).unwrap().0, FavKind::Album);
    b.switch(Section::Folders);
    b.push(Source::Folder(fx.root.clone()));
    let other = find(
        &b,
        |i| matches!(i, Item::Dir { name, .. } if name == "Other"),
    );
    b.toggle_favorite(&b.fav_target(&other).unwrap());
    let favs = |b: &Browser| -> Vec<(i64, String)> {
        b.sidebar
            .iter()
            .filter_map(|e| match e {
                SideEntry::Favorite { id, label, .. } => Some((*id, label.clone())),
                _ => None,
            })
            .collect()
    };
    let names: Vec<String> = favs(&b).into_iter().map(|f| f.1).collect();
    assert_eq!(names, vec!["Nirvana", "Thing", "Other"]);
    // Reorder: the folder to the top.
    let folder = favs(&b)[2].0;
    b.favorite_move(folder, -2);
    let names: Vec<String> = favs(&b).into_iter().map(|f| f.1).collect();
    assert_eq!(names, vec!["Other", "Nirvana", "Thing"]);
    // One click opens it where it lives.
    let at = b
        .sidebar
        .iter()
        .position(|e| matches!(e, SideEntry::Favorite { label, .. } if label == "Thing"))
        .unwrap();
    b.activate_sidebar(at);
    assert_eq!(b.section, Section::Albums);
    assert!(matches!(b.level().unwrap().source, Source::Album { .. }));
    // The open album's own toggle takes it out again.
    b.toggle_favorite_level();
    assert_eq!(favs(&b).len(), 2);
    let artist = favs(&b)[1].0;
    b.unfavorite(artist);
    let names: Vec<String> = favs(&b).into_iter().map(|f| f.1).collect();
    assert_eq!(names, vec!["Other"]);
}

#[test]
fn lyrics_wait_for_the_button_and_follow_the_song() {
    let fx = fixture();
    let mut b = browser(&fx);
    let song = fx.root.join("Other/Thing/01.flac");
    b.player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo {
            path: song.clone(),
            ..Default::default()
        }],
        start: 0,
        pos: Duration::ZERO,
    });
    let end = Instant::now() + Duration::from_secs(10);
    while b.player.snapshot().track.is_none() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Nothing until asked.
    b.poll();
    assert!(b.lyrics().is_none());
    b.lyrics_show();
    let end = Instant::now() + Duration::from_secs(10);
    let l = loop {
        b.poll();
        match &b.lyrics().unwrap().status {
            LyricsStatus::Found(l) => break l.clone(),
            LyricsStatus::Looking => {}
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(l.origin, Origin::Tags);
    assert!(l.synced);
    assert_eq!(l.line_at(Duration::from_millis(1800)), Some(1));
    assert!(!b.lyrics_stale());
    b.lyrics_hide();
    assert!(b.lyrics().is_none());
}

// ----- With a server -----

fn cfg(name: &str, url: &str, kind: ServerKind) -> ServerConfig {
    ServerConfig {
        name: name.into(),
        url: url.into(),
        user: "zack".into(),
        kind,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    }
}

fn sync_lib(lib: &mut Library, name: &str, kind: ServerKind, c: &dyn remote::Client) {
    sync::sync_library(
        lib,
        name,
        kind,
        c,
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
}

#[test]
fn ratings_and_stars_sync_both_ways_with_subsonic() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    {
        let mut s = srv.state.lock().unwrap();
        s.ratings.insert("11".into(), 4); // Blew
        s.ratings.insert("2".into(), 3); // Loveless
        s.starred_albums.push("1".into()); // Bleach
        s.starred_artists.push("ar-My Bloody Valentine".into());
    }
    let c = remote::connect(&cfg("Home", &srv.url, ServerKind::Subsonic), "pw").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();
    sync_lib(&mut lib, "Home", ServerKind::Subsonic, c.as_ref());
    // In: song and album ratings, and the starred list.
    let root = remote::root("Home");
    let blew = root.join("Nirvana/Bleach/01 - Blew.flac");
    assert_eq!(lib.song_rating(&blew).unwrap(), 4);
    let loveless = lib
        .albums(AlbumSort::Name)
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Loveless")
        .unwrap();
    assert_eq!(loveless.rating, 3);
    assert!(sync::pull_starred(&mut lib, "Home", ServerKind::Subsonic, c.as_ref()).unwrap());
    let favs: Vec<(FavKind, String)> = lib
        .favorites()
        .unwrap()
        .into_iter()
        .map(|f| (f.kind, f.label))
        .collect();
    assert_eq!(
        favs,
        vec![
            (FavKind::Album, "Bleach".into()),
            (FavKind::Artist, "My Bloody Valentine".into())
        ]
    );
    // Out: a rating changed here goes to the server, and a later sync
    // doesn't undo it while it waits.
    lib.set_song_rating(&blew, 2, true).unwrap();
    sync_lib(&mut lib, "Home", ServerKind::Subsonic, c.as_ref());
    assert_eq!(lib.song_rating(&blew).unwrap(), 2, "kept while waiting");
    lib.set_album_rating(&loveless.key, 0, true).unwrap();
    assert!(sync::push_marks(&lib, "Home", ServerKind::Subsonic, c.as_ref()).unwrap());
    {
        let s = srv.state.lock().unwrap();
        assert_eq!(s.ratings.get("11"), Some(&2));
        assert_eq!(s.ratings.get("2"), None, "taken away there too");
        assert!(s.rated.contains(&("2".into(), 0)));
    }
    assert!(lib.pending_song_ratings(1).unwrap().is_empty());
    // Unstar Bleach here, star Nirvana here: the server hears both.
    let bleach = lib
        .favorites()
        .unwrap()
        .into_iter()
        .find(|f| f.label == "Bleach")
        .unwrap();
    lib.remove_favorite(bleach.id).unwrap();
    lib.add_favorite(FavKind::Artist, b"Nirvana", "Nirvana", Some("Home"))
        .unwrap();
    sync::push_marks(&lib, "Home", ServerKind::Subsonic, c.as_ref()).unwrap();
    {
        let s = srv.state.lock().unwrap();
        assert!(s.starred_albums.is_empty(), "{:?}", s.starred_albums);
        assert!(s.starred_artists.contains(&"ar-Nirvana".to_string()));
    }
    // And the next pull agrees with what's here.
    sync::pull_starred(&mut lib, "Home", ServerKind::Subsonic, c.as_ref()).unwrap();
    let names: Vec<String> = lib
        .favorites()
        .unwrap()
        .into_iter()
        .map(|f| f.label)
        .collect();
    assert_eq!(names, vec!["My Bloody Valentine", "Nirvana"]);
}

#[test]
fn the_server_has_lyrics() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    srv.state.lock().unwrap().lyrics.insert(
        "12".into(),
        vec![(0, "Floyd".into()), (1500, "the barber".into())],
    );
    let c = remote::connect(&cfg("Lyr", &srv.url, ServerKind::Subsonic), "pw").unwrap();
    let got = c
        .lyrics("12", "Nirvana", "Floyd the Barber")
        .unwrap()
        .unwrap();
    assert!(got.synced);
    assert_eq!(
        got.lines[1],
        (Some(Duration::from_millis(1500)), "the barber".into())
    );
    assert_eq!(c.lyrics("11", "Nirvana", "Blew").unwrap(), None);
    // Through the lookup, as the Lyrics button does it.
    remote::registry().set_client("Lyr", Some(c.clone()));
    let info = TrackInfo {
        path: remote::root("Lyr").join("x.flac"),
        remote: Some(remote::RemoteRef {
            server: "Lyr".into(),
            id: "12".into(),
            episode: false,
        }),
        artist: Some("Nirvana".into()),
        title: Some("Floyd the Barber".into()),
        ..Default::default()
    };
    let l = patina_core::lyrics::find(&info, false).unwrap().unwrap();
    assert_eq!(l.origin, Origin::Server);
}

#[test]
fn lrclib_is_asked_only_when_allowed() {
    let hits = std::sync::Arc::new(std::sync::Mutex::new(0));
    let h2 = hits.clone();
    let url = common::serve(std::sync::Arc::new(move |r: &common::Request| {
        *h2.lock().unwrap() += 1;
        assert_eq!(r.path, "/api/get");
        assert_eq!(r.get("track_name"), Some("Song"));
        common::Response::json(serde_json::json!({
            "syncedLyrics": "[00:01.00]from lrclib", "plainLyrics": "from lrclib"
        }))
    }));
    // SAFETY: this test alone reads the variable, before and after.
    unsafe { std::env::set_var("PATINA_LRCLIB_URL", &url) };
    let info = TrackInfo {
        path: PathBuf::from("/nowhere/song.flac"),
        artist: Some("Someone".into()),
        title: Some("Song".into()),
        ..Default::default()
    };
    assert_eq!(patina_core::lyrics::find(&info, false).unwrap(), None);
    assert_eq!(*hits.lock().unwrap(), 0, "off means off");
    let l = patina_core::lyrics::find(&info, true).unwrap().unwrap();
    assert_eq!(l.origin, Origin::Lrclib);
    assert_eq!(*hits.lock().unwrap(), 1);
}

#[test]
fn ampache_hears_ratings_and_stars() {
    let srv = Ampache::start("zack", "pw");
    let c = remote::connect(&cfg("Amp", &srv.url, ServerKind::Ampache), "pw").unwrap();
    c.set_rating(Rated::Album, "7", 4).unwrap();
    c.star_album("7", true).unwrap();
    let s = srv.state.lock().unwrap();
    assert!(s.rated.contains(&("album:7".into(), 4)));
    assert!(s.stars.contains(&("album:7".into(), true)));
}

#[test]
fn hearted_songs_is_always_there_and_follows_the_hearts() {
    use patina_core::library::HEARTED;
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Playlists);
    let hearted = |b: &Browser| match find(b, |i| matches!(i, Item::Playlist(p) if p.id == HEARTED))
    {
        Item::Playlist(p) => p,
        _ => unreachable!(),
    };
    // There before anything is hearted, and empty.
    let p = hearted(&b);
    assert_eq!(p.name, "Hearted songs");
    assert_eq!(p.items, 0);

    // Loves match the tags without case, the way Last.fm keeps them.
    b.lib
        .as_ref()
        .unwrap()
        .set_loved("NIRVANA", "bleach song 2", true)
        .unwrap();
    b.refresh_section(Section::Playlists);
    assert_eq!(hearted(&b).items, 1);

    // Open it: the hearted song, playable like any playlist.
    let i = b
        .level()
        .unwrap()
        .items
        .iter()
        .position(|i| matches!(i, Item::Playlist(p) if p.id == HEARTED))
        .unwrap();
    assert!(b.select(i));
    b.activate();
    assert_eq!(b.in_playlist(), Some(HEARTED));
    let titles: Vec<String> = b
        .level()
        .unwrap()
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => Some(t.title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Bleach Song 2"]);

    // It can't be renamed or deleted, and removing a song unhearts it.
    b.rename_playlist(HEARTED, "Something else");
    b.delete_playlist(HEARTED);
    assert!(b.select(0));
    b.edit_playlist(true, 0);
    assert!(
        !b.lib
            .as_ref()
            .unwrap()
            .loved("Nirvana", "Bleach Song 2")
            .unwrap()
    );
    assert!(
        b.level()
            .unwrap()
            .items
            .iter()
            .all(|i| !matches!(i, Item::Track(_)))
    );
    b.pop();
    let p = hearted(&b);
    assert_eq!((p.name.as_str(), p.items), ("Hearted songs", 0));
}

#[test]
fn moving_the_selection_is_not_a_change_to_redraw() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    let g = b.generation();
    assert!(b.select(1));
    b.move_selection(1);
    assert_eq!(b.generation(), g);
    assert_eq!(b.level().unwrap().selected, 2);
}
