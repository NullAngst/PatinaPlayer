//! Bit-perfect output, checked bit for bit.
//!
//! The device is ALSA's `file` plugin, which writes everything played to
//! it into a file. That makes this a loopback test that needs no sound card
//! and no kernel module: whatever PatinaPlayer sent the "DAC" is compared
//! byte for byte with FFmpeg's own decode of the source. Any resampling,
//! volume, dither, padding or rounding anywhere in the chain shows up as a
//! mismatch.
//!
//! Needs the `ffmpeg` command and libasound (no hardware).

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use patina_core::audio::output::BitPerfect;
use patina_core::{Backend, Command, Event, OutputConfig, Player, PlayerConfig, TrackInfo};

fn ffmpeg(args: &[&str]) {
    let st = Proc::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg must be installed");
    assert!(st.success(), "ffmpeg {args:?}");
}

/// A FLAC of white noise (every bit busy) and FFmpeg's raw decode of it.
fn source(dir: &Path, name: &str, rate: u32, bits: u32) -> (PathBuf, Vec<u8>) {
    let flac = dir.join(format!("{name}.flac"));
    let raw = dir.join(format!("{name}.raw"));
    let noise = format!("anoisesrc=d=1.5:c=white:r={rate}:a=0.7");
    let (fmt, pcm, raw_fmt) = if bits == 16 {
        ("s16", "pcm_s16le", "s16le")
    } else {
        ("s32", "pcm_s24le", "s24le")
    };
    let bits_s = bits.to_string();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &noise,
        "-ac",
        "2",
        "-sample_fmt",
        fmt,
        "-bits_per_raw_sample",
        &bits_s,
        "-c:a",
        "flac",
        flac.to_str().unwrap(),
    ]);
    ffmpeg(&[
        "-i",
        flac.to_str().unwrap(),
        "-c:a",
        pcm,
        "-f",
        raw_fmt,
        raw.to_str().unwrap(),
    ]);
    (flac, std::fs::read(&raw).unwrap())
}

fn play_to_file(flac: &Path, out: &Path) -> patina_core::Snapshot {
    play_to(flac, &format!("file:'{}',raw", out.display()))
}

fn play_to(flac: &Path, device: &str) -> patina_core::Snapshot {
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            bit_perfect: BitPerfect::On,
            bit_perfect_device: Some(device.to_string()),
            ..Default::default()
        },
        // Volume isn't at 100% on purpose: bit-perfect has to ignore it.
        volume: 0.5,
        fade_ms: 0,
        ..Default::default()
    });
    player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo::unprobed(flac)],
        start: 0,
        pos: Duration::ZERO,
    });
    let mut seen = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "never finished");
        let s = player.snapshot();
        if s.exclusive.is_some() && seen.is_none() {
            seen = Some(s.clone());
        }
        if let Ok(Event::Finished) = player.events().recv_timeout(Duration::from_millis(20)) {
            break;
        }
    }
    player.send(Command::Quit);
    std::thread::sleep(Duration::from_millis(200));
    seen.expect("the exclusive output never opened")
}

#[test]
fn sixteen_bit_flac_arrives_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    let (flac, want) = source(dir.path(), "cd", 44100, 16);
    let out = dir.path().join("dac16.raw");
    let snap = play_to_file(&flac, &out);
    let ex = snap.exclusive.unwrap();
    assert!(ex.exact, "{ex:?}");
    assert_eq!(ex.format, "S16_LE");
    assert!(snap.volume_locked, "no mixer on a file, so volume is fixed");
    let got = std::fs::read(&out).unwrap();
    assert_eq!(got.len(), want.len(), "same length");
    if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
        let n = got.iter().zip(&want).filter(|(a, b)| a != b).count();
        panic!(
            "byte {i} of {} differs ({n} bytes in all): got {:?}, want {:?}",
            got.len(),
            &got[i..(i + 8).min(got.len())],
            &want[i..(i + 8).min(want.len())]
        );
    }
}

#[test]
fn twenty_four_bit_hires_arrives_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    let (flac, want) = source(dir.path(), "hires", 96000, 24);
    let out = dir.path().join("dac24.raw");
    let snap = play_to_file(&flac, &out);
    let ex = snap.exclusive.unwrap();
    assert!(ex.exact, "{ex:?}");
    assert_eq!(ex.format, "S24_3LE");
    let got = std::fs::read(&out).unwrap();
    assert_eq!(got.len(), want.len(), "same length");
    if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
        let n = got.iter().zip(&want).filter(|(a, b)| a != b).count();
        panic!(
            "byte {i} of {} differs ({n} bytes in all): got {:?}, want {:?}",
            got.len(),
            &got[i..(i + 8).min(got.len())],
            &want[i..(i + 8).min(want.len())]
        );
    }
}

#[test]
fn a_device_that_cant_take_it_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (flac, _) = source(dir.path(), "cd", 44100, 16);
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            bit_perfect: BitPerfect::On,
            bit_perfect_device: Some("hw:CARD=NoSuchDac,DEV=0".into()),
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    player.send(Command::SetQueueTracks {
        tracks: vec![TrackInfo::unprobed(&flac)],
        start: 0,
        pos: Duration::ZERO,
    });
    // It still plays, through the normal output, and says why.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no refusal reported");
        let s = player.snapshot();
        if let Some(r) = &s.bit_perfect_refused {
            assert!(r.contains("16/44.1"), "{r}");
            assert!(s.exclusive.is_none());
            assert_eq!(s.state, patina_core::PlayState::Playing);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    player.send(Command::Quit);
}

/// The same check through a real ALSA kernel driver: snd-aloop, a virtual
/// sound card whose capture side hears exactly what its playback side is
/// sent. CI loads the module and runs this; it needs `arecord` too.
#[test]
#[ignore = "needs the snd-aloop kernel module (CI loads it)"]
fn real_loopback_card_hears_every_bit() {
    let dir = tempfile::tempdir().unwrap();
    let (flac, want) = source(dir.path(), "cd", 44100, 16);
    let cap = dir.path().join("captured.raw");
    let mut rec = Proc::new("arecord")
        .args([
            "-q",
            "-D",
            "hw:Loopback,1,0",
            "-f",
            "S16_LE",
            "-r",
            "44100",
            "-c",
            "2",
        ])
        .args(["-t", "raw", "-d", "5"])
        .arg(&cap)
        .spawn()
        .expect("arecord (alsa-utils) must be installed");
    std::thread::sleep(Duration::from_millis(300));
    let snap = play_to(&flac, "hw:Loopback,0,0");
    assert!(snap.exclusive.unwrap().exact);
    rec.wait().unwrap();
    let got = std::fs::read(&cap).unwrap();
    // Recording started before playback, so the music sits after some
    // silence. Find where it starts, then it has to match to the end.
    let head = &want[..4096];
    let at = got
        .windows(head.len())
        .position(|w| w == head)
        .expect("the start of the music isn't in the recording");
    assert!(
        got[at..].starts_with(&want),
        "the recording differs from the source after byte {at}"
    );
}
