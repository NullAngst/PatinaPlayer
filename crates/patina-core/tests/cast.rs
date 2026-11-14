//! Playing on another device, end to end, with a stand-in snapserver: a
//! TCP listener here that takes what a snapserver's TCP source would.
//!
//! Needs the `ffmpeg` command on PATH to make the fixture.

use std::io::Read;
use std::net::TcpListener;
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use patina_core::audio::output::{Backend, OutputConfig};
use patina_core::cast::Target;
use patina_core::{Command, Player, PlayerConfig};

#[test]
fn a_song_plays_on_a_snapserver_at_48k_and_comes_back_after() {
    let dir = std::env::temp_dir().join(format!("patina-cast-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let song = dir.join("tone.flac");
    // 44.1 kHz on purpose: it goes out resampled to 48 kHz.
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
        .arg("sine=frequency=440:sample_rate=44100:duration=2")
        .args(["-c:a", "flac"])
        .arg(&song)
        .status()
        .expect("ffmpeg must be installed to run this test");
    assert!(st.success());

    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        volume: 1.0,
        ..Default::default()
    });
    let target = Target::snapcast(&format!("127.0.0.1:{port}")).unwrap();
    player.send(Command::SetCast(Some(target.clone())));
    player.send(Command::SetQueue {
        paths: vec![song],
        start: 0,
    });

    server.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut sock = loop {
        match server.accept() {
            Ok((s, _)) => break s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("never connected ({e}): {:?}", player.snapshot()),
        }
    };
    sock.set_nonblocking(false).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // A second and a half of it, at 4 bytes a frame.
    let want = 48_000 * 4 * 3 / 2;
    let mut got = vec![0u8; want];
    let started = Instant::now();
    sock.read_exact(&mut got).unwrap();
    // In real time, not as fast as it decodes.
    assert!(started.elapsed() > Duration::from_millis(1200));

    let snap = player.snapshot();
    assert_eq!(snap.cast.as_ref(), Some(&target));
    assert!(
        snap.output
            .as_deref()
            .is_some_and(|o| o.contains("Snapcast")),
        "{:?}",
        snap.output
    );

    // A 440 Hz tone at 48 kHz crosses zero about 880 times a second; at
    // 44.1 kHz read as 48 kHz it would be about 958.
    let left: Vec<i16> = got
        .chunks_exact(4)
        .map(|f| i16::from_le_bytes([f[0], f[1]]))
        .collect();
    let loud = left.iter().filter(|s| s.unsigned_abs() > 1000).count();
    assert!(loud > left.len() / 4, "mostly silence");
    let start = left.iter().position(|s| s.unsigned_abs() > 1000).unwrap();
    let second = &left[start..start + 48_000.min(left.len() - start)];
    let crossings = second
        .windows(2)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count() as f64
        * 48_000.0
        / second.len() as f64;
    assert!(
        (crossings - 880.0).abs() < 20.0,
        "{crossings} crossings a second"
    );

    // Back here: the snapserver's connection closes.
    player.send(Command::SetCast(None));
    let mut rest = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 65536];
    loop {
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => rest.extend_from_slice(&buf[..n]),
            Err(e) => panic!("the connection stayed open: {e}"),
        }
        assert!(Instant::now() < deadline, "the connection stayed open");
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while player.snapshot().cast.is_some() {
        assert!(Instant::now() < deadline, "still casting");
        std::thread::sleep(Duration::from_millis(20));
    }
    player.send(Command::Quit);
    let _ = std::fs::remove_dir_all(dir);
}
