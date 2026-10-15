//! Output controls end to end: resampling to a fixed rate, gapless joins
//! across a rate change, and crossfade. Played through the real player
//! with a capturing null output, against signals worked out exactly.
//!
//! Needs the `ffmpeg` command on PATH to make the fixtures.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::audio::StreamFormat;
use patina_core::audio::decoder::Source;
use patina_core::audio::output::OutputConfig;
use patina_core::audio::output::null::NullOutput;
use patina_core::dsp::resample::Quality;
use patina_core::level::{LevelConfig, LevelMode};
use patina_core::player::{Hooks, OutputFactory};
use patina_core::{Command, Event, Player, PlayerConfig};

fn ffmpeg(args: &[&str]) {
    let st = Proc::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg must be installed to run these tests");
    assert!(st.success(), "ffmpeg {args:?} failed");
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("patina-out-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The test tone at time `t`: a 441 Hz wobbling left, a 660 Hz right.
fn tone(t: f64) -> [f64; 2] {
    [
        0.4 * (2.0 * PI * 441.0 * t).sin() * (0.6 + 0.4 * (2.0 * PI * 0.7 * t).sin()),
        0.4 * (2.0 * PI * 660.0 * t).sin(),
    ]
}

/// The same tone for FFmpeg, starting `t0` seconds in.
fn tone_expr(t0: f64) -> String {
    let t = format!("(t+{t0})");
    format!("0.4*sin(2*PI*441*{t})*(0.6+0.4*sin(2*PI*0.7*{t}))|0.4*sin(2*PI*660*{t})")
}

/// A FLAC of the tone from `t0` for `frames` frames at `rate`, tagged.
fn tone_flac(path: &Path, rate: u32, t0: f64, frames: u64, album: &str, track: u32) {
    let d = frames as f64 / rate as f64;
    let src = format!("aevalsrc={}:s={rate}:d={d}", tone_expr(t0));
    let mut args = vec!["-f", "lavfi", "-i", &src];
    let (a, t) = (format!("album={album}"), format!("track={track}"));
    let trim = format!("atrim=end_sample={frames}");
    args.extend(["-af", &trim, "-sample_fmt", "s32", "-c:a", "flac"]);
    if !album.is_empty() {
        args.extend(["-metadata", &a, "-metadata", &t]);
    }
    args.push(path.to_str().unwrap());
    ffmpeg(&args);
}

/// A FLAC of a constant level, `secs` long at 44.1 kHz.
fn dc_flac(path: &Path, level: f64, secs: f64) {
    let src = format!("aevalsrc={level}|{level}:s=44100:d={secs}");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &src,
        "-sample_fmt",
        "s32",
        "-c:a",
        "flac",
        path.to_str().unwrap(),
    ]);
}

fn frames_in(path: &Path) -> usize {
    let mut src = Source::open(path).unwrap();
    let mut out = Vec::new();
    while src.decode_into(&mut out).unwrap() > 0 {}
    out.len() / 2
}

struct Played {
    samples: Vec<f64>,
    /// Every output opened, in order.
    opens: Vec<StreamFormat>,
}

fn play(parts: Vec<PathBuf>, output: OutputConfig) -> Played {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let opens = Arc::new(Mutex::new(Vec::new()));
    let (cap2, opens2) = (capture.clone(), opens.clone());
    let factory: OutputFactory = Box::new(move |_, renderer| {
        opens2.lock().unwrap().push(renderer.format());
        Ok(Box::new(NullOutput::start(renderer, Some(cap2.clone()))))
    });
    let cfg = PlayerConfig {
        volume: 1.0,
        fade_ms: 0,
        leveling: LevelConfig {
            mode: LevelMode::Off,
            ..Default::default()
        },
        // The limiter would touch the crossfade's sum.
        limiter_on: false,
        output,
        ..Default::default()
    };
    let player = Player::spawn_with(
        cfg,
        Hooks {
            output: Some(factory),
            ..Default::default()
        },
    );
    player.send(Command::SetQueue {
        paths: parts,
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
    let samples = capture.lock().unwrap().clone();
    let opens = opens.lock().unwrap().clone();
    Played { samples, opens }
}

/// Largest difference from the exact tone at `rate`, and where.
fn worst(samples: &[f64], rate: u32) -> (f64, usize) {
    let mut w = (0.0, 0);
    for (i, f) in samples.chunks(2).enumerate() {
        let r = tone(i as f64 / rate as f64);
        let e = (f[0] - r[0]).abs().max((f[1] - r[1]).abs());
        if e > w.0 {
            w = (e, i);
        }
    }
    w
}

#[test]
fn fixed_rate_resamples_an_album_as_one_stream() {
    // Three 44.1 kHz parts cut at odd places, played at a fixed 48 kHz:
    // one output, every frame there, and the result is the tone itself at
    // 48 kHz, joins included.
    let d = dir("fixed");
    let cuts = [0u64, 77_777, 173_311, 264_600];
    let mut parts = Vec::new();
    for i in 0..3 {
        let p = d.join(format!("{i}.flac"));
        tone_flac(
            &p,
            44100,
            cuts[i] as f64 / 44100.0,
            cuts[i + 1] - cuts[i],
            "Fixed",
            i as u32 + 1,
        );
        parts.push(p);
    }
    for q in [Quality::Balanced, Quality::Best] {
        let got = play(
            parts.clone(),
            OutputConfig {
                rate: 48000,
                resampler: q,
                ..Default::default()
            },
        );
        assert_eq!(got.opens.len(), 1, "{q:?}: one output for the album");
        assert_eq!(got.opens[0].rate, 48000);
        assert_eq!(got.samples.len() / 2, 288_000, "{q:?}: 6 s at 48 kHz");
        let (e, at) = worst(&got.samples, 48000);
        assert!(e < 3e-4, "{q:?}: off by {e} at frame {at}");
    }
}

#[test]
fn rate_change_inside_an_album_stays_gapless() {
    // Track 1 at 44.1 kHz, track 2 the same tone carrying on at 48 kHz.
    // Matching the source, the ring keeps 44.1 and track 2 is resampled
    // into it: one output, no gap, no click at the join.
    let d = dir("album");
    let (a, b) = (d.join("a.flac"), d.join("b.flac"));
    let cut = 100_003u64;
    tone_flac(&a, 44100, 0.0, cut, "Join", 1);
    let b_frames = 96_000u64;
    tone_flac(&b, 48000, cut as f64 / 44100.0, b_frames, "Join", 2);
    let got = play(vec![a, b], OutputConfig::default());
    assert_eq!(got.opens.len(), 1, "no new output at the join");
    assert_eq!(got.opens[0].rate, 44100);
    let want = cut as usize + (b_frames as f64 * 44100.0 / 48000.0).round() as usize;
    assert_eq!(got.samples.len() / 2, want);
    let (e, at) = worst(&got.samples, 44100);
    assert!(e < 5e-4, "off by {e} at frame {at} (join at {cut})");
}

#[test]
fn rate_change_between_albums_reopens_at_the_new_rate() {
    let d = dir("albums");
    let (a, b) = (d.join("a.flac"), d.join("b.flac"));
    tone_flac(&a, 44100, 0.0, 44100, "One", 1);
    tone_flac(&b, 48000, 0.0, 48000, "Two", 1);
    let got = play(vec![a, b], OutputConfig::default());
    let rates: Vec<u32> = got.opens.iter().map(|f| f.rate).collect();
    assert_eq!(rates, vec![44100, 48000], "each album at its own rate");
    assert_eq!(got.samples.len() / 2, 44100 + 48000);
}

#[test]
fn crossfade_mixes_with_equal_power() {
    // A at 0.5, B at 0.25, a one second crossfade: A plays alone, then
    // 0.5·cos + 0.25·sin over the fade, then B alone. The fade overlaps,
    // so the total is a second short.
    let d = dir("xfade");
    let (a, b) = (d.join("a.flac"), d.join("b.flac"));
    dc_flac(&a, 0.5, 3.0);
    dc_flac(&b, 0.25, 3.0);
    let (na, nb) = (frames_in(&a), frames_in(&b));
    let got = play(
        vec![a, b],
        OutputConfig {
            crossfade_ms: 1000,
            ..Default::default()
        },
    );
    let n = got.samples.len() / 2;
    assert_eq!(n, na + nb - 44100);
    let l = |i: usize| got.samples[i * 2];
    let fade0 = na - 44100;
    assert!((l(fade0 - 1) - 0.5).abs() < 1e-6, "A alone before the fade");
    for k in [0usize, 11025, 22050, 33075] {
        let t = k as f64 / 44100.0 * PI / 2.0;
        let want = 0.5 * t.cos() + 0.25 * t.sin();
        assert!(
            (l(fade0 + k) - want).abs() < 1e-6,
            "at {k}: {} vs {want}",
            l(fade0 + k)
        );
    }
    assert!((l(fade0 + 44100) - 0.25).abs() < 1e-6, "B alone after");
    assert!((l(n - 1) - 0.25).abs() < 1e-6);
}

#[test]
fn crossfade_leaves_an_album_in_order_alone() {
    let d = dir("xalbum");
    let (a, b) = (d.join("a.flac"), d.join("b.flac"));
    tone_flac(&a, 44100, 0.0, 88_200, "Live", 1);
    tone_flac(&b, 44100, 2.0, 88_200, "Live", 2);
    let got = play(
        vec![a, b],
        OutputConfig {
            crossfade_ms: 2000,
            ..Default::default()
        },
    );
    assert_eq!(got.samples.len() / 2, 176_400, "nothing overlapped");
    let (e, at) = worst(&got.samples, 44100);
    assert!(e < 1e-6, "off by {e} at {at}");
}

/// Run with a private PipeWire that has two null sinks, "headphones" and
/// "speakers" (CI sets this up).
#[cfg(feature = "pipewire")]
#[test]
#[ignore = "needs a running PipeWire (see the comment)"]
fn pipewire_lists_its_sinks() {
    use patina_core::audio::output::{Backend, list_outputs};
    let got = list_outputs(Backend::PipeWire);
    for (id, label) in [
        ("headphones", "Test headphones"),
        ("speakers", "Test speakers"),
    ] {
        assert!(
            got.iter().any(|d| d.id == id && d.label == label),
            "{id} missing from {got:?}"
        );
    }
}
