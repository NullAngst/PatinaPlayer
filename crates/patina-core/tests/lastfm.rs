//! Last.fm against a stand-in Last.fm that checks every signature: linking
//! through the approval page, Now Playing and scrobbles, the offline queue,
//! love, and who tells Last.fm about a server's songs. Needs `ffmpeg`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Flavor, Request, Response, Subsonic};
use md5::{Digest, Md5};
use patina_core::browse::{Browser, SettingsInfo, Startup};
use patina_core::config::{Config, ScrobbleMode};
use patina_core::lastfm::Play;
use patina_core::library::Library;
use patina_core::remote::{self, RemoteRef, ServerConfig, ServerKind, StreamFormat};
use patina_core::scrobble::{LastFmSetup, SECRET_NAME, ScrobbleEvent, ScrobbleService};
use patina_core::secrets::Secrets;
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig, TrackInfo};
use serde_json::{Value, json};

const KEY: &str = "testkey";
const SECRET: &str = "testsecret";

/// What the stand-in Last.fm has seen and how it behaves.
#[derive(Default)]
struct Fm {
    approved: bool,
    /// Answer every write with this error code.
    down: Option<i64>,
    calls: Vec<String>,
    now_playing: Vec<(String, String)>,
    /// (artist, track, timestamp), and how many arrived per request.
    scrobbles: Vec<(String, String, i64)>,
    batches: Vec<usize>,
    loves: Vec<(String, String, bool)>,
    bad_sigs: usize,
}

fn stand_in() -> (String, Arc<Mutex<Fm>>) {
    let st = Arc::new(Mutex::new(Fm::default()));
    let s2 = st.clone();
    let url = common::serve(Arc::new(move |r: &Request| lastfm(&s2, r)));
    (format!("{url}/2.0/"), st)
}

fn lastfm(st: &Mutex<Fm>, r: &Request) -> Response {
    let mut s = st.lock().unwrap();
    let method = r.get("method").unwrap_or("").to_string();
    s.calls.push(method.clone());
    let err = |code: i64, msg: &str| Response::json(json!({"error": code, "message": msg}));
    if r.get("api_key") != Some(KEY) {
        return err(10, "Invalid API key");
    }
    // Every call but the plain reads is signed.
    if let Some(sig) = r.get("api_sig") {
        let mut p: Vec<&(String, String)> = r
            .query
            .iter()
            .filter(|(k, _)| k != "format" && k != "api_sig" && k != "callback")
            .collect();
        p.sort_by(|a, b| a.0.cmp(&b.0));
        let mut text = String::new();
        for (k, v) in p {
            text.push_str(k);
            text.push_str(v);
        }
        text.push_str(SECRET);
        if hex::encode(Md5::digest(text.as_bytes())) != sig {
            s.bad_sigs += 1;
            return err(13, "Invalid method signature supplied");
        }
    } else if method != "track.getInfo" {
        return err(13, "Invalid method signature supplied");
    }
    let writes = matches!(
        method.as_str(),
        "track.scrobble" | "track.updateNowPlaying" | "track.love" | "track.unlove"
    );
    if writes {
        if r.method != "POST" {
            return err(3, "writes must be POSTed");
        }
        if let Some(code) = s.down {
            return err(code, "Service temporarily unavailable");
        }
        if r.get("sk") != Some("session-key") {
            return err(9, "Invalid session key");
        }
    }
    match method.as_str() {
        "auth.getToken" => Response::json(json!({"token": "tok123"})),
        "auth.getSession" if !s.approved => err(14, "Unauthorized Token"),
        "auth.getSession" => Response::json(
            json!({"session": {"name": "zack", "key": "session-key", "subscriber": 0}}),
        ),
        "track.updateNowPlaying" => {
            let a = r.get("artist").unwrap_or("").to_string();
            let t = r.get("track").unwrap_or("").to_string();
            s.now_playing.push((a, t));
            Response::json(json!({"nowplaying": {}}))
        }
        "track.scrobble" => {
            let mut n = 0;
            while let Some(a) = r.get(&format!("artist[{n}]")) {
                let t = r.get(&format!("track[{n}]")).unwrap_or("").to_string();
                let ts = r
                    .get(&format!("timestamp[{n}]"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                s.scrobbles.push((a.to_string(), t, ts));
                n += 1;
            }
            s.batches.push(n);
            Response::json(json!({"scrobbles": {"@attr": {"accepted": n, "ignored": 0}}}))
        }
        "track.love" | "track.unlove" => {
            let a = r.get("artist").unwrap_or("").to_string();
            let t = r.get("track").unwrap_or("").to_string();
            s.loves.push((a, t, method == "track.love"));
            Response::json(json!({}))
        }
        "track.getInfo" => {
            let loved = s
                .loves
                .iter()
                .rev()
                .find(|l| {
                    Some(l.0.as_str()) == r.get("artist") && Some(l.1.as_str()) == r.get("track")
                })
                .is_some_and(|l| l.2);
            Response::json(
                json!({"track": {"name": r.get("track"), "userloved": if loved {"1"} else {"0"}}}),
            )
        }
        _ => err(3, "Invalid method"),
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

fn setup(user: &str) -> LastFmSetup {
    LastFmSetup {
        key: KEY.into(),
        secret: SECRET.into(),
        user: user.into(),
    }
}

fn play(n: i64) -> Play {
    Play {
        artist: "Nirvana".into(),
        track: format!("Song {n}"),
        album: Some("Bleach".into()),
        timestamp: 1_700_000_000 + n,
        ..Default::default()
    }
}

/// A linked service: the session key already in the secrets file.
fn linked(dir: &Path, base: &str) -> ScrobbleService {
    let secrets = Arc::new(Secrets::with(None, dir.join("secrets.toml")));
    secrets.set(SECRET_NAME, "zack", "session-key").unwrap();
    ScrobbleService::start_with(&dir.join("lib.db"), setup("zack"), secrets, base).unwrap()
}

#[test]
fn linking_goes_through_the_approval_page() {
    let (base, fm) = stand_in();
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(Secrets::with(None, dir.path().join("secrets.toml")));
    let svc = ScrobbleService::start_with(
        &dir.path().join("lib.db"),
        setup(""),
        secrets.clone(),
        &base,
    )
    .unwrap();
    svc.link();
    let url = match svc.events().recv_timeout(Duration::from_secs(5)).unwrap() {
        ScrobbleEvent::Approve(u) => u,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        url,
        format!("https://www.last.fm/api/auth/?api_key={KEY}&token=tok123")
    );
    assert!(svc.status().linking);
    // Still waiting: nothing happens until it's approved.
    std::thread::sleep(Duration::from_millis(3500));
    assert!(svc.status().user.is_empty());
    fm.lock().unwrap().approved = true;
    match svc.events().recv_timeout(Duration::from_secs(10)).unwrap() {
        ScrobbleEvent::Linked(u) => assert_eq!(u, "zack"),
        other => panic!("{other:?}"),
    }
    assert_eq!(svc.status().user, "zack");
    assert_eq!(
        secrets.get(SECRET_NAME, "zack").unwrap().as_deref(),
        Some("session-key")
    );
    assert_eq!(fm.lock().unwrap().bad_sigs, 0);
    svc.unlink();
    assert!(wait_for(5, || svc.status().user.is_empty()));
    assert_eq!(secrets.get(SECRET_NAME, "zack").unwrap(), None);
}

#[test]
fn scrobbles_wait_out_an_outage_and_keep_their_times() {
    let (base, fm) = stand_in();
    let dir = tempfile::tempdir().unwrap();
    let svc = linked(dir.path(), &base);
    assert!(wait_for(5, || svc.status().user == "zack"));
    fm.lock().unwrap().down = Some(16);
    for n in 0..3 {
        svc.scrobble(play(n));
    }
    assert!(
        wait_for(5, || svc.status().waiting == 3),
        "{:?}",
        svc.status()
    );
    assert!(svc.status().problem.is_some());
    assert!(fm.lock().unwrap().scrobbles.is_empty());
    // Back up, plus a pile from a long time offline: batches of 50.
    let lib = Library::open(&dir.path().join("lib.db")).unwrap();
    for n in 3..120 {
        lib.queue_scrobble(&play(n)).unwrap();
    }
    fm.lock().unwrap().down = None;
    svc.flush();
    assert!(
        wait_for(10, || svc.status().waiting == 0),
        "{:?}",
        svc.status()
    );
    let f = fm.lock().unwrap();
    assert_eq!(f.batches, vec![50, 50, 20]);
    assert_eq!(f.scrobbles.len(), 120);
    // Oldest first, each at the time it was played.
    assert_eq!(
        f.scrobbles[0],
        ("Nirvana".into(), "Song 0".into(), 1_700_000_000)
    );
    assert_eq!(f.scrobbles[119].2, 1_700_000_119);
    assert_eq!(f.bad_sigs, 0);
}

#[test]
fn a_dead_session_keeps_the_queue_for_a_new_link() {
    let (base, fm) = stand_in();
    let dir = tempfile::tempdir().unwrap();
    let svc = linked(dir.path(), &base);
    assert!(wait_for(5, || svc.status().user == "zack"));
    fm.lock().unwrap().down = Some(9);
    svc.scrobble(play(1));
    let ev = svc.events().recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(ev, ScrobbleEvent::Unlinked(ref m) if m.contains("Link again")),
        "{ev:?}"
    );
    assert!(svc.status().user.is_empty());
    assert_eq!(svc.status().waiting, 1);
}

#[test]
fn love_and_loved() {
    let (base, fm) = stand_in();
    let dir = tempfile::tempdir().unwrap();
    let svc = linked(dir.path(), &base);
    assert!(wait_for(5, || svc.status().user == "zack"));
    svc.love("Nirvana", "Blew", true);
    svc.ask_loved("Nirvana", "Blew");
    match svc.events().recv_timeout(Duration::from_secs(5)).unwrap() {
        ScrobbleEvent::Loved { on, .. } => assert!(on),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fm.lock().unwrap().loves,
        vec![("Nirvana".into(), "Blew".into(), true)]
    );
}

// ----- The browser: what gets scrobbled, and by whom -----

fn tone(path: &Path, secs: u32) {
    let st = Proc::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("sine=f=440:r=44100:d={secs}"))
        .args([
            "-metadata",
            "title=Blew",
            "-metadata",
            "artist=Nirvana",
            "-metadata",
            "album=Bleach",
        ])
        .arg(path)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success());
}

struct Rig {
    _dir: tempfile::TempDir,
    fm: Arc<Mutex<Fm>>,
    srv: Subsonic,
    b: Browser,
    song: PathBuf,
}

fn rig(name: &str, mode: ScrobbleMode, linked_here: bool, song_secs: u32) -> Rig {
    let (base, fm) = stand_in();
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("blew.flac");
    tone(&song, song_secs);
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let sc = ServerConfig {
        name: name.into(),
        url: srv.url.clone(),
        user: "zack".into(),
        kind: ServerKind::Subsonic,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    };
    remote::registry().set_client(name, Some(remote::connect(&sc, "pw").unwrap()));
    let secrets = Arc::new(Secrets::with(None, dir.path().join("secrets.toml")));
    let user = if linked_here {
        secrets.set(SECRET_NAME, "zack", "session-key").unwrap();
        "zack"
    } else {
        ""
    };
    let db = dir.path().join("lib.db");
    let svc = ScrobbleService::start_with(&db, setup(user), secrets, &base).unwrap();
    let mut cfg = Config::default();
    cfg.lastfm.user = user.into();
    cfg.lastfm.servers.insert(name.into(), mode);
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "").unwrap();
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
        remote: None,
        cache: None,
        mixes: None,
        level: None,
        scrobble: Some(svc),
        config: cfg,
        settings_info: SettingsInfo {
            config_path: config,
            ..Default::default()
        },
    });
    if linked_here {
        assert!(wait_for(5, || b.lastfm_user().is_some()));
    }
    Rig {
        _dir: dir,
        fm,
        srv,
        b,
        song,
    }
}

fn server_song(server: &str) -> TrackInfo {
    TrackInfo {
        path: format!("server://{server}/Nirvana/Bleach/01 Blew.flac").into(),
        remote: Some(RemoteRef {
            server: server.into(),
            id: "11".into(),
            episode: false,
        }),
        title: Some("Blew".into()),
        artist: Some("Nirvana".into()),
        album: Some("Bleach".into()),
        duration: Some(Duration::from_secs(175)),
        ..Default::default()
    }
}

#[test]
fn each_server_song_is_scrobbled_once_by_whoever_should() {
    let cases = [
        // (mode, linked here, app scrobbles, server told)
        (ScrobbleMode::App, true, true, false),
        (ScrobbleMode::Server, true, false, true),
        (ScrobbleMode::Both, true, true, true),
        // Not linked: the server is told, since nothing can double up.
        (ScrobbleMode::App, false, false, true),
    ];
    for (i, (mode, here, app, server)) in cases.into_iter().enumerate() {
        let name = format!("ScrobbleSrv{i}");
        let mut r = rig(&name, mode, here, 1);
        let info = server_song(&name);
        r.b.lastfm_events(&[Event::Scrobble {
            info: Box::new(info),
            started: 1_700_000_000,
        }]);
        let got_server = wait_for(3, || !r.srv.state.lock().unwrap().reports.is_empty());
        let got_app = wait_for(if app { 5 } else { 1 }, || {
            !r.fm.lock().unwrap().scrobbles.is_empty()
        });
        assert_eq!(got_app, app, "{mode:?} linked={here}: app scrobble");
        assert_eq!(got_server, server, "{mode:?} linked={here}: server report");
        if server {
            assert_eq!(r.srv.state.lock().unwrap().reports[0], ("11".into(), true));
        }
    }
}

#[test]
fn a_song_played_long_enough_is_scrobbled_and_loved() {
    let mut r = rig("ScrobbleLocal", ScrobbleMode::App, true, 32);
    r.b.player.send(Command::SetQueue {
        paths: vec![r.song.clone()],
        start: 0,
    });
    // Now Playing as it starts.
    assert!(wait_for(10, || {
        r.b.poll();
        !r.fm.lock().unwrap().now_playing.is_empty()
    }));
    assert_eq!(
        r.fm.lock().unwrap().now_playing[0],
        ("Nirvana".into(), "Blew".into())
    );
    assert_eq!(r.b.loved_now, Some(false));
    r.b.toggle_love();
    assert_eq!(r.b.loved_now, Some(true));
    assert!(wait_for(5, || !r.fm.lock().unwrap().loves.is_empty()));
    // Half of 32 seconds, real time.
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(wait_for(25, || {
        r.b.poll();
        !r.fm.lock().unwrap().scrobbles.is_empty()
    }));
    let s = r.fm.lock().unwrap().scrobbles[0].clone();
    assert_eq!((s.0.as_str(), s.1.as_str()), ("Nirvana", "Blew"));
    assert!((s.2 - started).abs() <= 3, "started {} vs {}", s.2, started);
    let lib = r.b.lib.as_ref().unwrap();
    assert!(lib.loved("nirvana", "BLEW").unwrap());
}

#[test]
fn short_songs_and_podcasts_never_scrobble() {
    let mut r = rig("ScrobbleShort", ScrobbleMode::App, true, 3);
    r.b.player.send(Command::SetQueue {
        paths: vec![r.song.clone()],
        start: 0,
    });
    let end = Instant::now() + Duration::from_secs(6);
    while Instant::now() < end {
        r.b.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(r.fm.lock().unwrap().scrobbles.is_empty());
    let mut ep = server_song("ScrobbleShort");
    ep.kind = patina_core::track::MediaKind::Podcast;
    r.b.lastfm_events(&[Event::TrackChanged(None)]);
    assert!(
        !r.fm
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c == "track.scrobble")
    );
    let _ = Value::Null;
}

#[test]
fn stars_go_to_the_server() {
    let srv = Subsonic::start(Flavor::Navidrome, "zack", "pw", None);
    let sc = ServerConfig {
        name: "StarSrv".into(),
        url: srv.url.clone(),
        user: "zack".into(),
        kind: ServerKind::Subsonic,
        stream: StreamFormat::Raw,
        bitrate: 192,
        udn: String::new(),
    };
    let c = remote::connect(&sc, "pw").unwrap();
    c.star("11", true).unwrap();
    c.star("11", false).unwrap();
    c.report_play("12", 1_700_000_000, false).unwrap();
    let st = srv.state.lock().unwrap();
    assert_eq!(st.stars, vec![("11".into(), true), ("11".into(), false)]);
    assert_eq!(st.reports, vec![("12".into(), false)]);
}
