//! The amp stack end to end: what comes out of the player with it on and
//! off, the Amp view's rows, presets, per-device stacks and what's saved
//! to config.toml. Needs the `ffmpeg` command on PATH.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::amp::AmpSettings;
use patina_core::audio::output::null::NullOutput;
use patina_core::browse::{Browser, Change, Item, Key, Section, SettingsInfo, Startup};
use patina_core::config::Config;
use patina_core::dsp::amp::{AmpParams, Kind, Unit};
use patina_core::level::{LevelConfig, LevelMode};
use patina_core::player::{Hooks, OutputFactory};
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig, TrackInfo};

const RATE: u32 = 48000;

fn tone(path: &Path, amp: f64, freq: f64, secs: u32) {
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
        .arg(format!(
            "aevalsrc={amp}*sin(2*PI*{freq}*t)|{amp}*sin(2*PI*{freq}*t):s={RATE}:d={secs}"
        ))
        .args(["-c:a", "pcm_f32le"])
        .arg(path)
        .status()
        .expect("ffmpeg must be installed to run these tests");
    assert!(st.success());
}

fn play(path: &Path, amp: AmpParams) -> Vec<f64> {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let cap = capture.clone();
    let factory: OutputFactory =
        Box::new(move |_, r| Ok(Box::new(NullOutput::start(r, Some(cap.clone())))));
    let player = Player::spawn_with(
        PlayerConfig {
            volume: 1.0,
            fade_ms: 0,
            leveling: LevelConfig {
                mode: LevelMode::Off,
                ..Default::default()
            },
            amp,
            ..Default::default()
        },
        Hooks {
            output: Some(factory),
            ..Default::default()
        },
    );
    player.send(Command::SetQueue {
        paths: vec![path.to_path_buf()],
        start: 0,
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match player.events().recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Finished) => break,
            Ok(Event::Error(e)) => panic!("player error: {e}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "playback never finished");
    }
    player.send(Command::Quit);
    capture.lock().unwrap().clone()
}

/// Level of a frequency in the left channel between two times.
fn level(buf: &[f64], freq: f64, from_s: f64, to_s: f64) -> f64 {
    let a = (from_s * RATE as f64) as usize;
    let b = (to_s * RATE as f64) as usize;
    let (mut re, mut im) = (0.0, 0.0);
    for i in a..b {
        let s = buf[i * 2];
        let t = 2.0 * std::f64::consts::PI * freq * i as f64 / RATE as f64;
        re += s * t.cos();
        im += s * t.sin();
    }
    2.0 * (re * re + im * im).sqrt() / (b - a) as f64
}

#[test]
fn the_stack_shapes_what_plays_and_off_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("500.wav");
    tone(&f, 0.5, 500.0, 2);
    let orig = play(&f, AmpParams::default());
    // On with nothing switched on: sample for sample.
    let mut empty = AmpSettings {
        on: true,
        units: vec![Unit::new(Kind::TubePreamp)],
        ..Default::default()
    };
    empty.units[0].on = false;
    assert_eq!(play(&f, empty.params()), orig);
    // A driven tube preamp: same length, the tone still there, and a
    // second harmonic that wasn't.
    let mut pre = Unit::new(Kind::TubePreamp);
    pre.p[0] = 9.0;
    let s = AmpSettings {
        on: true,
        units: vec![pre],
        ..Default::default()
    };
    let out = play(&f, s.params());
    assert_eq!(out.len(), orig.len());
    let h1 = level(&out, 500.0, 0.5, 1.5);
    let h2 = level(&out, 1000.0, 0.5, 1.5);
    assert!(h1 > 0.35, "tone {h1}");
    assert!(h2 / h1 > 0.01, "second harmonic {}", h2 / h1);
    assert!(level(&orig, 1000.0, 0.5, 1.5) < 1e-5);
    // Every built-in preset plays without going anywhere near clipping.
    for name in patina_core::amp::BUILTIN {
        let s = patina_core::amp::builtin(name).unwrap();
        let out = play(&f, s.params());
        let peak = out.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        assert!(peak < 0.95, "{name}: peak {peak}");
        let h1 = level(&out, 500.0, 0.5, 1.5);
        assert!(h1 > 0.1, "{name}: tone {h1}");
    }
}

// ----- The browser side -----

struct Fx {
    _dir: tempfile::TempDir,
    base: PathBuf,
    config: PathBuf,
    song: PathBuf,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let config = base.join("config.toml");
    std::fs::write(&config, "# test config\n").unwrap();
    let song = base.join("song.wav");
    tone(&song, 0.2, 440.0, 1);
    Fx {
        _dir: dir,
        base,
        config,
        song,
    }
}

fn browser(f: &Fx, cfg: Config) -> Browser {
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        amp: cfg.amp.settings.params(),
        ..Default::default()
    });
    Browser::new(Startup {
        player,
        lib: None,
        service: None,
        pod: None,
        remote: None,
        cache: None,
        mixes: None,
        level: None,
        scrobble: None,
        config: cfg,
        settings_info: SettingsInfo {
            config_path: f.config.clone(),
            no_library: true,
            ..Default::default()
        },
    })
}

fn rows(b: &mut Browser) -> Vec<patina_core::browse::Setting> {
    b.switch(Section::Amp);
    b.level()
        .unwrap()
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Setting(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn row(b: &mut Browser, key: Key) -> String {
    rows(b)
        .into_iter()
        .find(|s| s.key == key)
        .map(|s| s.control.display())
        .unwrap_or_else(|| panic!("no row {key:?}"))
}

#[test]
fn the_amp_view_builds_a_rack_saves_and_reloads() {
    let f = fx();
    let mut b = browser(&f, Config::default());
    assert_eq!(row(&mut b, Key::AmpOn), "off");
    assert_eq!(row(&mut b, Key::AmpPreset), "Empty");
    // Add a tape deck from the list: it lands on, at its defaults, and
    // switching it on switches the rack on.
    let add = rows(&mut b)
        .into_iter()
        .find(|s| s.key == Key::AmpAdd)
        .unwrap();
    let tape = match &add.control {
        patina_core::browse::Control::Choice { options, .. } => {
            options.iter().position(|o| o.0 == "tape").unwrap()
        }
        _ => panic!(),
    };
    b.change_setting(&Key::AmpAdd, Change::Choose(tape));
    assert_eq!(b.amp_settings().units.len(), 1);
    assert!(b.amp_settings().on);
    assert_eq!(row(&mut b, Key::AmpParam(Kind::Tape, 0)), "15 ips");
    // Knobs step, and choices too.
    b.change_setting(&Key::AmpParam(Kind::Tape, 0), Change::Step(1));
    assert_eq!(row(&mut b, Key::AmpParam(Kind::Tape, 0)), "30 ips");
    b.change_setting(&Key::AmpParam(Kind::Tape, 2), Change::Set("55".into()));
    assert_eq!(row(&mut b, Key::AmpParam(Kind::Tape, 2)), "55%");
    b.change_setting(&Key::AmpTrimIn(Kind::Tape), Change::Step(-2));
    assert_eq!(row(&mut b, Key::AmpTrimIn(Kind::Tape)), "-1.0 dB");
    // A preset replaces the rack; ours goes in a file.
    b.change_setting(&Key::AmpPreset, Change::Step(1));
    let warm = b.amp_settings().clone();
    assert_eq!(warm.preset, "Warm Glow");
    assert_eq!(warm.units.len(), 2);
    // Order matters, and can change.
    b.change_setting(&Key::AmpMoveUp(Kind::Transformer), Change::Toggle);
    assert_eq!(b.amp_settings().units[0].kind, Kind::Transformer);
    assert_eq!(row(&mut b, Key::AmpPreset), "Warm Glow (changed)");
    b.amp_save_preset("Iron first");
    assert!(f.base.join("presets/amp/Iron first.toml").exists());
    b.change_setting(&Key::AmpRemove(Kind::TubePreamp), Change::Toggle);
    assert_eq!(b.amp_settings().units.len(), 1);
    b.amp_load_preset("Iron first");
    assert_eq!(b.amp_settings().units.len(), 2);
    // Oversampling, and bypass.
    b.change_setting(&Key::AmpOversample, Change::Choose(2));
    b.change_setting(&Key::AmpOn, Change::Toggle);
    assert!(!b.amp_settings().on);
    // To config.toml and back.
    b.amp_save_now();
    let text = std::fs::read_to_string(&f.config).unwrap();
    assert!(
        text.contains("[amp]") && text.contains("[[amp.units]]"),
        "{text}"
    );
    let c = Config::parse(&text, &f.config).unwrap();
    assert!(c.amp.settings.same_sound(b.amp_settings()));
    assert_eq!(c.amp.settings.oversample, 8);
    assert!(!c.amp.settings.on);
    let mut b2 = browser(&f, c);
    // 8x since it was loaded, so it shows as changed from the preset.
    assert_eq!(row(&mut b2, Key::AmpPreset), "Iron first (changed)");
    assert_eq!(b2.amp_settings().units[0].kind, Kind::Transformer);
}

#[test]
fn a_device_keeps_its_own_stack() {
    let f = fx();
    let mut b = browser(&f, Config::default());
    b.amp_load_preset("Late Night");
    b.player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo {
            path: f.song.clone(),
            ..Default::default()
        }],
        start: 0,
        pos: Duration::ZERO,
    });
    let end = Instant::now() + Duration::from_secs(10);
    while b.amp.device.is_none() {
        b.poll();
        assert!(Instant::now() < end, "device never seen");
        std::thread::sleep(Duration::from_millis(20));
    }
    b.amp_link_device();
    assert_eq!(b.amp.linked.as_deref(), Some("No output stack"));
    b.amp_remove(Kind::BassEnhancer);
    b.amp_save_now();
    let text = std::fs::read_to_string(&f.config).unwrap();
    let c = Config::parse(&text, &f.config).unwrap();
    assert_eq!(
        c.amp.devices.get("null").map(String::as_str),
        Some("No output stack")
    );
    assert_eq!(c.amp.settings.units.len(), 2, "the usual one keeps both");
    let own = std::fs::read_to_string(f.base.join("presets/amp/No output stack.toml")).unwrap();
    assert!(!own.contains("bass-enhancer"), "{own}");
    b.amp_unlink_device();
    assert_eq!(b.amp_settings().units.len(), 2);
}

#[test]
fn bit_perfect_greys_the_amp() {
    let f = fx();
    let mut cfg = Config::default();
    cfg.output.bit_perfect = patina_core::audio::output::BitPerfect::On;
    let mut b = browser(&f, cfg);
    assert!(rows(&mut b).iter().all(|s| s.disabled.is_some()));
    b.change_setting(&Key::AmpOn, Change::Toggle);
    assert!(!b.amp_settings().on);
}
