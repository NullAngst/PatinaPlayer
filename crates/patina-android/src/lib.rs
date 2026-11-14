//! PatinaPlayer for Android: the native library the app's activity runs.
//!
//! The window, the views and the player are the desktop app's own, from
//! `patina-gui` and `patina-core`. What's here is the part only Android
//! needs: finding folders to keep things in, picking up the music folder
//! once the permission to read it comes in, AAudio's device list and
//! bit-perfect USB (through the Java side in `android/java`), the system's
//! folder and file pickers and its text dialogs, and the media session and
//! notification, which the Java service runs from what this reports.
//!
//! The player can also start without the window: a car (Android Auto)
//! binds the media service with the app closed, and the service starts
//! the player on a thread of its own ([`headless`]). Opening the app then
//! takes that same player over, queue, song and all.
//!
//! On anything but Android this crate is empty.

#![cfg(target_os = "android")]

mod headless;
mod jni_glue;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use patina_core::session::{Options, Session};
use patina_core::track::MediaKind;
use patina_core::{PlayState, Player};
use patina_gui::platform::{self, Hooks, Picked, Prompt};
use patina_gui::{AppWindow, bridge};
use slint::ComponentHandle;
use slint::android::AndroidApp;

/// Where Android keeps a phone's own music.
pub(crate) const MUSIC: &str = "/storage/emulated/0/Music";

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("PatinaPlayer"),
    );
    std::panic::set_hook(Box::new(|p| log::error!("panic: {p}")));

    // Taken by the window from here on: a car connecting now uses this
    // player rather than starting one of its own.
    headless::window_starting();
    if let Some(base) = app.internal_data_path() {
        set_dirs(&base);
    } else {
        log::error!("Android gave no app storage");
    }

    // The Music folder goes in the library on first start whatever the
    // permission answer is. Without the permission it scans as empty, and
    // it's scanned again the moment the permission is granted. Waiting for
    // the answer here instead would hang the app: Android's lifecycle
    // calls wait on this thread, and the dialog needs them.
    first_run(Path::new(MUSIC));

    if let Err(e) = slint::android::init(app) {
        log::error!("can't start the window: {e}");
        return;
    }

    // Playing for a car already: carry on with that player.
    let session = match headless::take() {
        Some(s) => {
            log::info!("the window takes over the player the car started");
            s
        }
        None => match Session::open(Options::default()) {
            Ok(s) => s,
            Err(e) => {
                log::error!("can't start: {e}");
                headless::window_closed();
                return;
            }
        },
    };
    let fade_ms = session.fade_ms;
    let playing_files = session.playing_files;
    let player = session.browser.player.clone();

    jni_glue::set_player(player.clone());
    // Folders and files picked through the system's pickers, and text typed
    // in its own dialogs.
    platform::set_hooks(Box::new(AndroidHooks));
    patina_core::audio::output::aaudio::set_bit_perfect_hook(Box::new(jni_glue::set_bit_perfect));

    let ui = match AppWindow::new() {
        Ok(ui) => ui,
        Err(e) => {
            log::error!("can't make the window: {e}");
            return;
        }
    };
    // No tray on a phone: nothing hides on close.
    let bridge = Rc::new(bridge::install(
        &ui,
        session.browser,
        playing_files,
        Rc::new(Cell::new(false)),
    ));
    // Back with nowhere to go back to leaves the music playing.
    bridge.on_back_out(jni_glue::move_to_back);

    // The media session and notification follow the player.
    let media = slint::Timer::default();
    let mut m = Media::new();
    let p = player.clone();
    let b = bridge.clone();
    media.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            m.report(&p);
            // The permission to read music just came in: look again.
            if jni_glue::take_newly_granted() {
                b.rescan();
            }
        },
    );

    if let Err(e) = ui.run() {
        log::error!("the window stopped: {e}");
    }
    drop(media);
    bridge.shutdown(fade_ms);
    jni_glue::stopped();
    headless::window_closed();
}

/// What the window asks of Android, answered through the Java side.
struct AndroidHooks;

impl Hooks for AndroidHooks {
    fn pick_folder(&self) -> Picked {
        jni_glue::pick_folder()
    }

    fn pick_eq_file(&self) -> Picked {
        jni_glue::pick_eq_file()
    }

    fn pick_image(&self) -> Picked {
        jni_glue::pick_image()
    }

    fn prompt(&self, id: u64, p: &Prompt) -> bool {
        jni_glue::prompt(id, p)
    }

    fn system_colors(&self) -> Option<patina_gui::system_theme::SystemColors> {
        jni_glue::system_colors()
            .and_then(|v| patina_gui::system_theme::SystemColors::from_flat(&v))
    }

    fn widget_colors(&self, c: &patina_gui::platform::WidgetColors) {
        jni_glue::widget_colors(&c.to_array());
    }
}

/// Point the app's folders (config, library, cache) at Android's app
/// storage. The core finds them through the XDG variables, as on Linux.
/// Only the first call does anything: the window and the car's player
/// both set them, and once the player runs its threads read them.
pub(crate) fn set_dirs(base: &Path) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| set_dirs_now(base));
}

fn set_dirs_now(base: &Path) {
    let base = base.to_path_buf();
    let dirs = [
        ("HOME", base.clone()),
        ("XDG_CONFIG_HOME", base.join("config")),
        ("XDG_DATA_HOME", base.join("data")),
        ("XDG_CACHE_HOME", base.join("cache")),
    ];
    for (k, v) in dirs {
        let _ = std::fs::create_dir_all(&v);
        // SAFETY: this runs once, before the core or the window start
        // threads (first thing in android_main, or before the car's player
        // starts). The only other threads alive are Android's and the
        // activity glue's, which don't read these.
        unsafe { std::env::set_var(k, v) };
    }
}

/// First start: write the default config, and add the phone's Music
/// folder to the library. No check that it's a folder: without the
/// permission, Android may not even let the app see that much.
pub(crate) fn first_run(music: &Path) {
    let path = patina_core::config::config_path();
    if path.exists() {
        return;
    }
    if let Err(e) = patina_core::config::Config::load_or_create() {
        log::error!("can't write the config: {e}");
        return;
    }
    match patina_core::config::edit_library_folder(&path, music, true) {
        Ok(_) => log::info!("added {} to the library", music.display()),
        Err(e) => log::warn!("can't add {}: {e}", music.display()),
    }
}

/// What the media session was last told, so it's only told changes, and
/// the position now and then (it moves the lock screen's bar on its own in
/// between).
pub(crate) struct Media {
    track: Option<(u64, String, String, String)>,
    state: i32,
    /// The car's heart and shuffle buttons, as last shown.
    marks: (i32, bool),
    pos_s: u64,
    /// The cover for which track, found on a thread of its own since a
    /// server cover may have to be fetched.
    art_for: Option<u64>,
    art: Option<PathBuf>,
    art_tx: mpsc::Sender<(u64, PathBuf)>,
    art_rx: mpsc::Receiver<(u64, PathBuf)>,
    /// The queue the car was last shown: its version and the song
    /// playing, and when, so a queue still being read (each song's tags
    /// bump the version) isn't sent over and over.
    queue: (u64, Option<u64>),
    queue_at: Option<std::time::Instant>,
}

/// How much of the queue the car gets: a few songs back and the rest
/// ahead. Its queue screen is a short list; thousands would only be slow.
const CAR_QUEUE_BACK: usize = 5;
const CAR_QUEUE_AHEAD: usize = 95;

impl Media {
    pub(crate) fn new() -> Self {
        let (art_tx, art_rx) = mpsc::channel();
        Self {
            track: None,
            state: -1,
            marks: (-1, false),
            pos_s: 0,
            art_for: None,
            art: None,
            art_tx,
            art_rx,
            queue: (u64::MAX, None),
            queue_at: None,
        }
    }

    /// Tell the car's queue screen what's queued, when it changes.
    fn report_queue(&mut self, p: &Player, current: Option<u64>) {
        let q = p.queue();
        let now = (q.version, current);
        if now == self.queue {
            return;
        }
        // Only the tags filled in: at most every two seconds.
        if now.1 == self.queue.1
            && self
                .queue_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.queue = now;
        self.queue_at = Some(std::time::Instant::now());
        let order = q.play_order();
        let at = current.and_then(|c| q.position(c)).unwrap_or(0);
        let from = at.saturating_sub(CAR_QUEUE_BACK);
        let to = (at + CAR_QUEUE_AHEAD + 1).min(order.len());
        let items: Vec<jni_glue::QueueItem> = order[from.min(to)..to]
            .iter()
            .filter_map(|&id| q.get(id))
            .map(|e| jni_glue::QueueItem {
                id: e.id,
                title: e.info.display_title(),
                sub: e.info.artist.clone().unwrap_or_default(),
                art: (e.info.kind == MediaKind::File && !e.info.path.as_os_str().is_empty())
                    .then(|| e.info.path.clone()),
            })
            .collect();
        drop(q);
        jni_glue::on_queue(&items, current);
    }

    pub(crate) fn report(&mut self, p: &Player) {
        let snap = p.snapshot();
        let state = match snap.state {
            PlayState::Stopped => jni_glue::STOPPED,
            PlayState::Playing => jni_glue::PLAYING,
            PlayState::Paused => jni_glue::PAUSED,
        };
        let track = snap.current.zip(snap.track.as_ref()).map(|(id, t)| {
            // Radio: the stream's own title is the song.
            let title = match (&snap.live_title, t.kind) {
                (Some(lt), MediaKind::Radio) if !lt.trim().is_empty() => lt.clone(),
                _ => t.display_title(),
            };
            (
                id,
                title,
                t.artist.clone().unwrap_or_default(),
                t.album.clone().unwrap_or_default(),
            )
        });
        let id = track.as_ref().map(|t| t.0);

        let mut art_new = false;
        if id != self.art_for {
            self.art_for = id;
            if self.art.take().is_some() {
                art_new = true;
            }
            if let (Some(id), Some(t)) = (id, snap.track.clone())
                && !t.path.as_os_str().is_empty()
            {
                let tx = self.art_tx.clone();
                std::thread::spawn(move || {
                    if let Some(f) = patina_core::art::cached_file(&t.path, 512) {
                        let _ = tx.send((id, f));
                    }
                });
            }
        }
        while let Ok((aid, f)) = self.art_rx.try_recv() {
            if Some(aid) == self.art_for {
                self.art = Some(f);
                art_new = true;
            }
        }

        self.report_queue(p, snap.current);

        let pos_s = snap.position.as_secs();
        let marks = (
            match platform::now_loved() {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            },
            snap.shuffle,
        );
        let changed = track != self.track || state != self.state || art_new || marks != self.marks;
        self.marks = marks;
        let moved =
            pos_s.abs_diff(self.pos_s) >= 5 || (state != jni_glue::PLAYING && pos_s != self.pos_s);
        if !changed && !moved {
            return;
        }
        self.track = track.clone();
        self.state = state;
        self.pos_s = pos_s;
        let (title, artist, album) = track.map(|(_, t, ar, al)| (t, ar, al)).unwrap_or_default();
        jni_glue::on_state(
            &title,
            &artist,
            &album,
            self.art.as_deref(),
            snap.duration.map_or(0, |d| d.as_millis() as i64),
            snap.position.as_millis() as i64,
            state,
            marks.0,
            marks.1,
        );
    }
}
