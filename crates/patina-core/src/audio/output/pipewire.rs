//! Native PipeWire output. The stream runs in RT_PROCESS mode, so the
//! process callback (and so the renderer) runs on PipeWire's data thread.
//!
//! This path gets as close as PipeWire allows (f32 at the source rate, no
//! software volume on the node), but it is never called bit-perfect. Only
//! the ALSA hw path earns that badge.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use pipewire as pw;
use pw::spa;
use pw::spa::pod::Pod;
use pw::spa::sys as spa_sys;

use super::{Output, OutputConfig, OutputError, to_f32};
use crate::audio::render::{MAX_BLOCK_FRAMES, Renderer};

pub struct PwOutput {
    quit: Option<pw::channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
    target: Option<String>,
}

type StartResult = Result<(), (String, Option<Renderer>)>;

impl PwOutput {
    pub fn start(cfg: &OutputConfig, renderer: Renderer) -> Result<Self, OutputError> {
        Self::try_start(cfg, renderer).map_err(|(e, _)| e)
    }

    /// Like [`start`](Self::start), but hands the renderer back when the
    /// daemon can't be reached, so the caller can fall back to ALSA.
    #[allow(clippy::result_large_err)] // startup only, never on a hot path
    pub fn try_start(
        cfg: &OutputConfig,
        renderer: Renderer,
    ) -> Result<Self, (OutputError, Renderer)> {
        let (ready_tx, ready_rx) = mpsc::channel::<StartResult>();
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let error = Arc::new(Mutex::new(None));
        let error2 = error.clone();
        let cfg2 = cfg.clone();
        // The renderer goes to the PipeWire thread and comes back through
        // the ready channel if startup fails.
        let thread = std::thread::Builder::new()
            .name("patina-pw-main".into())
            .spawn(move || run(cfg2, renderer, ready_tx, quit_rx, error2))
            .expect("spawn PipeWire thread");

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                quit: Some(quit_tx),
                thread: Some(thread),
                error,
                target: cfg.device.clone(),
            }),
            Ok(Err((msg, renderer))) => {
                let _ = thread.join();
                match renderer {
                    Some(r) => Err((OutputError::PipeWire(msg), r)),
                    None => panic!("PipeWire startup failed after taking the renderer: {msg}"),
                }
            }
            Err(_) => panic!("PipeWire thread died during startup"),
        }
    }
}

fn run(
    cfg: OutputConfig,
    renderer: Renderer,
    ready: mpsc::Sender<StartResult>,
    quit: pw::channel::Receiver<()>,
    error: Arc<Mutex<Option<String>>>,
) {
    pw::init();
    let fmt = renderer.format();
    let slot = Rc::new(RefCell::new(Some(renderer)));
    let fail = |msg: String, slot: &Rc<RefCell<Option<Renderer>>>| {
        let r = slot.borrow_mut().take();
        let _ = ready.send(Err((msg, r)));
    };

    let mainloop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(m) => m,
        Err(e) => return fail(format!("main loop: {e}"), &slot),
    };
    let context = match pw::context::ContextRc::new(&mainloop, None) {
        Ok(c) => c,
        Err(e) => return fail(format!("context: {e}"), &slot),
    };
    let core = match context.connect_rc(None) {
        Ok(c) => c,
        Err(e) => return fail(format!("can't reach the PipeWire daemon ({e})"), &slot),
    };

    let mut props = pw::properties::PropertiesBox::new();
    props.insert(*pw::keys::MEDIA_TYPE, "Audio");
    props.insert(*pw::keys::MEDIA_CATEGORY, "Playback");
    props.insert(*pw::keys::MEDIA_ROLE, "Music");
    props.insert(*pw::keys::APP_NAME, "PatinaPlayer");
    props.insert(*pw::keys::APP_ID, "patinaplayer");
    props.insert(*pw::keys::NODE_NAME, "patinaplayer");
    props.insert(*pw::keys::AUDIO_CHANNELS, fmt.channels.to_string());
    let latency_frames = (cfg.buffer_ms.clamp(50, 1000) as u64 * fmt.rate as u64 / 1000).max(64);
    props.insert(
        *pw::keys::NODE_LATENCY,
        format!("{latency_frames}/{}", fmt.rate),
    );
    // Ask the graph to run at our rate. Whether it does depends on
    // default.clock.allowed-rates in the user's PipeWire config.
    props.insert("node.rate", format!("1/{}", fmt.rate));
    if let Some(t) = &cfg.device {
        // PW_KEY_TARGET_OBJECT, spelled out since the constant is gated
        // behind newer PipeWire versions than the oldest build target.
        props.insert("target.object", t.as_str());
    }

    let stream = match pw::stream::StreamBox::new(&core, "PatinaPlayer", props) {
        Ok(s) => s,
        Err(e) => return fail(format!("stream: {e}"), &slot),
    };

    let ch = fmt.channels as usize;
    let stride = std::mem::size_of::<f32>() * ch;
    let slot_rt = slot.clone();
    let error_cb = error.clone();
    let listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, _old, new| {
            if let pw::stream::StreamState::Error(msg) = new {
                log::error!("PipeWire stream error: {msg}");
                *error_cb.lock().unwrap() = Some(msg);
            }
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let data = &mut datas[0];
            let mut written = 0usize;
            if let Some(slice) = data.data() {
                let cap = slice.len() / stride;
                // pw_buffer.requested needs PipeWire 0.3.49, newer than the
                // oldest build target, so fill up to our latency target.
                // If the graph quantum is smaller, the stream queues the
                // rest, which costs latency, not samples.
                let want = cap.min(latency_frames as usize);
                let mut guard = slot_rt.borrow_mut();
                if let Some(r) = guard.as_mut() {
                    while written < want {
                        let n = (want - written).min(MAX_BLOCK_FRAMES);
                        let block = r.render(n);
                        let base = written * stride;
                        for (i, s) in block.iter().enumerate() {
                            let off = base + i * 4;
                            slice[off..off + 4].copy_from_slice(&to_f32(*s).to_le_bytes());
                        }
                        written += block.len() / ch;
                    }
                }
            }
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (stride * written) as u32;
        })
        .register();
    let _listener = match listener {
        Ok(l) => l,
        Err(e) => return fail(format!("listener: {e}"), &slot),
    };

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(fmt.rate);
    info.set_channels(fmt.channels as u32);
    info.set_position(positions(fmt.mask, fmt.channels));
    let pod_bytes: Vec<u8> = match spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa_sys::SPA_TYPE_OBJECT_Format,
            id: spa_sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    ) {
        Ok(v) => v.0.into_inner(),
        Err(e) => return fail(format!("format pod: {e:?}"), &slot),
    };
    let Some(pod) = Pod::from_bytes(&pod_bytes) else {
        return fail("format pod".into(), &slot);
    };
    let mut params = [pod];

    if let Err(e) = stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        return fail(format!("connect: {e}"), &slot);
    }

    let ml = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |()| ml.quit());
    let _ = ready.send(Ok(()));
    mainloop.run();
    let _ = stream.disconnect();
}

/// FFmpeg AV_CH_* bit order mapped to SPA channel positions.
fn positions(mask: u64, channels: u16) -> [u32; spa::param::audio::MAX_CHANNELS] {
    const MAP: [u32; 18] = [
        spa_sys::SPA_AUDIO_CHANNEL_FL,
        spa_sys::SPA_AUDIO_CHANNEL_FR,
        spa_sys::SPA_AUDIO_CHANNEL_FC,
        spa_sys::SPA_AUDIO_CHANNEL_LFE,
        spa_sys::SPA_AUDIO_CHANNEL_RL,
        spa_sys::SPA_AUDIO_CHANNEL_RR,
        spa_sys::SPA_AUDIO_CHANNEL_FLC,
        spa_sys::SPA_AUDIO_CHANNEL_FRC,
        spa_sys::SPA_AUDIO_CHANNEL_RC,
        spa_sys::SPA_AUDIO_CHANNEL_SL,
        spa_sys::SPA_AUDIO_CHANNEL_SR,
        spa_sys::SPA_AUDIO_CHANNEL_TC,
        spa_sys::SPA_AUDIO_CHANNEL_TFL,
        spa_sys::SPA_AUDIO_CHANNEL_TFC,
        spa_sys::SPA_AUDIO_CHANNEL_TFR,
        spa_sys::SPA_AUDIO_CHANNEL_TRL,
        spa_sys::SPA_AUDIO_CHANNEL_TRC,
        spa_sys::SPA_AUDIO_CHANNEL_TRR,
    ];
    let mut pos = [0u32; spa::param::audio::MAX_CHANNELS];
    if channels == 1 {
        pos[0] = spa_sys::SPA_AUDIO_CHANNEL_MONO;
        return pos;
    }
    let mut i = 0;
    for (bit, p) in MAP.iter().enumerate() {
        if i >= channels as usize {
            break;
        }
        if mask & (1 << bit) != 0 {
            pos[i] = *p;
            i += 1;
        }
    }
    // Anything unmapped gets an auxiliary position rather than a wrong one.
    for (k, slot) in pos.iter_mut().enumerate().take(channels as usize).skip(i) {
        *slot = spa_sys::SPA_AUDIO_CHANNEL_AUX0 + k as u32;
    }
    pos
}

impl Output for PwOutput {
    fn describe(&self) -> String {
        match &self.target {
            Some(t) => format!("PipeWire {t}"),
            None => "PipeWire".into(),
        }
    }
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for PwOutput {
    fn drop(&mut self) {
        if let Some(q) = self.quit.take() {
            let _ = q.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
