//! The clients against real servers: Navidrome, Gonic and Ampache in
//! containers, started by `.github/workflows/servers.yml`. Each test reads
//! where its server is from the environment and is ignored otherwise:
//!
//! - `PATINA_NAVIDROME=http://localhost:4533` (user `admin`, password `patina-pw`)
//! - `PATINA_GONIC=http://localhost:4747` (user `admin`, password `admin`)
//! - `PATINA_AMPACHE=http://localhost:8080` (user `admin`, password `patina-pw`)
//! - `PATINA_DLNA=http://localhost:8200/rootDesc.xml` (MiniDLNA or
//!   Gerbera; the server's description URL)
//!
//! `PATINA_SERVER_SONGS` says how many songs the test library holds.
//! Run with `cargo test --test real_servers -- --ignored`.

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use patina_core::library::Library;
use patina_core::remote::{self, ServerConfig, ServerKind, StreamFormat, sync};
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn songs_expected() -> usize {
    env("PATINA_SERVER_SONGS")
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

fn check(name: &str, url: &str, user: &str, pw: &str, want: ServerKind) {
    // Wrong passwords are told apart from everything else.
    assert_eq!(
        remote::detect(url, user, "definitely-wrong").err(),
        Some(remote::RemoteError::Auth),
        "{name}: a wrong password"
    );
    let (kind, info) = remote::detect(url, user, pw).expect("log in");
    assert_eq!(kind, want, "{name}: {info:?}");
    eprintln!("{name}: {} (API {})", info.product, info.api_version);
    let cfg = ServerConfig {
        name: name.into(),
        url: url.into(),
        user: user.into(),
        kind,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    };
    let c = remote::connect(&cfg, pw).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let mut lib = Library::open(&db).unwrap();
    let stats = sync::sync_library(
        &mut lib,
        name,
        kind,
        c.as_ref(),
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    eprintln!("{name}: {stats:?}");
    assert_eq!(
        lib.track_count().unwrap() as usize,
        songs_expected(),
        "{name}: every song synced"
    );
    let again = sync::sync_library(
        &mut lib,
        name,
        kind,
        c.as_ref(),
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(again.albums, 0, "{name}: a second sync fetches nothing new");

    // Playlists: make one, change it, read it back, delete it.
    let songs = lib.songs(Default::default(), false).unwrap();
    let ids: Vec<String> = songs.iter().filter_map(|t| t.remote_id.clone()).collect();
    let pid = c
        .create_playlist("PatinaPlayer test", &ids[..ids.len().min(3)])
        .unwrap();
    c.replace_playlist(&pid, &ids[..1]).unwrap();
    c.rename_playlist(&pid, "PatinaPlayer test 2").unwrap();
    sync::sync_playlists(&mut lib, name, kind, c.as_ref()).unwrap();
    let pl = lib
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "PatinaPlayer test 2")
        .expect("the playlist synced");
    assert_eq!(pl.items, 1, "{name}: replaced to one song");
    c.delete_playlist(&pid).unwrap();

    // The play queue, where the server keeps one.
    if c.info().play_queue {
        c.save_queue(
            &ids[..2.min(ids.len())],
            Some(&ids[0]),
            Duration::from_secs(3),
        )
        .unwrap();
        let q = c.load_queue().unwrap().expect("a saved queue");
        assert_eq!(q.ids.len(), 2.min(ids.len()), "{name}: the queue came back");
    }

    // A song plays all the way through, from the server.
    remote::registry().set_client(name, Some(c.clone()));
    remote::registry().set_db(name, Some(Library::open(&db).unwrap()));
    let t = &songs[0];
    if let Some(img) = patina_core::art::load(&t.path, 64) {
        assert_eq!(img.width(), 64);
    } else {
        eprintln!(
            "{name}: no cover art for {} (fine if the file has none)",
            t.title
        );
    }
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    player.send(Command::SetQueueTracks {
        tracks: vec![t.to_info()],
        start: 0,
        pos: Duration::ZERO,
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "{name}: never finished playing");
        match player.events().recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Finished) => break,
            Ok(Event::Error(e)) => panic!("{name}: {e}"),
            _ => {}
        }
    }
    player.send(Command::Quit);
    remote::registry().remove(name);
}

#[test]
#[ignore = "needs a Navidrome server (CI starts one)"]
fn navidrome() {
    let Some(url) = env("PATINA_NAVIDROME") else {
        return;
    };
    check(
        "Navidrome",
        &url,
        "admin",
        "patina-pw",
        ServerKind::Subsonic,
    );
}

#[test]
#[ignore = "needs a Gonic server (CI starts one)"]
fn gonic() {
    let Some(url) = env("PATINA_GONIC") else {
        return;
    };
    check("Gonic", &url, "admin", "admin", ServerKind::Subsonic);
}

#[test]
#[ignore = "needs an Ampache server (CI starts one)"]
fn ampache() {
    let Some(url) = env("PATINA_AMPACHE") else {
        return;
    };
    check("Ampache", &url, "admin", "patina-pw", ServerKind::Ampache);
}

#[test]
#[ignore = "needs a DLNA server (CI starts MiniDLNA)"]
fn dlna() {
    let Some(url) = env("PATINA_DLNA") else {
        return;
    };
    let dev = remote::dlna::describe(&url).expect("its description");
    eprintln!("DLNA: {} ({}), {}", dev.name, dev.model, dev.udn);
    // It answers a search on the network, by the same id.
    let found = remote::dlna::discover(Duration::from_secs(3));
    eprintln!("DLNA: found {found:?}");
    assert!(
        found.iter().any(|f| f.udn == dev.udn),
        "the server answered a search"
    );
    let cfg = ServerConfig {
        name: "NAS".into(),
        url: url.clone(),
        kind: ServerKind::Dlna,
        udn: dev.udn.clone(),
        ..Default::default()
    };
    let c = remote::connect(&cfg, "").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let mut lib = Library::open(&db).unwrap();
    let sync = |lib: &mut Library| {
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
    let stats = sync(&mut lib);
    eprintln!("DLNA: {stats:?}");
    let songs = lib.songs(Default::default(), false).unwrap();
    for t in &songs {
        eprintln!("  {} | {:?} | {:?}", t.path.display(), t.album, t.codec);
    }
    assert_eq!(songs.len(), songs_expected(), "every song synced once");
    assert!(
        songs.iter().all(|t| t.duration.is_some()),
        "every song has a length"
    );
    assert_eq!(
        sync(&mut lib).albums,
        0,
        "a second sync fetches nothing new"
    );

    // A song plays all the way through from the server, with its cover.
    remote::registry().set_client("NAS", Some(c.clone()));
    remote::registry().set_db("NAS", Some(Library::open(&db).unwrap()));
    // The test library has a cover.jpg in one album.
    assert!(
        songs
            .iter()
            .any(|t| patina_core::art::load(&t.path, 64).is_some()),
        "DLNA: a cover loads"
    );
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    player.send(Command::SetQueueTracks {
        tracks: vec![songs[0].to_info()],
        start: 0,
        pos: Duration::ZERO,
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        assert!(Instant::now() < deadline, "DLNA: never finished playing");
        match player.events().recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Finished) => break,
            Ok(Event::Error(e)) => panic!("DLNA: {e}"),
            _ => {}
        }
    }
    player.send(Command::Quit);
    remote::registry().remove("NAS");
}
