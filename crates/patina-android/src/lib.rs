//! PatinaPlayer for Android: the native library the app's activity runs.
//!
//! The window, the views and the player are the desktop app's own, from
//! `patina-gui` and `patina-core`. What's here is the part only Android
//! needs: finding folders to keep things in, picking up the music folder
//! once the permission to read it comes in, AAudio's device list and bit-perfect USB (through the Java
//! side in `android/java`), and the media session and notification, which
//! the Java service runs from what this reports.
//!
//! On anything but Android this crate is empty.

#![cfg(target_os = "android")]

mod jni_glue;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use patina_core::session::{Options, Session};
use patina_core::track::MediaKind;
use patina_core::{PlayState, Player};
use patina_gui::{AppWindow, bridge};
use slint::ComponentHandle;
use slint::android::AndroidApp;

/// Where Android keeps a phone's own music.
const MUSIC: &str = "/storage/emulated/0/Music";

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("PatinaPlayer"),
    );
    std::panic::set_hook(Box::new(|p| log::error!("panic: {p}")));

    set_dirs(&app);

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

    let session = match Session::open(Options::default()) {
        Ok(s) => s,
        Err(e) => {
            log::error!("can't start: {e}");
            return;
        }
    };
    let fade_ms = session.fade_ms;
    let playing_files = session.playing_files;
    let player = session.browser.player.clone();

    jni_glue::set_player(player.clone());
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
}

/// Point the app's folders (config, library, cache) at Android's app
/// storage. The core finds them through the XDG variables, as on Linux.
fn set_dirs(app: &AndroidApp) {
    let Some(base) = app.internal_data_path() else {
        log::error!("Android gave no app storage");
        return;
    };
    let dirs = [
        ("HOME", base.clone()),
        ("XDG_CONFIG_HOME", base.join("config")),
        ("XDG_DATA_HOME", base.join("data")),
        ("XDG_CACHE_HOME", base.join("cache")),
    ];
    for (k, v) in dirs {
        let _ = std::fs::create_dir_all(&v);
        // SAFETY: this runs first thing in android_main, before the core
        // or the window start threads. The only other threads alive are
        // Android's and the activity glue's, which don't read these.
        unsafe { std::env::set_var(k, v) };
    }
}

/// First start: write the default config, and add the phone's Music
/// folder to the library. No check that it's a folder: without the
/// permission, Android may not even let the app see that much.
fn first_run(music: &Path) {
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
struct Media {
    track: Option<(u64, String, String, String)>,
    state: i32,
    pos_s: u64,
    /// The cover for which track, found on a thread of its own since a
    /// server cover may have to be fetched.
    art_for: Option<u64>,
    art: Option<PathBuf>,
    art_tx: mpsc::Sender<(u64, PathBuf)>,
    art_rx: mpsc::Receiver<(u64, PathBuf)>,
}

impl Media {
    fn new() -> Self {
        let (art_tx, art_rx) = mpsc::channel();
        Self {
            track: None,
            state: -1,
            pos_s: 0,
            art_for: None,
            art: None,
            art_tx,
            art_rx,
        }
    }

    fn report(&mut self, p: &Player) {
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

        let pos_s = snap.position.as_secs();
        let changed = track != self.track || state != self.state || art_new;
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
        );
    }
}
