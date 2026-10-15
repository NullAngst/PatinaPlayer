//! ALSA output through the shared mixer path ("default", or whichever
//! device you name). This is the fallback when PipeWire isn't running.
//! The exclusive hw path for bit-perfect playback is its own module.
//!
//! The sample format is whichever you picked, or for Auto the best the
//! device takes: float, then 32, 24 and 16-bit. Integer formats below 32
//! bits get TPDF dither unless you turned it off.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};

use super::{Dither, Output, OutputConfig, OutputError, SampleFormat, to_f32, to_int};
use crate::audio::render::Renderer;

pub struct AlsaOutput {
    device: String,
    fmt: SampleFormat,
    dither: bool,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

fn err(e: alsa::Error) -> OutputError {
    OutputError::Alsa(e.to_string())
}

fn alsa_format(f: SampleFormat) -> Format {
    match f {
        SampleFormat::F32 | SampleFormat::Auto => Format::float(),
        SampleFormat::S32 => Format::s32(),
        SampleFormat::S24 => Format::s24(),
        SampleFormat::S16 => Format::s16(),
    }
}

impl AlsaOutput {
    pub fn start(cfg: &OutputConfig, mut renderer: Renderer) -> Result<Self, OutputError> {
        let device = cfg.device.clone().unwrap_or_else(|| "default".into());
        let sf = renderer.format();
        let pcm = PCM::new(&device, Direction::Playback, false).map_err(err)?;

        let (fmt, period) = {
            let hwp = HwParams::any(&pcm).map_err(err)?;
            hwp.set_access(Access::RWInterleaved).map_err(err)?;
            hwp.set_channels(sf.channels as u32).map_err(err)?;
            hwp.set_rate(sf.rate, ValueOr::Nearest).map_err(err)?;
            let fmt = match cfg.format {
                // Float first, since that skips a conversion; the plug
                // layer on "default" accepts it nearly everywhere.
                SampleFormat::Auto => [
                    SampleFormat::F32,
                    SampleFormat::S32,
                    SampleFormat::S24,
                    SampleFormat::S16,
                ]
                .into_iter()
                .find(|f| hwp.test_format(alsa_format(*f)).is_ok())
                .ok_or_else(|| OutputError::Alsa(format!("{device} takes no format we write")))?,
                f => f,
            };
            hwp.set_format(alsa_format(fmt)).map_err(|e| {
                OutputError::Alsa(format!("{device} can't take {}: {e}", fmt.label()))
            })?;
            let buffer_us = cfg.buffer_ms.clamp(50, 1000) * 1000;
            hwp.set_buffer_time_near(buffer_us, ValueOr::Nearest)
                .map_err(err)?;
            hwp.set_period_time_near(buffer_us / 4, ValueOr::Nearest)
                .map_err(err)?;
            pcm.hw_params(&hwp).map_err(err)?;
            let rate = hwp.get_rate().map_err(err)?;
            if rate != sf.rate {
                // The core resamples to the rate on offer and tries again.
                return Err(OutputError::Rate {
                    device,
                    want: sf.rate,
                    got: rate,
                });
            }
            let period = hwp.get_period_size().map_err(err)?.max(64) as usize;
            (fmt, period)
        };

        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (stop2, error2) = (stop.clone(), error.clone());
        let ch = sf.channels as usize;
        let dither = cfg.dither && matches!(fmt, SampleFormat::S16 | SampleFormat::S24);

        let thread = std::thread::Builder::new()
            .name("patina-alsa-out".into())
            .spawn(move || {
                let mut f32buf = vec![0f32; period * ch];
                let mut i32buf = vec![0i32; period * ch];
                let mut i16buf = vec![0i16; period * ch];
                let mut d = Dither::default();
                while !stop2.load(Ordering::Relaxed) {
                    let block = renderer.render(period);
                    let res = match fmt {
                        SampleFormat::F32 | SampleFormat::Auto => {
                            for (o, s) in f32buf.iter_mut().zip(block) {
                                *o = to_f32(*s);
                            }
                            pcm.io_f32()
                                .and_then(|io| io.writei(&f32buf[..block.len()]))
                        }
                        SampleFormat::S32 | SampleFormat::S24 => {
                            let bits = fmt.int_bits().unwrap();
                            for (o, s) in i32buf.iter_mut().zip(block) {
                                *o = if dither {
                                    d.to_int(*s, bits)
                                } else {
                                    to_int(*s, bits)
                                } as i32;
                            }
                            pcm.io_i32()
                                .and_then(|io| io.writei(&i32buf[..block.len()]))
                        }
                        SampleFormat::S16 => {
                            for (o, s) in i16buf.iter_mut().zip(block) {
                                *o = if dither {
                                    d.to_int(*s, 16)
                                } else {
                                    to_int(*s, 16)
                                } as i16;
                            }
                            pcm.io_i16()
                                .and_then(|io| io.writei(&i16buf[..block.len()]))
                        }
                    };
                    if let Err(e) = res {
                        // Underruns land here as EPIPE; recover and carry on.
                        if let Err(e2) = pcm.try_recover(e, true) {
                            *error2.lock().unwrap() = Some(e2.to_string());
                            log::error!("ALSA write failed: {e2}");
                            break;
                        }
                    }
                }
                let _ = pcm.drop();
            })
            .map_err(|e| OutputError::Alsa(e.to_string()))?;

        Ok(Self {
            device,
            fmt,
            dither,
            stop,
            error,
            thread: Some(thread),
        })
    }
}

impl Output for AlsaOutput {
    fn describe(&self) -> String {
        let d = if self.dither { " dithered" } else { "" };
        format!("ALSA {} {}{d}", self.device, self.fmt.as_str())
    }
    fn device(&self) -> Option<crate::eq::OutputDevice> {
        Some(crate::eq::OutputDevice {
            key: format!("alsa:{}", self.device),
            name: format!("ALSA {}", self.device),
        })
    }
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for AlsaOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
