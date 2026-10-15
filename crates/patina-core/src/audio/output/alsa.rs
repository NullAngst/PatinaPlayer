//! ALSA output through the shared mixer path ("default", or whichever
//! device you name). This is the fallback when PipeWire isn't running.
//! The exclusive hw path for bit-perfect playback is its own module and
//! lands in milestone 2.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};

use super::{Output, OutputConfig, OutputError, to_f32, to_int};
use crate::audio::render::Renderer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleFmt {
    F32,
    S32,
    S16,
}

pub struct AlsaOutput {
    device: String,
    fmt: SampleFmt,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

fn err(e: alsa::Error) -> OutputError {
    OutputError::Alsa(e.to_string())
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
            // Float first, since that skips a conversion; the plug layer on
            // "default" accepts it nearly everywhere.
            let fmt = if hwp.set_format(Format::float()).is_ok() {
                SampleFmt::F32
            } else if hwp.set_format(Format::s32()).is_ok() {
                SampleFmt::S32
            } else {
                hwp.set_format(Format::s16()).map_err(err)?;
                SampleFmt::S16
            };
            let buffer_us = cfg.buffer_ms.clamp(50, 1000) * 1000;
            hwp.set_buffer_time_near(buffer_us, ValueOr::Nearest)
                .map_err(err)?;
            hwp.set_period_time_near(buffer_us / 4, ValueOr::Nearest)
                .map_err(err)?;
            pcm.hw_params(&hwp).map_err(err)?;
            let rate = hwp.get_rate().map_err(err)?;
            if rate != sf.rate {
                return Err(OutputError::Alsa(format!(
                    "{device} can't play {} Hz (got {rate} Hz), and resampling isn't in yet",
                    sf.rate
                )));
            }
            let period = hwp.get_period_size().map_err(err)?.max(64) as usize;
            (fmt, period)
        };

        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (stop2, error2) = (stop.clone(), error.clone());
        let ch = sf.channels as usize;

        let thread = std::thread::Builder::new()
            .name("patina-alsa-out".into())
            .spawn(move || {
                let mut f32buf = vec![0f32; period * ch];
                let mut i32buf = vec![0i32; period * ch];
                let mut i16buf = vec![0i16; period * ch];
                while !stop2.load(Ordering::Relaxed) {
                    let block = renderer.render(period);
                    let res = match fmt {
                        SampleFmt::F32 => {
                            for (d, s) in f32buf.iter_mut().zip(block) {
                                *d = to_f32(*s);
                            }
                            pcm.io_f32()
                                .and_then(|io| io.writei(&f32buf[..block.len()]))
                        }
                        SampleFmt::S32 => {
                            for (d, s) in i32buf.iter_mut().zip(block) {
                                *d = to_int(*s, 32) as i32;
                            }
                            pcm.io_i32()
                                .and_then(|io| io.writei(&i32buf[..block.len()]))
                        }
                        SampleFmt::S16 => {
                            // TPDF dither arrives with the dither stage; at
                            // 16-bit output the plain rounding is the same
                            // thing every basic player does.
                            for (d, s) in i16buf.iter_mut().zip(block) {
                                *d = to_int(*s, 16) as i16;
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
            stop,
            error,
            thread: Some(thread),
        })
    }
}

impl Output for AlsaOutput {
    fn describe(&self) -> String {
        let f = match self.fmt {
            SampleFmt::F32 => "f32",
            SampleFmt::S32 => "s32",
            SampleFmt::S16 => "s16",
        };
        format!("ALSA {} {f}", self.device)
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
