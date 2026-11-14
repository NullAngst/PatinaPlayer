//! Steering another player from this window: its Now Playing, kept up to
//! date from its remote control server ([`patina_core::control`]).
//!
//! The network is never touched on the window's thread. One thread asks
//! for the state once a second (sooner after a press) and fetches the
//! cover and the queue when they change; another sends the presses.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use patina_core::browse::fmt_duration;
use patina_core::control::{Client, Cmd, Queue, State};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::AppWindow;

enum Msg {
    State(State),
    Art(Option<SharedPixelBuffer<Rgba8Pixel>>),
    Queue(Queue),
    Error(String),
}

/// What came in that the window should know about.
pub enum Heard {
    /// The first answer: the code was right.
    Connected,
    /// Something went wrong (said once per new error).
    Error(String),
}

pub struct RemoteView {
    pub name: String,
    pub addr: String,
    pub code: String,
    rx: Receiver<Msg>,
    cmds: Sender<Cmd>,
    stop: Arc<AtomicBool>,
    state: Option<State>,
    /// When the state arrived, so the position moves on in between.
    got_at: Instant,
    art: Option<Image>,
    pub queue: Queue,
    last_error: Option<String>,
}

impl RemoteView {
    pub fn start(client: Client, name: String, code: String) -> Self {
        let (tx, rx) = unbounded();
        let (cmds, cmd_rx) = unbounded::<Cmd>();
        let (poke, poked) = crossbeam_channel::bounded::<()>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let addr = client.addr().to_string();

        let (c2, tx2, stop2) = (client.clone(), tx.clone(), stop.clone());
        std::thread::Builder::new()
            .name("patina-remote-poll".into())
            .spawn(move || {
                let (mut art_key, mut queue_ver) = (None::<String>, None::<u64>);
                while !stop2.load(Ordering::Relaxed) {
                    let msg = match c2.state() {
                        Ok(s) => {
                            if art_key.as_ref() != Some(&s.art) {
                                art_key = Some(s.art.clone());
                                let img = if s.art.is_empty() {
                                    None
                                } else {
                                    c2.art().ok().flatten().and_then(|b| decode(&b))
                                };
                                let _ = tx2.send(Msg::Art(img));
                            }
                            if queue_ver != Some(s.queue)
                                && let Ok(q) = c2.queue()
                            {
                                queue_ver = Some(s.queue);
                                let _ = tx2.send(Msg::Queue(q));
                            }
                            Msg::State(s)
                        }
                        Err(e) => Msg::Error(e),
                    };
                    if tx2.send(msg).is_err() {
                        return;
                    }
                    let _ = poked.recv_timeout(Duration::from_secs(1));
                }
            })
            .expect("spawn remote poll thread");
        std::thread::Builder::new()
            .name("patina-remote-send".into())
            .spawn(move || {
                for c in cmd_rx {
                    if let Err(e) = client.send(&c) {
                        let _ = tx.send(Msg::Error(e));
                    }
                    // Show what the press did straight away.
                    let _ = poke.try_send(());
                }
            })
            .expect("spawn remote send thread");
        Self {
            name,
            addr,
            code,
            rx,
            cmds,
            stop,
            state: None,
            got_at: Instant::now(),
            art: None,
            queue: Queue::default(),
            last_error: None,
        }
    }

    pub fn send(&mut self, c: Cmd) {
        // Shown at once; the next answer sets it right if it didn't take.
        if let Some(s) = &mut self.state {
            match &c {
                Cmd::Toggle if s.state == "playing" => s.state = "paused".into(),
                Cmd::Toggle => s.state = "playing".into(),
                Cmd::Shuffle => s.shuffle = !s.shuffle,
                Cmd::Mute => s.muted = !s.muted,
                Cmd::Volume { value } => s.volume = *value,
                Cmd::Seek { ms } => {
                    s.position_ms = *ms;
                    self.got_at = Instant::now();
                }
                _ => {}
            }
        }
        let _ = self.cmds.send(c);
    }

    pub fn duration_ms(&self) -> u64 {
        self.state.as_ref().map_or(0, |s| s.duration_ms)
    }

    /// Take what the threads sent.
    pub fn poll(&mut self) -> Vec<Heard> {
        let mut heard = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::State(s) => {
                    if self.state.is_none() {
                        heard.push(Heard::Connected);
                    }
                    self.state = Some(s);
                    self.got_at = Instant::now();
                    self.last_error = None;
                }
                Msg::Art(img) => self.art = img.map(Image::from_rgba8),
                Msg::Queue(q) => self.queue = q,
                Msg::Error(e) => {
                    if self.last_error.as_ref() != Some(&e) {
                        self.last_error = Some(e.clone());
                        heard.push(Heard::Error(e));
                    }
                }
            }
        }
        heard
    }

    pub fn show(&self, ui: &AppWindow) {
        ui.set_rm_name(SharedString::from(self.name.as_str()));
        let Some(s) = &self.state else {
            ui.set_rm_has_track(false);
            ui.set_rm_title(SharedString::from("Connecting…"));
            return;
        };
        let playing = s.state == "playing";
        let mut pos = s.position_ms;
        if playing {
            pos += self.got_at.elapsed().as_millis() as u64;
        }
        if s.duration_ms > 0 {
            pos = pos.min(s.duration_ms);
        }
        ui.set_rm_has_track(!s.title.is_empty());
        ui.set_rm_title(SharedString::from(s.title.as_str()));
        ui.set_rm_artist(SharedString::from(s.artist.as_str()));
        ui.set_rm_has_art(self.art.is_some());
        ui.set_rm_art(self.art.clone().unwrap_or_default());
        ui.set_rm_playing(playing);
        ui.set_rm_live(s.live);
        ui.set_rm_position(if s.duration_ms > 0 {
            pos as f32 / s.duration_ms as f32
        } else {
            0.0
        });
        ui.set_rm_elapsed(SharedString::from(fmt_duration(Duration::from_millis(pos))));
        ui.set_rm_remaining(SharedString::from(if s.duration_ms > 0 {
            format!(
                "-{}",
                fmt_duration(Duration::from_millis(s.duration_ms - pos))
            )
        } else {
            String::new()
        }));
        ui.set_rm_shuffle(s.shuffle);
        ui.set_rm_repeat(match s.repeat.as_str() {
            "all" => 1,
            "one" => 2,
            _ => 0,
        });
        ui.set_rm_volume(s.volume as f32);
        ui.set_rm_muted(s.muted);
        ui.set_rm_love_state(match s.loved {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        });
    }
}

impl Drop for RemoteView {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn decode(bytes: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let img = image::load_from_memory(bytes)
        .ok()?
        .thumbnail(640, 640)
        .to_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(
        img.as_raw(),
        img.width(),
        img.height(),
    ))
}
