//! End-to-end checks of the engine against FFmpeg-generated files.
//!
//! A continuous tone is cut into three tracks at boundaries that don't
//! line up with codec frames, then played through the real player with a
//! capturing null output. If the joins are gapless, the capture is the
//! original tone, sample for sample.
//!
//! Needs the `ffmpeg` command on PATH to make the fixtures.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::audio::decoder::Source;
use patina_core::audio::output::null::NullOutput;
use patina_core::player::OutputFactory;
use patina_core::{Command, Event, Player, PlayerConfig};

const RATE: u32 = 44100;
const TOTAL: u64 = RATE as u64 * 6;
/// Cut points in samples. Deliberately odd numbers.
const CUTS: [u64; 2] = [77_777, 173_311];

fn ffmpeg(args: &[&str]) {
    let st = Proc::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg must be installed to run these tests");
    assert!(st.success(), "ffmpeg {args:?} failed");
}

fn fixtures(tag: &str, ext: &str, codec_args: &[&str]) -> (PathBuf, Vec<PathBuf>) {
    let dir = std::env::temp_dir().join(format!("patina-gapless-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let whole = dir.join("whole.wav");
    // A tone with a slow amplitude wobble, so a dropped or repeated block
    // can't hide in a perfectly periodic signal.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!(
            "aevalsrc=0.4*sin(2*PI*441*t)*(0.6+0.4*sin(2*PI*0.7*t))|0.4*sin(2*PI*660*t):s={RATE}:d=6"
        ),
        "-c:a",
        "pcm_s16le",
        whole.to_str().unwrap(),
    ]);
    let bounds = [0, CUTS[0], CUTS[1], TOTAL];
    let mut parts = Vec::new();
    for i in 0..3 {
        let out = dir.join(format!("{:02}.{ext}", i + 1));
        let filt = format!(
            "atrim=start_sample={}:end_sample={},asetpts=PTS-STARTPTS",
            bounds[i],
            bounds[i + 1]
        );
        let mut args = vec!["-i", whole.to_str().unwrap(), "-af", &filt];
        args.extend_from_slice(codec_args);
        args.push(out.to_str().unwrap());
        ffmpeg(&args);
        parts.push(out);
    }
    (whole, parts)
}

fn decode_all(path: &Path) -> Vec<f64> {
    let mut src = Source::open(path).unwrap();
    let mut out = Vec::new();
    while src.decode_into(&mut out).unwrap() > 0 {}
    out
}

fn play_captured(parts: Vec<PathBuf>) -> Vec<f64> {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let cap2 = capture.clone();
    let factory: OutputFactory =
        Box::new(move |_, renderer| Ok(Box::new(NullOutput::start(renderer, Some(cap2.clone())))));
    let cfg = PlayerConfig {
        volume: 1.0,
        fade_ms: 0,
        ..Default::default()
    };
    let player = Player::spawn_with(cfg, Some(factory));
    player.send(Command::SetQueue {
        paths: parts,
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
    let c = capture.lock().unwrap();
    c.clone()
}

#[test]
fn flac_joins_are_sample_exact() {
    let (whole, parts) = fixtures("flac", "flac", &["-c:a", "flac"]);
    let reference = decode_all(&whole);
    assert_eq!(reference.len() as u64, TOTAL * 2);
    let got = play_captured(parts);
    assert_eq!(got.len(), reference.len(), "frame count differs");
    let first_bad = got.iter().zip(&reference).position(|(a, b)| a != b);
    assert_eq!(first_bad, None, "sample mismatch at index {first_bad:?}");
}

#[test]
fn mp3_padding_is_trimmed() {
    // MP3 is lossy, so samples won't match. What gapless needs is that
    // FFmpeg's LAME-tag trimming leaves exactly the source frame count.
    let (_, parts) = fixtures("mp3", "mp3", &["-c:a", "libmp3lame", "-b:a", "256k"]);
    let got = play_captured(parts);
    assert_eq!(
        got.len() as u64,
        TOTAL * 2,
        "encoder delay or padding leaked into playback"
    );
}

#[test]
fn seek_lands_on_the_exact_sample() {
    let (whole, _) = fixtures("seek", "flac", &["-c:a", "flac"]);
    let flac = whole.with_extension("flac");
    ffmpeg(&[
        "-i",
        whole.to_str().unwrap(),
        "-c:a",
        "flac",
        flac.to_str().unwrap(),
    ]);
    let reference = decode_all(&flac);
    let mut src = Source::open(&flac).unwrap();
    let target = Duration::from_millis(2500);
    src.seek(target).unwrap();
    let mut out = Vec::new();
    src.decode_into(&mut out).unwrap();
    let start = (2.5 * RATE as f64) as usize * 2;
    assert_eq!(&out[..64], &reference[start..start + 64]);
}

#[test]
fn rate_change_reopens_and_keeps_every_frame() {
    let dir = std::env::temp_dir().join(format!("patina-gapless-{}-rates", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.flac");
    let b = dir.join("b.flac");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:r=44100:d=1",
        "-ac",
        "2",
        a.to_str().unwrap(),
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:r=48000:d=1",
        "-ac",
        "2",
        b.to_str().unwrap(),
    ]);
    let expected = decode_all(&a).len() + decode_all(&b).len();
    let got = play_captured(vec![a, b]);
    assert_eq!(got.len(), expected);
}
