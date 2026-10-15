//! Volume leveling end to end: songs measured into the index, gains looked
//! up as the decoder opens them, and the samples that come out the other
//! side at the level they should be. Needs the `ffmpeg` command on PATH.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use patina_core::audio::output::BitPerfect;
use patina_core::audio::output::null::NullOutput;
use patina_core::browse::settings::{self, Key, SettingsInfo};
use patina_core::config::Config;
use patina_core::level::{self, LevelConfig, LevelEvent, LevelMode, LevelService, Origin};
use patina_core::library::Library;
use patina_core::player::{Hooks, OutputFactory};
use patina_core::{Command, Event, Player, PlayerConfig, TrackInfo};

const RATE: u32 = 44100;

/// A 1 kHz sine at `amp`, `secs` long, mono, as FLAC with these tags.
fn tone(path: &Path, amp: f64, secs: u32, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cmd = Proc::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
    ])
    .arg(format!("aevalsrc={amp}*sin(2*PI*1000*t):s={RATE}:d={secs}"));
    for (k, v) in tags {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    let st = cmd
        .arg(path)
        .status()
        .expect("ffmpeg must be installed to run these tests");
    assert!(st.success());
}

struct Fixture {
    _dir: tempfile::TempDir,
    db: PathBuf,
    loose: PathBuf,
    /// Album "Loud": track 1 at 0.5, track 2 at 0.25, no tags.
    loud: Vec<TrackInfo>,
    /// Album "Tagged": ReplayGain tags, track gains -3 and -5, album -9.
    tagged: Vec<TrackInfo>,
}

/// One library for the whole file, measured once. The decoder's lookup
/// is process-wide, so tests that play take the lock below.
fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("Music");
        for (n, amp) in [(1, 0.5), (2, 0.25)] {
            tone(
                &root.join(format!("Loud/0{n}.flac")),
                amp,
                4,
                &[
                    ("title", &format!("Loud {n}")),
                    ("artist", "Band"),
                    ("album", "Loud"),
                    ("track", &n.to_string()),
                ],
            );
        }
        for (n, g) in [(1, "-3.00 dB"), (2, "-5.00 dB")] {
            tone(
                &root.join(format!("Tagged/0{n}.flac")),
                0.5,
                2,
                &[
                    ("title", &format!("Tagged {n}")),
                    ("artist", "Band"),
                    ("album", "Tagged"),
                    ("track", &n.to_string()),
                    ("REPLAYGAIN_TRACK_GAIN", g),
                    ("REPLAYGAIN_TRACK_PEAK", "0.5"),
                    ("REPLAYGAIN_ALBUM_GAIN", "-9.00 dB"),
                    ("REPLAYGAIN_ALBUM_PEAK", "0.5"),
                ],
            );
        }
        let loose = dir.path().join("loose.flac");
        tone(&loose, 0.5, 3, &[("title", "Loose")]);
        let db = dir.path().join("lib.db");
        let mut lib = Library::open(&db).unwrap();
        lib.index_folders(std::slice::from_ref(&root)).unwrap();
        let info = |p: &str| lib.track_by_path(&root.join(p)).unwrap().unwrap().to_info();
        let loud = vec![info("Loud/01.flac"), info("Loud/02.flac")];
        let tagged = vec![info("Tagged/01.flac"), info("Tagged/02.flac")];

        // Measure the library: the two untagged songs.
        let svc = LevelService::start(&db).unwrap();
        svc.measure_library();
        let ev = svc
            .events()
            .iter()
            .find(|e| matches!(e, LevelEvent::LibraryDone { .. }))
            .unwrap();
        assert_eq!(
            ev,
            LevelEvent::LibraryDone {
                measured: 2,
                failed: 0
            }
        );
        Fixture {
            _dir: dir,
            db,
            loose,
            loud,
            tagged,
        }
    })
}

fn serial() -> MutexGuard<'static, ()> {
    static M: Mutex<()> = Mutex::new(());
    M.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn measured_gains_and_album_from_songs() {
    let f = fixture();
    let lib = Library::open(&f.db).unwrap();
    // A 0.5 sine is about -6 LUFS (mono counts as both speakers): -12 dB
    // to reach -18. The 0.25 one is 6 dB quieter.
    let g1 = lib.gains_for(&f.loud[0]).unwrap();
    assert_eq!(g1.track_from, Some(Origin::Measured));
    assert!((g1.track_gain.unwrap() + 12.0).abs() < 0.3, "{g1:?}");
    assert!((g1.track_peak.unwrap() - 0.5).abs() < 0.02);
    let g2 = lib.gains_for(&f.loud[1]).unwrap();
    assert!((g2.track_gain.unwrap() + 6.0).abs() < 0.3, "{g2:?}");
    // No album tags: the album gain comes from both songs, the louder one
    // weighing more. Energy average of -6 and -12 LUFS is -8.04.
    assert_eq!(g1.album_from, Some(Origin::Measured));
    assert!((g1.album_gain.unwrap() + 9.96).abs() < 0.3, "{g1:?}");
    assert_eq!(g1.album_gain, g2.album_gain);
    // Tagged songs keep their tags and were never measured.
    let t = lib.gains_for(&f.tagged[1]).unwrap();
    assert_eq!(t.track_from, Some(Origin::Tags));
    assert_eq!(t.track_gain, Some(-5.0));
    assert_eq!(t.album_gain, Some(-9.0));
    assert!(
        lib.loudness(&patina_core::library::file_uri(&f.tagged[1].path))
            .unwrap()
            .is_none()
    );
}

/// Play tracks with this leveling and capture what comes out.
fn play(tracks: Vec<TrackInfo>, leveling: LevelConfig, shuffle: bool) -> Vec<f64> {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let cap = capture.clone();
    let factory: OutputFactory =
        Box::new(move |_, r| Ok(Box::new(NullOutput::start(r, Some(cap.clone())))));
    let player = Player::spawn_with(
        PlayerConfig {
            volume: 1.0,
            fade_ms: 0,
            leveling,
            ..Default::default()
        },
        Hooks {
            output: Some(factory),
            ..Default::default()
        },
    );
    if shuffle {
        // Shuffled from the top, so every song plays.
        player.send(Command::PlayAll {
            tracks,
            shuffle: true,
        });
    } else {
        player.send(Command::SetQueueTracks {
            tracks,
            start: 0,
            pos: Duration::ZERO,
        });
    }
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

/// Peak in seconds `from`..`to` of a mono capture.
fn peak(buf: &[f64], from: f64, to: f64) -> f64 {
    let a = (from * RATE as f64) as usize;
    let b = ((to * RATE as f64) as usize).min(buf.len());
    buf[a..b].iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

#[test]
fn track_and_album_gains_reach_the_output() {
    let f = fixture();
    let _g = serial();
    level::lookup::install(Some(&f.db));

    // Track mode: both songs come out at the same level, 0.5 at -12 dB
    // and 0.25 at -6 dB, about 0.125 each.
    let track = LevelConfig {
        mode: LevelMode::Track,
        ..Default::default()
    };
    let out = play(f.loud.clone(), track, false);
    assert_eq!(out.len(), RATE as usize * 8);
    let (a, b) = (peak(&out, 0.5, 3.5), peak(&out, 4.5, 7.5));
    assert!((db(a) - db(0.125)).abs() < 0.4, "track 1 at {} dB", db(a));
    assert!((db(b) - db(0.125)).abs() < 0.4, "track 2 at {} dB", db(b));

    // Smart, in order: the album gain for both, so track 2 stays 6 dB
    // quieter, the way the album was made.
    let out = play(f.loud.clone(), LevelConfig::default(), false);
    let (a, b) = (peak(&out, 0.5, 3.5), peak(&out, 4.5, 7.5));
    assert!(
        (db(a) - (db(0.5) - 9.96)).abs() < 0.4,
        "track 1 at {} dB",
        db(a)
    );
    assert!(
        (db(a) - db(b) - 6.02).abs() < 0.2,
        "difference {}",
        db(a) - db(b)
    );

    // Smart on shuffle: track gains again.
    let out = play(f.loud.clone(), LevelConfig::default(), true);
    let (a, b) = (peak(&out, 0.5, 3.5), peak(&out, 4.5, 7.5));
    assert!(
        (db(a) - db(b)).abs() < 0.4,
        "shuffle: {} vs {}",
        db(a),
        db(b)
    );

    // Tags, with a louder target: -5 dB plus 8 is +3 dB, which the 0.5
    // peak has room for.
    let loud = LevelConfig {
        mode: LevelMode::Track,
        target: -10.0,
        ..Default::default()
    };
    let out = play(vec![f.tagged[1].clone()], loud, false);
    let p = peak(&out, 0.2, 1.8);
    assert!((db(p) - (db(0.5) + 3.0)).abs() < 0.3, "{} dB", db(p));
    level::lookup::install(None);
}

#[test]
fn unknown_songs_play_at_the_fallback_and_off_is_untouched() {
    let f = fixture();
    let _g = serial();
    level::lookup::install(Some(&f.db));
    let loose = TrackInfo {
        path: f.loose.clone(),
        ..Default::default()
    };
    let out = play(vec![loose.clone()], LevelConfig::default(), false);
    let p = peak(&out, 0.5, 2.5);
    assert!((db(p) - (db(0.5) - 6.0)).abs() < 0.2, "{} dB", db(p));

    let off = LevelConfig {
        mode: LevelMode::Off,
        ..Default::default()
    };
    let out = play(vec![loose], off, false);
    let p = peak(&out, 0.5, 2.5);
    assert!((p - 0.5).abs() < 0.01, "{p}");
    level::lookup::install(None);
}

#[test]
fn songs_coming_up_get_measured() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("next.flac");
    tone(&file, 0.25, 2, &[]);
    let db = dir.path().join("lib.db");
    let svc = LevelService::start(&db).unwrap();
    let info = TrackInfo {
        path: file.clone(),
        ..Default::default()
    };
    svc.ahead(vec![info.clone()]);
    let key = patina_core::library::file_uri(&file);
    let got = svc
        .events()
        .recv_timeout(Duration::from_secs(30))
        .expect("measured");
    assert_eq!(got, LevelEvent::Measured(key.clone()));
    let lib = Library::open(&db).unwrap();
    let row = lib.loudness(&key).unwrap().unwrap();
    assert_eq!(row.source, "measured");
    // 0.25 is about -12 LUFS: -6 dB to the reference.
    assert!((row.track_gain.unwrap() + 6.0).abs() < 0.3, "{row:?}");
    assert!(!lib.needs_measuring(&info).unwrap());
    // A changed file is measured again.
    tone(&file, 0.5, 3, &[]);
    assert!(lib.needs_measuring(&info).unwrap());
}

#[test]
fn settings_rows_and_bit_perfect() {
    let mut cfg = Config::default();
    let rows = |cfg: &Config| {
        settings::rows(
            cfg,
            &SettingsInfo::default(),
            1.0,
            false,
            &[],
            &[],
            settings::Live {
                measuring: Some(false),
                lastfm: None,
            },
        )
    };
    let lv: Vec<_> = rows(&cfg)
        .into_iter()
        .filter(|(g, _)| *g == Some("Leveling"))
        .map(|(_, s)| s)
        .collect();
    assert_eq!(lv.len(), 7);
    assert_eq!(lv[0].key, Key::LevelMode);
    assert_eq!(
        lv[0].control.display(),
        "Smart (album in order, else track)"
    );
    assert_eq!(lv[1].control.display(), "-18 LUFS");
    assert_eq!(lv[2].control.display(), "-6 dB");
    assert!(lv.iter().all(|s| s.disabled.is_none()));
    // Bit-perfect on greys every leveling row out, with the reason.
    cfg.output.bit_perfect = BitPerfect::On;
    let lv: Vec<_> = rows(&cfg)
        .into_iter()
        .filter(|(g, _)| *g == Some("Leveling"))
        .collect();
    // Measuring only fills in the index, so that row stays open.
    for (_, s) in &lv {
        let want = (s.key != Key::MeasureLibrary).then_some(settings::LEVELING_BIT_PERFECT);
        assert_eq!(s.disabled, want, "{:?}", s.key);
    }
    // Auto leaves leveling alone, and leveling keeps Auto off.
    assert!(!BitPerfect::Auto.wanted(true));
    assert!(BitPerfect::Auto.wanted(false));
}

#[test]
fn config_section_reads_and_clamps() {
    let p = Path::new("/tmp/config.toml");
    let c = Config::parse(
        "[leveling]\nmode = \"album\"\ntarget_lufs = -30\nfallback_db = -3.5\nlive_radio = false\n",
        p,
    )
    .unwrap();
    assert_eq!(c.leveling.mode, LevelMode::Album);
    assert_eq!(c.leveling.target, level::TARGET_MIN);
    assert_eq!(c.leveling.fallback, -3.5);
    assert!(!c.leveling.live_streams);
    assert!(c.leveling.clip_guard);
    assert!(Config::parse("[leveling]\nmode = \"loud\"\n", p).is_err());
    // The default file has the section, and it reads back as the default.
    let d = Config::parse(&patina_core::config::default_text(), p).unwrap();
    assert_eq!(d.leveling, LevelConfig::default());
}

#[test]
fn the_live_normalizer_rides_a_podcast_to_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("episode.flac");
    // About -6 LUFS: 12 dB over the target.
    tone(&file, 0.5, 6, &[]);
    let ep = TrackInfo {
        path: file,
        kind: patina_core::track::MediaKind::Podcast,
        ..Default::default()
    };
    let out = play(vec![ep], LevelConfig::default(), false);
    // It starts at the fallback gain...
    let start = peak(&out, 0.0, 0.1);
    assert!(
        (db(start) - (db(0.5) - 6.0)).abs() < 0.5,
        "starts at {} dB",
        db(start)
    );
    // ...and a few seconds in sits at the target.
    let end = peak(&out, 4.5, 5.9);
    assert!(
        (db(end) - (db(0.5) - 12.0)).abs() < 1.0,
        "ends at {} dB",
        db(end)
    );
}
