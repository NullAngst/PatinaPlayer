//! The shared Browser that both frontends drive: navigation, search,
//! playlists, stations and the Settings lines it keeps current. Uses a real
//! indexed folder of FFmpeg-made files and the null output. Needs the
//! `ffmpeg` command on PATH.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use patina_core::browse::{Activated, Browser, Item, Section, Settings, Startup};
use patina_core::library::Library;
use patina_core::{Backend, OutputConfig, Player, PlayerConfig};

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
        "sine=f=440:r=44100:d=1",
    ]);
    for (k, v) in tags {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    let st = cmd.arg(path).status().expect("ffmpeg must be installed");
    assert!(st.success(), "ffmpeg failed for {}", path.display());
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
}

fn fixture() -> Fixture {
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
                ("genre", "Grunge"),
            ],
        );
    }
    make(
        &root.join("Other/Thing/01.flac"),
        &[
            ("title", "Other Thing"),
            ("artist", "Someone"),
            ("album", "Thing"),
            ("genre", "Folk"),
        ],
    );
    let config = base.join("config.toml");
    std::fs::write(&config, "# test config\n").unwrap();
    Fixture {
        _dir: dir,
        root,
        config,
    }
}

fn browser(fx: &Fixture) -> Browser {
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
        config_path: fx.config.clone(),
        stations: Vec::new(),
        feeds: Vec::new(),
        podcast_tempo: 1.0,
        podcast_trim: false,
        settings: Settings {
            lines: vec![
                ("Radio stations".into(), "0".into()),
                ("Podcast speed".into(), "1.0x".into()),
            ],
        },
    })
}

fn items(b: &Browser) -> &[Item] {
    &b.level().expect("a level is open").items
}

fn index_of(b: &Browser, f: impl Fn(&Item) -> bool) -> usize {
    items(b).iter().position(f).expect("item not found")
}

fn setting(b: &Browser, label: &str) -> String {
    b.settings
        .lines
        .iter()
        .find(|(l, _)| l == label)
        .map(|(_, v)| v.clone())
        .unwrap()
}

#[test]
fn drill_down_back_and_forward() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Albums);
    assert_eq!(items(&b).len(), 2);
    let i = index_of(&b, |it| matches!(it, Item::Album(a) if a.name == "Bleach"));
    assert!(b.select(i));
    assert!(matches!(b.activate(), Activated::Opened));
    assert_eq!(b.depth(), 2);
    assert_eq!(
        b.crumbs().last().map(String::as_str),
        Some("Bleach · Nirvana")
    );
    let titles: Vec<_> = items(&b)
        .iter()
        .filter_map(|it| match it {
            Item::Track(t) => Some(t.title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Bleach Song 1", "Bleach Song 2"]);

    assert!(b.pop());
    assert_eq!(b.depth(), 1);
    // Back leaves the selection where it was.
    assert_eq!(b.level().unwrap().selected, i);
    assert!(b.go_forward());
    assert_eq!(
        b.crumbs().last().map(String::as_str),
        Some("Bleach · Nirvana")
    );
    assert!(!b.go_forward());

    // Switching away and back keeps the drill-down.
    b.switch(Section::Songs);
    b.switch(Section::Albums);
    assert_eq!(b.depth(), 2);
}

#[test]
fn activating_a_track_plays_from_there() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    let i = index_of(
        &b,
        |it| matches!(it, Item::Track(t) if t.title == "Bleach Song 2"),
    );
    b.select(i);
    assert!(matches!(b.activate(), Activated::Played));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        b.poll();
        let snap = b.player.snapshot();
        if let Some(t) = &snap.track {
            assert_eq!(t.title.as_deref(), Some("Bleach Song 2"));
            // The whole list went in, so the queue holds every song.
            assert_eq!(b.player.queue().paths_in_order().len(), 3);
            break;
        }
        assert!(Instant::now() < deadline, "never started playing");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn search_groups_results() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    b.start_search();
    assert!(b.in_search());
    b.set_search("bleach");
    let it = items(&b);
    assert!(it.iter().any(|i| matches!(i, Item::Header("Albums"))));
    assert!(it.iter().any(|i| matches!(i, Item::Header("Songs"))));
    let songs = it.iter().filter(|i| matches!(i, Item::Track(_))).count();
    assert_eq!(songs, 2);
    assert!(
        !it.iter()
            .any(|i| matches!(i, Item::Track(t) if t.title == "Other Thing"))
    );
    // Headers can't be selected, so the first selection lands on a result.
    assert!(b.level().unwrap().selected_item().unwrap().selectable());
}

#[test]
fn playlists_from_a_selection() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Albums);
    let i = index_of(&b, |it| matches!(it, Item::Album(a) if a.name == "Bleach"));
    b.select(i);
    let paths = b.playlist_paths_for_selection();
    assert_eq!(paths.len(), 2);
    b.create_playlist("Mix", &paths);

    b.switch(Section::Playlists);
    let i = index_of(&b, |it| matches!(it, Item::Playlist(p) if p.name == "Mix"));
    b.select(i);
    assert!(matches!(b.activate(), Activated::Opened));
    let n = items(&b)
        .iter()
        .filter(|i| matches!(i, Item::Track(_)))
        .count();
    assert_eq!(n, 2);
}

#[test]
fn stations_save_to_the_config_and_settings_follow() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.save_station(None, " Test FM ", "http://127.0.0.1:9/stream");
    assert_eq!(b.stations.len(), 1);
    assert_eq!(b.stations[0].name, "Test FM");
    let text = std::fs::read_to_string(&fx.config).unwrap();
    assert!(text.contains("# test config"), "comments kept");
    assert!(text.contains("Test FM"));
    assert_eq!(setting(&b, "Radio stations"), "1");

    b.switch(Section::Radio);
    assert!(matches!(items(&b)[0], Item::Station { .. }));

    // A station needs both fields.
    b.save_station(None, "", "http://x");
    assert_eq!(b.stations.len(), 1);

    b.delete_station(0);
    assert!(b.stations.is_empty());
    assert_eq!(setting(&b, "Radio stations"), "0");
    assert!(
        !std::fs::read_to_string(&fx.config)
            .unwrap()
            .contains("Test FM")
    );
}

#[test]
fn podcast_speed_is_rounded_saved_and_shown() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.set_podcast_playback(1.25, true);
    assert!((b.podcast_tempo - 1.3).abs() < 1e-9);
    assert_eq!(setting(&b, "Podcast speed"), "1.3x, trimming silence");
    b.nudge_tempo(5.0);
    assert!((b.podcast_tempo - 3.0).abs() < 1e-9, "clamped to 3x");
    let text = std::fs::read_to_string(&fx.config).unwrap();
    assert!(text.contains("speed = 3.0"), "{text}");
    assert!(text.contains("trim_silence = true"));
}
