//! MPRIS2 against a private session bus, driven the way a desktop's media
//! widget drives it: method calls, property reads and writes, and the
//! change signals. Skipped when `dbus-daemon` isn't installed. Needs the
//! `ffmpeg` command on PATH for the test tones.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Proc, Stdio};
use std::time::{Duration, Instant};

use patina_core::mpris::{Abilities, Mpris, Request};
use patina_core::{Backend, Command, OutputConfig, PlayState, Player, PlayerConfig, Repeat};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::{ObjectPath, OwnedValue};

const NAME: &str = "org.mpris.MediaPlayer2.patinatest";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

struct Bus {
    child: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn private_bus() -> Option<Bus> {
    let mut child = Proc::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut()?)
        .read_line(&mut line)
        .ok()?;
    Some(Bus {
        child,
        address: line.trim().to_string(),
    })
}

fn connect(bus: &Bus) -> Connection {
    zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap()
}

fn tone(dir: &Path, name: &str, secs: f32) -> PathBuf {
    let p = dir.join(name);
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
        .args(["-metadata", "title=Tone", "-metadata", "artist=Tester"])
        .arg(&p)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success());
    p
}

/// Run the service's update loop until `done` says so, like a frontend's
/// timer would.
fn pump(m: &mut Mpris, p: &Player, secs: u64, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        let snap = p.snapshot();
        let (len, next) = {
            let q = p.queue();
            let next = snap.current.and_then(|c| q.upcoming(c)).is_some();
            (q.len(), next)
        };
        m.update(&snap, len, next);
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("timed out");
}

fn prop<T: TryFrom<OwnedValue>>(proxy: &Proxy, name: &str) -> T
where
    T::Error: std::fmt::Debug,
{
    let v: OwnedValue = proxy.get_property(name).unwrap();
    T::try_from(v).unwrap()
}

#[test]
fn desktop_controls_and_signals() {
    let Some(bus) = private_bus() else {
        eprintln!("no dbus-daemon, skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 4.0);
    let b = tone(dir.path(), "b.flac", 4.0);

    let p = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    let (mut m, requests) =
        Mpris::start_on(connect(&bus), NAME, p.clone(), Abilities { raise: true }).unwrap();

    let client = connect(&bus);
    // Listen for property changes before anything happens.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)
        .unwrap()
        .build();
    let signals = MessageIterator::for_match_rule(rule, &client, None).unwrap();
    // Read them on a thread, so a missing one fails the test instead of
    // hanging it.
    let (sig_tx, sig_rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        for msg in signals.flatten() {
            let member = msg.header().member().map(|m| m.as_str().to_string());
            let keys = match member.as_deref() {
                Some("PropertiesChanged") => {
                    let (iface, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                        msg.body().deserialize().unwrap();
                    if iface == PLAYER {
                        changed.into_keys().collect()
                    } else {
                        Vec::new()
                    }
                }
                _ => Vec::new(),
            };
            if sig_tx.send((member, keys)).is_err() {
                return;
            }
        }
    });

    // No property cache: every read asks the service, like `playerctl`.
    let proxy = |iface: &'static str| -> Proxy<'static> {
        zbus::blocking::proxy::Builder::new(&client)
            .destination(NAME)
            .unwrap()
            .path(PATH)
            .unwrap()
            .interface(iface)
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .unwrap()
    };
    let player = proxy(PLAYER);
    let root = proxy("org.mpris.MediaPlayer2");
    assert_eq!(prop::<String>(&root, "Identity"), "PatinaPlayer");
    assert!(prop::<bool>(&root, "CanRaise"));
    assert_eq!(prop::<String>(&player, "PlaybackStatus"), "Stopped");

    p.send(Command::SetQueue {
        paths: vec![a.clone(), b.clone()],
        start: 0,
    });
    pump(&mut m, &p, 10, || {
        let s = p.snapshot();
        s.state == PlayState::Playing && s.track.is_some()
    });
    pump(&mut m, &p, 2, || true);
    assert_eq!(prop::<String>(&player, "PlaybackStatus"), "Playing");
    let meta: HashMap<String, OwnedValue> = prop(&player, "Metadata");
    let title: String = meta["xesam:title"].clone().try_into().unwrap();
    assert_eq!(title, "Tone");
    let artists: Vec<String> = meta["xesam:artist"].clone().try_into().unwrap();
    assert_eq!(artists, vec!["Tester".to_string()]);
    let tid: ObjectPath<'_> = meta["mpris:trackid"].downcast_ref().unwrap();
    let tid = tid.to_string();
    assert!(tid.starts_with("/org/patinaplayer/track/"));
    assert!(prop::<bool>(&player, "CanGoNext"));
    assert!(prop::<bool>(&player, "CanSeek"));

    // The widget's play/pause button.
    let _: () = player.call("PlayPause", &()).unwrap();
    pump(&mut m, &p, 5, || p.snapshot().state == PlayState::Paused);
    pump(&mut m, &p, 1, || true);
    assert_eq!(prop::<String>(&player, "PlaybackStatus"), "Paused");

    // Dragging the widget's position slider.
    let path = ObjectPath::try_from(tid.as_str()).unwrap();
    let _: () = player.call("SetPosition", &(path, 2_500_000i64)).unwrap();
    pump(&mut m, &p, 5, || {
        p.snapshot().position >= Duration::from_millis(2400)
    });
    pump(&mut m, &p, 1, || true);
    let pos: i64 = prop(&player, "Position");
    assert!((2_400_000..2_700_000).contains(&pos), "position {pos}");

    // Loop and shuffle set from outside.
    player.set_property("LoopStatus", "Track").unwrap();
    pump(&mut m, &p, 5, || p.snapshot().repeat == Repeat::One);
    player.set_property("Shuffle", true).unwrap();
    pump(&mut m, &p, 5, || p.snapshot().shuffle);
    pump(&mut m, &p, 1, || true);
    assert_eq!(prop::<String>(&player, "LoopStatus"), "Track");
    assert!(prop::<bool>(&player, "Shuffle"));

    player.set_property("Volume", 0.25f64).unwrap();
    pump(&mut m, &p, 5, || (p.snapshot().volume - 0.25).abs() < 1e-9);

    // Next moves to the second track, which the metadata follows.
    let _: () = player.call("Next", &()).unwrap();
    pump(&mut m, &p, 5, || {
        p.snapshot().track.is_some_and(|t| t.path == b)
    });
    pump(&mut m, &p, 1, || true);
    let meta: HashMap<String, OwnedValue> = prop(&player, "Metadata");
    let url: String = meta["xesam:url"].clone().try_into().unwrap();
    assert!(url.ends_with("/b.flac"), "{url}");

    // Raise and Quit come back to the frontend.
    let _: () = root.call("Raise", &()).unwrap();
    let _: () = root.call("Quit", &()).unwrap();
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(2)),
        Ok(Request::Raise)
    );
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(2)),
        Ok(Request::Quit)
    );

    // Along the way the service said what changed, and that we seeked.
    let mut seen = std::collections::HashSet::new();
    let mut seeked = false;
    while let Ok((member, keys)) = sig_rx.recv_timeout(Duration::from_secs(2)) {
        match member.as_deref() {
            Some("PropertiesChanged") => seen.extend(keys),
            Some("Seeked") => seeked = true,
            _ => {}
        }
        if seeked
            && [
                "PlaybackStatus",
                "Metadata",
                "LoopStatus",
                "Shuffle",
                "Volume",
            ]
            .iter()
            .all(|k| seen.contains(*k))
        {
            break;
        }
    }
    assert!(seeked, "no Seeked signal");
    for k in [
        "PlaybackStatus",
        "Metadata",
        "LoopStatus",
        "Shuffle",
        "Volume",
    ] {
        assert!(seen.contains(k), "no change signal for {k}: {seen:?}");
    }
    p.send(Command::Quit);
}
