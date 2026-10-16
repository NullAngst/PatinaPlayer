//! Android output: AAudio, the NDK's audio API.
//!
//! The shared output is a float stream at the source's rate; Android's
//! mixer takes it from there. The bit-perfect output needs Android 14 or
//! later and a USB DAC: the app (on the Java side) asks the system to put
//! the DAC's mixer into bit-perfect mode at the song's rate and depth, and
//! this opens a stream at exactly that format, so the samples reach the DAC
//! untouched. Older Android, or a DAC the system won't do that for, gets a
//! refusal with the reason, and the shared output plays instead.
//!
//! What devices there are comes from the Java side through
//! [`set_devices`], since only Java's AudioManager can list them.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use ndk::audio::{
    AudioCallbackResult, AudioContentType, AudioDirection, AudioFormat, AudioPerformanceMode,
    AudioSharingMode, AudioStream, AudioStreamBuilder, AudioUsage,
};

use super::exclusive::{ExclusiveInfo, HwDevice, Refused, Sample, candidates, describe, encode};
use super::{OutDevice, Output, OutputConfig, OutputError};
use crate::audio::render::Renderer;
use crate::audio::{Depth, SharedRef};

/// An output device as Android's AudioManager lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidDevice {
    /// AudioDeviceInfo.getId(): what AAudio's device id takes.
    pub id: i32,
    /// "USB-Audio - Modi 3", "Speaker", "Headphones".
    pub name: String,
    /// A USB audio device, the only kind bit-perfect works with.
    pub usb: bool,
}

static DEVICES: Mutex<Vec<AndroidDevice>> = Mutex::new(Vec::new());

/// Asks the system to put a USB device's mixer into bit-perfect mode for
/// a format: (device id, rate, bits, channels). Ok once it's set; Err with
/// a reason a person can read. Bits 0 means float. A rate of 0 hands the
/// device back to the mixer (device -1: every device). Installed by the app.
pub type BitPerfectHook = Box<dyn Fn(i32, u32, u32, u16) -> Result<(), String> + Send>;

static HOOK: Mutex<Option<BitPerfectHook>> = Mutex::new(None);

/// The output devices there are now. The app calls this whenever Android
/// says one came or went.
pub fn set_devices(devices: Vec<AndroidDevice>) {
    *DEVICES.lock().unwrap() = devices;
}

pub fn set_bit_perfect_hook(hook: BitPerfectHook) {
    *HOOK.lock().unwrap() = Some(hook);
}

/// Devices for the Device setting. The ids are AAudio device ids.
pub fn list_outputs() -> Vec<OutDevice> {
    DEVICES
        .lock()
        .unwrap()
        .iter()
        .map(|d| OutDevice {
            id: d.id.to_string(),
            label: d.name.clone(),
        })
        .collect()
}

/// USB DACs, for the bit-perfect device setting: `usb:<id>`.
pub fn list_devices() -> Vec<HwDevice> {
    DEVICES
        .lock()
        .unwrap()
        .iter()
        .filter(|d| d.usb)
        .map(|d| HwDevice {
            id: format!("usb:{}", d.id),
            name: d.name.clone(),
        })
        .collect()
}

/// AAudio's stream handle, which the NDK allows calls on from any thread.
struct Stream(AudioStream);
// SAFETY: AAudio stream control calls (start, stop, close) are thread
// safe; the callbacks run on AAudio's own thread and only touch what they
// own.
unsafe impl Send for Stream {}

fn open_error(e: impl std::fmt::Display) -> OutputError {
    OutputError::AAudio(e.to_string())
}

/// The shared output.
pub struct AAudioOutput {
    stream: Stream,
    error: Arc<Mutex<Option<String>>>,
    label: String,
}

impl AAudioOutput {
    pub fn start(cfg: &OutputConfig, mut renderer: Renderer) -> Result<Self, OutputError> {
        // A USB DAC left in bit-perfect mode would play only this app, and
        // only at one format: hand it back to Android's mixer first.
        if let Some(hook) = HOOK.lock().unwrap().as_ref() {
            let _ = hook(-1, 0, 0, 0);
        }
        let sf = renderer.format();
        let ch = sf.channels as usize;
        let error = Arc::new(Mutex::new(None));
        let err2 = error.clone();
        let mut b = AudioStreamBuilder::new()
            .map_err(open_error)?
            .direction(AudioDirection::Output)
            .format(AudioFormat::PCM_Float)
            .channel_count(sf.channels as i32)
            .sample_rate(sf.rate as i32)
            .sharing_mode(AudioSharingMode::Shared)
            // Music, not games: bigger buffers save battery.
            .performance_mode(AudioPerformanceMode::PowerSaving)
            .usage(AudioUsage::Media)
            .content_type(AudioContentType::Music)
            .buffer_capacity_in_frames((sf.rate as u64 * cfg.buffer_ms as u64 / 1000) as i32);
        let device = cfg.device.as_deref().and_then(|d| d.parse::<i32>().ok());
        if let Some(id) = device {
            b = b.device_id(id);
        }
        let b = b
            .data_callback(Box::new(move |_s, data: *mut c_void, frames: i32| {
                let n = frames.max(0) as usize * ch;
                // SAFETY: AAudio hands a buffer of `frames` frames in the
                // stream's format, float here.
                let out = unsafe { std::slice::from_raw_parts_mut(data as *mut f32, n) };
                let mut done = 0;
                while done < n {
                    let want = (n - done) / ch;
                    let block = renderer.render(want);
                    for (o, s) in out[done..done + block.len()].iter_mut().zip(block) {
                        *o = *s as f32;
                    }
                    done += block.len();
                    if block.is_empty() {
                        out[done..].fill(0.0);
                        break;
                    }
                }
                AudioCallbackResult::Continue
            }))
            .error_callback(Box::new(move |_s, e| {
                *err2.lock().unwrap() = Some(format!("AAudio: {e}"));
            }));
        let stream = b.open_stream().map_err(open_error)?;
        stream.request_start().map_err(open_error)?;
        let label = DEVICES
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == stream.device_id())
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "Android audio".into());
        Ok(Self {
            stream: Stream(stream),
            error,
            label,
        })
    }
}

impl Output for AAudioOutput {
    fn describe(&self) -> String {
        format!("AAudio {}", self.label)
    }
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    fn device(&self) -> Option<crate::eq::OutputDevice> {
        // By name: Android hands a device a new id each time it's plugged in.
        Some(crate::eq::OutputDevice {
            key: format!("android:{}", self.label),
            name: self.label.clone(),
        })
    }
}

impl Drop for AAudioOutput {
    fn drop(&mut self) {
        let _ = self.stream.0.request_stop();
    }
}

/// AAudio's integer formats beyond 16 bits (API 31), which this version of
/// the bindings doesn't name.
const AAUDIO_FORMAT_PCM_I24_PACKED: i32 = 3;
const AAUDIO_FORMAT_PCM_I32: i32 = 4;

fn aaudio_format(s: Sample) -> AudioFormat {
    match s {
        Sample::S16 => AudioFormat::PCM_I16,
        Sample::S24Packed => AudioFormat::from(AAUDIO_FORMAT_PCM_I24_PACKED),
        Sample::S24In32 | Sample::S32 => AudioFormat::from(AAUDIO_FORMAT_PCM_I32),
        Sample::F32 => AudioFormat::PCM_Float,
    }
}

fn sample_name(s: Sample) -> &'static str {
    match s {
        Sample::S16 => "PCM 16",
        Sample::S24Packed => "PCM 24",
        Sample::S24In32 | Sample::S32 => "PCM 32",
        Sample::F32 => "float",
    }
}

/// The bit-perfect output on Android.
pub struct ExclusiveOutput {
    stream: Stream,
    info: ExclusiveInfo,
    error: Arc<Mutex<Option<String>>>,
}

impl ExclusiveOutput {
    pub fn try_start(
        cfg: &OutputConfig,
        depth: Depth,
        renderer: Renderer,
        _shared: SharedRef,
    ) -> Result<Self, Refused> {
        let sf = renderer.format();
        let want = describe(depth, sf.rate);
        let refuse = |why: String, r: Renderer| -> Refused { Box::new((why, r)) };
        let dev = {
            let devices = DEVICES.lock().unwrap();
            let wanted = cfg
                .bit_perfect_device
                .as_deref()
                .and_then(|d| d.strip_prefix("usb:"))
                .and_then(|d| d.parse::<i32>().ok());
            devices
                .iter()
                .filter(|d| d.usb)
                .find(|d| wanted.is_none_or(|w| w == d.id))
                .cloned()
        };
        let Some(dev) = dev else {
            return Err(refuse("no USB DAC is connected".into(), renderer));
        };
        // Ask for the DAC's mixer at this format: the system does the rest.
        // Formats that carry the source exactly come first.
        let (exact_list, close_list) = candidates(depth);
        let guard = HOOK.lock().unwrap();
        let Some(hook) = guard.as_ref() else {
            return Err(refuse(
                "bit-perfect isn't set up in this build".into(),
                renderer,
            ));
        };
        let mut chosen = None;
        let mut last_err = String::new();
        'pick: for (list, exact) in [(exact_list, true), (close_list, false)] {
            for s in list {
                // Packed 24 and 24-in-32 are both "24 bits" to Android; it
                // picks the container. 32 means a 32-bit integer.
                let bits = match s {
                    Sample::S16 => 16,
                    Sample::S24Packed | Sample::S24In32 => 24,
                    Sample::S32 => 32,
                    Sample::F32 => 0,
                };
                match hook(dev.id, sf.rate, bits, sf.channels) {
                    Ok(()) => {
                        chosen = Some((s, exact));
                        break 'pick;
                    }
                    Err(e) => last_err = e,
                }
            }
        }
        drop(guard);
        let Some((sample, exact)) = chosen else {
            let why = if last_err.is_empty() {
                format!("{} doesn't support {want}", dev.name)
            } else {
                last_err
            };
            return Err(refuse(why, renderer));
        };

        let ch = sf.channels as usize;
        let bytes = sample.bytes();
        let error = Arc::new(Mutex::new(None));
        let err2 = error.clone();
        // The renderer goes to the callback through a slot, so a stream that
        // fails to open can hand it back for the shared output. The
        // callback only ever try-locks it, which never waits.
        let slot = Arc::new(Mutex::new(Some(renderer)));
        let slot2 = slot.clone();
        let opened = AudioStreamBuilder::new().and_then(|b| {
            b.direction(AudioDirection::Output)
                .device_id(dev.id)
                .format(aaudio_format(sample))
                .channel_count(sf.channels as i32)
                .sample_rate(sf.rate as i32)
                .sharing_mode(AudioSharingMode::Shared)
                .performance_mode(AudioPerformanceMode::PowerSaving)
                .usage(AudioUsage::Media)
                .content_type(AudioContentType::Music)
                .data_callback(Box::new(move |_s, data: *mut c_void, frames: i32| {
                    let n = frames.max(0) as usize * ch;
                    // SAFETY: AAudio hands `frames` frames in the stream's
                    // format, `bytes` per sample.
                    let out = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, n * bytes) };
                    // Only the music: nothing while paused or flushed. The
                    // callback has to fill its buffer, so a short block is
                    // finished with digital silence.
                    let mut w = 0;
                    if let Ok(mut g) = slot2.try_lock()
                        && let Some(r) = g.as_mut()
                    {
                        let block = r.render_exact(n / ch);
                        w = encode(block, sample, out);
                    }
                    out[w..].fill(0);
                    AudioCallbackResult::Continue
                }))
                .error_callback(Box::new(move |_s, e| {
                    *err2.lock().unwrap() = Some(format!("AAudio: {e}"));
                }))
                .open_stream()
        });
        let back = |why: String| -> Refused {
            let r = slot
                .lock()
                .unwrap()
                .take()
                .expect("renderer still in its slot");
            Box::new((why, r))
        };
        let stream = match opened {
            Ok(s) => s,
            Err(e) => {
                return Err(back(format!(
                    "Android wouldn't open {want} on {}: {e}",
                    dev.name
                )));
            }
        };
        if stream.sample_rate() != sf.rate as i32 || stream.format() != aaudio_format(sample) {
            let got = format!("{} Hz, {:?}", stream.sample_rate(), stream.format());
            drop(stream);
            return Err(back(format!(
                "Android opened {} at {got}, not {want}",
                dev.name
            )));
        }
        if let Err(e) = stream.request_start() {
            drop(stream);
            return Err(back(format!("AAudio: {e}")));
        }
        Ok(Self {
            info: ExclusiveInfo {
                device: dev.name.clone(),
                format: sample_name(sample).into(),
                exact,
                hw_volume: false,
            },
            stream: Stream(stream),
            error,
        })
    }
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
}

impl Drop for ExclusiveOutput {
    fn drop(&mut self) {
        let _ = self.stream.0.request_stop();
    }
}
