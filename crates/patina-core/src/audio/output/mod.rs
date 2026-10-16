//! Output backends. Each one owns a thread that calls the renderer and
//! pushes the result to a device. Dropping the handle stops the thread.

#[cfg(target_os = "android")]
pub mod aaudio;
#[cfg(feature = "alsa")]
pub mod alsa;
pub mod exclusive;
#[cfg(feature = "alsa")]
mod exclusive_alsa;
pub mod null;
#[cfg(feature = "pipewire")]
pub mod pipewire;
pub mod reserve;

use super::render::Renderer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// PipeWire, falling back to ALSA if no PipeWire daemon answers.
    #[default]
    Auto,
    PipeWire,
    Alsa,
    /// No device. Renders in real time and throws the audio away.
    Null,
}

impl Backend {
    /// As written in config.toml and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Auto => "auto",
            Backend::PipeWire => "pipewire",
            Backend::Alsa => "alsa",
            Backend::Null => "null",
        }
    }
}

impl std::str::FromStr for Backend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "pipewire" | "pw" => Ok(Self::PipeWire),
            "alsa" => Ok(Self::Alsa),
            "null" | "none" => Ok(Self::Null),
            _ => Err(format!(
                "unknown backend '{s}' (auto, pipewire, alsa, null)"
            )),
        }
    }
}

/// When to use the exclusive, bit-perfect output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BitPerfect {
    /// Always the shared output. Other apps keep the sound card.
    #[default]
    Off,
    /// Local files go out bit-perfect. Radio and podcasts never do.
    On,
    /// Bit-perfect whenever nothing would change the sound (no EQ, no
    /// leveling). Until those exist, the same as On.
    Auto,
}

impl BitPerfect {
    pub fn as_str(self) -> &'static str {
        match self {
            BitPerfect::Off => "off",
            BitPerfect::On => "on",
            BitPerfect::Auto => "auto",
        }
    }

    /// Wanted for this kind of playback, given whether any DSP is active.
    pub fn wanted(self, dsp_active: bool) -> bool {
        match self {
            BitPerfect::Off => false,
            BitPerfect::On => true,
            BitPerfect::Auto => !dsp_active,
        }
    }

    pub(crate) fn to_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            1 => BitPerfect::On,
            2 => BitPerfect::Auto,
            _ => BitPerfect::Off,
        }
    }
}

impl std::str::FromStr for BitPerfect {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "false" => Ok(Self::Off),
            "on" | "true" => Ok(Self::On),
            "auto" => Ok(Self::Auto),
            _ => Err(format!("unknown bit_perfect '{s}' (off, on, auto)")),
        }
    }
}

/// Sample format asked of the shared output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SampleFormat {
    /// Whatever the device takes best: float, then 32, 24, 16-bit.
    #[default]
    Auto,
    F32,
    S32,
    S24,
    S16,
}

impl SampleFormat {
    pub const ALL: [SampleFormat; 5] = [
        SampleFormat::Auto,
        SampleFormat::F32,
        SampleFormat::S32,
        SampleFormat::S24,
        SampleFormat::S16,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SampleFormat::Auto => "auto",
            SampleFormat::F32 => "f32",
            SampleFormat::S32 => "s32",
            SampleFormat::S24 => "s24",
            SampleFormat::S16 => "s16",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SampleFormat::Auto => "Auto",
            SampleFormat::F32 => "32-bit float",
            SampleFormat::S32 => "32-bit",
            SampleFormat::S24 => "24-bit",
            SampleFormat::S16 => "16-bit",
        }
    }

    /// Integer bits, None for float.
    pub fn int_bits(self) -> Option<u32> {
        match self {
            SampleFormat::S32 => Some(32),
            SampleFormat::S24 => Some(24),
            SampleFormat::S16 => Some(16),
            _ => None,
        }
    }
}

impl std::str::FromStr for SampleFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_lowercase();
        let t = match t.as_str() {
            "float" | "f32le" => "f32",
            "32" => "s32",
            "24" => "s24",
            "16" => "s16",
            x => x,
        };
        SampleFormat::ALL
            .into_iter()
            .find(|f| f.as_str() == t)
            .ok_or_else(|| format!("unknown format '{s}' (auto, f32, s32, s24, s16)"))
    }
}

/// Output rates offered in Settings. Any rate works in the config file.
pub const RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

#[derive(Debug, Clone, PartialEq)]
pub struct OutputConfig {
    pub backend: Backend,
    /// ALSA device name ("default", "hw:1,0") or PipeWire target node name.
    /// None picks the system default.
    pub device: Option<String>,
    /// Output latency asked of the device, 50 to 1000 ms.
    pub buffer_ms: u32,
    pub bit_perfect: BitPerfect,
    /// ALSA hardware device for bit-perfect playback (`hw:CARD=DAC,DEV=0`).
    /// None takes the first sound card.
    pub bit_perfect_device: Option<String>,
    /// Output sample rate. 0 follows the source.
    pub rate: u32,
    pub resampler: crate::dsp::resample::Quality,
    pub format: SampleFormat,
    /// TPDF dither on integer outputs below 32 bits.
    pub dither: bool,
    /// Crossfade between songs, 0 to 12000 ms.
    pub crossfade_ms: u32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            backend: Backend::Auto,
            device: None,
            buffer_ms: 200,
            bit_perfect: BitPerfect::Off,
            bit_perfect_device: None,
            rate: 0,
            resampler: crate::dsp::resample::Quality::Balanced,
            format: SampleFormat::Auto,
            dither: true,
            crossfade_ms: 0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("PipeWire: {0}")]
    PipeWire(String),
    #[error("ALSA: {0}")]
    Alsa(String),
    #[error("PipeWire support was not compiled in")]
    NoPipeWire,
    #[error("ALSA support was not compiled in")]
    NoAlsa,
    #[error("{0}")]
    AAudio(String),
    /// The device can't play this rate. It offered `got`.
    #[error("{device} can't play {want} Hz (it offered {got} Hz)")]
    Rate { device: String, want: u32, got: u32 },
}

/// A running output. Stops when dropped.
pub trait Output: Send {
    /// Short name for the status line: "PipeWire", "ALSA default".
    fn describe(&self) -> String;
    /// Set if the device failed after opening.
    fn error(&self) -> Option<String>;
    /// The bit-perfect output says what it's doing; shared outputs don't.
    fn exclusive(&self) -> Option<exclusive::ExclusiveInfo> {
        None
    }
    /// Set the device's own volume, slider 0 to 1. False if it has none
    /// and volume is the renderer's job.
    fn set_hw_volume(&self, _slider: f64) -> bool {
        false
    }
    /// The device the sound is going to, for per-device EQ. None while
    /// that isn't known yet.
    fn device(&self) -> Option<crate::eq::OutputDevice> {
        None
    }
}

/// A device the shared output can play to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutDevice {
    /// What goes in `output.device`: a PipeWire node name or an ALSA PCM.
    pub id: String,
    pub label: String,
}

/// The devices this backend can play to. For Auto, PipeWire's sinks if
/// the daemon answers, else ALSA's. Never includes the system default,
/// which is no device at all.
pub fn list_outputs(backend: Backend) -> Vec<OutDevice> {
    #[cfg(feature = "pipewire")]
    if matches!(backend, Backend::Auto | Backend::PipeWire) {
        match pipewire::list_sinks() {
            Ok(v) => {
                return v
                    .into_iter()
                    .map(|(id, label)| OutDevice { id, label })
                    .collect();
            }
            Err(e) if backend == Backend::PipeWire => {
                log::info!("can't list PipeWire sinks: {e}");
                return Vec::new();
            }
            Err(_) => {}
        }
    }
    #[cfg(target_os = "android")]
    if backend == Backend::Auto {
        return aaudio::list_outputs();
    }
    match backend {
        Backend::Null => Vec::new(),
        #[cfg(feature = "pipewire")]
        Backend::PipeWire => Vec::new(),
        #[cfg(feature = "alsa")]
        _ => alsa_outputs(),
        #[cfg(not(feature = "alsa"))]
        _ => Vec::new(),
    }
}

/// ALSA's playback PCMs, as `aplay -L` lists them, minus the ones that
/// are for surround layouts or digital passthrough.
#[cfg(feature = "alsa")]
fn alsa_outputs() -> Vec<OutDevice> {
    let Ok(hints) = ::alsa::device_name::HintIter::new_str(None, "pcm") else {
        return Vec::new();
    };
    hints
        .filter(|h| h.direction != Some(::alsa::Direction::Capture))
        .filter_map(|h| {
            let id = h.name?;
            // "default" is the system default, offered on its own.
            if id == "null" || id == "default" {
                return None;
            }
            if ["surround", "iec958", "spdif", "dsnoop"]
                .iter()
                .any(|p| id.starts_with(p))
            {
                return None;
            }
            let label = h
                .desc
                .map(|d| d.lines().map(str::trim).collect::<Vec<_>>().join(", "))
                .filter(|d| !d.is_empty())
                .map(|d| format!("{d} ({id})"))
                .unwrap_or_else(|| id.clone());
            Some(OutDevice { id, label })
        })
        .collect()
}

/// Open an output for this renderer's format.
pub fn open(cfg: &OutputConfig, renderer: Renderer) -> Result<Box<dyn Output>, OutputError> {
    match cfg.backend {
        Backend::Null => Ok(Box::new(null::NullOutput::start(renderer, None))),
        // Android has one way out: AAudio.
        #[cfg(target_os = "android")]
        Backend::Auto => Ok(Box::new(aaudio::AAudioOutput::start(cfg, renderer)?)),
        #[cfg(feature = "alsa")]
        Backend::Alsa => Ok(Box::new(alsa::AlsaOutput::start(cfg, renderer)?)),
        #[cfg(not(feature = "alsa"))]
        Backend::Alsa => Err(OutputError::NoAlsa),
        #[cfg(feature = "pipewire")]
        Backend::PipeWire => Ok(Box::new(pipewire::PwOutput::start(cfg, renderer)?)),
        #[cfg(not(feature = "pipewire"))]
        Backend::PipeWire => Err(OutputError::NoPipeWire),
        #[cfg(not(target_os = "android"))]
        Backend::Auto => {
            #[cfg(feature = "pipewire")]
            {
                // PipeWire hands the renderer back if it can't connect, so
                // the ring survives the fallback.
                match pipewire::PwOutput::try_start(cfg, renderer) {
                    Ok(o) => Ok(Box::new(o)),
                    #[cfg(feature = "alsa")]
                    Err((e, renderer)) => {
                        log::info!("{e}, falling back to ALSA");
                        let mut alsa_cfg = cfg.clone();
                        alsa_cfg.device = None;
                        Ok(Box::new(alsa::AlsaOutput::start(&alsa_cfg, renderer)?))
                    }
                    #[cfg(not(feature = "alsa"))]
                    Err((e, _)) => Err(e),
                }
            }
            #[cfg(all(not(feature = "pipewire"), feature = "alsa"))]
            {
                Ok(Box::new(alsa::AlsaOutput::start(cfg, renderer)?))
            }
            #[cfg(all(not(feature = "pipewire"), not(feature = "alsa")))]
            {
                let _ = renderer;
                Err(OutputError::NoAlsa)
            }
        }
    }
}

/// f64 to f32 sample. Plain cast: f32 has 24 bits of mantissa, enough for
/// any source. Dither for integer outputs belongs to the backend that
/// knows the target depth.
#[inline]
#[cfg_attr(not(any(feature = "alsa", target_os = "android")), allow(dead_code))]
pub(crate) fn to_f32(s: f64) -> f32 {
    s as f32
}

/// TPDF dither: the sum of two uniform random values, one quantization
/// step wide each, added before rounding. It turns the rounding error
/// into steady, signal-independent hiss instead of distortion that
/// follows the music. A sample already on the integer grid (untouched
/// 16-bit audio going out at 16 bits) passes through exact, since it has
/// no rounding error to hide.
pub struct Dither {
    state: u64,
}

impl Default for Dither {
    fn default() -> Self {
        Self {
            state: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl Dither {
    /// xorshift64*: fast, and plenty random for noise.
    #[inline]
    fn uniform(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Like [`to_int`], dithered.
    #[inline]
    pub fn to_int(&mut self, s: f64, bits: u32) -> i64 {
        let scale = (1i64 << (bits - 1)) as f64;
        let v = s * scale;
        let v = if v == v.round() {
            v
        } else {
            (v + self.uniform() - self.uniform()).round()
        };
        v.clamp(-scale, scale - 1.0) as i64
    }
}

/// f64 in [-1, 1] to a full-scale signed integer of `bits` width, rounded.
/// The inverse of the decoder's scale, so 16 and 24-bit PCM round-trip
/// exactly when nothing touched the samples.
#[inline]
#[cfg_attr(not(any(feature = "alsa", target_os = "android")), allow(dead_code))]
pub(crate) fn to_int(s: f64, bits: u32) -> i64 {
    let scale = (1i64 << (bits - 1)) as f64;
    let v = (s * scale).round();
    v.clamp(-scale, scale - 1.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_roundtrip_16() {
        for v in [-32768i16, -1, 0, 1, 12345, 32767] {
            let f = v as f64 / 32768.0;
            assert_eq!(to_int(f, 16), v as i64);
        }
    }

    #[test]
    fn dither_is_tpdf_and_leaves_exact_samples_alone() {
        let mut d = Dither::default();
        // On the grid: exact.
        for v in [-32768i16, -1, 0, 1, 12345, 32767] {
            assert_eq!(d.to_int(v as f64 / 32768.0, 16), v as i64);
        }
        // A quarter step off the grid: dithered values spread over three
        // steps, average to the true value, and the error has the
        // variance TPDF gives: 1/6 of a step squared from the dither,
        // plus 1/12 from rounding, 1/4 in all, whatever the signal.
        let x = 100.25 / 32768.0;
        let n = 200_000;
        let mut sum = 0.0;
        let mut sq = 0.0;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..n {
            let q = d.to_int(x, 16) as f64;
            seen.insert(q as i64);
            sum += q;
            sq += (q - 100.25).powi(2);
        }
        let mean = sum / n as f64;
        assert!((mean - 100.25).abs() < 0.01, "mean {mean}");
        let var = sq / n as f64;
        assert!((var - 0.25).abs() < 0.01, "variance {var}");
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![99, 100, 101]);
    }

    #[test]
    fn dither_turns_a_quiet_tone_into_tone_plus_hiss() {
        // A sine a third of a step tall: plain rounding flattens it to
        // zero; dither keeps it, buried in noise but there on average.
        let mut d = Dither::default();
        let amp = 0.33 / 32768.0;
        let n = 48000 * 4;
        let (mut plain, mut dith) = (0.0, 0.0);
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48000.0).sin();
            plain += to_int(amp * s, 16) as f64 * s;
            dith += d.to_int(amp * s, 16) as f64 * s;
        }
        assert_eq!(plain, 0.0);
        // Correlation with the tone: about amp/2 per sample.
        let c = dith / n as f64;
        assert!((c - 0.165).abs() < 0.02, "{c}");
    }

    #[test]
    fn int_roundtrip_24_in_32() {
        // 24-bit PCM is decoded by FFmpeg as s32 with the low byte zero.
        for v in [-8388608i32, -1, 0, 1, 8388607] {
            let s32 = v << 8;
            let f = s32 as f64 / 2147483648.0;
            assert_eq!(to_int(f, 32), s32 as i64);
        }
    }
}
