//! Radio and podcast tests against a small HTTP server running inside the
//! test: an Icecast-style ICY stream that drops on purpose, a .pls with a
//! dead mirror, HLS segments, an RSS feed and a download that breaks
//! halfway. Needs the `ffmpeg` command on PATH, and no internet.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command as Proc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::audio::decoder::{OpenOptions, Source};
use patina_core::library::Library;
use patina_core::player::Hooks;
use patina_core::track::MediaKind;
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig, TrackInfo};

fn ffmpeg(args: &[&str]) {
    let st = Proc::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success(), "ffmpeg {args:?}");
}

struct Server {
    port: u16,
    dir: PathBuf,
    icy_connections: Arc<AtomicUsize>,
    episode_requests: Arc<Mutex<Vec<Option<String>>>>,
    _tmp: tempfile::TempDir,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

fn read_request(stream: &TcpStream) -> Option<(String, HashMap<String, String>)> {
    let mut r = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let path = line.split_whitespace().nth(1)?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some((path, headers))
}

fn respond(mut s: &TcpStream, status: &str, ctype: &str, body: &[u8], extra: &str) {
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    let _ = s.write_all(body);
}

/// ICY stream: MP3 bytes with a metadata block every `metaint` bytes.
/// Sends about `limit` bytes, then hangs up, to exercise reconnects.
fn serve_icy(mut s: &TcpStream, mp3: &[u8], wants_meta: bool, limit: usize) {
    let metaint = 8192;
    let mut hdr = String::from(
        "HTTP/1.0 200 OK\r\nContent-Type: audio/mpeg\r\nicy-name: Test FM\r\nicy-br: 128\r\n",
    );
    if wants_meta {
        hdr.push_str(&format!("icy-metaint: {metaint}\r\n"));
    }
    hdr.push_str("\r\n");
    if s.write_all(hdr.as_bytes()).is_err() {
        return;
    }
    let meta = {
        let text = b"StreamTitle='Band - Song';";
        let blocks = text.len().div_ceil(16);
        let mut m = vec![blocks as u8];
        m.extend_from_slice(text);
        m.resize(1 + blocks * 16, 0);
        m
    };
    let mut sent = 0;
    let mut pos = 0;
    while sent < limit {
        let end = (pos + metaint).min(mp3.len());
        let chunk = &mp3[pos..end];
        if s.write_all(chunk).is_err() {
            return;
        }
        sent += chunk.len();
        pos = if end >= mp3.len() { 0 } else { end };
        if wants_meta && chunk.len() == metaint && s.write_all(&meta).is_err() {
            return;
        }
        // Roughly real time for 128 kbps: 8 KB is about half a second.
        std::thread::sleep(Duration::from_millis(60));
    }
}

/// An endless stream with no length and no ICY metadata: the file over
/// and over, at roughly real time.
fn serve_raw(mut s: &TcpStream, ctype: &str, data: &[u8]) {
    if write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nConnection: close\r\n\r\n"
    )
    .is_err()
    {
        return;
    }
    for _ in 0..4 {
        for chunk in data.chunks(8192) {
            if s.write_all(chunk).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }
}

fn start_server() -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let d = |n: &str| dir.join(n).to_str().unwrap().to_string();
    // A long MP3 for the station, an episode, and HLS segments.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:r=44100:d=20",
        "-ac",
        "2",
        "-b:a",
        "128k",
        &d("station.mp3"),
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=330:r=44100:d=12",
        "-ac",
        "2",
        "-b:a",
        "128k",
        &d("ep1.mp3"),
    ]);
    // A raw ADTS AAC station, the kind public broadcasters serve.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=500:r=48000:d=20",
        "-ac",
        "2",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-f",
        "adts",
        &d("station.aac"),
    ]);
    // A station that switches from 44.1 kHz mono to 48 kHz stereo partway
    // through, the way ad breaks and HE-AAC streams do.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:r=44100:d=2",
        "-ac",
        "1",
        "-c:a",
        "aac",
        "-b:a",
        "64k",
        "-f",
        "adts",
        &d("part1.aac"),
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=600:r=48000:d=8",
        "-ac",
        "2",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-f",
        "adts",
        &d("part2.aac"),
    ]);
    let mut mixed = std::fs::read(d("part1.aac")).unwrap();
    mixed.extend(std::fs::read(d("part2.aac")).unwrap());
    std::fs::write(d("mixed.aac"), mixed).unwrap();
    std::fs::create_dir_all(dir.join("hls")).unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=550:r=48000:d=6",
        "-ac",
        "2",
        "-c:a",
        "aac",
        "-b:a",
        "96k",
        "-f",
        "hls",
        "-hls_time",
        "2",
        "-hls_playlist_type",
        "vod",
        "-hls_segment_filename",
        &d("hls/seg%d.ts"),
        &d("hls/live.m3u8"),
    ]);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let icy_connections = Arc::new(AtomicUsize::new(0));
    let episode_requests = Arc::new(Mutex::new(Vec::new()));
    let (icy_c, ep_r, root) = (
        icy_connections.clone(),
        episode_requests.clone(),
        dir.clone(),
    );
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (icy_c, ep_r, root) = (icy_c.clone(), ep_r.clone(), root.clone());
            std::thread::spawn(move || {
                let Some((path, headers)) = read_request(&stream) else {
                    return;
                };
                let base = format!("http://127.0.0.1:{port}");
                match path.as_str() {
                    "/station.pls" => {
                        let body = format!(
                            "[playlist]\nNumberOfEntries=2\nFile1={base}/dead\nFile2={base}/icy\n"
                        );
                        respond(&stream, "200 OK", "audio/x-scpls", body.as_bytes(), "");
                    }
                    "/dead" => respond(&stream, "404 Not Found", "text/plain", b"gone", ""),
                    "/mixed.aac" => {
                        let aac = std::fs::read(root.join("mixed.aac")).unwrap();
                        serve_raw(&stream, "audio/aac", &aac);
                    }
                    p if p.starts_with("/stream.aac") => {
                        let aac = std::fs::read(root.join("station.aac")).unwrap();
                        serve_raw(&stream, "audio/aac", &aac);
                    }
                    "/icy" => {
                        icy_c.fetch_add(1, Ordering::SeqCst);
                        let mp3 = std::fs::read(root.join("station.mp3")).unwrap();
                        let wants = headers.get("icy-metadata").is_some_and(|v| v == "1");
                        // About 2.5 seconds of audio per connection.
                        serve_icy(&stream, &mp3, wants, 40_000);
                    }
                    "/feed.xml" => {
                        let body = format!(
                            r#"<?xml version="1.0"?><rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel><title>Local Show</title><itunes:author>Host</itunes:author><item><title>First</title><guid>g1</guid><pubDate>Sat, 03 Oct 2026 10:00:00 +0000</pubDate><itunes:duration>0:12</itunes:duration><enclosure url="{base}/ep1.mp3" type="audio/mpeg" length="0"/></item></channel></rss>"#
                        );
                        respond(
                            &stream,
                            "200 OK",
                            "application/rss+xml",
                            body.as_bytes(),
                            "",
                        );
                    }
                    "/ep1.mp3" => {
                        let data = std::fs::read(root.join("ep1.mp3")).unwrap();
                        let range = headers.get("range").cloned();
                        let first = {
                            let mut r = ep_r.lock().unwrap();
                            r.push(range.clone());
                            r.len() == 1
                        };
                        let mut s = &stream;
                        match range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
                            Some(spec) => {
                                let start: usize = spec
                                    .trim_end_matches('-')
                                    .split('-')
                                    .next()
                                    .unwrap()
                                    .parse()
                                    .unwrap();
                                let body = &data[start.min(data.len())..];
                                let _ = write!(
                                    s,
                                    "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\n\r\n",
                                    body.len(),
                                    start,
                                    data.len() - 1,
                                    data.len()
                                );
                                let _ = s.write_all(body);
                            }
                            None => {
                                let _ = write!(
                                    s,
                                    "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n",
                                    data.len()
                                );
                                if first {
                                    // Break the very first download halfway.
                                    let _ = s.write_all(&data[..data.len() / 2]);
                                    let _ = stream.shutdown(std::net::Shutdown::Both);
                                } else {
                                    let _ = s.write_all(&data);
                                }
                            }
                        }
                    }
                    p if p.starts_with("/hls/") => {
                        let f = root.join(p.trim_start_matches('/'));
                        match std::fs::read(&f) {
                            Ok(b) => {
                                let ct = if p.ends_with(".m3u8") {
                                    "application/vnd.apple.mpegurl"
                                } else {
                                    "video/mp2t"
                                };
                                respond(&stream, "200 OK", ct, &b, "");
                            }
                            Err(_) => respond(&stream, "404 Not Found", "text/plain", b"", ""),
                        }
                    }
                    _ => respond(&stream, "404 Not Found", "text/plain", b"", ""),
                }
            });
        }
    });
    Server {
        port,
        dir,
        icy_connections,
        episode_requests,
        _tmp: tmp,
    }
}

fn wait<T>(secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn null_player(hooks: Hooks) -> Player {
    Player::spawn_with(
        PlayerConfig {
            output: OutputConfig {
                backend: Backend::Null,
                ..Default::default()
            },
            fade_ms: 10,
            ..Default::default()
        },
        hooks,
    )
}

#[test]
fn pls_skips_the_dead_mirror_and_reads_icy() {
    let srv = start_server();
    let r = patina_core::radio::resolve(&srv.url("/station.pls")).unwrap();
    assert_eq!(r.kind, patina_core::radio::Kind::Playlist);
    assert_eq!(r.streams.len(), 2);
    let c = patina_core::radio::check(&srv.url("/station.pls")).unwrap();
    assert_eq!(c.working, Some(1), "the 404 mirror is skipped");
    assert_eq!(c.codec.as_deref(), Some("MP3"));
    assert_eq!(c.kbps, Some(128));
    assert_eq!(c.name.as_deref(), Some("Test FM"));
    assert_eq!(
        c.summary(),
        "Playlist, 2 mirrors, MP3 128 kbps, 44.1 kHz stereo"
    );
}

#[test]
fn station_plays_shows_titles_and_reconnects() {
    let srv = start_server();
    let p = null_player(Hooks::default());
    let station = TrackInfo {
        kind: MediaKind::Radio,
        url: Some(srv.url("/station.pls")),
        title: Some("Local Station".into()),
        ..Default::default()
    };
    p.send(Command::SetQueueTracks {
        tracks: vec![station],
        start: 0,
        pos: Duration::ZERO,
    });
    // Title arrives through ICY metadata.
    let title = wait(15, || match p.events().try_recv() {
        Ok(Event::NowPlaying(t)) => Some(t),
        _ => None,
    });
    assert_eq!(title, "Band - Song");
    let snap = p.snapshot();
    assert!(snap.live, "a station is live");
    assert_eq!(snap.duration, None);
    assert_eq!(snap.track.unwrap().title.as_deref(), Some("Local Station"));

    // The server hangs up after a couple of seconds; the player comes back.
    wait(20, || {
        (srv.icy_connections.load(Ordering::SeqCst) >= 2).then_some(())
    });
    wait(10, || {
        let s = p.snapshot();
        (s.reconnect.is_none() && s.state == patina_core::PlayState::Playing && s.track.is_some())
            .then_some(())
    });
    p.send(Command::Quit);
}

#[test]
fn hls_is_handed_to_ffmpeg() {
    let srv = start_server();
    let url = srv.url("/hls/live.m3u8");
    let r = patina_core::radio::resolve(&url).unwrap();
    assert_eq!(r.kind, patina_core::radio::Kind::Hls);
    let mut src = Source::open_with(
        &r.streams[0],
        OpenOptions {
            live: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(src.format().rate, 48000);
    let mut out = Vec::new();
    let mut frames = 0;
    while frames < 48000 * 3 {
        let n = src.decode_into(&mut out).unwrap();
        assert!(n > 0, "HLS ended early");
        frames += n;
    }
    let _ = &srv.dir;
}

#[test]
fn feed_download_resumes_and_progress_is_saved() {
    let srv = start_server();
    let data = patina_core::podcast::fetch_feed(&srv.url("/feed.xml")).unwrap();
    assert_eq!(data.title.as_deref(), Some("Local Show"));
    let mut lib = Library::open_in_memory().unwrap();
    lib.sync_podcasts(&[srv.url("/feed.xml")]).unwrap();
    let pid = lib.store_feed(&srv.url("/feed.xml"), &data).unwrap();
    let eps = lib.episodes(pid).unwrap();
    assert_eq!(eps.len(), 1);
    let ep = &eps[0];

    // First attempt breaks halfway; the second picks up with a Range.
    use patina_core::cache::fetch;
    let dest = srv.dir.join("dl").join("ep1.mp3");
    let cancel = AtomicBool::new(false);
    let http = fetch::client();
    // The server cuts the first try halfway. The retry built into fetch
    // asks for the rest with a range, and a half file never takes the
    // real name in between.
    let progress = fetch::Progress::new();
    let first = fetch::fetch(&http, &ep.audio_url, &dest, true, &progress, &cancel);
    assert!(first.is_ok(), "{first:?}");
    assert_eq!(progress.state(), fetch::State::Done);
    let reqs = srv.episode_requests.lock().unwrap().clone();
    assert_eq!(reqs[0], None);
    assert!(
        reqs[1]
            .as_deref()
            .is_some_and(|r| r.starts_with("bytes=") && r != "bytes=0-"),
        "{reqs:?}"
    );
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        std::fs::read(srv.dir.join("ep1.mp3")).unwrap()
    );

    // Play it streamed at 1.5x and check the saved position follows the
    // source, not the wall clock.
    let saved: Arc<Mutex<Option<(Duration, bool)>>> = Arc::new(Mutex::new(None));
    let s2 = saved.clone();
    let p = null_player(Hooks {
        progress: Some(Box::new(move |_t: &TrackInfo, pos, fin| {
            *s2.lock().unwrap() = Some((pos, fin));
        })),
        ..Default::default()
    });
    p.send(Command::SetPodcastPlayback {
        tempo: 1.5,
        trim_silence: false,
    });
    p.send(Command::SetQueueTracks {
        tracks: vec![ep.to_info()],
        start: 0,
        pos: Duration::ZERO,
    });
    wait(10, || {
        (p.snapshot().position > Duration::ZERO).then_some(())
    });
    let t0 = Instant::now();
    let pos0 = p.snapshot().position;
    std::thread::sleep(Duration::from_millis(2000));
    let elapsed = t0.elapsed().as_secs_f64();
    let pos1 = p.snapshot().position;
    let rate = (pos1 - pos0).as_secs_f64() / elapsed;
    assert!(
        (rate - 1.5).abs() < 0.2,
        "position should run at 1.5x, ran at {rate:.2}x"
    );
    p.send(Command::Pause);
    let (pos, fin) = wait(5, || *saved.lock().unwrap());
    assert!(!fin);
    assert!(pos > Duration::from_secs(2), "saved {pos:?}");

    // Resuming starts a few seconds back from the saved spot.
    lib.set_episode_progress(ep.id, Duration::from_secs(20), false)
        .unwrap();
    let again = lib.episode(ep.id).unwrap().unwrap();
    assert_eq!(again.resume_at(), Duration::from_secs(15));
    lib.set_episode_progress(ep.id, Duration::ZERO, true)
        .unwrap();
    assert!(lib.episode(ep.id).unwrap().unwrap().played);
    p.send(Command::Quit);
}

#[test]
fn podcast_service_refreshes_and_the_cache_downloads() {
    use patina_core::cache::{CacheConfig, CacheEvent, CacheService, Roots};
    use patina_core::podcast::PodcastService;
    let srv = start_server();
    let db = srv.dir.join("lib.db");
    let offline = srv.dir.join("offline");
    let feed = srv.url("/feed.xml");
    let svc = PodcastService::start(&db, vec![feed.clone()]).unwrap();
    let lib = Library::open(&db).unwrap();
    let ep = wait(10, || {
        let _ = svc.events().try_recv();
        let pods = lib.podcasts().ok()?;
        let p = pods.first()?;
        (p.title == "Local Show").then(|| lib.episodes(p.id).ok()?.into_iter().next())?
    });
    assert_eq!(ep.title, "First");
    // Episodes download through the cache's queue. The server breaks the
    // first download of this file halfway; the download picks it up again
    // with a range on its own.
    let cache = CacheService::start_at(
        &db,
        CacheConfig::default(),
        Vec::new(),
        Roots {
            cache: srv.dir.join("cache"),
            offline: srv.dir.join("pinned"),
            podcasts: offline.clone(),
        },
    )
    .unwrap();
    cache.download_episode(ep.id);
    wait(20, || match cache.events().try_recv() {
        Ok(CacheEvent::EpisodeDone(id)) if id == ep.id => Some(()),
        _ => None,
    });
    let ep = lib.episode(ep.id).unwrap().unwrap();
    let path = ep.download.clone().expect("download path stored");
    assert!(path.starts_with(offline.join("Local Show")), "{path:?}");
    assert_eq!(
        ep.to_info().path,
        path,
        "a downloaded episode plays from disk"
    );
    patina_core::podcast::delete_download(&lib, ep.id).unwrap();
    assert!(!path.exists());
}

#[test]
fn raw_aac_station_with_a_query_string_plays() {
    let srv = start_server();
    let p = null_player(Hooks::default());
    let station = TrackInfo {
        kind: MediaKind::Radio,
        url: Some(srv.url("/stream.aac?aggregator=web")),
        title: Some("AAC Station".into()),
        ..Default::default()
    };
    let check = patina_core::radio::check(station.url.as_deref().unwrap()).unwrap();
    assert!(check.error.is_none(), "{:?}", check.error);
    p.send(Command::SetQueueTracks {
        tracks: vec![station],
        start: 0,
        pos: Duration::ZERO,
    });
    wait(15, || {
        let s = p.snapshot();
        assert!(s.last_error.is_none(), "{:?}", s.last_error);
        (s.state == patina_core::PlayState::Playing
            && !s.buffering
            && s.position > Duration::from_secs(2))
        .then_some(())
    });
    p.send(Command::Quit);
}

#[test]
fn station_that_changes_format_keeps_playing() {
    let srv = start_server();
    let p = null_player(Hooks::default());
    let url = srv.url("/mixed.aac");
    let check = patina_core::radio::check(&url).unwrap();
    assert!(check.error.is_none(), "{:?}", check.error);
    p.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo {
            kind: MediaKind::Radio,
            url: Some(url),
            title: Some("Switching".into()),
            ..Default::default()
        }],
        start: 0,
        pos: Duration::ZERO,
    });
    // Past the switch at 2 s, with no reconnect along the way, and the
    // badge showing the new format.
    let mut reconnected = false;
    wait(20, || {
        let s = p.snapshot();
        reconnected |= s.reconnect.is_some();
        let t = s.track.as_ref()?;
        (s.position > Duration::from_secs(4) && t.sample_rate == 48000 && t.channels == 2)
            .then_some(())
    });
    assert!(!reconnected, "a format change isn't a dropped connection");
    p.send(Command::Quit);
}

/// Tapping a song over and over: each tap cuts the last one's open short.
/// FFmpeg reports a probe stopped halfway as "Invalid data found when
/// processing input" (or "End of file"), and that used to come out as an
/// error on the song, which was marked failed and skipped past.
#[test]
fn repeated_taps_dont_report_a_cut_short_open() {
    let tmp = tempfile::tempdir().unwrap();
    let m4a = tmp.path().join("song.m4a");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:r=44100:d=8",
        "-ac",
        "2",
        "-c:a",
        "aac",
        "-movflags",
        "+faststart",
        m4a.to_str().unwrap(),
    ]);
    let data = Arc::new(std::fs::read(&m4a).unwrap());
    // Trickles the file out, so an open spans several taps' worth of time.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/song.m4a", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let data = data.clone();
            std::thread::spawn(move || {
                if read_request(&s).is_none() {
                    return;
                }
                let mut s = &s;
                if write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                )
                .is_err()
                {
                    return;
                }
                for chunk in data.chunks(256) {
                    if s.write_all(chunk).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            });
        }
    });

    let p = null_player(Hooks::default());
    let song = |n: u32| TrackInfo {
        // An episode, so the queue keeps its title.
        kind: MediaKind::Podcast,
        url: Some(format!("{url}?n={n}")),
        title: Some(format!("Song {n}")),
        ..Default::default()
    };
    p.send(Command::SetQueueTracks {
        tracks: (0..3).map(song).collect(),
        start: 0,
        pos: Duration::ZERO,
    });
    // Taps at uneven gaps, so some land mid-probe.
    for i in 0..40u64 {
        std::thread::sleep(Duration::from_millis(5 + (i * 7) % 23));
        p.send(Command::SetQueueTracks {
            tracks: (0..3).map(song).collect(),
            start: 0,
            pos: Duration::ZERO,
        });
    }
    // The last tap plays, from the first song, with nothing reported.
    wait(20, || {
        let s = p.snapshot();
        assert!(s.last_error.is_none(), "{:?}", s.last_error);
        (s.state == patina_core::PlayState::Playing && s.position > Duration::from_millis(500))
            .then_some(())
    });
    let s = p.snapshot();
    assert_eq!(s.track.and_then(|t| t.title).as_deref(), Some("Song 0"));
    while let Ok(e) = p.events().try_recv() {
        assert!(!matches!(e, Event::Error(_)), "{e:?}");
    }
    p.send(Command::Quit);
}
