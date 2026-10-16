//! Bit-perfect output on Linux: an ALSA hardware device, opened
//! exclusively. No plug layer, so no hidden resampling or conversion. See
//! `exclusive` for what bit-perfect promises.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alsa::mixer::{Mixer, Selem, SelemId};
use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, Round, ValueOr};

use super::exclusive::{ExclusiveInfo, HwDevice, Refused, Sample, candidates, describe, encode};
use super::reserve::{self, Reservation};
use super::{Output, OutputConfig};
use crate::audio::render::Renderer;
use crate::audio::{Depth, SharedRef};

fn alsa_format(s: Sample) -> Format {
    match s {
        Sample::S16 => Format::s16(),
        Sample::S24In32 => Format::s24(),
        Sample::S24Packed => Format::s24_3(),
        Sample::S32 => Format::s32(),
        Sample::F32 => Format::float(),
    }
}

/// Every hardware playback device ALSA knows about.
pub fn list_devices() -> Vec<HwDevice> {
    let mut out = Vec::new();
    for card in alsa::card::Iter::new().flatten() {
        let Ok(ctl) = alsa::Ctl::from_card(&card, false) else {
            continue;
        };
        let Ok(info) = ctl.card_info() else { continue };
        let (Ok(id), Ok(card_name)) = (info.get_id(), info.get_name()) else {
            continue;
        };
        let (id, card_name) = (id.to_string(), card_name.to_string());
        for dev in alsa::ctl::DeviceIter::new(&ctl) {
            let Ok(pi) = ctl.pcm_info(dev as u32, 0, Direction::Playback) else {
                continue;
            };
            let pcm_name = pi.get_name().unwrap_or("").to_string();
            out.push(HwDevice {
                id: format!("hw:CARD={id},DEV={dev}"),
                name: if pcm_name.is_empty() {
                    card_name.clone()
                } else {
                    format!("{card_name}: {pcm_name}")
                },
            });
        }
    }
    out
}

/// The card number behind `hw:CARD=X,DEV=n`, `hw:1,0` or `hw:X`.
fn card_index(device: &str) -> Option<i32> {
    let rest = device
        .strip_prefix("hw:")
        .or_else(|| device.strip_prefix("plughw:"))?;
    let first = rest.split(',').next()?;
    let card = first.strip_prefix("CARD=").unwrap_or(first);
    if let Ok(n) = card.parse::<i32>() {
        return Some(n);
    }
    let c = std::ffi::CString::new(card).ok()?;
    alsa::card::Card::from_str(&c).ok().map(|c| c.get_index())
}

/// The DAC's own volume control, if it has one worth using.
struct HwVolume {
    mixer: Mixer,
    id: SelemId,
}

impl HwVolume {
    fn open(card: i32) -> Option<Self> {
        let mixer = Mixer::new(&format!("hw:{card}"), false).ok()?;
        // Prefer the controls DACs actually wire to their output.
        let preferred = ["Master", "PCM", "Digital", "Headphone", "Speaker"];
        let mut found: Option<SelemId> = None;
        for elem in mixer.iter() {
            let Some(se) = Selem::new(elem) else { continue };
            if !se.has_playback_volume() {
                continue;
            }
            let id = se.get_id();
            let name = id.get_name().unwrap_or("").to_string();
            let better = match &found {
                None => true,
                Some(f) => {
                    let fname = f.get_name().unwrap_or("");
                    let rank = |n: &str| preferred.iter().position(|p| *p == n).unwrap_or(99);
                    rank(&name) < rank(fname)
                }
            };
            if better {
                found = Some(SelemId::new(&name, id.get_index()));
            }
        }
        found.map(|id| Self { mixer, id })
    }

    /// Slider position 0 to 1. dB where the control knows its dB scale,
    /// following the same cubic curve as the software volume.
    fn set(&self, slider: f64) {
        let Some(se) = self.mixer.find_selem(&self.id) else {
            return;
        };
        let gain = crate::audio::volume_to_gain(slider);
        let (min_db, max_db) = se.get_playback_db_range();
        if max_db.0 > min_db.0 {
            let db = if gain <= 0.0 {
                min_db.0
            } else {
                ((20.0 * gain.log10()) * 100.0).round() as i64
            };
            let target = alsa::mixer::MilliBel(db.clamp(min_db.0, max_db.0));
            if se.set_playback_db_all(target, Round::Floor).is_ok() {
                return;
            }
        }
        let (lo, hi) = se.get_playback_volume_range();
        let v = lo + ((hi - lo) as f64 * slider.clamp(0.0, 1.0)).round() as i64;
        let _ = se.set_playback_volume_all(v);
    }
}

pub struct ExclusiveOutput {
    info: ExclusiveInfo,
    volume: Option<Mutex<HwVolume>>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
    /// Dropped after the thread, so the card is only handed back once we've
    /// closed it.
    _reservation: Option<Reservation>,
}

/// How long to keep retrying a busy device after asking for it. PipeWire
/// closes a card a moment after it agrees to release it.
const BUSY_WAIT: Duration = Duration::from_secs(3);

impl ExclusiveOutput {
    /// Open the bit-perfect device for this renderer's format. On failure
    /// the renderer comes back with the reason, so the player can fall back
    /// to the normal output without losing what's in the ring.
    pub fn try_start(
        cfg: &OutputConfig,
        depth: Depth,
        renderer: Renderer,
        shared: SharedRef,
    ) -> Result<Self, Refused> {
        let device = match cfg.bit_perfect_device.clone() {
            Some(d) => d,
            None => match list_devices().into_iter().next() {
                Some(d) => d.id,
                None => return Err(Box::new(("no sound card found".into(), renderer))),
            },
        };
        let card = card_index(&device);
        let reservation = match card {
            Some(c) => match reserve::reserve(c, &device) {
                Ok(r) => r,
                Err(e) => return Err(Box::new((e, renderer))),
            },
            None => None,
        };
        match Self::open(&device, depth, cfg.buffer_ms, renderer, shared) {
            Ok(mut out) => {
                out._reservation = reservation;
                out.volume = card.and_then(HwVolume::open).map(Mutex::new);
                out.info.hw_volume = out.volume.is_some();
                Ok(out)
            }
            Err(e) => Err(e),
        }
    }

    fn open(
        device: &str,
        depth: Depth,
        buffer_ms: u32,
        mut renderer: Renderer,
        shared: SharedRef,
    ) -> Result<Self, Refused> {
        let sf = renderer.format();
        let want = describe(depth, sf.rate);

        // Busy right after a release is normal; give it a moment.
        let deadline = Instant::now() + BUSY_WAIT;
        let pcm = loop {
            match PCM::new(device, Direction::Playback, false) {
                Ok(p) => break p,
                Err(e) if e.errno() == libc_ebusy() && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) if e.errno() == libc_ebusy() => {
                    return Err(Box::new((
                        format!("{device} is busy: another app has it open"),
                        renderer,
                    )));
                }
                Err(e) => return Err(Box::new((format!("can't open {device}: {e}"), renderer))),
            }
        };

        let setup = (|| -> Result<(Sample, bool, usize), String> {
            let hwp = HwParams::any(&pcm).map_err(|e| e.to_string())?;
            hwp.set_access(Access::RWInterleaved)
                .map_err(|e| e.to_string())?;
            // Plug devices would resample behind our back. Refuse to.
            let _ = hwp.set_rate_resample(false);
            if hwp.set_channels(sf.channels as u32).is_err() {
                return Err(format!(
                    "the DAC doesn't take {} channel{}",
                    sf.channels,
                    if sf.channels == 1 { "" } else { "s" }
                ));
            }
            if hwp.test_rate(sf.rate).is_err() {
                return Err(format!("the DAC doesn't support {want}"));
            }
            hwp.set_rate(sf.rate, ValueOr::Nearest)
                .map_err(|e| e.to_string())?;
            let (exact_list, close_list) = candidates(depth);
            let mut chosen = None;
            for (list, exact) in [(exact_list, true), (close_list, false)] {
                for s in list {
                    if hwp.test_format(alsa_format(s)).is_ok() {
                        chosen = Some((s, exact));
                        break;
                    }
                }
                if chosen.is_some() {
                    break;
                }
            }
            let Some((sample, exact)) = chosen else {
                return Err(format!("the DAC doesn't support {want}"));
            };
            hwp.set_format(alsa_format(sample))
                .map_err(|e| e.to_string())?;
            let buffer_us = buffer_ms.clamp(50, 1000) * 1000;
            hwp.set_buffer_time_near(buffer_us, ValueOr::Nearest)
                .map_err(|e| e.to_string())?;
            hwp.set_period_time_near(buffer_us / 4, ValueOr::Nearest)
                .map_err(|e| e.to_string())?;
            pcm.hw_params(&hwp).map_err(|e| e.to_string())?;
            if hwp.get_rate().map_err(|e| e.to_string())? != sf.rate {
                return Err(format!("the DAC doesn't support {want}"));
            }
            let period = hwp.get_period_size().map_err(|e| e.to_string())?.max(64) as usize;
            let buffer = hwp.get_buffer_size().map_err(|e| e.to_string())?;
            // Start once two periods are queued, so a fresh start never
            // underruns straight away.
            let swp = pcm.sw_params_current().map_err(|e| e.to_string())?;
            swp.set_start_threshold((period as alsa::pcm::Frames * 2).min(buffer))
                .map_err(|e| e.to_string())?;
            pcm.sw_params(&swp).map_err(|e| e.to_string())?;
            Ok((sample, exact, period))
        })();
        let (sample, exact, period) = match setup {
            Ok(v) => v,
            Err(e) => return Err(Box::new((e, renderer))),
        };

        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (stop2, error2) = (stop.clone(), error.clone());
        let ch = sf.channels as usize;
        let shared2 = shared.clone();
        let thread = std::thread::Builder::new()
            .name("patina-exclusive".into())
            .spawn(move || {
                let mut bytes = vec![0u8; period * ch * sample.bytes()];
                // The device is started by the first writes; stopped (or
                // drained) whenever there's nothing to play.
                let mut running = false;
                while !stop2.load(Ordering::Relaxed) {
                    let block = renderer.render_exact(period);
                    if block.is_empty() {
                        if running {
                            // A seek or stop throws the queued audio away;
                            // a pause or the end lets it play out.
                            let _ = if renderer.last_was_flush() {
                                pcm.drop()
                            } else {
                                pcm.drain()
                            };
                            running = false;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    if !running {
                        let _ = pcm.prepare();
                        running = true;
                    }
                    let n = encode(block, sample, &mut bytes);
                    let res = pcm.io_bytes().writei(&bytes[..n]);
                    if let Err(e) = res
                        && let Err(e2) = pcm.try_recover(e, true)
                    {
                        *error2.lock().unwrap() = Some(e2.to_string());
                        log::error!("bit-perfect write failed: {e2}");
                        break;
                    }
                }
                let _ = if shared2.end_of_queue.load(Ordering::Acquire) {
                    pcm.drain()
                } else {
                    pcm.drop()
                };
            })
            .expect("spawn exclusive output thread");

        Ok(Self {
            info: ExclusiveInfo {
                device: device.to_string(),
                format: format!("{}", alsa_format(sample)),
                exact,
                hw_volume: false,
            },
            volume: None,
            stop,
            error,
            thread: Some(thread),
            _reservation: None,
        })
    }
}

fn libc_ebusy() -> i32 {
    16 // EBUSY on Linux
}

impl Output for ExclusiveOutput {
    fn describe(&self) -> String {
        format!("{} {}", self.info.device, self.info.format)
    }
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    fn exclusive(&self) -> Option<ExclusiveInfo> {
        Some(self.info.clone())
    }
    fn set_hw_volume(&self, slider: f64) -> bool {
        match &self.volume {
            Some(v) => {
                v.lock().unwrap().set(slider);
                true
            }
            None => false,
        }
    }
}

impl Drop for ExclusiveOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_names() {
        assert_eq!(card_index("hw:2,0"), Some(2));
        assert_eq!(card_index("hw:CARD=3,DEV=0"), Some(3));
        assert_eq!(card_index("file:'/tmp/x',raw"), None);
    }
}
