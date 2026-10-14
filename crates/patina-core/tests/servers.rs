//! The Subsonic and Ampache clients and the sync into the index, against
//! the stand-in servers in `common`. Needs the `ffmpeg` command.

mod common;

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use common::{Ampache, Flavor, Subsonic};
use patina_core::library::Library;
use patina_core::remote::{
    self, Client, RemoteError, ServerConfig, ServerKind, StreamFormat, sync,
};

fn cfg(name: &str, url: &str, user: &str, kind: ServerKind) -> ServerConfig {
    ServerConfig {
        name: name.into(),
        url: url.into(),
        user: user.into(),
        kind,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    }
}

fn sync_all(lib: &mut Library, name: &str, kind: ServerKind, c: &dyn Client) -> sync::SyncStats {
    let s = sync::sync_library(
        lib,
        name,
        kind,
        c,
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    sync::sync_playlists(lib, name, kind, c).unwrap();
    s
}

#[test]
fn wrong_passwords_are_told_apart_from_dead_servers() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let bad = remote::connect(&cfg("A", &srv.url, "zack", ServerKind::Subsonic), "nope");
    assert_eq!(bad.err(), Some(RemoteError::Auth));
    let dead = remote::connect(
        &cfg("B", "http://127.0.0.1:9", "zack", ServerKind::Subsonic),
        "pw",
    );
    assert!(matches!(dead.err(), Some(RemoteError::Http(_))));
    let amp = Ampache::start("zack", "pw");
    let bad = remote::connect(&cfg("C", &amp.url, "zack", ServerKind::Ampache), "nope");
    assert_eq!(bad.err(), Some(RemoteError::Auth));
}

#[test]
fn the_kind_of_server_is_found_by_logging_in() {
    let nd = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let (kind, info) = remote::detect(&nd.url, "zack", "pw").unwrap();
    assert_eq!(kind, ServerKind::Subsonic);
    assert_eq!(info.product, "Navidrome 0.58.0");
    assert!(info.open_subsonic);
    let amp = Ampache::start("zack", "pw");
    let (kind, info) = remote::detect(&format!("{}/", amp.url), "zack", "pw").unwrap();
    assert_eq!(kind, ServerKind::Ampache);
    assert_eq!(info.product, "Ampache 6.6.0");
    // Ampache that says no is a wrong password, not "maybe Subsonic".
    assert_eq!(
        remote::detect(&amp.url, "zack", "nope").err(),
        Some(RemoteError::Auth)
    );
}

#[test]
fn api_keys_work_where_the_server_offers_them() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", Some("KEY123"));
    let c = remote::connect(&cfg("K", &srv.url, "", ServerKind::Subsonic), "KEY123").unwrap();
    assert_eq!(c.albums().unwrap().len(), 2);
    let calls = srv.state.lock().unwrap().calls.clone();
    assert!(calls.contains(&"getAlbumList2".to_string()));
}

#[test]
fn navidrome_syncs_whole_then_only_what_changed() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let c = remote::connect(&cfg("Home", &srv.url, "zack", ServerKind::Subsonic), "pw").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();

    let s = sync_all(&mut lib, "Home", ServerKind::Subsonic, c.as_ref());
    assert_eq!((s.albums, s.added), (2, 5));
    let albums = lib.albums(Default::default()).unwrap();
    assert_eq!(albums.len(), 2);
    let bleach = albums.iter().find(|a| a.name == "Bleach").unwrap();
    assert_eq!((bleach.artist.as_str(), bleach.track_count), ("Nirvana", 3));
    // Folders come from the paths Navidrome makes up from the tags.
    let root = remote::root("Home");
    let top: Vec<String> = lib
        .folder(&root)
        .unwrap()
        .into_iter()
        .map(|e| match e {
            patina_core::library::DirEntry::Dir { name, .. } => name,
            patina_core::library::DirEntry::Track(t) => t.title,
        })
        .collect();
    assert_eq!(top, vec!["My Bloody Valentine", "Nirvana"]);
    let t = lib
        .track_by_path(&root.join("Nirvana/Bleach/01 - Blew.flac"))
        .unwrap()
        .unwrap();
    assert_eq!(t.remote_id.as_deref(), Some("11"));
    assert_eq!(
        (t.sample_rate, t.bits, t.codec.as_str()),
        (44100, Some(16), "FLAC")
    );
    let info = t.to_info();
    assert_eq!(info.remote.as_ref().map(|r| r.id.as_str()), Some("11"));
    assert!(
        info.location().is_none(),
        "nothing for FFmpeg to open directly"
    );

    // Nothing changed: the album list is read, no album is fetched.
    let before = srv.state.lock().unwrap().count("getAlbum");
    let s = sync_all(&mut lib, "Home", ServerKind::Subsonic, c.as_ref());
    assert_eq!(s.albums, 0);
    assert_eq!(srv.state.lock().unwrap().count("getAlbum"), before);

    // A song added to Loveless, and Bleach gone from the server.
    {
        let mut st = srv.state.lock().unwrap();
        let mut extra = st.albums[1].songs[0].clone();
        extra.id = "23".into();
        extra.title = "When You Sleep".into();
        extra.track = 3;
        st.albums[1].songs.push(extra);
        st.albums.remove(0);
    }
    let s = sync_all(&mut lib, "Home", ServerKind::Subsonic, c.as_ref());
    assert_eq!(s.albums, 1, "only the changed album is fetched");
    assert_eq!((s.added, s.removed), (1, 3));
    assert_eq!(lib.albums(Default::default()).unwrap().len(), 1);
    assert!(lib.track_by_path(&t.path).unwrap().is_none());
}

#[test]
fn gonic_quirks_sync_the_same() {
    let srv = Subsonic::start(Flavor::Gonic, "zack", "pw", None);
    let c = remote::connect(&cfg("G", &srv.url, "zack", ServerKind::Subsonic), "pw").unwrap();
    assert_eq!(c.info().product, "gonic 0.16.4");
    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();
    let s = sync_all(&mut lib, "G", ServerKind::Subsonic, c.as_ref());
    assert_eq!(s.added, 5);
    // Gonic's real paths, and its prefixed ids.
    let t = lib
        .track_by_path(&remote::root("G").join("MBV/Loveless/02 Loomer.flac"))
        .unwrap()
        .unwrap();
    assert_eq!(t.remote_id.as_deref(), Some("tr-22"));
    assert_eq!(
        t.album_artist.as_deref(),
        Some("My Bloody Valentine"),
        "from the album"
    );
    let rows = lib
        .tracks_by_remote("G", &["tr-11".into(), "tr-99".into()])
        .unwrap();
    assert!(rows[0].is_some() && rows[1].is_none());
}

#[test]
fn playlists_round_trip_through_the_server() {
    for flavor in [Flavor::Navidrome, Flavor::Gonic] {
        let srv = Subsonic::start(flavor, "zack", "pw", None);
        let c = remote::connect(&cfg("P", &srv.url, "zack", ServerKind::Subsonic), "pw").unwrap();
        let ids: Vec<String> = srv.state.lock().unwrap().albums[0]
            .songs
            .iter()
            .map(|s| s.id.clone())
            .collect();
        // Long lists go in a POST on Navidrome, in pieces on Gonic.
        let many: Vec<String> = ids.iter().cycle().take(400).cloned().collect();
        let id = c.create_playlist("Mix", &many).unwrap();
        assert_eq!(srv.state.lock().unwrap().playlists[0].songs.len(), 400);
        c.replace_playlist(&id, &ids[..2]).unwrap();
        c.rename_playlist(&id, "Mix 2").unwrap();
        let lists = c.playlists().unwrap();
        assert_eq!((lists[0].name.as_str(), lists[0].song_count), ("Mix 2", 2));
        assert!(!lists[0].readonly);

        let dir = tempfile::tempdir().unwrap();
        let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();
        sync_all(&mut lib, "P", ServerKind::Subsonic, c.as_ref());
        let pl = lib.playlists().unwrap();
        assert_eq!(pl.len(), 1);
        assert_eq!(pl[0].server.as_deref(), Some("P"));
        let items = lib.playlist_items(pl[0].id).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].1.as_ref().unwrap().title, "Blew");

        c.delete_playlist(&id).unwrap();
        sync::sync_playlists(&mut lib, "P", ServerKind::Subsonic, c.as_ref()).unwrap();
        assert!(lib.playlists().unwrap().is_empty());
    }
}

#[test]
fn radio_podcasts_queue_and_bookmarks() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let c = remote::connect(&cfg("Q", &srv.url, "zack", ServerKind::Subsonic), "pw").unwrap();
    let r = c.radio().unwrap();
    assert_eq!(
        (r[0].name.as_str(), r[0].url.as_str()),
        ("Server FM", "http://127.0.0.1:9/fm")
    );

    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();
    sync::sync_podcasts(&mut lib, "Q", c.as_ref()).unwrap();
    let pods = lib.podcasts().unwrap();
    assert_eq!(pods[0].title, "Server Pod");
    let eps = lib.episodes(pods[0].id).unwrap();
    assert_eq!(eps.len(), 1, "only what the server has downloaded");
    let info = eps[0].to_info();
    assert_eq!(info.remote.as_ref().unwrap().id, "ep1s");
    assert!(info.url.is_none());
    // The config's feed list doesn't sweep server podcasts away.
    lib.sync_podcasts(&[]).unwrap();
    assert_eq!(lib.podcasts().unwrap().len(), 1);

    assert!(c.load_queue().unwrap().is_none());
    c.save_queue(
        &["12".into(), "13".into()],
        Some("13"),
        Duration::from_millis(4500),
    )
    .unwrap();
    let q = c.load_queue().unwrap().unwrap();
    assert_eq!(q.ids, vec!["12", "13"]);
    assert_eq!(
        (q.current.as_deref(), q.position),
        (Some("13"), Duration::from_millis(4500))
    );
    c.bookmark("ep1", Duration::from_secs(30)).unwrap();
    assert_eq!(srv.state.lock().unwrap().bookmarks["ep1"], 30_000);
}

#[test]
fn ampache_syncs_and_logs_in_again_when_its_session_ends() {
    let srv = Ampache::start("zack", "pw");
    let c = remote::connect(&cfg("Amp", &srv.url, "zack", ServerKind::Ampache), "pw").unwrap();
    assert!(!c.info().play_queue);
    // The session dies every few calls; the client keeps up.
    srv.state.lock().unwrap().session_life = 3;
    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&dir.path().join("lib.db")).unwrap();
    let s = sync_all(&mut lib, "Amp", ServerKind::Ampache, c.as_ref());
    assert_eq!(s.added, 5);
    assert!(srv.state.lock().unwrap().count("handshake") > 1);
    // Paths are relative to the catalog folder.
    let t = lib
        .track_by_path(&remote::root("Amp").join("Nirvana/Bleach/02 Floyd the Barber.flac"))
        .unwrap()
        .unwrap();
    assert_eq!((t.kbps, t.sample_rate), (Some(900), 44100));

    srv.state.lock().unwrap().session_life = 0;
    let id = c
        .create_playlist("Amp mix", &["11".into(), "21".into()])
        .unwrap();
    c.replace_playlist(&id, &["22".into()]).unwrap();
    let lists = c.playlists().unwrap();
    let smart = lists.iter().find(|p| p.id == "smart_1").unwrap();
    assert!(smart.readonly, "smart lists can't be edited");
    let mine = lists.iter().find(|p| p.id == id).unwrap();
    assert_eq!(mine.song_count, 1);
    assert_eq!(c.radio().unwrap()[0].name, "Ampache FM");
    assert_eq!(
        c.podcasts().unwrap()[0].episodes[0].stream_id.as_deref(),
        Some("51")
    );
    assert_eq!(
        c.load_queue().err(),
        Some(RemoteError::Unsupported("a saved play queue"))
    );
    assert!(c.cover("album:1", 64).unwrap().starts_with(b"\x89PNG"));
}

#[test]
fn stream_urls_carry_a_token_not_the_password() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "s3cret-pw", None);
    let c = remote::connect(
        &cfg("T", &srv.url, "zack", ServerKind::Subsonic),
        "s3cret-pw",
    )
    .unwrap();
    let (url, raw) = c.stream_url("11").unwrap();
    assert!(raw);
    assert!(url.contains("format=raw") && url.contains("t=") && url.contains("s="));
    assert!(!url.contains("s3cret"));
}

#[test]
fn server_songs_play_and_show_their_covers() {
    use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig};
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let name = "PlayMe";
    let c = remote::connect(&cfg(name, &srv.url, "zack", ServerKind::Subsonic), "pw").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let mut lib = Library::open(&db).unwrap();
    sync_all(&mut lib, name, ServerKind::Subsonic, c.as_ref());
    remote::registry().set_client(name, Some(c.clone()));
    remote::registry().set_db(name, Some(Library::open(&db).unwrap()));

    let t = lib
        .tracks_by_remote(name, &["12".into()])
        .unwrap()
        .pop()
        .flatten()
        .unwrap();
    // The cover comes from the server, by the song's path, and for its folder.
    let img = patina_core::art::load(&t.path, 32).unwrap();
    assert_eq!(img.width(), 32);
    assert!(patina_core::art::load_dir(t.path.parent().unwrap(), 32).is_some());

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
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut played = false;
    loop {
        assert!(std::time::Instant::now() < deadline, "never finished");
        match player.events().recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Played(p)) => {
                assert_eq!(p, t.path);
                played = true;
            }
            Ok(Event::Finished) => break,
            Ok(Event::Error(e)) => panic!("{e}"),
            _ => {}
        }
    }
    assert!(played, "a whole song counts as a play");
    let snap = player.snapshot();
    assert!(snap.track.is_none() || snap.track.as_ref().unwrap().codec == "FLAC");
    player.send(Command::Quit);
    remote::registry().remove(name);
}
