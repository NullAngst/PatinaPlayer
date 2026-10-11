//! A device that isn't there. In normal use it paces itself like a real
//! sound card and discards the audio, which keeps the player usable on a
//! box with no sound hardware. With a capture buffer it runs as fast as
//! it can and records every sample that came out of the ring, which is
//! what the gapless tests compare against.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::Output;
use crate::audio::render::Renderer;

pub type Capture = Arc<Mutex<Vec<f64>>>;

pub struct NullOutput {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

const BLOCK: usize = 1024;

impl NullOutput {
    pub fn start(mut renderer: Renderer, capture: Option<Capture>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("patina-null-out".into())
            .spawn(move || {
                let fmt = renderer.format();
                let block_time = Duration::from_secs_f64(BLOCK as f64 / fmt.rate as f64);
                let mut next = Instant::now();
                while !stop2.load(Ordering::Relaxed) {
                    renderer.render(BLOCK);
                    match &capture {
                        Some(cap) => {
                            let n = renderer.last_from_ring() * fmt.channels as usize;
                            if n > 0 {
                                cap.lock()
                                    .unwrap()
                                    .extend_from_slice(&renderer.last_block()[..n]);
                            } else {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        }
                        None => {
                            next += block_time;
                            let now = Instant::now();
                            if next > now {
                                std::thread::sleep(next - now);
                            } else {
                                next = now;
                            }
                        }
                    }
                }
            })
            .expect("spawn null output thread");
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Output for NullOutput {
    fn describe(&self) -> String {
        "no output".into()
    }
    fn error(&self) -> Option<String> {
        None
    }
}

impl Drop for NullOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
