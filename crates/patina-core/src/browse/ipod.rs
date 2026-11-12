//! The iPod from the browser's side: noticing one plugged in, its view,
//! Send to iPod from any list, and taking things off.

use std::time::{Duration, Instant};

use super::settings::Control;
use super::{Browser, Item, ListAction, Section, Source, count};
use crate::config;
use crate::ipod::{Contents, Ipod, IpodEvent, Removal, SendSong, Target};
use crate::library::TrackRow;

/// How often mounts are looked at for an iPod.
const CHECK_EVERY: Duration = Duration::from_secs(3);

/// A row in the iPod view.
#[derive(Debug, Clone, PartialEq)]
pub struct IpodRow {
    pub kind: IpodKind,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IpodKind {
    Playlist(String),
    Album { artist: String, album: String },
}

/// What the iPod view is built from.
#[derive(Debug, Clone, Default)]
pub struct IpodView {
    pub mount: Option<std::path::PathBuf>,
    pub contents: Option<Contents>,
    pub progress: Option<(usize, usize, String)>,
    pub problem: Option<String>,
}

impl Browser {
    pub fn ipod_connected(&self) -> bool {
        self.ipod.ipod.is_some()
    }

    /// The iPod's name, for the sidebar.
    pub fn ipod_name(&self) -> Option<String> {
        self.ipod.ipod.as_ref()?;
        Some(
            self.ipod
                .contents
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "iPod".into()),
        )
    }

    pub(super) fn ipod_view(&self) -> IpodView {
        IpodView {
            mount: self.ipod.ipod.as_ref().map(|i| i.mount.clone()),
            contents: self.ipod.contents.clone(),
            progress: self.ipod.progress.clone(),
            problem: self.ipod.problem.clone(),
        }
    }

    /// Look for an iPod now and then, and handle what its thread reports.
    pub(super) fn poll_ipod(&mut self) {
        // Phones have no iPods to plug in.
        if cfg!(target_os = "android") || self.lib.is_none() {
            return;
        }
        let due = self.ipod.checked.is_none_or(|t| t.elapsed() >= CHECK_EVERY);
        if due {
            self.ipod.checked = Some(Instant::now());
            let found = crate::ipod::find();
            let have = self.ipod.ipod.as_ref().map(|i| i.mount.clone());
            match (found.first(), have) {
                (Some((m, dev)), None) => {
                    log::info!("iPod at {}", m.display());
                    self.ipod.ipod = Some(Ipod::open(m.clone(), dev.clone()));
                    self.ipod.problem = None;
                    self.rebuild_sidebar();
                }
                (None, Some(_)) => {
                    self.ipod = Default::default();
                    self.ipod.checked = Some(Instant::now());
                    self.stacks.remove(&Section::Ipod);
                    if self.section == Section::Ipod {
                        self.switch(Section::Folders);
                    }
                    self.rebuild_sidebar();
                }
                _ => {}
            }
        }
        let Some(ip) = &self.ipod.ipod else { return };
        let events: Vec<IpodEvent> = ip.events().try_iter().collect();
        if events.is_empty() {
            return;
        }
        let mut sidebar = false;
        for ev in events {
            match ev {
                IpodEvent::Contents(c) => {
                    sidebar |= self.ipod.contents.as_ref().map(|x| &x.name) != Some(&c.name);
                    self.ipod.contents = Some(c);
                }
                IpodEvent::Progress { done, total, now } => {
                    self.ipod.progress = (done < total).then_some((done, total, now));
                }
                IpodEvent::Done(msg) => {
                    self.ipod.progress = None;
                    self.ipod.problem = None;
                    self.note(msg);
                }
                IpodEvent::Error(e) => {
                    self.ipod.progress = None;
                    self.ipod.problem = Some(e.clone());
                    self.error(format!("iPod: {e}"));
                }
                IpodEvent::Ejected => {
                    self.note("The iPod is safe to unplug");
                    self.ipod = Default::default();
                    // Don't find it again while it's still showing up in
                    // the mounts for a moment.
                    self.ipod.checked = Some(Instant::now() + CHECK_EVERY);
                    self.stacks.remove(&Section::Ipod);
                    if self.section == Section::Ipod {
                        self.switch(Section::Folders);
                    }
                    sidebar = true;
                }
            }
        }
        if sidebar {
            self.rebuild_sidebar();
        }
        self.refresh_section(Section::Ipod);
        self.touch();
    }

    /// A song as the iPod sync needs it, with the file it's in here. Server
    /// songs only go if they're cached or kept offline.
    fn send_song(&self, t: &TrackRow) -> Option<SendSong> {
        let lib = self.lib.as_ref()?;
        let file = if crate::remote::is_remote(&t.path) {
            let r = t.remote()?;
            lib.cached_file(&r.server, &r.id)?
        } else {
            t.path.clone()
        };
        if !file.is_file() {
            return None;
        }
        let artist = t.artist.clone().unwrap_or_default();
        Some(SendSong {
            file,
            title: t.title.clone(),
            artist: artist.clone(),
            album_artist: t.album_artist.clone().unwrap_or_default(),
            album: t.album.clone().unwrap_or_default(),
            genre: t.genre.clone().unwrap_or_default(),
            year: t.year.unwrap_or(0),
            track_no: t.track_no.unwrap_or(0) as i32,
            disc_no: t.disc_no.unwrap_or(0) as i32,
            duration_ms: t.duration.map(|d| d.as_millis() as i32).unwrap_or(0),
            codec: t.codec.clone(),
            lossless: t.lossless,
            sample_rate: t.sample_rate,
            bits: t.bits,
            kbps: t.kbps.unwrap_or(0),
            art_from: t.path.clone(),
        })
    }

    fn send_tracks(&mut self, tracks: Vec<TrackRow>, playlist: Option<String>) {
        let Some(ip) = &self.ipod.ipod else {
            return self
                .note("Plug in an iPod first. It shows up in the sidebar once it's mounted.");
        };
        if self.ipod.progress.is_some() {
            return self.note("The iPod is busy sending; wait for it to finish, or stop it");
        }
        let n = tracks.len();
        let songs: Vec<SendSong> = tracks.iter().filter_map(|t| self.send_song(t)).collect();
        let left = n - songs.len();
        if songs.is_empty() {
            return self.note(if n == 0 {
                "Nothing there to send".to_string()
            } else {
                "None of those are here to copy: server songs have to be cached or kept offline first".to_string()
            });
        }
        let m = songs.len();
        ip.send_songs(songs, playlist, self.config.ipod.target);
        self.ipod.progress = Some((0, m, String::new()));
        let mut msg = format!("Sending {} to the iPod", count(m));
        if left > 0 {
            msg.push_str(&format!(
                ". {left} on a server and not kept here are left out; Keep offline gets them"
            ));
        }
        self.note(msg);
        self.refresh_section(Section::Ipod);
    }

    /// Send the marked rows, or the selected one, to the iPod. A playlist
    /// (or smart playlist, or saved mix) on its own goes as a playlist of
    /// the same name.
    pub fn send_selection_to_ipod(&mut self) {
        let items = self.selection_items();
        let Some(lib) = &self.lib else { return };
        let playlist = match items.as_slice() {
            [Item::Playlist(p)] => Some(p.name.clone()),
            [Item::Smart(p)] => Some(p.name.clone()),
            [Item::Mix(m)] => Some(m.name.clone()),
            _ => None,
        };
        let tracks: Vec<TrackRow> = items
            .iter()
            .flat_map(|i| super::tracks_of(lib, i, &self.prefs))
            .collect();
        self.send_tracks(tracks, playlist);
    }

    /// Send the open list (an album, a playlist) to the iPod. A playlist
    /// goes as a playlist.
    pub fn send_view_to_ipod(&mut self) {
        let playlist = match self.level().map(|l| &l.source) {
            Some(Source::Playlist { name, .. } | Source::Smart { name, .. }) => Some(name.clone()),
            _ => None,
        };
        let tracks = self.view_tracks();
        self.send_tracks(tracks, playlist);
    }

    /// Everything starred in Favorites.
    pub fn send_favorites_to_ipod(&mut self) {
        let Some(lib) = &self.lib else { return };
        let mut tracks = Vec::new();
        for f in self.favorites() {
            if let Some(item) = self.favorite_item(&f) {
                tracks.extend(super::tracks_of(lib, &item, &self.prefs));
            }
        }
        if tracks.is_empty() {
            return self.note("Star a folder, album or artist to put it in Favorites first");
        }
        self.send_tracks(tracks, None);
    }

    /// Take a playlist or an album in the iPod view off the iPod.
    pub fn remove_from_ipod(&mut self, row: &IpodRow) {
        let Some(ip) = &self.ipod.ipod else { return };
        if self.ipod.progress.is_some() {
            return self.note("The iPod is busy sending; wait for it to finish");
        }
        ip.remove(match &row.kind {
            IpodKind::Playlist(n) => Removal::Playlist(n.clone()),
            IpodKind::Album { artist, album } => Removal::Album {
                artist: artist.clone(),
                album: album.clone(),
            },
        });
    }

    pub(super) fn ipod_action(&mut self, a: ListAction) {
        let Some(ip) = &self.ipod.ipod else { return };
        match a {
            ListAction::IpodFavorites => self.send_favorites_to_ipod(),
            ListAction::IpodEject => {
                if self.ipod.progress.is_some() {
                    ip.stop();
                }
                ip.eject();
                self.note("Ejecting the iPod…");
            }
            ListAction::IpodStop => {
                ip.stop();
                self.note("Stopping after the song it's on");
            }
            ListAction::IpodRefresh => ip.scan(),
            _ => {}
        }
    }

    pub(super) fn change_ipod_setting(&mut self, new: &Control) -> Result<(), String> {
        if let Control::Choice { options, index } = new {
            let t: Target = options[*index].0.parse().unwrap_or_default();
            config::set_value(&self.config_path, "ipod", "target", t.as_str())
                .map_err(|e| e.to_string())?;
            self.config.ipod.target = t;
        }
        Ok(())
    }
}

/// Rows for the iPod view.
pub(super) fn rows(v: &IpodView, items: &mut Vec<Item>) {
    let Some(mount) = &v.mount else {
        items.push(Item::Info {
            label: "No iPod".into(),
            value: "plug one in. It shows up here once it's mounted.".into(),
        });
        return;
    };
    items.push(Item::Header("iPod".into()));
    match &v.contents {
        Some(c) => {
            items.push(Item::Info {
                label: c.name.clone(),
                value: format!(
                    "{} · {} free of {} · {}",
                    super::model::songs(c.songs),
                    super::fmt_bytes(c.free),
                    super::fmt_bytes(c.total),
                    mount.display()
                ),
            });
        }
        None => items.push(Item::Info {
            label: "Reading it".into(),
            value: mount.display().to_string(),
        }),
    }
    if let Some((done, total, now)) = &v.progress {
        items.push(Item::Info {
            label: "Sending".into(),
            value: if now.is_empty() {
                format!("{done} of {total}")
            } else {
                format!("{done} of {total} · {now}")
            },
        });
        items.push(Item::Action {
            action: ListAction::IpodStop,
            label: "Stop sending".into(),
        });
    } else {
        items.push(Item::Action {
            action: ListAction::IpodFavorites,
            label: "Send Favorites".into(),
        });
    }
    if let Some(p) = &v.problem {
        items.push(Item::Info {
            label: "Problem".into(),
            value: p.clone(),
        });
    }
    items.push(Item::Action {
        action: ListAction::IpodRefresh,
        label: "Read it again".into(),
    });
    items.push(Item::Action {
        action: ListAction::IpodEject,
        label: "Eject".into(),
    });
    let Some(c) = &v.contents else { return };
    if !c.playlists.is_empty() {
        items.push(Item::Header("Playlists on the iPod".into()));
        for (name, n) in &c.playlists {
            items.push(Item::Ipod(IpodRow {
                kind: IpodKind::Playlist(name.clone()),
                label: name.clone(),
                detail: super::model::songs(*n),
            }));
        }
    }
    if !c.albums.is_empty() {
        items.push(Item::Header("Albums on the iPod".into()));
        for (artist, album, n, len) in &c.albums {
            items.push(Item::Ipod(IpodRow {
                kind: IpodKind::Album {
                    artist: artist.clone(),
                    album: album.clone(),
                },
                label: if album.is_empty() {
                    "(no album)".into()
                } else {
                    album.clone()
                },
                detail: format!(
                    "{} · {} · {}",
                    if artist.is_empty() {
                        "Unknown artist"
                    } else {
                        artist
                    },
                    super::model::songs(*n),
                    super::fmt_length(*len)
                ),
            }));
        }
    }
}
