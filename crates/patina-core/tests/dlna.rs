//! DLNA servers against the stand-in in `common`: found with an SSDP
//! search, read each of the three ways a server can allow, synced into the
//! index, played, and added and removed through the Browser. Needs `ffmpeg`.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use common::{Dlna, DlnaFlavor};
use patina_core::browse::{Browser, Item, Key, Note, Section, SettingsInfo, SideEntry, Startup};
use patina_core::config::Config;
use patina_core::library::Library;
use patina_core::remote::{
    self, Client, RemoteService, ServerConfig, ServerKind, ServerState, dlna, sync,
};
use patina_core::secrets::Secrets;
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig};

fn cfg(name: &str, d: &Dlna) -> ServerConfig {
    ServerConfig {
        name: name.into(),
        url: d.url.clone(),
        kind: ServerKind::Dlna,
        udn: d.udn.clone(),
        bitrate: 192,
        ..Default::default()
    }
}

fn player() -> Player {
    Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    })
}

#[test]
fn a_search_finds_the_server() {
    let d = Dlna::start(DlnaFlavor::Folders, "Living Room");
    let found = dlna::discover_at(d.ssdp, Duration::from_millis(800));
    assert_eq!(found.len(), 1, "asked twice, listed once: {found:?}");
    assert_eq!(found[0].name, "Living Room");
    assert_eq!(found[0].udn, d.udn);
    assert_eq!(found[0].location, d.url);
    assert_eq!(found[0].model, "Mock Media Server 1.0");
    // Nobody there: an empty list, not a hang.
    let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let t = Instant::now();
    assert!(dlna::discover_at(quiet.local_addr().unwrap(), Duration::from_millis(300)).is_empty());
    assert!(t.elapsed() < Duration::from_secs(3));
}

fn sync_from(flavor: DlnaFlavor) -> (Dlna, Arc<dyn Client>, tempfile::TempDir, Library) {
    let d = Dlna::start(flavor, "NAS");
    let c = remote::connect(&cfg("NAS", &d), "").unwrap();
    assert_eq!(c.info().kind, ServerKind::Dlna);
    assert!(!c.info().play_queue);
    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();
    let stats = sync::sync_library(
        &mut lib,
        "NAS",
        ServerKind::Dlna,
        c.as_ref(),
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(stats.added, 5, "{flavor:?}: {stats:?}");
    (d, c, dir, lib)
}

fn check_library(flavor: DlnaFlavor, lib: &Library) {
    let songs = lib.songs(Default::default(), false).unwrap();
    assert_eq!(songs.len(), 5, "{flavor:?}: each song once");
    let blew = songs.iter().find(|t| t.title == "Blew").unwrap();
    assert_eq!(blew.album.as_deref(), Some("Bleach"), "{flavor:?}");
    assert_eq!(blew.track_no, Some(1));
    assert_eq!(blew.duration, Some(Duration::from_secs(1)));
    assert_eq!(
        blew.codec, "FLAC",
        "{flavor:?}: the original, not the MP3 copy"
    );
    assert_eq!(blew.sample_rate, 44100);
    let albums = lib.albums(Default::default()).unwrap();
    let mut names: Vec<&str> = albums.iter().map(|a| a.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["Bleach", "Loveless"], "{flavor:?}");
    let loveless = albums.iter().find(|a| a.name == "Loveless").unwrap();
    assert_eq!(loveless.artist_line(), "My Bloody Valentine · NAS");
}

#[test]
fn a_folder_view_gives_the_real_folders() {
    let (d, c, _dir, lib) = sync_from(DlnaFlavor::Folders);
    check_library(DlnaFlavor::Folders, &lib);
    let paths: Vec<String> = lib
        .songs(Default::default(), false)
        .unwrap()
        .iter()
        .map(|t| t.path.display().to_string())
        .collect();
    assert!(
        paths.contains(&"server://NAS/Nirvana/Bleach/01 Blew.flac".to_string()),
        "{paths:?}"
    );
    // The folder view only: not the Music or Video views beside it.
    let calls = d.state.lock().unwrap().calls.clone();
    assert!(
        !calls
            .iter()
            .any(|c| c == "Browse 1" || c == "Browse 2" || c == "Browse 64$1")
    );
    // The folder view titles songs by file name; a search brings the
    // tags. Blew is still Blew, not "01 Blew".
    assert!(calls.iter().any(|c| c == "Search"));
    // Covers come from the server.
    let song = lib.songs(Default::default(), false).unwrap()[0].clone();
    let img = c.cover(&lib.remote_cover_for(&song.path).unwrap().unwrap(), 64);
    assert!(img.is_ok(), "{img:?}");
}

#[test]
fn search_reads_a_server_without_folders() {
    let (d, _c, _dir, lib) = sync_from(DlnaFlavor::Search);
    check_library(DlnaFlavor::Search, &lib);
    assert!(d.state.lock().unwrap().calls.iter().any(|c| c == "Search"));
    // Paths made up from the tags.
    let t = lib.songs(Default::default(), false).unwrap();
    assert!(
        t.iter()
            .any(|t| t.path.display().to_string() == "server://NAS/Nirvana/Bleach/01 - Blew.flac"),
        "{:?}",
        t.iter()
            .map(|t| t.path.display().to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_bare_tree_is_walked_and_songs_kept_once() {
    let (d, _c, _dir, lib) = sync_from(DlnaFlavor::Tree);
    check_library(DlnaFlavor::Tree, &lib);
    let calls = d.state.lock().unwrap().calls.clone();
    assert!(calls.iter().any(|c| c == "Browse all"));
    assert!(calls.iter().any(|c| c.starts_with("Browse artist:")));
}

#[test]
fn nothing_is_read_again_until_the_server_changes() {
    let (d, c, _dir, mut lib) = sync_from(DlnaFlavor::Folders);
    let before = d.browses();
    let again = |lib: &mut Library| {
        sync::sync_library(
            lib,
            "NAS",
            ServerKind::Dlna,
            c.as_ref(),
            false,
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap()
    };
    let s = again(&mut lib);
    assert_eq!((s.albums, s.added), (0, 0));
    assert_eq!(d.browses(), before, "same update id, no walk");

    // A new song on the server, and its update id moves.
    {
        let mut st = d.state.lock().unwrap();
        let a = &mut st.lib.albums[1];
        a.songs.push(common::MSong {
            id: "23".into(),
            title: "To Here Knows When".into(),
            track: 3,
            artist: "My Bloody Valentine".into(),
            path: String::new(),
            plays: 0,
        });
        st.update_id += 1;
    }
    let s = again(&mut lib);
    assert_eq!((s.albums, s.added), (1, 1), "only the changed album: {s:?}");
    assert_eq!(lib.track_count().unwrap(), 6);
}

#[test]
fn a_song_plays_from_the_server() {
    let (d, c, dir, lib) = sync_from(DlnaFlavor::Search);
    remote::registry().set_client("NAS", Some(c.clone()));
    remote::registry().set_db(
        "NAS",
        Some(Library::open(&dir.path().join("lib.db")).unwrap()),
    );
    let t = lib
        .songs(Default::default(), false)
        .unwrap()
        .into_iter()
        .find(|t| t.title == "Loomer")
        .unwrap();
    // The id is kept without the address, so a new address keeps the song.
    let id = t.remote_id.clone().unwrap();
    assert_eq!(id, "/media/22.flac");
    let (url, raw) = c.stream_url(&id).unwrap();
    assert!(url.starts_with("http://127.0.0.1:") && url.ends_with("/media/22.flac"));
    assert!(raw);
    let p = player();
    p.send(Command::SetQueueTracks {
        tracks: vec![t.to_info()],
        start: 0,
        pos: Duration::ZERO,
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "never finished");
        match p.events().recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Finished) => break,
            Ok(Event::Error(e)) => panic!("{e}"),
            _ => {}
        }
    }
    p.send(Command::Quit);
    assert!(
        d.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c == "GET 22.flac")
    );
    remote::registry().remove("NAS");
}

#[test]
fn a_dead_address_is_an_error_not_a_hang() {
    let gone = ServerConfig {
        name: "Gone".into(),
        url: "http://127.0.0.1:9/desc.xml".into(),
        kind: ServerKind::Dlna,
        ..Default::default()
    };
    let t = Instant::now();
    assert!(matches!(
        remote::connect(&gone, ""),
        Err(remote::RemoteError::Http(_))
    ));
    assert!(t.elapsed() < Duration::from_secs(10));
}

fn wait(b: &mut Browser, what: &str, mut f: impl FnMut(&mut Browser) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        b.poll();
        if f(b) {
            return;
        }
        if Instant::now() > deadline {
            let notes: Vec<Note> = b.take_notes();
            panic!(
                "timed out waiting for {what}; notes: {notes:?}; servers: {:?}",
                b.servers
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn setting_keys(b: &Browser) -> Vec<Key> {
    b.level()
        .unwrap()
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Setting(s) => Some(s.key.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_browser_finds_adds_renames_and_removes_one() {
    let d = Dlna::start(DlnaFlavor::Folders, "Living Room");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "# dlna test\n").unwrap();
    let secrets = Arc::new(Secrets::with(None, dir.path().join("secrets.toml")));
    let mut b = Browser::new(Startup {
        player: player(),
        lib: Some(Library::open(&db).unwrap()),
        service: None,
        pod: None,
        remote: Some(RemoteService::start(&db, Vec::new(), secrets)),
        config: Config::default(),
        settings_info: SettingsInfo {
            config_path: config.clone(),
            db_path: db,
            ..Default::default()
        },
    });
    b.dlna_probe = Some(d.ssdp);

    b.switch(Section::Settings);
    assert!(setting_keys(&b).contains(&Key::FindDlna));
    b.find_dlna();
    assert!(b.finding_dlna());
    let mut found = None;
    wait(&mut b, "the search", |b| {
        found = b.take_dlna_found();
        found.is_some()
    });
    let found = found.unwrap();
    assert_eq!(found.len(), 1);
    b.add_dlna_server(&found[0]);
    wait(&mut b, "the sync", |b| {
        b.servers.iter().any(|s| {
            s.name == "Living Room" && s.state == ServerState::Ready && s.last_sync.is_some()
        })
    });
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(
        text.contains("kind = \"dlna\"") && text.contains(&d.udn),
        "{text}"
    );
    assert!(!text.contains("user ="), "no login to save: {text}");
    assert!(!std::path::Path::new(&dir.path().join("secrets.toml")).exists());
    assert!(b.sidebar.iter().any(|e| matches!(
        e,
        SideEntry::Source {
            server: Some(ServerState::Ready),
            tracks: 5,
            ..
        }
    )));

    // Settings: the server, but no streaming choice (DLNA doesn't convert).
    b.switch(Section::Settings);
    let keys = setting_keys(&b);
    assert!(keys.contains(&Key::Server("Living Room".into())));
    assert!(!keys.contains(&Key::ServerStream("Living Room".into())));

    // Found again: it's already there, so nothing new to pick.
    b.find_dlna();
    wait(&mut b, "the second search", |b| !b.finding_dlna());
    assert!(b.take_dlna_found().is_none());
    assert!(
        b.take_notes()
            .iter()
            .any(|n| matches!(n, Note::Info(m) if m.contains("already added")))
    );

    // Renamed: same server, new name, songs under the new name.
    b.edit_server("Living Room", "Den", "", "", None);
    wait(&mut b, "the rename", |b| {
        b.config.servers.iter().any(|s| s.name == "Den")
            && b.servers
                .iter()
                .any(|s| s.name == "Den" && s.state == ServerState::Ready && s.last_sync.is_some())
    });
    assert_eq!(b.config.servers.len(), 1);
    assert_eq!(b.config.servers[0].udn, d.udn);
    wait(&mut b, "the songs to move", |b| {
        b.lib
            .as_ref()
            .unwrap()
            .songs(Default::default(), false)
            .unwrap()
            .iter()
            .all(|t| t.path.display().to_string().starts_with("server://Den/"))
    });

    b.remove_server("Den");
    wait(&mut b, "the songs to go", |b| {
        b.lib.as_ref().unwrap().track_count().unwrap() == 0
    });
    assert!(b.take_notes().iter().all(|n| !matches!(n, Note::Error(_))));
    b.player.send(Command::Quit);
}
