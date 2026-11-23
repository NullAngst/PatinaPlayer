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

#[test]
fn sleep_timer_pauses_on_time_and_plays_again_after() {
    use patina_core::player::PlayState;
    use patina_core::sleep::SleepTimer;
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 30.0);
    let p = player(Arc::new(Mutex::new(Vec::new())));
    p.send(Command::SetQueue {
        paths: vec![a],
        start: 0,
    });
    wait(&p, 10, |e| {
        matches!(e, Event::StateChanged(PlayState::Playing))
    });
    let set = Instant::now();
    p.send(Command::SetSleep(Some(SleepTimer::after(
        Duration::from_millis(1500),
    ))));
    std::thread::sleep(Duration::from_millis(300));
    assert!(p.snapshot().sleep.is_some(), "the snapshot shows the timer");
    wait(&p, 10, |e| matches!(e, Event::SleepEnded));
    let took = set.elapsed();
    assert!(
        took >= Duration::from_millis(1400) && took < Duration::from_millis(2500),
        "paused after {took:?}"
    );
    std::thread::sleep(Duration::from_millis(200));
    let s = p.snapshot();
    assert_eq!(s.state, PlayState::Paused);
    assert!(s.sleep.is_none(), "the timer is gone once it's done");
    // And play works again, with nothing left to stop it.
    let at = s.position;
    p.send(Command::Play);
    std::thread::sleep(Duration::from_millis(800));
    let s = p.snapshot();
    assert_eq!(s.state, PlayState::Playing);
    assert!(s.position > at, "playing on from where it paused");
    p.send(Command::Quit);
}

#[test]
fn sleep_timer_end_of_song_stops_before_the_next_one() {
    use patina_core::player::PlayState;
    use patina_core::sleep::SleepTimer;
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 2.0);
    let b = tone(dir.path(), "b.flac", 2.0);
    let p = player(Arc::new(Mutex::new(Vec::new())));
    p.send(Command::SetQueue {
        paths: vec![a, b],
        start: 0,
    });
    // The first song's own change of track, so only a later one counts.
    let mut first = None;
    wait(&p, 10, |e| match e {
        Event::TrackChanged(Some(id)) => {
            first = Some(*id);
            true
        }
        _ => false,
    });
    p.send(Command::SetSleep(Some(SleepTimer::EndOfSong)));
    let mut changed = false;
    wait(&p, 10, |e| {
        if matches!(e, Event::TrackChanged(Some(_))) {
            changed = true;
        }
        matches!(e, Event::SleepEnded)
    });
    assert!(!changed, "the next song never started");
    std::thread::sleep(Duration::from_millis(200));
    let s = p.snapshot();
    assert_eq!(s.state, PlayState::Paused);
    assert_eq!(s.current, first, "still on the first song");
    let left = s.duration.unwrap().saturating_sub(s.position);
    assert!(
        left < Duration::from_millis(300),
        "{left:?} left of the song"
    );
    p.send(Command::Quit);
}

#[test]
fn sleep_timer_running_out_while_paused_does_nothing() {
    use patina_core::player::PlayState;
    use patina_core::sleep::SleepTimer;
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.flac", 30.0);
    let p = player(Arc::new(Mutex::new(Vec::new())));
    p.send(Command::SetQueue {
        paths: vec![a],
        start: 0,
    });
    wait(&p, 10, |e| {
        matches!(e, Event::StateChanged(PlayState::Playing))
    });
    p.send(Command::Pause);
    p.send(Command::SetSleep(Some(SleepTimer::after(
        Duration::from_millis(300),
    ))));
    std::thread::sleep(Duration::from_millis(700));
    assert!(p.snapshot().sleep.is_none(), "dropped once its time passed");
    p.send(Command::Play);
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        p.snapshot().state,
        PlayState::Playing,
        "it doesn't pause later"
    );
    p.send(Command::Quit);
}

/// Songs on a server that isn't connected fail in a millisecond each.
/// Skipping on ran through a whole queue of them; now three in a row stop
/// playback, with one message, and they aren't marked as broken.
#[test]
fn a_run_of_songs_on_a_server_thats_away_stops_playback() {
    let dir = tempfile::tempdir().unwrap();
    let local = tone(dir.path(), "after.flac", 1.0);
    let played = Arc::new(Mutex::new(Vec::new()));
    let p = player(played.clone());
    let away = |i: usize| TrackInfo {
        path: PathBuf::from(format!("server://Nowhere/song{i}.flac")),
        remote: Some(patina_core::remote::RemoteRef {
            server: "Nowhere".into(),
            id: i.to_string(),
            episode: false,
        }),
        ..Default::default()
    };
    let mut tracks: Vec<TrackInfo> = (0..5).map(away).collect();
    tracks.push(TrackInfo {
        path: local.clone(),
        ..Default::default()
    });
    p.send(Command::SetQueueTracks {
        tracks,
        start: 0,
        pos: Duration::ZERO,
    });
    let mut errors = Vec::new();
    wait(&p, 5, |e| {
        if let Event::Error(m) = e {
            errors.push(m.clone());
        }
        errors
            .iter()
            .any(|m| m.starts_with("Stopped: Nowhere isn't connected"))
    });
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        played.lock().unwrap().is_empty(),
        "it stopped, not skipped on"
    );
    assert_eq!(errors.len(), 1, "one message, not one a song: {errors:?}");
    p.send(Command::Quit);
}

/// An output that fails, the way a stream does when Android moves the sound
/// to a Bluetooth headset, once `fail` is set.
struct Failing {
    _inner: patina_core::audio::output::null::NullOutput,
    fail: Arc<std::sync::atomic::AtomicBool>,
}

impl patina_core::audio::output::Output for Failing {
    fn describe(&self) -> String {
        "failing".into()
    }
    fn error(&self) -> Option<String> {
        self.fail
            .load(std::sync::atomic::Ordering::Relaxed)
            .then(|| "disconnected".to_string())
    }
}

#[test]
fn a_lost_output_pauses_where_it_was() {
    let dir = std::env::temp_dir().join(format!("patina-lost-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let song = tone(&dir, "long.wav", 20.0);
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (f2, o2) = (fail.clone(), opened.clone());
    let factory: patina_core::player::OutputFactory = Box::new(move |_, renderer| {
        o2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Only the first output fails; the one after it is the new device.
        let first = o2.load(std::sync::atomic::Ordering::Relaxed) == 1;
        Ok(Box::new(Failing {
            _inner: patina_core::audio::output::null::NullOutput::start(renderer, None),
            fail: if first {
                f2.clone()
            } else {
                Arc::new(std::sync::atomic::AtomicBool::new(false))
            },
        }))
    });
    let p = Player::spawn_with(
        PlayerConfig {
            fade_ms: 0,
            ..Default::default()
        },
        Hooks {
            output: Some(factory),
            ..Default::default()
        },
    );
    p.send(Command::SetQueue {
        paths: vec![song],
        start: 0,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while p.snapshot().position < Duration::from_millis(1500) {
        assert!(Instant::now() < deadline, "it never got going");
        std::thread::sleep(Duration::from_millis(50));
    }
    fail.store(true, std::sync::atomic::Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = p.snapshot();
        if s.state == patina_core::PlayState::Paused
            && opened.load(std::sync::atomic::Ordering::Relaxed) >= 2
        {
            assert!(
                s.position >= Duration::from_millis(1200),
                "it went back to {:?}, not where it was",
                s.position
            );
            break;
        }
        assert!(Instant::now() < deadline, "it never paused: {:?}", s.state);
        std::thread::sleep(Duration::from_millis(50));
    }
    // Play goes on from there, on the new output.
    let at = p.snapshot().position;
    p.send(Command::Play);
    std::thread::sleep(Duration::from_millis(800));
    let s = p.snapshot();
    assert_eq!(s.state, patina_core::PlayState::Playing);
    assert!(s.position > at, "{:?} should be past {at:?}", s.position);
    p.send(Command::Quit);
    let _ = std::fs::remove_dir_all(&dir);
}
