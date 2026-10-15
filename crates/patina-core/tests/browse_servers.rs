//! Servers from the Browser's side, the way both apps use them: add one,
//! watch it sync into the views, edit its playlists, keep the queue on it,
//! remove it. Against the stand-in Navidrome in `common`. Needs `ffmpeg`.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Flavor, Subsonic};
use patina_core::browse::{Browser, Item, Key, Note, Section, SettingsInfo, SideEntry, Startup};
use patina_core::config::Config;
use patina_core::library::Library;
use patina_core::remote::{RemoteService, ServerState};
use patina_core::secrets::Secrets;
use patina_core::{Backend, Command, OutputConfig, Player, PlayerConfig};

struct Setup {
    _dir: tempfile::TempDir,
    b: Browser,
    config: std::path::PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "# servers test\n").unwrap();
    let secrets = Arc::new(Secrets::with(None, dir.path().join("secrets.toml")));
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    let b = Browser::new(Startup {
        player,
        lib: Some(Library::open(&db).unwrap()),
        service: None,
        pod: None,
        remote: Some(RemoteService::start(&db, Vec::new(), secrets)),
        cache: None,
        mixes: None,
        level: None,
        config: Config::default(),
        settings_info: SettingsInfo {
            config_path: config.clone(),
            db_path: db,
            ..Default::default()
        },
    });
    Setup {
        _dir: dir,
        b,
        config,
    }
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

fn names(b: &Browser) -> Vec<String> {
    b.level()
        .map(|l| {
            l.items
                .iter()
                .map(|i| match i {
                    Item::Album(a) => a.name.clone(),
                    Item::Track(t) => t.title.clone(),
                    Item::Playlist(p) => p.name.clone(),
                    Item::Station { station, .. } => station.name.clone(),
                    Item::Header(h) => format!("[{h}]"),
                    Item::Dir { name, .. } => name.clone(),
                    Item::Root { path, .. } => path.display().to_string(),
                    other => format!("{other:?}"),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_server_is_added_synced_used_and_removed() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let Setup {
        _dir,
        mut b,
        config,
    } = setup();

    // A wrong password doesn't get saved.
    b.add_server("Home", &srv.url, "zack", "nope");
    wait(&mut b, "the refusal", |b| {
        b.take_notes()
            .iter()
            .any(|n| matches!(n, Note::Error(e) if e.contains("wrong user name or password")))
    });
    assert!(b.config.servers.is_empty());

    b.add_server("Home", &srv.url, "zack", "pw");
    wait(&mut b, "the sync", |b| {
        b.servers
            .iter()
            .any(|s| s.name == "Home" && s.state == ServerState::Ready && s.last_sync.is_some())
    });
    // Saved to the config, password not in it.
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("[[server]]") && text.contains("name = \"Home\""));
    assert!(!text.contains("pw\""));
    assert!(text.contains("# servers test"), "comments kept");
    assert_eq!(
        b.config.servers[0].kind,
        patina_core::remote::ServerKind::Subsonic
    );

    // The sidebar shows it as a source, and its songs are in the views.
    assert!(b.sidebar.iter().any(|e| matches!(
        e,
        SideEntry::Source {
            server: Some(ServerState::Ready),
            tracks: 5,
            ..
        }
    )));
    b.switch(Section::Albums);
    wait(&mut b, "albums", |b| names(b).len() == 2);
    assert_eq!(names(&b), vec!["Bleach", "Loveless"]);

    // Its station is in Radio, read-only.
    b.switch(Section::Radio);
    assert_eq!(names(&b), vec!["[On your servers]", "Server FM"]);

    // Settings has it, with its streaming choice.
    b.switch(Section::Settings);
    let keys: Vec<Key> = b
        .level()
        .unwrap()
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Setting(s) => Some(s.key.clone()),
            _ => None,
        })
        .collect();
    assert!(keys.contains(&Key::Server("Home".into())));
    assert!(keys.contains(&Key::ServerStream("Home".into())));
    assert!(keys.contains(&Key::AddServer));
    b.change_setting(
        &Key::ServerStream("Home".into()),
        patina_core::browse::Change::Choose(2),
    );
    assert_eq!(
        b.config.servers[0].stream,
        patina_core::remote::StreamFormat::Opus
    );
    assert_eq!(b.config.servers[0].bitrate, 128);
    b.change_setting(
        &Key::ServerStream("Home".into()),
        patina_core::browse::Change::Choose(0),
    );

    // A playlist on the server, filled from a view.
    b.create_server_playlist("Home", "Road", &[]);
    wait(&mut b, "the new server playlist", |b| {
        b.playlists().iter().any(|p| p.name == "Road")
    });
    let road = b
        .playlists()
        .into_iter()
        .find(|p| p.name == "Road")
        .unwrap();
    assert_eq!(road.server.as_deref(), Some("Home"));
    b.switch(Section::Albums);
    b.select(0);
    assert!(matches!(
        b.activate(),
        patina_core::browse::Activated::Opened
    ));
    let paths = b.view_playlist_paths();
    assert_eq!(paths.len(), 3);
    b.add_to_playlist(road.id, &road.name, &paths);
    wait(&mut b, "the server to have the songs", |_| {
        srv.state
            .lock()
            .unwrap()
            .playlists
            .iter()
            .any(|p| p.name == "Road" && p.songs.len() == 3)
    });
    // Local files can't go in a server playlist.
    b.add_to_playlist(
        road.id,
        &road.name,
        &[Path::new("/music/x.flac").to_path_buf()],
    );
    assert!(
        b.take_notes()
            .iter()
            .any(|n| matches!(n, Note::Info(m) if m.contains("Only songs from Home")))
    );

    // Playing the album saves the queue on the server.
    b.play_view(false);
    wait(&mut b, "the queue on the server", |_| {
        srv.state
            .lock()
            .unwrap()
            .queue
            .as_ref()
            .is_some_and(|q| q.0 == vec!["11", "12", "13"])
    });
    // And a queue saved elsewhere loads back in.
    srv.state.lock().unwrap().queue =
        Some((vec!["22".into(), "21".into()], Some("21".into()), 500));
    b.player.send(Command::ClearQueue);
    b.load_server_queue("Home");
    wait(&mut b, "the loaded queue", |b| {
        let q = b.player.queue();
        q.len() == 2
            && q.at(0)
                .is_some_and(|e| e.info.title.as_deref() == Some("Loomer"))
    });

    // Removing it takes its songs, playlists and password.
    b.remove_server("Home");
    wait(&mut b, "the songs to go", |b| {
        b.lib.as_ref().unwrap().track_count().unwrap() == 0
    });
    assert!(b.playlists().is_empty());
    assert!(b.config.servers.is_empty());
    assert!(
        !std::fs::read_to_string(&config)
            .unwrap()
            .contains("[[server]]")
    );
    let secrets = std::fs::read_to_string(_dir.path().join("secrets.toml")).unwrap_or_default();
    assert!(!secrets.contains("zack"), "{secrets}");
    b.player.send(Command::Quit);
}
