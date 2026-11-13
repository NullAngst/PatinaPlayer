//! The player without the window, for a car.
//!
//! Android Auto binds the media service whether or not the app is open.
//! With it closed there was no player for the car to browse or play, only
//! "Open PatinaPlayer on the phone". Now the service starts one here, on a
//! thread of its own, with the same session the window would open: the
//! library, the servers, the last queue and where it was. Picks from the
//! car, its buttons and the media session drive it as they drive the
//! window's.
//!
//! When the app is opened while this runs, the window takes the session
//! over (see [`take`]) and this thread ends, so there's only ever one
//! player.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use patina_core::session::{Options, Session};
use patina_gui::platform;

use crate::jni_glue;

/// Where to send a session to when the window wants it.
type HandOver = mpsc::Sender<Session>;

/// The running headless player's line, to ask for its session back.
static HOST: Mutex<Option<mpsc::Sender<HandOver>>> = Mutex::new(None);
/// The window has the player (or is starting it).
static WINDOW: AtomicBool = AtomicBool::new(false);

/// Start the player without the window, unless it's already running in
/// one or the other. `base` is the app's storage (Context.getFilesDir).
pub fn start(base: &Path) {
    if WINDOW.load(Ordering::Acquire) {
        return;
    }
    let mut host = HOST.lock().unwrap();
    if host.is_some() {
        return;
    }
    crate::set_dirs(base);
    crate::first_run(Path::new(crate::MUSIC));
    let (tx, rx) = mpsc::channel::<HandOver>();
    let spawned = std::thread::Builder::new()
        .name("patina-car".into())
        .spawn(move || run(rx));
    match spawned {
        Ok(_) => *host = Some(tx),
        Err(e) => log::error!("can't start the player for the car: {e}"),
    }
}

/// The window is starting: from now on it runs the player.
pub fn window_starting() {
    WINDOW.store(true, Ordering::Release);
}

/// The window closed and stopped its player. A car can start one again.
pub fn window_closed() {
    WINDOW.store(false, Ordering::Release);
}

/// The session the car's player is running, for the window to carry on
/// with. None if none is running.
pub fn take() -> Option<Session> {
    let host = HOST.lock().unwrap().take()?;
    let (tx, rx) = mpsc::channel();
    host.send(tx).ok()?;
    rx.recv_timeout(Duration::from_secs(10)).ok()
}

fn run(handover: mpsc::Receiver<HandOver>) {
    let mut session = match Session::open(Options::default()) {
        Ok(s) => s,
        Err(e) => {
            log::error!("can't start the player for the car: {e}");
            HOST.lock().unwrap().take();
            return;
        }
    };
    log::info!("player started without the window, for the car");
    jni_glue::set_player(session.browser.player.clone());
    patina_core::audio::output::aaudio::set_bit_perfect_hook(Box::new(jni_glue::set_bit_perfect));
    let mut media = crate::Media::new();
    loop {
        match handover.recv_timeout(Duration::from_millis(250)) {
            Ok(tx) => {
                // Whatever the window shows starts from here.
                let _ = session.browser.take_notes();
                if let Err(mpsc::SendError(s)) = tx.send(session) {
                    // The window gave up waiting: stop rather than run two.
                    let mut s = s;
                    patina_core::session::shutdown(&mut s.browser, s.fade_ms);
                }
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                patina_core::session::shutdown(&mut session.browser, session.fade_ms);
                return;
            }
        }
        let b = &mut session.browser;
        b.poll();
        for id in platform::take_media_ids_pub() {
            b.auto_play(&id);
        }
        for a in platform::take_outside_actions() {
            if a == "love" {
                b.toggle_love();
            }
        }
        platform::set_now_loved(b.loved_now);
        // No window to show notes in. The log keeps them.
        for n in b.take_notes() {
            log::info!("car: {n:?}");
        }
        if jni_glue::take_newly_granted() {
            b.rescan();
        }
        media.report(&b.player);
    }
}
