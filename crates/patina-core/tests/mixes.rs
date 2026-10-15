//! Genre, artist and track mixes against the stand-in Subsonic servers in
//! `common`, and a stand-in Last.fm. Needs the `ffmpeg` command.

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Flavor, Subsonic};
use patina_core::lastfm::LastFm;
use patina_core::library::{Library, TrackRow};
use patina_core::mix::{Avail, MixEvent, MixSeed, MixService, Source};
use patina_core::remote::{self, ServerConfig, ServerKind, StreamFormat, sync};

/// Mixes ask every connected server, and the servers are one list per
/// process, so these tests take turns and tidy up after themselves.
static TURN: Mutex<()> = Mutex::new(());

struct Servers(Vec<String>);

impl Drop for Servers {
    fn drop(&mut self) {
        for n in &self.0 {
            remote::registry().remove(n);
        }
    }
}

/// Connect to a stand-in server and sync it into `lib`.
fn add(srv: &Subsonic, name: &str, db: &Path) {
    let sc = ServerConfig {
        name: name.into(),
        url: srv.url.clone(),
        user: "zack".into(),
        kind: ServerKind::Subsonic,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    };
    let c = remote::connect(&sc, "pw").unwrap();
    let mut lib = Library::open(db).unwrap();
    sync::sync_library(
        &mut lib,
        name,
        ServerKind::Subsonic,
        c.as_ref(),
        // Every album, so a retag between syncs is seen.
        true,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    remote::registry().set_client(name, Some(c));
}

fn next(m: &MixService) -> MixEvent {
    m.events()
        .recv_timeout(Duration::from_secs(10))
        .expect("the mix answers")
}

fn started(m: &MixService) -> (Vec<TrackRow>, patina_core::mix::Similar) {
    match next(m) {
        MixEvent::Started {
            tracks, similar, ..
        } => (tracks, similar),
        other => panic!("{other:?}"),
    }
}

fn titles(t: &[TrackRow]) -> HashSet<String> {
    t.iter().map(|t| t.title.clone()).collect()
}

/// Loveless tagged Shoegaze and Dream Pop.
fn two_genres(srv: &Subsonic) {
    srv.state.lock().unwrap().albums[1].genre = "Shoegaze|Dream Pop".into();
}

#[test]
fn genre_mixes_take_albums_with_every_genre_once_each() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let a = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let b = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    two_genres(&a);
    two_genres(&b);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let _s = Servers(vec!["MixGA".into(), "MixGB".into()]);
    add(&a, "MixGA", &db);
    add(&b, "MixGB", &db);
    let m = MixService::start_with(&db, None).unwrap();
    let genre = |norms: &[&str]| MixSeed::Genre {
        norms: norms.iter().map(|s| s.to_string()).collect(),
        names: norms.iter().map(|s| s.to_string()).collect(),
        by_track: false,
    };

    // The same two songs on two servers: each plays once.
    m.play(1, genre(&["shoegaze"]), Avail::default());
    let (t, _) = started(&m);
    assert_eq!(
        titles(&t),
        HashSet::from(["Only Shallow".into(), "Loomer".into()])
    );
    assert_eq!(t.len(), 2, "{t:?}");

    // Both genres: the album carries both, so it's in.
    m.play(2, genre(&["shoegaze", "dream-pop"]), Avail::default());
    assert_eq!(started(&m).0.len(), 2);

    // Shoegaze and Grunge: no album carries both.
    m.play(3, genre(&["shoegaze", "grunge"]), Avail::default());
    assert!(matches!(next(&m), MixEvent::Empty { .. }));

    // One server away: the songs come from the other.
    let avail = Avail {
        offline: HashSet::from(["MixGA".to_string()]),
        kept: HashSet::new(),
    };
    m.play(4, genre(&["grunge"]), avail);
    let (t, _) = started(&m);
    assert_eq!(t.len(), 3);
    assert!(
        t.iter()
            .all(|t| remote::server_of(&t.path) == Some("MixGB")),
        "{t:?}"
    );
}

#[test]
fn artist_mixes_draw_on_the_servers_similar_artists_and_go_on() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    srv.state.lock().unwrap().similar_artists.insert(
        "Nirvana".into(),
        vec!["My Bloody Valentine".into(), "Not In Your Library".into()],
    );
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let _s = Servers(vec!["MixA".into()]);
    add(&srv, "MixA", &db);
    let m = MixService::start_with(&db, None).unwrap();
    m.play(
        7,
        MixSeed::Artist {
            name: "nirvana".into(),
        },
        Avail::default(),
    );
    let (t, similar) = started(&m);
    assert_eq!(similar.source, Source::Server);
    assert_eq!(
        similar.artists,
        vec!["My Bloody Valentine".to_string()],
        "only artists in your library"
    );
    assert_eq!(t.len(), patina_core::mix::BATCH);
    let artists: HashSet<String> = t.iter().filter_map(|t| t.artist.clone()).collect();
    assert_eq!(
        artists,
        HashSet::from(["Nirvana".to_string(), "My Bloody Valentine".to_string()])
    );
    // Five songs to pick from: a song never comes straight back.
    for w in t.windows(2) {
        assert_ne!(w[0].path, w[1].path);
    }
    // Endless.
    m.more(7, Avail::default());
    match next(&m) {
        MixEvent::More { run, tracks } => {
            assert_eq!(run, 7);
            assert_eq!(tracks.len(), patina_core::mix::BATCH);
        }
        other => panic!("{other:?}"),
    }
    // An older mix's request is ignored.
    m.more(6, Avail::default());
    assert!(m.events().recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn track_mixes_ask_the_server_then_last_fm_then_your_tags() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    // The server knows: Blew is like the Loveless songs.
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    srv.state
        .lock()
        .unwrap()
        .similar_songs
        .insert("11".into(), vec!["21".into(), "22".into()]);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let s = Servers(vec!["MixT".into()]);
    add(&srv, "MixT", &db);
    let m = MixService::start_with(&db, None).unwrap();
    let blew = MixSeed::Track {
        artist: "Nirvana".into(),
        title: "Blew".into(),
        server: Some("MixT".into()),
        id: Some("11".into()),
    };
    m.play(1, blew.clone(), Avail::default());
    let (t, similar) = started(&m);
    assert_eq!(similar.source, Source::Server);
    assert_eq!(t[0].title, "Blew", "the song itself leads");
    let found: HashSet<(String, String)> = similar.tracks.into_iter().collect();
    assert!(found.contains(&("My Bloody Valentine".into(), "Loomer".into())));
    drop(m);
    drop(s);

    // A server with no similarity data: Last.fm answers.
    let fm_calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let c2 = fm_calls.clone();
    let fm = common::serve(Arc::new(move |r: &common::Request| {
        let method = r.get("method").unwrap_or("").to_string();
        c2.lock().unwrap().push(method.clone());
        assert_eq!(r.get("api_key"), Some("KEY"));
        match method.as_str() {
            "track.getsimilar" => common::Response::json(serde_json::json!({
                "similartracks": {"track": [
                    {"name": "Loomer", "artist": {"name": "My Bloody Valentine"}},
                    {"name": "Not Here", "artist": {"name": "Nobody"}}
                ]}
            })),
            "artist.getsimilar" => common::Response::json(serde_json::json!({
                "similarartists": {"artist": {"name": "My Bloody Valentine"}}
            })),
            _ => {
                common::Response::json(serde_json::json!({"error": 3, "message": "Invalid Method"}))
            }
        }
    }));
    let srv2 = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let dir2 = tempfile::tempdir().unwrap();
    let db2 = dir2.path().join("lib.db");
    let _s2 = Servers(vec!["MixT2".into()]);
    add(&srv2, "MixT2", &db2);
    let m = MixService::start_with(&db2, LastFm::with_base(&fm, "KEY")).unwrap();
    m.play(
        2,
        MixSeed::Track {
            artist: "Nirvana".into(),
            title: "Blew".into(),
            server: Some("MixT2".into()),
            id: Some("11".into()),
        },
        Avail::default(),
    );
    let (t, similar) = started(&m);
    assert_eq!(similar.source, Source::LastFm);
    assert_eq!(
        similar.tracks,
        vec![("My Bloody Valentine".to_string(), "Loomer".to_string())],
        "only songs in your library"
    );
    assert!(titles(&t).contains("Loomer"));
    assert!(
        fm_calls
            .lock()
            .unwrap()
            .contains(&"track.getsimilar".to_string())
    );
    drop(m);

    // No server data, no Last.fm: artists that share a genre.
    srv2.state.lock().unwrap().albums[1].genre = "Shoegaze|Grunge".into();
    add(&srv2, "MixT2", &db2);
    let m = MixService::start_with(&db2, None).unwrap();
    m.play(
        3,
        MixSeed::Artist {
            name: "Nirvana".into(),
        },
        Avail::default(),
    );
    let (_, similar) = started(&m);
    assert_eq!(similar.source, Source::Tags);
    assert_eq!(similar.artists, vec!["My Bloody Valentine".to_string()]);
}

#[test]
fn saved_mixes_play_again_and_show_in_the_sidebar() {
    use patina_core::browse::{Browser, Section, SettingsInfo, SideEntry, Startup};
    use patina_core::config::Config;
    use patina_core::{Backend, OutputConfig, Player, PlayerConfig};
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    srv.state
        .lock()
        .unwrap()
        .similar_artists
        .insert("Nirvana".into(), vec!["My Bloody Valentine".into()]);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let _s = Servers(vec!["MixS".into()]);
    add(&srv, "MixS", &db);
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    let mut b = Browser::new(Startup {
        player,
        lib: Some(Library::open(&db).unwrap()),
        service: None,
        pod: None,
        remote: None,
        cache: None,
        mixes: Some(MixService::start_with(&db, None).unwrap()),
        config: Config::default(),
        settings_info: SettingsInfo {
            db_path: db.clone(),
            ..Default::default()
        },
    });
    b.switch(Section::Artists);
    let i = b
        .level()
        .unwrap()
        .items
        .iter()
        .position(|it| matches!(it, patina_core::browse::Item::Artist(a) if a.name == "Nirvana"))
        .unwrap();
    b.select(i);
    b.mix_selected();
    let wait = |b: &mut Browser, f: &dyn Fn(&Browser) -> bool| {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            b.poll();
            if f(b) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    assert!(wait(&mut b, &|b| b.mix.as_ref().is_some_and(|m| m.started)));
    assert_eq!(b.mix_title().as_deref(), Some("Nirvana mix"));
    assert!(wait(&mut b, &|b| b.player.queue().len()
        >= patina_core::mix::BATCH));

    // Endless: running low on songs tops the queue up.
    while b.player.queue().len() > 5 {
        let last = b.player.queue().len() - 1;
        b.remove_queue_position(last);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        wait(&mut b, &|b| b.player.queue().len()
            >= 5 + patina_core::mix::BATCH),
        "{} in the queue",
        b.player.queue().len()
    );

    b.save_mix();
    let saved = b.saved_mixes();
    assert_eq!(saved.len(), 1);
    assert!(
        b.sidebar
            .iter()
            .any(|e| matches!(e, SideEntry::Mix { name, .. } if name == "Nirvana mix"))
    );
    // Offline, a saved mix stands for the artist and who was similar.
    let lib = Library::open(&db).unwrap();
    let songs = patina_core::mix::resolve_saved(&lib, &saved[0]);
    assert_eq!(songs.len(), 5);

    // Playing something else ends the mix; the saved one starts it again.
    b.switch(Section::Albums);
    b.play_view(false);
    assert!(b.mix.is_none());
    b.play_saved_mix(saved[0].id);
    assert!(wait(&mut b, &|b| b
        .mix
        .as_ref()
        .is_some_and(|m| m.started && m.saved.is_some())));
    b.rename_mix(saved[0].id, "Grunge and noise");
    assert_eq!(b.saved_mixes()[0].name, "Grunge and noise");
    b.delete_mix(saved[0].id);
    assert!(b.saved_mixes().is_empty());
    assert!(!b.sidebar.iter().any(|e| matches!(e, SideEntry::Mix { .. })));
}
