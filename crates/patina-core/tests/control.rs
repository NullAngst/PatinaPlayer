//! Remote control end to end: a server on a real player, driven by the
//! client over HTTP on this machine.
//!
//! Needs the `ffmpeg` command on PATH to make the fixtures.

use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::audio::output::{Backend, OutputConfig};
use patina_core::control::{Ask, Client, Cmd, Server};
use patina_core::{Command, Player, PlayerConfig};

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn another_player_sees_and_steers_this_one() {
    let dir = std::env::temp_dir().join(format!("patina-control-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut songs = Vec::new();
    for (i, f) in [440, 550, 660].iter().enumerate() {
        let p = dir.join(format!("{i}.flac"));
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
            .arg(format!("sine=frequency={f}:duration=30"))
            .args(["-metadata", &format!("title=Tone {f}"), "-c:a", "flac"])
            .arg(&p)
            .status()
            .expect("ffmpeg must be installed to run this test");
        assert!(st.success());
        songs.push(p);
    }
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    player.send(Command::SetQueue {
        paths: songs,
        start: 0,
    });
    let (tx, rx) = crossbeam_channel::unbounded();
    let loved = Arc::new(Mutex::new(Some(false)));
    let server = Server::start(player.clone(), 0, "424242", tx, loved).unwrap();
    let addr = format!("127.0.0.1:{}", server.port());

    // The wrong code gets nowhere.
    let bad = Client::new(&addr, "000000").unwrap();
    assert_eq!(bad.state().unwrap_err(), "wrong code");

    let c = Client::new(&addr, "424242").unwrap();
    wait_for("the first song", || c.state().unwrap().title == "Tone 440");
    let s = c.state().unwrap();
    assert_eq!(s.state, "playing");
    assert_eq!(s.duration_ms / 1000, 30);
    assert_eq!(s.loved, Some(false));
    assert!(!s.art.is_empty());

    c.send(&Cmd::Toggle).unwrap();
    wait_for("pause", || c.state().unwrap().state == "paused");
    c.send(&Cmd::Next).unwrap();
    wait_for("the next song", || c.state().unwrap().title == "Tone 550");

    let q = c.queue().unwrap();
    assert_eq!(
        q.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["Tone 440", "Tone 550", "Tone 660"]
    );
    c.send(&Cmd::PlayEntry { id: q.items[2].id }).unwrap();
    wait_for("the picked song", || c.state().unwrap().title == "Tone 660");

    c.send(&Cmd::Volume { value: 0.25 }).unwrap();
    wait_for("the volume", || {
        (c.state().unwrap().volume - 0.25).abs() < 1e-6
    });

    // The heart is the browser's to do.
    c.send(&Cmd::Love).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Ask::Love);

    drop(server);
    player.send(Command::Quit);
    let _ = std::fs::remove_dir_all(dir);
}
