//! Output backends. Each one owns a thread that calls the renderer and
//! pushes the result to a device. Dropping the handle stops the thread.

pub mod alsa;
pub mod null;
#[cfg(feature = "pipewire")]
pub mod pipewire;

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

#[derive(Debug, Clone, PartialEq)]
pub struct OutputConfig {
    pub backend: Backend,
    /// ALSA device name ("default", "hw:1,0") or PipeWire target node name.
    /// None picks the system default.
    pub device: Option<String>,
    /// Output latency asked of the device, 50 to 1000 ms.
    pub buffer_ms: u32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            backend: Backend::Auto,
            device: None,
            buffer_ms: 200,
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
}

/// A running output. Stops when dropped.
pub trait Output: Send {
    /// Short name for the status line: "PipeWire", "ALSA default".
    fn describe(&self) -> String;
    /// Set if the device failed after opening.
    fn error(&self) -> Option<String>;
}

/// Open an output for this renderer's format.
pub fn open(cfg: &OutputConfig, renderer: Renderer) -> Result<Box<dyn Output>, OutputError> {
    match cfg.backend {
        Backend::Null => Ok(Box::new(null::NullOutput::start(renderer, None))),
        Backend::Alsa => Ok(Box::new(alsa::AlsaOutput::start(cfg, renderer)?)),
        #[cfg(feature = "pipewire")]
        Backend::PipeWire => Ok(Box::new(pipewire::PwOutput::start(cfg, renderer)?)),
        #[cfg(not(feature = "pipewire"))]
        Backend::PipeWire => Err(OutputError::NoPipeWire),
        Backend::Auto => {
            #[cfg(feature = "pipewire")]
            {
                // PipeWire hands the renderer back if it can't connect, so
                // the ring survives the fallback.
                match pipewire::PwOutput::try_start(cfg, renderer) {
                    Ok(o) => Ok(Box::new(o)),
                    Err((e, renderer)) => {
                        log::info!("{e}, falling back to ALSA");
                        let mut alsa_cfg = cfg.clone();
                        alsa_cfg.device = None;
                        Ok(Box::new(alsa::AlsaOutput::start(&alsa_cfg, renderer)?))
                    }
                }
            }
            #[cfg(not(feature = "pipewire"))]
            Ok(Box::new(alsa::AlsaOutput::start(cfg, renderer)?))
        }
    }
}

/// f64 to f32 sample. Plain cast: f32 has 24 bits of mantissa, enough for
/// any source. Dither for integer outputs belongs to the backend that
/// knows the target depth.
#[inline]
pub(crate) fn to_f32(s: f64) -> f32 {
    s as f32
}

/// f64 in [-1, 1] to a full-scale signed integer of `bits` width, rounded.
/// The inverse of the decoder's scale, so 16 and 24-bit PCM round-trip
/// exactly when nothing touched the samples.
#[inline]
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
    fn int_roundtrip_24_in_32() {
        // 24-bit PCM is decoded by FFmpeg as s32 with the low byte zero.
        for v in [-8388608i32, -1, 0, 1, 8388607] {
            let s32 = v << 8;
            let f = s32 as f64 / 2147483648.0;
            assert_eq!(to_int(f, 32), s32 as i64);
        }
    }
}
