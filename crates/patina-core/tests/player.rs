//! Player behavior in real time, with the null output pacing itself like a
//! sound card. Needs the `ffmpeg` command on PATH.

use std::path::PathBuf;
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::player::Hooks;
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig, TrackInfo};

fn tone(dir: &std::path::Path, name: &str, secs: f32) -> PathBuf {
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
        .arg(&p)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success());
    p
}

fn player(played: Arc<Mutex<Vec<PathBuf>>>) -> Player {
    Player::spawn_with(
        PlayerConfig {
            output: OutputConfig {
                backend: Backend::Null,
                ..Default::default()
            },
            fade_ms: 10,
            ..Default::default()
        },
        Hooks {
            played: Some(Box::new(move |t: &TrackInfo| {
                played.lock().unwrap().push(t.path.clone())
            })),
            ..Default::default()
        },
    )
}

fn wait(p: &Player, secs: u64, mut f: impl FnMut(&Event) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Ok(e) = p.events().recv_timeout(Duration::from_millis(50))
            && f(&e)
        {
            return;
        }
    }
    panic!("timed out");
}

#[test]
fn listening_counts_each_track_once() {
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 1.5);
    let b = tone(dir.path(), "b.flac", 1.5);
    let played = Arc::new(Mutex::new(Vec::new()));
    let p = player(played.clone());
    p.send(Command::SetQueue {
        paths: vec![a.clone(), b.clone()],
        start: 0,
    });
    wait(&p, 10, |e| matches!(e, Event::Finished));
    assert_eq!(*played.lock().unwrap(), vec![a, b]);
    p.send(Command::Quit);
}

#[test]
fn removing_the_playing_track_moves_on() {
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 5.0);
    let b = tone(dir.path(), "b.flac", 5.0);
    let p = player(Arc::new(Mutex::new(Vec::new())));
    p.send(Command::SetQueue {
        paths: vec![a, b.clone()],
        start: 0,
    });
    wait(&p, 5, |e| matches!(e, Event::TrackChanged(Some(_))));
    let first = p.snapshot().current;
    let first = first.or_else(|| p.queue().first()).unwrap();
    p.send(Command::RemoveEntry(first));
    wait(
        &p,
        5,
        |e| matches!(e, Event::TrackChanged(Some(id)) if *id != first),
    );
    std::thread::sleep(Duration::from_millis(100));
    let snap = p.snapshot();
    assert_eq!(snap.track.unwrap().path, b);
    assert_eq!(p.queue().len(), 1);
    p.send(Command::Quit);
}

#[test]
fn clearing_the_queue_lets_the_song_finish_then_shows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 1.5);
    let b = tone(dir.path(), "b.flac", 1.5);
    let c = tone(dir.path(), "c.flac", 1.5);
    let played = Arc::new(Mutex::new(Vec::new()));
    let p = player(played.clone());
    p.send(Command::SetQueue {
        paths: vec![a.clone(), b, c],
        start: 0,
    });
    wait(&p, 5, |e| matches!(e, Event::TrackChanged(Some(_))));
    std::thread::sleep(Duration::from_millis(300));
    p.send(Command::ClearQueue);
    std::thread::sleep(Duration::from_millis(200));
    // What was coming up is gone; the song playing isn't, and still plays.
    assert_eq!(p.queue().len(), 1);
    let s = p.snapshot();
    assert_eq!(s.state, patina_core::PlayState::Playing);
    assert_eq!(s.track.as_ref().map(|t| t.path.clone()), Some(a.clone()));
    wait(&p, 10, |e| matches!(e, Event::Finished));
    std::thread::sleep(Duration::from_millis(100));
    // It played out, nothing followed, and nothing shows.
    assert_eq!(*played.lock().unwrap(), vec![a]);
    assert!(p.queue().is_empty());
    let s = p.snapshot();
    assert!(s.track.is_none(), "still showing {:?}", s.track);
    assert!(s.current.is_none());
    p.send(Command::Quit);
}

#[test]
fn clearing_a_stopped_queue_shows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 1.0);
    let p = player(Arc::new(Mutex::new(Vec::new())));
    p.send(Command::SetQueue {
        paths: vec![a],
        start: 0,
    });
    wait(&p, 5, |e| matches!(e, Event::TrackChanged(Some(_))));
    p.send(Command::Stop);
    std::thread::sleep(Duration::from_millis(200));
    assert!(p.snapshot().track.is_some(), "a stopped song still shows");
    p.send(Command::ClearQueue);
    std::thread::sleep(Duration::from_millis(200));
    assert!(p.queue().is_empty());
    assert!(p.snapshot().track.is_none());
    p.send(Command::Quit);
}

#[test]
fn clearing_near_the_end_drops_a_song_already_lined_up() {
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 1.5);
    let b = tone(dir.path(), "b.flac", 1.5);
    let played = Arc::new(Mutex::new(Vec::new()));
    let p = player(played.clone());
    p.send(Command::SetQueue {
        paths: vec![a.clone(), b],
        start: 0,
    });
    wait(&p, 5, |e| matches!(e, Event::TrackChanged(Some(_))));
    // By now the decoder is well into the next song.
    std::thread::sleep(Duration::from_millis(1250));
    p.send(Command::ClearQueue);
    wait(&p, 10, |e| matches!(e, Event::Finished));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(*played.lock().unwrap(), vec![a]);
    assert!(p.queue().is_empty());
    assert!(p.snapshot().track.is_none());
    p.send(Command::Quit);
}
