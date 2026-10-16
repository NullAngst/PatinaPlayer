//! Bit-perfect output: the DAC opened for this app alone, at the source's
//! own rate and sample depth, with nothing in between.
//!
//! "Nothing in between" means: no hidden resampling or format conversion,
//! no software volume (the DAC's own mixer if it has one, otherwise volume
//! stays at 100%), and no samples that aren't the music. While playing,
//! only what comes out of the ring is written; a starved ring stops the
//! device rather than padding it with silence.
//!
//! When the device can't take the source as it is (a rate it doesn't do, a
//! 24-bit file on a 16-bit-only DAC), this refuses with the reason. The
//! player then plays through the normal output and shows why.
//!
//! On Linux it's an ALSA hardware device (`exclusive_alsa`). On Android
//! it's AAudio on a USB DAC the system has set to bit-perfect (`aaudio`).
//! The parts here are shared.

// A build with no bit-perfect path (no ALSA, not Android) keeps these for
// the tests only.
#![cfg_attr(not(any(feature = "alsa", target_os = "android")), allow(dead_code))]

use super::{to_f32, to_int};
use crate::audio::Depth;
#[cfg(not(any(feature = "alsa", target_os = "android")))]
use crate::audio::SharedRef;
use crate::audio::render::Renderer;

#[cfg(all(not(feature = "alsa"), target_os = "android"))]
pub use super::aaudio::{ExclusiveOutput, list_devices};
#[cfg(feature = "alsa")]
pub use super::exclusive_alsa::{ExclusiveOutput, list_devices};

/// A playback device you can pick for bit-perfect output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HwDevice {
    /// `hw:CARD=DAC,DEV=0`. Stable across reboots, unlike card numbers.
    pub id: String,
    /// "Schiit Modi 3: USB Audio"
    pub name: String,
}

/// What the bit-perfect output is doing, for the badge and Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExclusiveInfo {
    pub device: String,
    /// The ALSA sample format in use: "S24_3LE".
    pub format: String,
    /// The source's exact bits reach the DAC. False for a lossy or float
    /// source going out as integers: still exclusive and unprocessed, but
    /// not "bit-perfect" in the strict sense.
    pub exact: bool,
    /// Volume goes to the DAC's own mixer. False means it's fixed at 100%.
    pub hw_volume: bool,
}

/// A sample format a bit-perfect device can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sample {
    S16,
    S24In32,
    S24Packed,
    S32,
    F32,
}

impl Sample {
    pub(crate) fn bytes(self) -> usize {
        match self {
            Sample::S16 => 2,
            Sample::S24Packed => 3,
            Sample::S24In32 | Sample::S32 | Sample::F32 => 4,
        }
    }
}

/// Formats that carry `depth` without losing a bit, best first. A 16-bit
/// source in a 32-bit container is still exact: the low bits are zero.
pub(crate) fn candidates(depth: Depth) -> (Vec<Sample>, Vec<Sample>) {
    use Sample::*;
    match depth {
        Depth::Int(b) if b <= 16 => (vec![S16, S24Packed, S24In32, S32], vec![]),
        Depth::Int(b) if b <= 24 => (vec![S24Packed, S24In32, S32], vec![]),
        Depth::Int(_) => (vec![S32], vec![]),
        // Float sources are exact only as float. Integer output is still
        // untouched apart from that one rounding.
        Depth::Float => (vec![F32], vec![S32, S24Packed, S24In32, S16]),
    }
}

/// "24/96", "44.1 kHz": the source, as people say it.
pub fn describe(depth: Depth, rate: u32) -> String {
    let khz = if rate.is_multiple_of(1000) {
        format!("{}", rate / 1000)
    } else {
        format!("{:.1}", rate as f64 / 1000.0)
    };
    match depth {
        Depth::Int(b) => format!("{b}/{khz}"),
        Depth::Float => format!("{khz} kHz"),
    }
}

/// Why the device was refused, with the renderer handed back so the
/// player can use it for the shared output instead.
pub type Refused = Box<(String, Renderer)>;

/// Pack a block of f64 samples into `out` as the device's format. Returns
/// the bytes written. Integer conversion is the exact inverse of the
/// decoder's, so 16 and 24-bit sources come out bit for bit.
pub(crate) fn encode(block: &[f64], sample: Sample, out: &mut [u8]) -> usize {
    let mut i = 0;
    for &s in block {
        match sample {
            Sample::S16 => {
                out[i..i + 2].copy_from_slice(&(to_int(s, 16) as i16).to_le_bytes());
                i += 2;
            }
            Sample::S24In32 => {
                out[i..i + 4].copy_from_slice(&(to_int(s, 24) as i32).to_le_bytes());
                i += 4;
            }
            Sample::S24Packed => {
                let b = (to_int(s, 24) as i32).to_le_bytes();
                out[i..i + 3].copy_from_slice(&b[..3]);
                i += 3;
            }
            Sample::S32 => {
                out[i..i + 4].copy_from_slice(&(to_int(s, 32) as i32).to_le_bytes());
                i += 4;
            }
            Sample::F32 => {
                out[i..i + 4].copy_from_slice(&to_f32(s).to_le_bytes());
                i += 4;
            }
        }
    }
    i
}

/// No bit-perfect path on this platform.
#[cfg(not(any(feature = "alsa", target_os = "android")))]
pub struct ExclusiveOutput;

#[cfg(not(any(feature = "alsa", target_os = "android")))]
impl ExclusiveOutput {
    pub fn try_start(
        _cfg: &super::OutputConfig,
        _depth: Depth,
        renderer: Renderer,
        _shared: SharedRef,
    ) -> Result<Self, Refused> {
        Err(Box::new((
            "this build has no bit-perfect output".into(),
            renderer,
        )))
    }
}

#[cfg(not(any(feature = "alsa", target_os = "android")))]
impl super::Output for ExclusiveOutput {
    fn describe(&self) -> String {
        String::new()
    }
    fn error(&self) -> Option<String> {
        None
    }
}

#[cfg(not(any(feature = "alsa", target_os = "android")))]
pub fn list_devices() -> Vec<HwDevice> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bits_survive_every_container() {
        for v in [-32768i16, -1, 0, 1, 12345, 32767] {
            let s = v as f64 / 32768.0;
            let mut b = [0u8; 4];
            encode(&[s], Sample::S16, &mut b);
            assert_eq!(i16::from_le_bytes([b[0], b[1]]), v);
            encode(&[s], Sample::S24Packed, &mut b);
            let v24 = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
            assert_eq!(v24, (v as i32) << 8);
            encode(&[s], Sample::S32, &mut b);
            assert_eq!(i32::from_le_bytes(b), (v as i32) << 16);
        }
    }

    #[test]
    fn source_labels() {
        assert_eq!(describe(Depth::Int(24), 96000), "24/96");
        assert_eq!(describe(Depth::Int(16), 44100), "16/44.1");
        assert_eq!(describe(Depth::Float, 48000), "48 kHz");
    }
}
