//! Audio engine: decoding on a worker thread, a lock-free ring, and a
//! real-time render callback that feeds the output backend.
//!
//! The rule for this module: anything reachable from [`render::Renderer::render`]
//! runs on the output thread and must never lock, allocate or block.

pub mod decoder;
pub mod feeder;
pub mod output;
pub mod render;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

/// Seconds of decoded audio the ring holds ahead of the output.
/// Big enough that opening the next local file never starves the output.
pub const RING_SECONDS: usize = 2;

/// Sample layout the decoder hands to the output. The ring always carries
/// interleaved f64, at the source rate or the rate it was resampled to;
/// this is what changes between tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    pub rate: u32,
    pub channels: u16,
    /// FFmpeg channel mask (AV_CH_* bits) in native order. Channels in the
    /// ring appear in increasing bit order.
    pub mask: u64,
}

/// Sample depth of a source, as far as the bit-perfect output cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Integer PCM of this many bits (16, 24, 32): FLAC, ALAC, WAV and the
    /// like. Bit-perfect means these exact bits reach the DAC.
    Int(u8),
    /// Floating point out of the decoder: every lossy codec, float WAV, DSD.
    Float,
}

impl StreamFormat {
    pub fn ring_capacity(&self) -> usize {
        self.rate as usize * self.channels as usize * RING_SECONDS
    }
}

/// Atomic f64, stored as bits. Lock-free, which is the whole point.
#[derive(Debug, Default)]
pub struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }
    pub fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// State shared between the core thread, the decoder thread and the
/// real-time render callback. Atomics, plus the EQ settings behind a mutex
/// the renderer only ever `try_lock`s, so it never waits on it.
#[derive(Debug)]
pub struct Shared {
    /// Linear volume gain applied in the render callback.
    pub volume: AtomicF64,
    /// True while the user wants sound. False ramps the output down to
    /// silence and then stops pulling from the ring.
    pub playing: AtomicBool,
    /// Fade length for pause, resume, seek and stop, in milliseconds.
    pub fade_ms: AtomicU32,
    /// Set by the renderer once its gain has fully reached zero.
    pub silent: AtomicBool,
    /// Frames pulled out of the ring, played or discarded. Never resets,
    /// so track positions can be computed against it across rings.
    pub consumed_frames: AtomicU64,
    /// Bumped by the decoder to ask the renderer to throw away whatever
    /// is in the ring. The renderer copies it into `flush_ack` when done.
    pub flush_req: AtomicU64,
    pub flush_ack: AtomicU64,
    /// Decoder has pushed the last sample of the last track.
    pub end_of_queue: AtomicBool,
    /// Renderer ran dry while `end_of_queue` was set: playback finished.
    pub drained: AtomicBool,
    /// Renderer ran dry while more audio was expected.
    pub underrun: AtomicBool,
    /// Set by the core before it sends the decoder a command, so a network
    /// read stuck waiting gives up and the command is seen at once.
    pub abort_io: Arc<AtomicBool>,
    /// Podcast playback speed (pitch kept) and silence trimming. Read by
    /// the decoder whenever it opens a podcast episode.
    pub podcast_tempo: AtomicF64,
    pub podcast_trim: AtomicBool,
    /// Bit-perfect mode, as [`output::BitPerfect`]. Read by the decoder
    /// when it opens a track, to pick the output kind.
    pub bit_perfect: AtomicU8,
    /// Leveling settings, read by the decoder as it opens each track.
    pub level: crate::level::LevelShared,
    /// EQ settings, and a counter bumped on every change so the renderer
    /// knows to pick them up.
    pub eq: std::sync::Mutex<crate::dsp::EqParams>,
    pub eq_gen: AtomicU64,
    /// The true-peak limiter: on, and its ceiling as a linear gain.
    pub limiter_on: AtomicBool,
    pub limiter_ceiling: AtomicF64,
    /// Deepest gain reduction since the core last looked, in dB.
    pub limiter_gr: AtomicF64,
    /// Output sample rate. 0 follows the source; anything else and every
    /// track is resampled to it.
    pub out_rate: AtomicU32,
    /// A rate the shared device insisted on (an ALSA hw device that can't
    /// play the source rate). 0 when there's none. Followed like
    /// `out_rate`, which wins when both are set.
    pub device_rate: AtomicU32,
    /// Resampler quality, as [`crate::dsp::resample::Quality`].
    pub resample_quality: AtomicU8,
    /// Crossfade between songs, in milliseconds. 0 is off.
    pub crossfade_ms: AtomicU32,
    /// The amp stack, and its change counter, the same way as the EQ.
    pub amp: std::sync::Mutex<crate::dsp::amp::AmpParams>,
    pub amp_gen: AtomicU64,
    /// Channel tools, the same way again.
    pub channel: std::sync::Mutex<crate::dsp::channel::ChannelParams>,
    pub channel_gen: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            volume: AtomicF64::new(1.0),
            playing: AtomicBool::new(false),
            fade_ms: AtomicU32::new(100),
            silent: AtomicBool::new(true),
            consumed_frames: AtomicU64::new(0),
            flush_req: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
            end_of_queue: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            underrun: AtomicBool::new(false),
            abort_io: Arc::new(AtomicBool::new(false)),
            podcast_tempo: AtomicF64::new(1.0),
            podcast_trim: AtomicBool::new(false),
            bit_perfect: AtomicU8::new(0),
            level: crate::level::LevelShared::default(),
            eq: std::sync::Mutex::new(crate::dsp::EqParams::default()),
            eq_gen: AtomicU64::new(0),
            limiter_on: AtomicBool::new(true),
            limiter_ceiling: AtomicF64::new(10f64.powf(-1.0 / 20.0)),
            limiter_gr: AtomicF64::new(0.0),
            out_rate: AtomicU32::new(0),
            device_rate: AtomicU32::new(0),
            resample_quality: AtomicU8::new(crate::dsp::resample::Quality::Balanced.to_u8()),
            crossfade_ms: AtomicU32::new(0),
            amp: std::sync::Mutex::new(crate::dsp::amp::AmpParams::default()),
            amp_gen: AtomicU64::new(0),
            channel: std::sync::Mutex::new(crate::dsp::channel::ChannelParams::default()),
            channel_gen: AtomicU64::new(0),
        }
    }
}

pub type SharedRef = Arc<Shared>;

impl Shared {
    /// Whether anything in the chain would change the sound, which keeps
    /// Auto bit-perfect off. A fixed output rate and crossfade count.
    pub fn dsp_active(&self) -> bool {
        self.level.mode() != crate::level::LevelMode::Off
            || self.out_rate.load(Ordering::Relaxed) != 0
            || self.crossfade_ms.load(Ordering::Relaxed) != 0
            || self.eq.lock().unwrap().changes_sound()
            || self.amp.lock().unwrap().changes_sound()
            || self.channel.lock().unwrap().changes_sound()
    }

    /// New channel tool settings for the renderer to pick up.
    pub fn set_channel(&self, p: crate::dsp::channel::ChannelParams) {
        *self.channel.lock().unwrap() = p;
        self.channel_gen.fetch_add(1, Ordering::Release);
    }

    /// A new amp stack for the renderer to pick up.
    pub fn set_amp(&self, p: crate::dsp::amp::AmpParams) {
        *self.amp.lock().unwrap() = p;
        self.amp_gen.fetch_add(1, Ordering::Release);
    }

    /// New EQ settings for the renderer to pick up.
    pub fn set_eq(&self, p: crate::dsp::EqParams) {
        *self.eq.lock().unwrap() = p;
        self.eq_gen.fetch_add(1, Ordering::Release);
    }
}

/// Volume slider position (0.0 to 1.0) to linear gain. Cubic, which tracks
/// perceived loudness closely enough and is what PulseAudio uses too.
pub fn volume_to_gain(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    v * v * v
}

/// Linear gain to dB, for display.
pub fn gain_to_db(g: f64) -> f64 {
    if g <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * g.log10()
    }
}
