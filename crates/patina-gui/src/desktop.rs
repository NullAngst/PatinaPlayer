//! The desktop around the window: MPRIS2 for media keys and the panel's
//! media widget, and the tray icon (a StatusNotifierItem, which is what
//! Plasma's system tray and most others show).
//!
//! Both run on threads of their own. What they need from the window comes
//! back as [`Action`]s, which the bridge applies on the UI thread.
//!
//! Android has neither: its media session and notification live in the
//! app's Java service, fed from `crates/patina-android`. There, this is a
//! stand-in that does nothing.

#[cfg(target_os = "android")]
pub use stand_in::Desktop;

#[cfg(not(target_os = "android"))]
pub use linux::Desktop;

/// What the desktop asked the window to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show the window and bring it forward.
    Show,
    /// The tray icon was clicked: show it if hidden, hide it if showing.
    Toggle,
    Quit,
}

#[cfg(target_os = "android")]
mod stand_in {
    use super::Action;
    use patina_core::{Player, Snapshot};

    pub struct Desktop;

    impl Desktop {
        pub fn start(_player: Player, _tray: bool) -> Self {
            Desktop
        }
        pub fn has_tray(&self) -> bool {
            false
        }
        pub fn set_tray(&mut self, _on: bool) {}
        pub fn tick(&mut self, _snap: &Snapshot) -> Vec<Action> {
            Vec::new()
        }
    }
}

#[cfg(not(target_os = "android"))]
mod linux {
    use super::Action;

    use crossbeam_channel::{Receiver, Sender, unbounded};
    use ksni::blocking::{Handle, TrayMethods};
    use patina_core::mpris::{Abilities, Mpris, Request};
    use patina_core::{Command, PlayState, Player, Snapshot};

    pub struct Desktop {
        mpris: Option<(Mpris, Receiver<Request>)>,
        tray: Option<Handle<Tray>>,
        /// The tray setting as last applied, so a failed start isn't retried
        /// every tick.
        tray_wanted: bool,
        tray_rx: Receiver<Action>,
        tray_tx: Sender<Action>,
        /// What the tray shows now, so it's only touched on a change.
        shown: (PlayState, String),
        player: Player,
    }

    impl Desktop {
        pub fn start(player: Player, tray: bool) -> Self {
            let mpris = match Mpris::start(player.clone(), Abilities { raise: true }) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::info!("no MPRIS (media keys, panel widget): {e}");
                    None
                }
            };
            let (tray_tx, tray_rx) = unbounded();
            let mut d = Self {
                mpris,
                tray: None,
                tray_wanted: tray,
                tray_rx,
                tray_tx,
                shown: (PlayState::Stopped, String::new()),
                player,
            };
            if tray {
                d.start_tray();
            }
            d
        }

        fn start_tray(&mut self) {
            let t = Tray {
                player: self.player.clone(),
                actions: self.tray_tx.clone(),
                state: PlayState::Stopped,
                title: String::new(),
                icons: Icons::new(),
            };
            match t.spawn() {
                Ok(h) => self.tray = Some(h),
                Err(e) => log::info!("no tray icon: {e}"),
            }
        }

        /// Whether the tray icon is up, so closing the window can hide to it.
        pub fn has_tray(&self) -> bool {
            self.tray.as_ref().is_some_and(|h| !h.is_closed())
        }

        /// Turn the tray icon on or off, from Settings.
        pub fn set_tray(&mut self, on: bool) {
            if on == self.tray_wanted {
                return;
            }
            self.tray_wanted = on;
            if on && self.tray.is_none() {
                self.start_tray();
                self.shown = (PlayState::Stopped, String::new());
            } else if !on && let Some(h) = self.tray.take() {
                h.shutdown().wait();
            }
        }

        /// Publish the player's state, and collect what was asked of the window.
        pub fn tick(&mut self, snap: &Snapshot) -> Vec<Action> {
            let mut out = Vec::new();
            if let Some((m, rx)) = &mut self.mpris {
                let (len, next) = {
                    let q = self.player.queue();
                    let next = snap.current.is_some_and(|c| q.upcoming(c).is_some());
                    (q.len(), next)
                };
                m.update(snap, len, next);
                for r in rx.try_iter() {
                    out.push(match r {
                        Request::Raise => Action::Show,
                        Request::Quit => Action::Quit,
                    });
                }
            }
            out.extend(self.tray_rx.try_iter());
            if let Some(h) = &self.tray {
                let title = match &snap.track {
                    Some(t) => match &t.artist {
                        Some(a) => format!("{} · {a}", t.display_title()),
                        None => t.display_title(),
                    },
                    None => String::new(),
                };
                let now = (snap.state, title);
                if now != self.shown {
                    self.shown = now.clone();
                    h.update(|t| {
                        t.state = now.0;
                        t.title = now.1;
                    });
                }
            }
            out
        }
    }

    /// The tray icon. Play, pause, next and back go straight to the player;
    /// showing the window and quitting go back to the UI thread.
    struct Tray {
        player: Player,
        actions: Sender<Action>,
        state: PlayState,
        title: String,
        icons: Icons,
    }

    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "patinaplayer".into()
        }

        fn title(&self) -> String {
            "PatinaPlayer".into()
        }

        fn category(&self) -> ksni::Category {
            ksni::Category::ApplicationStatus
        }

        // A pixmap rather than a theme icon name: hosts prefer the name when
        // they have it, and the name can't show whether it's playing.
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icons.for_state(self.state)
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            let what = match self.state {
                PlayState::Playing => "Playing",
                PlayState::Paused => "Paused",
                PlayState::Stopped => "Stopped",
            };
            ksni::ToolTip {
                title: "PatinaPlayer".into(),
                description: if self.title.is_empty() {
                    what.into()
                } else {
                    format!("{what}: {}", self.title)
                },
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.actions.send(Action::Toggle);
        }

        /// Middle click: play or pause, the way most players do it.
        fn secondary_activate(&mut self, _x: i32, _y: i32) {
            self.player.send(Command::TogglePause);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            let item = |label: &str, f: fn(&mut Self)| -> ksni::MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(f),
                    ..Default::default()
                }
                .into()
            };
            let mut v = Vec::new();
            if !self.title.is_empty() {
                v.push(
                    StandardItem {
                        label: self.title.replace('_', "__"),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                v.push(ksni::MenuItem::Separator);
            }
            v.push(item(
                if self.state == PlayState::Playing {
                    "Pause"
                } else {
                    "Play"
                },
                |t| t.player.send(Command::TogglePause),
            ));
            v.push(item("Next", |t| t.player.send(Command::Next)));
            v.push(item("Back", |t| t.player.send(Command::Back)));
            v.push(ksni::MenuItem::Separator);
            v.push(item("Show PatinaPlayer", |t| {
                let _ = t.actions.send(Action::Show);
            }));
            v.push(item("Quit", |t| {
                let _ = t.actions.send(Action::Quit);
            }));
            v
        }
    }

    /// The app icon at tray sizes, with a play or pause mark in the corner.
    struct Icons {
        base: Vec<image::RgbaImage>,
    }

    const SIZES: [u32; 3] = [22, 32, 64];

    impl Icons {
        fn new() -> Self {
            let src = image::load_from_memory(include_bytes!("../../../assets/patinaplayer.png"))
                .map(|i| i.to_rgba8())
                .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
            Self {
                base: SIZES
                    .iter()
                    .map(|&s| {
                        image::imageops::resize(&src, s, s, image::imageops::FilterType::Lanczos3)
                    })
                    .collect(),
            }
        }

        fn for_state(&self, state: PlayState) -> Vec<ksni::Icon> {
            self.base
                .iter()
                .map(|b| {
                    let mut img = b.clone();
                    mark(&mut img, state);
                    argb(&img)
                })
                .collect()
        }
    }

    /// A small badge in the bottom-right corner: a triangle while playing,
    /// two bars while paused, nothing when stopped.
    fn mark(img: &mut image::RgbaImage, state: PlayState) {
        if state == PlayState::Stopped {
            return;
        }
        let s = img.width() as f64;
        let r = s * 0.24;
        let (cx, cy) = (s - r - 0.5, s - r - 0.5);
        // Copper disc, the app's accent for "something's happening".
        let disc = image::Rgba([0xC8, 0x7F, 0x4A, 0xFF]);
        let ink = image::Rgba([0x10, 0x16, 0x14, 0xFF]);
        for y in 0..img.height() {
            for x in 0..img.width() {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                if d > r {
                    continue;
                }
                let (u, v) = ((px - cx) / r, (py - cy) / r);
                let on = match state {
                    // Triangle pointing right.
                    PlayState::Playing => u > -0.35 && u < 0.5 && v.abs() < (0.5 - u) * 0.62,
                    // Two bars.
                    _ => v.abs() < 0.45 && ((u > -0.42 && u < -0.12) || (u > 0.12 && u < 0.42)),
                };
                img.put_pixel(x, y, if on { ink } else { disc });
            }
        }
    }

    fn argb(img: &image::RgbaImage) -> ksni::Icon {
        let mut data = Vec::with_capacity(img.len());
        for p in img.pixels() {
            let [r, g, b, a] = p.0;
            data.extend_from_slice(&[a, r, g, b]);
        }
        ksni::Icon {
            width: img.width() as i32,
            height: img.height() as i32,
            data,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn icons_carry_the_state() {
            let i = Icons::new();
            let stopped = i.for_state(PlayState::Stopped);
            let playing = i.for_state(PlayState::Playing);
            let paused = i.for_state(PlayState::Paused);
            assert_eq!(stopped.len(), SIZES.len());
            for (k, s) in SIZES.iter().enumerate() {
                assert_eq!(playing[k].width, *s as i32);
                assert_eq!(playing[k].data.len(), (s * s * 4) as usize);
                assert_ne!(playing[k].data, stopped[k].data);
                assert_ne!(playing[k].data, paused[k].data);
            }
        }
    }
}
