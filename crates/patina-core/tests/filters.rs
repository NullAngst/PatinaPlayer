//! Tempo and silence trimming through the decoder's filter graph.
//! Needs the `ffmpeg` command on PATH.

use std::process::Command as Proc;
use std::time::Duration;

use patina_core::audio::decoder::{OpenOptions, Source};

fn ffmpeg(args: &[&str]) {
    let st = Proc::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success());
}

fn frames(path: &str, opts: OpenOptions) -> (usize, Source) {
    let mut s = Source::open_with(path, opts).unwrap();
    let mut out = Vec::new();
    while s.decode_into(&mut out).unwrap() > 0 {}
    (out.len() / 2, s)
}

#[test]
fn tempo_keeps_rate_and_shortens() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("t.flac");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:r=44100:d=10",
        "-ac",
        "2",
        f.to_str().unwrap(),
    ]);
    let f = f.to_str().unwrap();
    let (plain, _) = frames(f, OpenOptions::default());
    let (fast, src) = frames(
        f,
        OpenOptions {
            tempo: 2.0,
            ..Default::default()
        },
    );
    assert_eq!(src.format().rate, 44100, "tempo must not change the rate");
    let ratio = fast as f64 / plain as f64;
    assert!(
        (ratio - 0.5).abs() < 0.02,
        "2x should halve the length, got {ratio}"
    );
    let pos = src.source_position().unwrap();
    assert!(
        pos > Duration::from_millis(9900),
        "source position tracks the input: {pos:?}"
    );

    let (slow, _) = frames(
        f,
        OpenOptions {
            tempo: 0.5,
            ..Default::default()
        },
    );
    let ratio = slow as f64 / plain as f64;
    assert!(
        (ratio - 2.0).abs() < 0.05,
        "0.5x should double, got {ratio}"
    );

    let (faster, _) = frames(
        f,
        OpenOptions {
            tempo: 3.0,
            ..Default::default()
        },
    );
    let ratio = faster as f64 / plain as f64;
    assert!((ratio - 1.0 / 3.0).abs() < 0.02, "3x, got {ratio}");
}

#[test]
fn silence_trimming_cuts_long_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("s.flac");
    // 2 s tone, 4 s silence, 2 s tone.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:r=44100:d=2",
        "-f",
        "lavfi",
        "-t",
        "4",
        "-i",
        "anullsrc=r=44100:cl=mono",
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:r=44100:d=2",
        "-filter_complex",
        "[0][1][2]concat=n=3:v=0:a=1,aformat=channel_layouts=stereo",
        f.to_str().unwrap(),
    ]);
    let (n, _) = frames(
        f.to_str().unwrap(),
        OpenOptions {
            trim_silence: true,
            ..Default::default()
        },
    );
    let secs = n as f64 / 44100.0;
    assert!(
        secs > 4.0 && secs < 5.3,
        "expected about 4.7 s left of 8, got {secs}"
    );
}
