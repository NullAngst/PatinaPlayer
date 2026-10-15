//! The song cache: playing through it, prefetch, the size cap, pins,
//! converted downloads, podcast episodes, reading a download while it
//! runs, and what plays while a server is away. Runs against the
//! stand-in Subsonic server in `common`. Needs the `ffmpeg` command.

mod common;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Flavor, Subsonic};
use patina_core::browse::Item;
use patina_core::browse::offline::Availability;
use patina_core::cache::fetch::{Progress, State};
use patina_core::cache::reader::PartReader;
use patina_core::cache::{
    self, CacheConfig, CacheEvent, CacheService, Opening, Pin, PinKind, PrefetchMode, Roots,
    SongKey, Tier,
};
use patina_core::library::{ArtistMode, Library};
use patina_core::remote::{self, RemoteRef, ServerConfig, ServerKind, StreamFormat, sync};
use patina_core::{Backend, Command, OutputConfig, PlayState, Player, PlayerConfig, TrackInfo};

/// The player reaches the cache through one handle per process, so tests
/// that go through it take turns.
static TURN: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

struct Rig {
    srv: Subsonic,
    _dir: tempfile::TempDir,
    db: PathBuf,
    roots: Roots,
    name: String,
    cache: CacheService,
}

impl Rig {
    fn lib(&self) -> Library {
        Library::open(&self.db).unwrap()
    }

    fn key(&self, id: &str) -> SongKey {
        SongKey::new(self.name.clone(), id)
    }

    fn streams(&self) -> Vec<String> {
        self.srv.state.lock().unwrap().streams.clone()
    }

    fn album_key(&self, name: &str) -> String {
        self.lib()
            .albums(Default::default())
            .unwrap()
            .into_iter()
            .find(|a| a.name == name)
            .unwrap()
            .key
    }
}

/// A synced stand-in server, connected, with a cache in a temp folder.
/// `body` replaces the song the server sends, and its size is reported.
fn rig(name: &str, cfg: CacheConfig, body: Option<Vec<u8>>) -> Rig {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    if let Some(b) = body {
        let mut st = srv.state.lock().unwrap();
        for a in &mut st.albums {
            for s in &mut a.songs {
                s.size = b.len() as u64;
            }
        }
        st.stream_body = Some(Arc::new(b));
    }
    let sc = ServerConfig {
        name: name.into(),
        url: srv.url.clone(),
        user: "zack".into(),
        kind: ServerKind::Subsonic,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    };
    let client = remote::connect(&sc, "pw").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let mut lib = Library::open(&db).unwrap();
    sync::sync_library(
        &mut lib,
        name,
        ServerKind::Subsonic,
        client.as_ref(),
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    remote::registry().set_client(name, Some(client));
    let roots = Roots {
        cache: dir.path().join("cache"),
        offline: dir.path().join("offline"),
        podcasts: dir.path().join("podcasts"),
    };
    let cache = CacheService::start_at(&db, cfg, vec![name.into()], roots.clone()).unwrap();
    Rig {
        srv,
        _dir: dir,
        db,
        roots,
        name: name.into(),
        cache,
    }
}

fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

fn noise(len: usize) -> Vec<u8> {
    // Cheap pseudo-random bytes; the cache doesn't care what's in a file.
    let mut x: u32 = 0x9e37_79b9;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
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

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn songs_play_through_the_cache_and_the_next_ones_come_prefetched() {
    let _t = turn();
    let r = rig("CacheA", CacheConfig::default(), None);
    let lib = r.lib();
    let bleach = lib.album_tracks(&r.album_key("Bleach")).unwrap();
    let infos: Vec<TrackInfo> = bleach.iter().map(|t| t.to_info()).collect();
    let p = player();
    p.send(Command::SetQueueTracks {
        tracks: infos.clone(),
        start: 0,
        pos: Duration::ZERO,
    });
    // What the browser does on a queue change: the current song first.
    r.cache
        .upcoming(vec![r.key("11"), r.key("12"), r.key("13")]);

    assert!(
        wait_for(15, || p.snapshot().track.is_some_and(|t| t.codec == "FLAC")),
        "the first song plays from its download: {:?}",
        p.snapshot().last_error
    );
    assert!(
        wait_for(15, || r.cache.status().cache_files == 3),
        "played and prefetched songs are kept: {:?}",
        r.cache.status()
    );
    let files = files_under(&r.roots.cache);
    assert!(
        files.contains(&r.roots.cache.join("CacheA/Nirvana/Bleach/01 - Blew.flac")),
        "readable names from the server's paths: {files:?}"
    );
    assert!(files.iter().all(|f| f.extension().unwrap() == "flac"));
    let streams = r.streams();
    let calls: Vec<String> = r
        .srv
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.starts_with("stream "))
        .cloned()
        .collect();
    assert_eq!(streams.len(), 3, "one download per song: {calls:?}");
    assert!(streams.iter().all(|s| s == "raw"), "originals by default");
    let kept = std::fs::read(r.roots.cache.join("CacheA/Nirvana/Bleach/01 - Blew.flac")).unwrap();
    assert_eq!(kept, *common::audio(), "byte for byte what the server sent");

    // Played again: from disk, the server isn't asked.
    p.send(Command::SetQueueTracks {
        tracks: infos,
        start: 2,
        pos: Duration::ZERO,
    });
    assert!(wait_for(10, || {
        let s = p.snapshot();
        s.state == PlayState::Playing && s.current.is_some() && s.track.is_some()
    }));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.streams().len(), 3, "nothing new asked of the server");
    match cache::open(&bleach[1].remote().unwrap(), None) {
        Some(Ok(Opening::File { path, original })) => {
            assert!(path.starts_with(&r.roots.cache));
            assert!(original);
        }
        _ => panic!("a kept song opens as a file"),
    }
    p.send(Command::Quit);
}

#[test]
fn pinned_songs_live_offline_and_drop_back_into_the_cache() {
    let _t = turn();
    let r = rig("CacheB", CacheConfig::default(), None);
    let loveless = r.album_key("Loveless");
    r.cache
        .pin(Pin::new(PinKind::Album, loveless.clone(), "Loveless"));
    assert!(
        wait_for(15, || r.cache.status().offline_files == 2),
        "{:?}",
        r.cache.status()
    );
    let st = r.cache.status();
    assert_eq!(st.cache_files, 0, "pinned songs don't count as cache");
    assert_eq!(
        (st.pins.len(), st.pins[0].songs, st.pins[0].stored),
        (1, 2, 2)
    );
    let offline = files_under(&r.roots.offline);
    assert_eq!(offline.len(), 2, "{offline:?}");
    assert!(
        offline
            .iter()
            .all(|f| f.starts_with(r.roots.offline.join("CacheB")))
    );

    // Unpinned: they become ordinary cache, not deleted outright.
    let id = st.pins[0].pin.id;
    r.cache.unpin(id);
    assert!(wait_for(10, || {
        let s = r.cache.status();
        s.offline_files == 0 && s.cache_files == 2
    }));
    assert_eq!(files_under(&r.roots.cache).len(), 2);
    assert!(
        files_under(&r.roots.offline).is_empty(),
        "no empty folders left"
    );

    // Pinned again: moved back, not downloaded again.
    let before = r.streams().len();
    r.cache.pin(Pin::new(PinKind::Album, loveless, "Loveless"));
    assert!(wait_for(10, || r.cache.status().offline_files == 2));
    assert_eq!(r.streams().len(), before);

    // A song that's pinned plays from the offline folder.
    match cache::open(
        &RemoteRef {
            server: "CacheB".into(),
            id: "21".into(),
            episode: false,
        },
        None,
    ) {
        Some(Ok(Opening::File { path, .. })) => assert!(path.starts_with(&r.roots.offline)),
        _ => panic!("pinned songs open as files"),
    }
}

/// Play a song through the cache the way the decoder does, reading the
/// download to its end.
fn play_through(name: &str, id: &str) {
    let r = RemoteRef {
        server: name.into(),
        id: id.into(),
        episode: false,
    };
    match cache::open(&r, None) {
        Some(Ok(Opening::Growing { mut reader, .. })) => {
            let mut v = Vec::new();
            reader.read_to_end(&mut v).unwrap();
        }
        Some(Ok(Opening::File { .. })) => {}
        Some(Err(e)) => panic!("{e}"),
        None => panic!("the cache is on"),
    }
}

#[test]
fn the_cache_keeps_to_its_cap_least_recently_played_first() {
    let _t = turn();
    let cfg = CacheConfig {
        size_mb: 1,
        prefetch_mb: 0,
        ..Default::default()
    };
    let r = rig("CacheC", cfg, Some(noise(400 * 1024)));
    for (i, id) in ["11", "12", "13"].iter().enumerate() {
        play_through("CacheC", id);
        assert!(
            wait_for(10, || r.cache.stored().contains_key(&r.key(id))),
            "{id} is kept once played"
        );
        if i < 2 {
            // Last played is kept to the second.
            std::thread::sleep(Duration::from_millis(1100));
        }
    }
    // 1.2 MB of songs in a 1 MB cache: the first one played goes.
    assert!(wait_for(5, || r.cache.status().cache_files == 2));
    let kept = r.cache.stored();
    assert!(!kept.contains_key(&r.key("11")), "{kept:?}");
    assert!(kept.contains_key(&r.key("12")) && kept.contains_key(&r.key("13")));
    assert_eq!(files_under(&r.roots.cache).len(), 2, "its file went too");

    // Clearing empties it.
    r.cache.clear_cache();
    assert!(wait_for(5, || r.cache.status().cache_files == 0));
    assert!(files_under(&r.roots.cache).is_empty());
}

#[test]
fn prefetch_stops_at_its_size_or_its_count() {
    let _t = turn();
    let cfg = CacheConfig {
        size_mb: 10,
        prefetch: PrefetchMode::Size,
        prefetch_mb: 1,
        ..Default::default()
    };
    let r = rig("CacheD", cfg.clone(), Some(noise(400 * 1024)));
    let all: Vec<SongKey> = ["11", "12", "13", "21", "22"]
        .iter()
        .map(|i| r.key(i))
        .collect();
    r.cache.upcoming(all.clone());
    // 400 KB songs in 1 MB: two fit, nearest first.
    assert!(wait_for(10, || r.cache.status().cache_files == 2));
    std::thread::sleep(Duration::from_millis(400));
    let kept = r.cache.stored();
    assert_eq!(kept.len(), 2);
    assert!(kept.contains_key(&all[0]) && kept.contains_key(&all[1]));

    r.cache.set_config(CacheConfig {
        prefetch: PrefetchMode::Tracks,
        prefetch_tracks: 4,
        ..cfg
    });
    assert!(wait_for(10, || r.cache.status().cache_files == 4));
    std::thread::sleep(Duration::from_millis(400));
    assert!(!r.cache.stored().contains_key(&all[4]), "the fifth waits");
}

#[test]
fn songs_can_be_kept_converted_to_save_space() {
    let _t = turn();
    let cfg = CacheConfig {
        format: StreamFormat::Opus,
        bitrate: 128,
        ..Default::default()
    };
    let r = rig("CacheE", cfg, None);
    r.cache.upcoming(vec![r.key("11")]);
    assert!(wait_for(10, || r.cache.status().cache_files == 1));
    assert_eq!(r.streams(), vec!["opus:128".to_string()]);
    let files = files_under(&r.roots.cache);
    assert!(
        files[0].to_string_lossy().ends_with("01 - Blew.opus"),
        "{files:?}"
    );
    match cache::open(
        &RemoteRef {
            server: "CacheE".into(),
            id: "11".into(),
            episode: false,
        },
        None,
    ) {
        Some(Ok(Opening::File { original, .. })) => {
            assert!(!original, "a converted song can't play bit-perfect")
        }
        _ => panic!("kept songs open as files"),
    }
}

#[test]
fn podcast_episodes_download_through_the_same_queue() {
    let _t = turn();
    let r = rig("CacheF", CacheConfig::default(), None);
    let body = noise(200 * 1024);
    let b2 = Arc::new(body.clone());
    let url = common::serve(Arc::new(move |req: &common::Request| {
        if req.path == "/ep.mp3" {
            common::Response::bytes("audio/mpeg", b2.to_vec())
        } else {
            common::Response::not_found()
        }
    }));
    let mut lib = r.lib();
    let feed = format!("{url}/feed.xml");
    let pid = lib
        .store_feed(
            &feed,
            &patina_core::library::podcasts::FeedData {
                title: Some("Local Show".into()),
                author: None,
                description: None,
                image_url: None,
                link: None,
                episodes: vec![patina_core::library::podcasts::EpisodeData {
                    guid: "g1".into(),
                    title: "First".into(),
                    published: Some(1_760_000_000),
                    duration: None,
                    audio_url: format!("{url}/ep.mp3"),
                    mime: Some("audio/mpeg".into()),
                    size: Some(body.len() as i64),
                    description: None,
                }],
            },
        )
        .unwrap();
    let ep = lib.episodes(pid).unwrap()[0].clone();
    r.cache.download_episode(ep.id);
    let mut done = false;
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !done {
        if let Ok(CacheEvent::EpisodeDone(id)) =
            r.cache.events().recv_timeout(Duration::from_millis(100))
        {
            done = id == ep.id;
        }
    }
    assert!(done, "the episode downloads");
    let path = lib.episode(ep.id).unwrap().unwrap().download.unwrap();
    assert!(path.starts_with(&r.roots.podcasts), "{path:?}");
    assert_eq!(std::fs::read(&path).unwrap(), body);
}

#[test]
fn a_song_decodes_the_same_while_it_downloads() {
    // Three seconds of noise as FLAC: nothing compresses away.
    let dir = tempfile::tempdir().unwrap();
    let full = dir.path().join("full.flac");
    let st = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("anoisesrc=d=3:c=white:r=44100:a=0.3")
        .args(["-ac", "2", "-sample_fmt", "s16"])
        .arg(&full)
        .status()
        .unwrap();
    assert!(st.success());
    let bytes = std::fs::read(&full).unwrap();

    let decode = |mut src: patina_core::audio::decoder::Source| {
        let mut all = Vec::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = src.decode_into(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf);
        }
        all
    };
    let want = decode(patina_core::audio::decoder::Source::open(&full).unwrap());

    // The same file, arriving in pieces while it's read.
    let dest = dir.path().join("song.flac");
    let part = dir.path().join("song.flac.part");
    std::fs::write(&part, b"").unwrap();
    let progress = Arc::new(Progress::new());
    progress.preset(0, Some(bytes.len() as u64));
    progress.set_state(State::Running);
    let (p2, part2, b2) = (progress.clone(), part.clone(), bytes.clone());
    let writer = std::thread::spawn(move || {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&part2)
            .unwrap();
        let mut done = 0;
        for chunk in b2.chunks(16 * 1024) {
            std::thread::sleep(Duration::from_millis(4));
            f.write_all(chunk).unwrap();
            done += chunk.len();
            p2.preset(done as u64, None);
        }
        p2.set_state(State::Done);
    });
    let reader = PartReader::open(&dest, &part, progress, None, None).unwrap();
    let src =
        patina_core::audio::decoder::Source::open_reader(reader, "song.flac", Default::default())
            .unwrap();
    assert_eq!(src.info().codec, "FLAC");
    assert_eq!(src.info().bits, Some(16));
    let got = decode(src);
    writer.join().unwrap();
    assert_eq!(got.len(), want.len(), "every sample, no more, no fewer");
    assert!(got == want, "sample for sample the same");
}

#[test]
fn a_read_far_ahead_of_the_download_asks_the_server_for_that_spot() {
    let body = Arc::new(noise(2 * 1024 * 1024));
    let ranges: Arc<Mutex<Vec<String>>> = Arc::default();
    let (b2, r2) = (body.clone(), ranges.clone());
    let url = common::serve(Arc::new(move |req: &common::Request| {
        if let Some(r) = req.headers.get("range") {
            r2.lock().unwrap().push(r.clone());
        }
        common::Response::bytes("audio/flac", b2.to_vec())
    }));
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("s.flac.part");
    // Only the first 64 KB are in, and the download has stalled there.
    std::fs::write(&part, &body[..64 * 1024]).unwrap();
    let progress = Arc::new(Progress::new());
    progress.preset(64 * 1024, Some(body.len() as u64));
    progress.set_state(State::Running);
    let mut r = PartReader::open(
        &dir.path().join("s.flac"),
        &part,
        progress,
        Some(format!("{url}/song")),
        None,
    )
    .unwrap();
    let mut head = [0u8; 100];
    r.read_exact(&mut head).unwrap();
    assert_eq!(&head[..], &body[..100], "from the file");
    let at = body.len() as u64 - 1000;
    r.seek(SeekFrom::Start(at)).unwrap();
    let mut tail = Vec::new();
    r.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, body[at as usize..], "from the server, by range");
    assert_eq!(ranges.lock().unwrap().as_slice(), [format!("bytes={at}-")]);
}

#[test]
fn while_a_server_is_away_only_kept_songs_play() {
    let r = {
        let _t = turn();
        rig("CacheG", CacheConfig::default(), None)
    };
    let lib = r.lib();
    let offline: HashSet<String> = ["CacheG".to_string()].into();
    let kept: HashMap<SongKey, Tier> = [(r.key("11"), Tier::Cache)].into();
    let a = Availability::compute(&lib, offline, &kept);
    assert!(a.any_offline());
    let songs = lib.songs(Default::default(), false).unwrap();
    let plays = |id: &str| {
        let t = songs
            .iter()
            .find(|t| t.remote_id.as_deref() == Some(id))
            .unwrap();
        a.plays(&Item::Track(t.clone()), ArtistMode::AlbumArtist)
    };
    assert!(plays("11"), "kept");
    assert!(!plays("12"), "not kept, server away");
    let albums = lib.albums(Default::default()).unwrap();
    let album = |n: &str| Item::Album(albums.iter().find(|x| x.name == n).unwrap().clone());
    assert!(a.plays(&album("Bleach"), ArtistMode::AlbumArtist));
    assert!(!a.plays(&album("Loveless"), ArtistMode::AlbumArtist));
    let artists = lib.artists(ArtistMode::AlbumArtist).unwrap();
    let artist = |n: &str| Item::Artist(artists.iter().find(|x| x.name == n).unwrap().clone());
    assert!(a.plays(&artist("Nirvana"), ArtistMode::AlbumArtist));
    assert!(!a.plays(&artist("My Bloody Valentine"), ArtistMode::AlbumArtist));
    let genres = lib.genres().unwrap();
    let genre = |n: &str| Item::Genre(genres.iter().find(|x| x.name == n).unwrap().clone());
    assert!(a.plays(&genre("Grunge"), ArtistMode::AlbumArtist));
    assert!(!a.plays(&genre("Shoegaze"), ArtistMode::AlbumArtist));
    let dir = |p: &str| Item::Dir {
        path: remote::root("CacheG").join(p),
        name: p.into(),
    };
    assert!(a.plays(&dir("Nirvana"), ArtistMode::AlbumArtist));
    assert!(a.plays(&dir("Nirvana/Bleach"), ArtistMode::AlbumArtist));
    assert!(!a.plays(&dir("My Bloody Valentine"), ArtistMode::AlbumArtist));
    // Everything plays again once it's back.
    let back = Availability::compute(&lib, HashSet::new(), &kept);
    assert!(back.plays(&album("Loveless"), ArtistMode::AlbumArtist));
}

#[test]
fn settings_keep_prefetch_inside_the_cache_and_views_pin_whole() {
    use patina_core::browse::{Browser, Change, Key, Section, SettingsInfo, Startup};
    use patina_core::config::Config;
    let _t = turn();
    let Rig {
        srv: _srv,
        _dir,
        db,
        cache,
        ..
    } = rig("CacheH", CacheConfig::default(), None);
    let config = db.with_file_name("config.toml");
    std::fs::write(&config, "# cache test\n").unwrap();
    let mut b = Browser::new(Startup {
        player: player(),
        lib: Some(Library::open(&db).unwrap()),
        service: None,
        pod: None,
        remote: None,
        cache: Some(cache),
        mixes: None,
        config: Config::default(),
        settings_info: SettingsInfo {
            config_path: config.clone(),
            db_path: db.clone(),
            ..Default::default()
        },
    });
    b.take_notes();

    // 10 GB of cache, 1 GB of prefetch: prefetch can't go past the cache,
    // and the cache can't shrink under prefetch.
    b.change_setting(&Key::PrefetchAmount, Change::Set("20000".into()));
    assert_eq!(b.config.cache.prefetch_mb, 1024, "refused");
    assert!(b.take_notes().iter().any(|n| matches!(n,
        patina_core::browse::Note::Error(e) if e.contains("Make the cache bigger"))));
    b.change_setting(&Key::CacheSize, Change::Set("0.5".into()));
    assert_eq!(b.config.cache.size_mb, 10 * 1024, "refused");
    b.change_setting(&Key::PrefetchAmount, Change::Set("256".into()));
    assert_eq!(b.config.cache.prefetch_mb, 256);
    b.change_setting(&Key::CacheSize, Change::Set("0.5".into()));
    assert_eq!(b.config.cache.size_mb, 512, "now it fits");
    let saved = Config::load_from(&config).unwrap();
    assert_eq!((saved.cache.size_mb, saved.cache.prefetch_mb), (512, 256));

    // Open a server album and keep the whole view offline.
    b.switch(Section::Albums);
    let i = b
        .level()
        .unwrap()
        .items
        .iter()
        .position(|it| matches!(it, Item::Album(a) if a.name == "Loveless"))
        .unwrap();
    b.select(i);
    b.activate();
    assert_eq!(b.view_pin_state(), Some(false));
    b.toggle_pin_view();
    assert!(wait_for(10, || {
        b.poll();
        b.cache_status.offline_files == 2
    }));
    assert_eq!(b.view_pin_state(), Some(true));
    b.switch(Section::Downloads);
    let items = &b.level().unwrap().items;
    assert!(
        items
            .iter()
            .any(|it| matches!(it, Item::Pinned(p) if p.pin.name == "Loveless" && p.stored == 2)),
        "{items:?}"
    );
    // Local files have nothing to keep.
    let local = Item::Album(patina_core::library::AlbumRow {
        key: "x".into(),
        name: "Local".into(),
        artist: "Me".into(),
        year: None,
        genre: None,
        track_count: 1,
        duration: Duration::ZERO,
        added_at: 0,
        play_count: 0,
        dir: PathBuf::from("/music/Me/Local"),
        lossless: true,
        cover_track: None,
    });
    assert!(!b.can_pin(&local));
}
