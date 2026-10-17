//! The shared Browser that both frontends drive: navigation, search,
//! playlists, stations and the Settings lines it keeps current. Uses a real
//! indexed folder of FFmpeg-made files and the null output. Needs the
//! `ffmpeg` command on PATH.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use patina_core::browse::{
    Activated, Browser, Change, Control, Item, Key, Section, SettingsInfo, Startup,
};
use patina_core::config::Config;
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

fn items(b: &Browser) -> &[Item] {
    &b.level().expect("a level is open").items
}

fn index_of(b: &Browser, f: impl Fn(&Item) -> bool) -> usize {
    items(b).iter().position(f).expect("item not found")
}

/// The value a Settings row shows.
fn setting(b: &mut Browser, key: Key) -> String {
    b.switch(Section::Settings);
    items(b)
        .iter()
        .find_map(|i| match i {
            Item::Setting(s) if s.key == key => Some(s.control.display()),
            _ => None,
        })
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
fn a_view_or_the_queue_goes_into_an_existing_playlist_without_duplicates() {
    let fx = fixture();
    let mut b = browser(&fx);

    // One song of Bleach in a playlist to start with.
    b.switch(Section::Songs);
    let i = index_of(
        &b,
        |it| matches!(it, Item::Track(t) if t.title == "Bleach Song 1"),
    );
    b.select(i);
    let one = b.playlist_paths_for_selection();
    b.create_playlist("Mix", &one);
    let mix = b.playlists().into_iter().find(|p| p.name == "Mix").unwrap();
    assert_eq!(mix.items, 1);

    // The whole Folders root view: all three songs, one already there.
    b.switch(Section::Folders);
    b.select(0);
    assert!(matches!(b.activate(), Activated::Opened));
    let view = b.view_playlist_paths();
    assert_eq!(view.len(), 3);
    b.add_to_playlist(mix.id, &mix.name, &view);
    let mix = b.playlists().into_iter().find(|p| p.name == "Mix").unwrap();
    assert_eq!(mix.items, 3, "the song already in it is left out");
    let notes: Vec<String> = b
        .take_notes()
        .into_iter()
        .map(|n| format!("{n:?}"))
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("Added 2 tracks") && n.contains("left out 1")),
        "{notes:?}"
    );

    // The queue: the view queued twice over, plus a radio station. Only
    // the songs can go in, each once.
    b.queue_view();
    b.queue_view();
    b.player.send(patina_core::Command::AppendTracks(vec![
        patina_core::browse::station_info(&patina_core::config::Station {
            name: "Live".into(),
            url: "http://127.0.0.1:1/live".into(),
        }),
    ]));
    wait_queue(&b, "six songs and a station", |q| q.len() == 7);
    let queued = b.queue_playlist_paths();
    assert_eq!(queued.len(), 6, "no station in the list");
    b.create_playlist("From the queue", &queued);
    let q = b
        .playlists()
        .into_iter()
        .find(|p| p.name == "From the queue")
        .unwrap();
    assert_eq!(q.items, 3, "each song once");
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

    b.switch(Section::Radio);
    assert!(matches!(items(&b)[0], Item::Station { .. }));

    // A station needs both fields.
    b.save_station(None, "", "http://x");
    assert_eq!(b.stations.len(), 1);

    b.delete_station(0);
    assert!(b.stations.is_empty());
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
    assert_eq!(setting(&mut b, Key::PodcastSpeed), "1.3x");
    assert_eq!(setting(&mut b, Key::PodcastTrim), "on");
    b.nudge_tempo(5.0);
    assert!((b.podcast_tempo - 3.0).abs() < 1e-9, "clamped to 3x");
    let text = std::fs::read_to_string(&fx.config).unwrap();
    assert!(text.contains("speed = 3.0"), "{text}");
    assert!(text.contains("trim_silence = true"));
}

/// Wait until the player's queue matches `f`.
fn wait_queue(b: &Browser, what: &str, f: impl Fn(&patina_core::queue::Queue) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f(&b.player.queue()) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_folder_plays_its_whole_tree_in_order_or_shuffled() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Folders);
    b.select(0);
    assert!(matches!(b.activate(), Activated::Opened));
    // The library root holds two artist folders and no files of its own.
    assert!(items(&b).iter().all(|i| matches!(i, Item::Dir { .. })));
    assert!(b.can_play_view());

    let mut expected: Vec<PathBuf> = b.view_tracks().into_iter().map(|t| t.path).collect();
    assert_eq!(expected.len(), 3);
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(expected, sorted, "folder order");

    b.play_view(false);
    wait_queue(&b, "the folder in order", |q| {
        q.paths_in_order() == expected && !q.shuffle()
    });

    b.play_view(true);
    wait_queue(&b, "the folder shuffled", |q| q.shuffle() && q.len() == 3);
    let mut got = b.player.queue().paths_in_order();
    got.sort();
    expected.sort();
    assert_eq!(got, expected, "same songs, any order");

    // Turning shuffle off afterwards goes back to folder order.
    b.player.send(patina_core::Command::ToggleShuffle);
    wait_queue(&b, "folder order again", |q| {
        !q.shuffle() && q.paths_in_order() == sorted
    });
}

#[test]
fn only_library_views_play_as_a_whole() {
    let fx = fixture();
    let mut b = browser(&fx);
    for s in [
        Section::Radio,
        Section::Genres,
        Section::Playlists,
        Section::Settings,
    ] {
        b.switch(s);
        assert!(!b.can_play_view(), "{s:?}");
    }
    for s in [Section::Albums, Section::Artists, Section::Songs] {
        b.switch(s);
        assert!(b.can_play_view(), "{s:?}");
        assert_eq!(b.view_tracks().len(), 3, "{s:?}");
    }
}

#[test]
fn listening_stats_rank_by_plays() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::MostListened);
    assert!(items(&b).is_empty(), "nothing played yet");
    assert!(!b.can_play_view());

    let path = |rel: &str| fx.root.join(rel);
    {
        let lib = b.lib.as_ref().unwrap();
        for _ in 0..3 {
            lib.record_play(&path("Nirvana/Bleach/02.flac")).unwrap();
        }
        lib.record_play(&path("Nirvana/Bleach/01.flac")).unwrap();
        for _ in 0..2 {
            lib.record_play(&path("Other/Thing/01.flac")).unwrap();
        }
    }
    b.refresh_all();

    let names: Vec<String> = items(&b)
        .iter()
        .map(|i| match i {
            Item::Header(h) => format!("[{h}]"),
            Item::Track(t) => format!("{} {}", t.title, t.play_count),
            Item::Album(a) => format!("{} {}", a.name, a.play_count),
            Item::Artist(a) => format!("{} {}", a.name, a.plays),
            _ => "?".into(),
        })
        .collect();
    assert_eq!(
        names,
        [
            "[Songs]",
            "Bleach Song 2 3",
            "Other Thing 2",
            "Bleach Song 1 1",
            "[Albums]",
            "Bleach 4",
            "Thing 2",
            "[Artists]",
            "Nirvana 4",
            "Someone 2",
        ]
    );
    // Grouped, so playing it all plays the songs once each.
    assert_eq!(b.view_tracks().len(), 3);

    b.switch(Section::TopAlbums);
    assert!(matches!(&items(&b)[0], Item::Album(a) if a.name == "Bleach"));
    b.switch(Section::TopArtists);
    assert!(matches!(&items(&b)[0], Item::Artist(a) if a.name == "Nirvana" && a.plays == 4));
    // Open the top artist and their albums are there.
    b.select(0);
    assert!(matches!(b.activate(), Activated::Opened));
    assert!(matches!(&items(&b)[0], Item::Album(a) if a.name == "Bleach"));
}

#[test]
fn settings_are_saved_and_applied() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Settings);
    // Every group is there.
    for h in [
        "Library",
        "Appearance",
        "Playback",
        "Output",
        "Bit-perfect",
        "Podcasts",
        "Files",
    ] {
        assert!(
            items(&b)
                .iter()
                .any(|i| matches!(i, Item::Header(x) if *x == h)),
            "{h}"
        );
    }

    b.change_setting(&Key::BufferMs, Change::Step(2));
    assert_eq!(setting(&mut b, Key::BufferMs), "300 ms");
    b.change_setting(&Key::FadeMs, Change::Set("0".into()));
    assert_eq!(setting(&mut b, Key::FadeMs), "0 ms");
    // Output controls.
    b.change_setting(&Key::OutRate, Change::Choose(2));
    assert_eq!(setting(&mut b, Key::OutRate), "48 kHz");
    b.change_setting(&Key::Resampler, Change::Step(1));
    assert_eq!(setting(&mut b, Key::Resampler), "Best");
    b.change_setting(&Key::Crossfade, Change::Set("2.5".into()));
    assert_eq!(setting(&mut b, Key::Crossfade), "2.5 s");
    b.change_setting(&Key::Dither, Change::Toggle);
    b.change_setting(&Key::OutFormat, Change::Choose(4));

    // Bit-perfect asks first, then saves.
    assert!(
        b.setting_warning(&Key::BitPerfect, &Change::Step(1))
            .is_some()
    );
    b.change_setting(&Key::BitPerfect, Change::Step(1));
    assert!(
        b.setting_warning(&Key::BitPerfect, &Change::Step(1))
            .is_none(),
        "already on"
    );

    // The theme switches live and is saved.
    b.change_setting(&Key::Theme, Change::Choose(5));
    assert_eq!(setting(&mut b, Key::Theme), "Kawaii Dark");
    b.change_setting(&Key::Theme, Change::Choose(3));
    assert_eq!(setting(&mut b, Key::Theme), "OLED (true black)");
    assert_eq!(b.config.appearance.theme, patina_core::config::Theme::Oled);

    let cfg = Config::load_from(&fx.config).unwrap();
    assert_eq!(cfg.appearance.theme, patina_core::config::Theme::Oled);
    assert_eq!(cfg.output.buffer_ms, 300);
    assert_eq!(cfg.playback.fade_ms, 0);
    assert_eq!(cfg.output.rate, 48000);
    assert_eq!(
        cfg.output.resampler,
        patina_core::dsp::resample::Quality::Best
    );
    assert_eq!(cfg.output.crossfade_ms, 2500);
    assert!(!cfg.output.dither);
    assert_eq!(
        cfg.output.format,
        patina_core::audio::output::SampleFormat::S16
    );
    assert_eq!(
        cfg.output.bit_perfect,
        patina_core::audio::output::BitPerfect::On
    );
    let text = std::fs::read_to_string(&fx.config).unwrap();
    assert!(text.contains("# test config"), "comments kept");
}

#[test]
fn folders_are_added_and_removed_from_the_app() {
    let fx = fixture();
    let mut b = browser(&fx);
    let extra = tempfile::tempdir().unwrap();
    let extra_path = std::fs::canonicalize(extra.path()).unwrap();

    assert!(!b.add_folder("/no/such/folder"));
    assert!(b.add_folder(extra_path.to_str().unwrap()));
    assert!(!b.add_folder(extra_path.to_str().unwrap()), "already there");
    let cfg = Config::load_from(&fx.config).unwrap();
    assert!(cfg.library.folders.contains(&extra_path));

    b.switch(Section::Settings);
    assert!(items(&b).iter().any(|i| matches!(
        i,
        Item::Setting(s) if s.key == Key::Folder(extra_path.clone())
            && s.control == Control::Folder
    )));

    b.remove_folder(&extra_path);
    let cfg = Config::load_from(&fx.config).unwrap();
    assert!(!cfg.library.folders.contains(&extra_path));
}

#[test]
fn a_song_goes_to_its_album_and_its_artist() {
    use patina_core::browse::Source;
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    let i = index_of(
        &b,
        |it| matches!(it, Item::Track(t) if t.title == "Bleach Song 2"),
    );
    b.select(i);
    b.go_to_selected(false);
    assert_eq!(b.section, Section::Albums);
    assert_eq!(b.depth(), 2);
    assert!(matches!(&b.level().unwrap().source, Source::Album { .. }));
    // The song is the one selected there.
    let sel = b.level().unwrap().selected;
    assert!(matches!(&items(&b)[sel], Item::Track(t) if t.title == "Bleach Song 2"));

    b.switch(Section::Songs);
    b.go_to_selected(true);
    assert_eq!(b.section, Section::Artists);
    assert_eq!(b.depth(), 2);
    assert!(matches!(&b.level().unwrap().source, Source::ArtistAlbums(n) if n == "Nirvana"));
    assert!(
        items(&b)
            .iter()
            .any(|it| matches!(it, Item::Album(a) if a.name == "Bleach"))
    );

    // An album isn't a song: nothing moves.
    b.switch(Section::Albums);
    b.pop();
    b.go_to_selected(true);
    assert_eq!(b.section, Section::Albums);
}
