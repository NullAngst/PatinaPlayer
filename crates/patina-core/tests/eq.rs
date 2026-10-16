//! The EQ and the limiter end to end: what comes out of the player with
//! them on, the EQ view's rows and presets, imports, per-device EQ and
//! what's saved to config.toml. Needs the `ffmpeg` command on PATH.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::audio::output::null::NullOutput;
use patina_core::browse::{Browser, Change, Item, Key, Section, SettingsInfo, Startup};
use patina_core::config::Config;
use patina_core::dsp::EqParams;
use patina_core::dsp::biquad::{Band, Kind};
use patina_core::eq::{EqSettings, Mode};
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
        .arg("-c:a")
        .arg("pcm_f32le")
        .arg(path)
        .status()
        .expect("ffmpeg must be installed to run these tests");
    assert!(st.success());
}

fn peak_dbfs(buf: &[f64], from_s: f64, to_s: f64) -> f64 {
    let a = (from_s * RATE as f64) as usize * 2;
    let b = ((to_s * RATE as f64) as usize * 2).min(buf.len());
    let p = buf[a..b].iter().fold(0.0f64, |m, s| m.max(s.abs()));
    20.0 * p.log10()
}

/// Play a file with this EQ and limiter, leveling off, and capture it.
fn play(path: &Path, eq: EqParams, limiter_on: bool, ceiling_db: f64) -> Vec<f64> {
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
            eq,
            limiter_on,
            limiter_ceiling_db: ceiling_db,
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
    let deadline = Instant::now() + Duration::from_secs(30);
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

fn boost_1k(db: f64) -> EqParams {
    EqSettings {
        mode: Mode::Parametric,
        parametric: vec![Band {
            kind: Kind::Peak,
            freq: 1000.0,
            gain: db,
            q: 1.0,
        }],
        auto_preamp: false,
        ..Default::default()
    }
    .params()
}

#[test]
fn the_eq_shapes_what_plays_and_bypass_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("1k.wav");
    tone(&f, 0.25, 1000.0, 2);
    let orig = play(&f, EqParams::default(), true, -1.0);
    // +6 dB at 1 kHz: twice as big.
    let out = play(&f, boost_1k(6.0206), true, -1.0);
    assert_eq!(out.len(), orig.len());
    let got = peak_dbfs(&out, 0.5, 1.9) - peak_dbfs(&orig, 0.5, 1.9);
    assert!((got - 6.02).abs() < 0.05, "{got} dB");
    // Bypassed: sample for sample what came in.
    let mut p = boost_1k(6.0);
    p.active = false;
    assert_eq!(play(&f, p, true, -1.0), orig);
    // A cut far from 1 kHz leaves it alone.
    let mut far = boost_1k(-12.0);
    far.bands[0].freq = 100.0;
    let out = play(&f, far, true, -1.0);
    let got = peak_dbfs(&out, 0.5, 1.9) - peak_dbfs(&orig, 0.5, 1.9);
    assert!(got.abs() < 0.3, "{got} dB");
}

#[test]
fn the_limiter_holds_eq_boosts_under_the_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("hot.wav");
    tone(&f, 0.7, 997.0, 2);
    // +9 dB on a 0.7 sine: 2 linear, way past full scale.
    let out = play(&f, boost_1k(9.0), true, -1.0);
    let p = peak_dbfs(&out, 0.0, 2.0);
    assert!(p <= -1.0 + 1e-6, "peak {p} dBFS");
    assert!(p > -1.6, "limited too hard: {p} dBFS");
    // With it off, it hits full scale and is clipped, never wrapped.
    let out = play(&f, boost_1k(9.0), false, -1.0);
    let p = peak_dbfs(&out, 0.0, 2.0);
    assert!(p.abs() < 1e-9, "clipped at {p}");
    // A lower ceiling is honoured.
    let out = play(&f, boost_1k(9.0), true, -3.0);
    assert!(peak_dbfs(&out, 0.0, 2.0) <= -3.0 + 1e-6);
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
        eq: cfg.eq.settings.params(),
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

fn row(b: &mut Browser, key: Key) -> String {
    b.switch(Section::Eq);
    b.level()
        .unwrap()
        .items
        .iter()
        .find_map(|i| match i {
            Item::Setting(s) if s.key == key => Some(s.control.display()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no row {key:?}"))
}

fn label(b: &mut Browser, key: Key) -> String {
    b.switch(Section::Eq);
    b.level()
        .unwrap()
        .items
        .iter()
        .find_map(|i| match i {
            Item::Setting(s) if s.key == key => Some(s.label.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn the_eq_view_edits_saves_and_reloads() {
    let f = fx();
    let mut b = browser(&f, Config::default());
    assert_eq!(row(&mut b, Key::EqPreset), "Flat");
    assert_eq!(row(&mut b, Key::EqGraphic(5)), "0.0 dB");
    // l on 1 kHz, three times: +1.5 dB, and the preset shows it changed.
    for _ in 0..3 {
        b.change_setting(&Key::EqGraphic(5), Change::Step(1));
    }
    assert_eq!(row(&mut b, Key::EqGraphic(5)), "1.5 dB");
    assert_eq!(row(&mut b, Key::EqPreset), "Flat (changed)");
    // Auto preamp is off to start with, so a boost is louder; on, it takes
    // the boost back off everything.
    assert!(label(&mut b, Key::EqAutoPreamp).contains("+0.0 dB now"));
    b.change_setting(&Key::EqAutoPreamp, Change::Toggle);
    assert!(label(&mut b, Key::EqAutoPreamp).contains("-1.5 dB now"));
    b.change_setting(&Key::EqAutoPreamp, Change::Toggle);
    // Typed in.
    b.change_setting(&Key::EqGraphic(0), Change::Set("-3".into()));
    assert_eq!(row(&mut b, Key::EqGraphic(0)), "-3.0 dB");
    // 31 bands keep the curve.
    b.change_setting(&Key::EqBands, Change::Choose(2));
    assert_eq!(b.eq_settings().graphic.len(), 31);
    assert!((b.eq_settings().response_at(1000.0) - 1.5).abs() < 0.5);
    // Save as a preset, load a built-in, load ours back.
    b.eq_save_preset("Mine");
    assert_eq!(row(&mut b, Key::EqPreset), "Mine (yours)");
    assert!(f.base.join("presets/eq/Mine.toml").exists());
    b.eq_load_preset("Rock");
    assert_eq!(b.eq_settings().graphic_bands, 10);
    b.eq_load_preset("Mine");
    assert_eq!(b.eq_settings().graphic_bands, 31);
    // Bypass, then parametric with a typed band.
    b.change_setting(&Key::EqOn, Change::Toggle);
    assert!(!b.eq_settings().on);
    b.change_setting(&Key::EqMode, Change::Choose(1));
    b.change_setting(&Key::EqAddBand, Change::Toggle);
    b.change_setting(
        &Key::EqBand(0),
        Change::Set("LS 105 Hz 4.5 dB Q 0.7".into()),
    );
    assert_eq!(row(&mut b, Key::EqBand(0)), "LS 105 Hz +4.5 dB Q 0.70");
    b.change_setting(&Key::EqBand(0), Change::Step(-1));
    assert_eq!(b.eq_settings().parametric[0].gain, 4.0);
    // Written to config.toml, and read back the same.
    b.eq_save_now();
    let text = std::fs::read_to_string(&f.config).unwrap();
    assert!(text.contains("[eq]"), "{text}");
    let c = Config::parse(&text, &f.config).unwrap();
    assert!(c.eq.settings.same_sound(b.eq_settings()));
    assert!(!c.eq.settings.on);
    // A fresh start picks it all up.
    let mut b2 = browser(&f, c);
    assert_eq!(row(&mut b2, Key::EqBand(0)), "LS 105 Hz +4.0 dB Q 0.70");
}

#[test]
fn autoeq_imports_become_presets() {
    let f = fx();
    let mut b = browser(&f, Config::default());
    let p = f.base.join("Sennheiser HD 650 ParametricEQ.txt");
    std::fs::write(
        &p,
        "Preamp: -6.4 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.7 dB Q 0.70\nFilter 2: ON PK Fc 2960 Hz Gain 3.4 dB Q 2.35\n",
    )
    .unwrap();
    b.eq_import(&p);
    let s = b.eq_settings();
    assert_eq!(s.mode, Mode::Parametric);
    assert_eq!(s.parametric.len(), 2);
    assert_eq!(s.preamp, -6.4);
    assert_eq!(s.preset, "Sennheiser HD 650");
    assert!(b.eq_presets().iter().any(|p| p.name == "Sennheiser HD 650"));
    let g = f.base.join("GraphicEQ.txt");
    std::fs::write(&g, "GraphicEQ: 20 3; 1000 0; 20000 -3").unwrap();
    b.eq_import(&g);
    assert_eq!(b.eq_settings().graphic_bands, 31);
    assert_eq!(b.eq_settings().preset, "Imported");
    // A broken file is an error, and the EQ stays as it was.
    let bad = f.base.join("bad.txt");
    std::fs::write(&bad, "nothing here").unwrap();
    b.eq_import(&bad);
    assert_eq!(b.eq_settings().preset, "Imported");
    assert!(
        b.take_notes()
            .iter()
            .any(|n| format!("{n:?}").contains("no EQ filters"))
    );
}

fn wait_device(b: &mut Browser) {
    let end = Instant::now() + Duration::from_secs(10);
    while b.eq.device.is_none() {
        b.poll();
        assert!(Instant::now() < end, "device never seen");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_device_keeps_its_own_eq() {
    let f = fx();
    let mut b = browser(&f, Config::default());
    b.eq_load_preset("Rock");
    // Play, so the output (here the null one) is known.
    b.player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo {
            path: f.song.clone(),
            ..Default::default()
        }],
        start: 0,
        pos: Duration::ZERO,
    });
    wait_device(&mut b);
    assert_eq!(b.eq.device.as_ref().unwrap().key, "null");
    b.eq_link_device();
    assert_eq!(b.eq.linked.as_deref(), Some("No output"));
    // A change here goes to the device's preset, not the usual EQ.
    b.eq_set_gain(0, -6.0);
    b.eq_save_now();
    let text = std::fs::read_to_string(&f.config).unwrap();
    let c = Config::parse(&text, &f.config).unwrap();
    assert_eq!(
        c.eq.devices.get("null").map(String::as_str),
        Some("No output"),
        "{text}"
    );
    assert_eq!(c.eq.settings.preset, "Rock");
    assert_eq!(c.eq.settings.graphic[0], 4.5);
    let dev = std::fs::read_to_string(f.base.join("presets/eq/No output.toml")).unwrap();
    assert!(dev.contains("-6.0"), "{dev}");
    // Next start: the usual EQ until the device turns up, then its own.
    let mut b2 = browser(&f, c);
    assert_eq!(b2.eq_settings().graphic[0], 4.5);
    b2.player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo {
            path: f.song.clone(),
            ..Default::default()
        }],
        start: 0,
        pos: Duration::ZERO,
    });
    wait_device(&mut b2);
    assert_eq!(b2.eq_settings().graphic[0], -6.0);
    assert!(row(&mut b2, Key::EqDevice).contains("usual"));
    // Unlinked: back to the usual one.
    b2.eq_unlink_device();
    assert_eq!(b2.eq_settings().graphic[0], 4.5);
}

#[test]
fn bit_perfect_greys_the_eq_and_limiter_rows_work() {
    let f = fx();
    let mut cfg = Config::default();
    cfg.output.bit_perfect = patina_core::audio::output::BitPerfect::On;
    let mut b = browser(&f, cfg);
    b.switch(Section::Eq);
    let greyed = b.level().unwrap().items.iter().all(|i| match i {
        Item::Setting(s) => s.disabled.is_some(),
        _ => true,
    });
    assert!(greyed);
    b.change_setting(&Key::EqGraphic(0), Change::Step(1));
    assert_eq!(b.eq_settings().graphic[0], 0.0);
    // Limiter rows in Settings.
    b.change_setting(&Key::LimiterCeiling, Change::Step(-1));
    b.change_setting(&Key::Limiter, Change::Toggle);
    let text = std::fs::read_to_string(&f.config).unwrap();
    let c = Config::parse(&text, &f.config).unwrap();
    assert_eq!(c.limiter.ceiling_db, -1.1);
    assert!(!c.limiter.on);
}

/// PipeWire's default sink, followed as it changes. Needs a running
/// PipeWire with two sinks called headphones and speakers:
/// `PATINA_PW=1 cargo test -p patina-core --test eq pipewire -- --ignored`
#[cfg(feature = "pipewire")]
#[test]
#[ignore = "needs a running PipeWire (see the comment)"]
fn pipewire_follows_the_default_sink() {
    use patina_core::audio::output::Output;
    use patina_core::audio::output::pipewire::PwOutput;
    use patina_core::audio::render::Renderer;
    use patina_core::audio::{Shared, StreamFormat};
    let set_default = |name: &str| {
        let st = Proc::new("pw-metadata")
            .args([
                "0",
                "default.audio.sink",
                &format!("{{ \"name\": \"{name}\" }}"),
                "Spa:String:JSON",
            ])
            .output()
            .unwrap();
        assert!(st.status.success());
    };
    set_default("headphones");
    let fmt = StreamFormat {
        rate: 48000,
        channels: 2,
        mask: 0b11,
    };
    let (_p, c) = rtrb::RingBuffer::new(4096);
    let r = Renderer::new(c, Arc::new(Shared::default()), fmt);
    let out = PwOutput::start(&OutputConfig::default(), r).expect("PipeWire running");
    let wait = |want: &str| {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if out.device().is_some_and(|d| d.key == want) {
                return out.device().unwrap();
            }
            assert!(Instant::now() < end, "never saw {want}: {:?}", out.device());
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let d = wait("headphones");
    assert_eq!(d.name, "Test headphones");
    set_default("speakers");
    assert_eq!(wait("speakers").name, "Test speakers");
}

/// Play a file with these settings (leveling off, unity volume) and
/// capture it.
fn play_cfg(path: &Path, cfg: PlayerConfig) -> Vec<f64> {
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
            ..cfg
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
    let deadline = Instant::now() + Duration::from_secs(30);
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

fn peak_ch(buf: &[f64], c: usize, from_s: f64, to_s: f64) -> f64 {
    let a = (from_s * RATE as f64) as usize;
    let b = ((to_s * RATE as f64) as usize).min(buf.len() / 2);
    buf[a * 2..b * 2]
        .chunks_exact(2)
        .fold(0.0f64, |m, f| m.max(f[c].abs()))
}

#[test]
fn channel_tools_shape_what_plays_and_save() {
    use patina_core::dsp::channel::{ChannelParams, Crossfeed};
    let f = fx();
    // Full right: the left goes quiet, the right is untouched.
    let orig = play_cfg(&f.song, PlayerConfig::default());
    let out = play_cfg(
        &f.song,
        PlayerConfig {
            channel: ChannelParams {
                on: true,
                balance: 1.0,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert_eq!(out.len(), orig.len());
    assert!(peak_ch(&out, 0, 0.2, 0.9) < 1e-6);
    let r = peak_ch(&out, 1, 0.2, 0.9) / peak_ch(&orig, 1, 0.2, 0.9);
    assert!((r - 1.0).abs() < 1e-6, "{r}");
    // Off: sample for sample.
    let off = play_cfg(
        &f.song,
        PlayerConfig {
            channel: ChannelParams {
                on: false,
                mono: true,
                balance: 1.0,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert_eq!(off, orig);

    // From Settings, saved to config.toml.
    let mut b = browser(&f, Config::default());
    b.change_setting(&Key::ChannelOn, Change::Toggle);
    b.change_setting(&Key::ChannelBalance, Change::Set("-30".into()));
    b.change_setting(&Key::ChannelCrossfeed, Change::Choose(2));
    b.change_setting(&Key::ChannelWidth, Change::Step(-1));
    let text = std::fs::read_to_string(&f.config).unwrap();
    assert!(text.contains("[channel]"), "{text}");
    let c = Config::parse(&text, &f.config).unwrap();
    assert!(c.channel.on);
    assert_eq!(c.channel.balance, -30);
    assert_eq!(c.channel.crossfeed, Crossfeed::Medium);
    assert_eq!(c.channel.width, 90);
}
