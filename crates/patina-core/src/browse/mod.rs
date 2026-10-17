//! Browsing and every action you can take from a view, shared by the TUI
//! and the GUI. Frontends turn keys and clicks into calls on [`Browser`]
//! and draw what it holds; neither one decides what a key does to the
//! library, the queue or the config. That's how the two stay identical.

pub mod amp;
mod downloads;
pub mod eq;
pub mod lastfm;
mod leveling;
pub mod marks;
mod mixes;
pub mod model;
pub mod offline;
mod servers;
pub mod settings;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::audio::output::BitPerfect;
use crate::audio::output::exclusive::{self, HwDevice};
use crate::config::{self, Config, Station};
use crate::library::podcasts::EpisodeRow;
use crate::library::{
    ArtistMode, HEARTED, LibEvent, Library, LibraryService, PlaylistRow, TrackRow,
};
use crate::podcast::{PodEvent, PodcastService};
use crate::remote::{RemoteService, ServerState, ServerStatus};
use crate::{Command, Event, Player, TrackInfo};

pub use model::*;
pub use settings::{Change, Control, Key, Setting, SettingsInfo};

/// A message for the person, shown however the frontend shows them.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    Info(String),
    Error(String),
}

/// A row in the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub enum SideEntry {
    Section(Section),
    /// A library folder root. `present` is false while it's missing (an
    /// unmounted drive), checked every few seconds rather than on every
    /// draw, since a stat on a dead network mount can hang.
    Source {
        path: PathBuf,
        tracks: i64,
        present: bool,
        /// A music server's state; None for a local folder.
        server: Option<ServerState>,
    },
    /// A saved mix. Opening it plays it.
    Mix {
        id: i64,
        name: String,
    },
    /// A starred folder, album or artist. Opening it opens that.
    Favorite {
        id: i64,
        label: String,
        kind: crate::library::marks::FavKind,
    },
    Label(&'static str),
    Gap,
}

/// What [`Browser::activate`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activated {
    Nothing,
    Opened,
    Played,
}

/// Everything the browser needs at startup.
pub struct Startup {
    pub player: Player,
    pub lib: Option<Library>,
    pub service: Option<LibraryService>,
    pub pod: Option<PodcastService>,
    /// Music servers. None without the library.
    pub remote: Option<RemoteService>,
    /// The song cache and download queue. None without the library.
    pub cache: Option<crate::cache::CacheService>,
    /// Genre, artist and track mixes. None without the library.
    pub mixes: Option<crate::mix::MixService>,
    /// Loudness measuring for leveling. None without the library.
    pub level: Option<crate::level::LevelService>,
    /// Last.fm. None without the library, which holds the scrobble queue.
    pub scrobble: Option<crate::scrobble::ScrobbleService>,
    /// The config as loaded. Settings edits it, and the file, together.
    pub config: Config,
    pub settings_info: SettingsInfo,
}

pub struct Browser {
    pub player: Player,
    pub lib: Option<Library>,
    pub service: Option<LibraryService>,
    pub pod: Option<PodcastService>,
    pub remote: Option<RemoteService>,
    pub cache: Option<crate::cache::CacheService>,
    pub mixes: Option<crate::mix::MixService>,
    pub level: Option<crate::level::LevelService>,
    pub scrobble: Option<crate::scrobble::ScrobbleService>,
    /// Whether the song playing is loved. None for no song, or radio.
    pub loved_now: Option<bool>,
    /// Love was clicked on this song, so Last.fm's earlier answer is stale.
    love_by_hand: bool,
    radio_song: Option<lastfm::RadioSong>,
    /// The EQ: what plays, presets, device links.
    pub eq: eq::EqState,
    pub amp: amp::AmpState,
    /// The songs last handed to the measuring thread.
    sent_ahead: Vec<Vec<u8>>,
    /// The mix playing, if one is.
    pub mix: Option<mixes::MixRun>,
    mix_runs: u64,
    /// Genres marked for a genre mix.
    pub genre_marks: Vec<crate::library::GenreRow>,
    /// The cache as of the last poll, for Downloads.
    pub cache_status: crate::cache::Status,
    /// The songs last handed to prefetch, so the same list isn't sent
    /// again on every queue event.
    sent_upcoming: Vec<crate::cache::SongKey>,
    /// What can play while a server is unreachable.
    pub avail: offline::Availability,
    /// Something changed that may change what greys out.
    avail_dirty: bool,
    /// The servers' status as of the last poll.
    pub servers: Vec<ServerStatus>,
    /// Server changes waiting to be redrawn, at most a couple a second
    /// while a big sync runs.
    servers_changed: Option<Instant>,
    queue_sync: servers::QueueSync,
    /// Where "Find DLNA servers" asks. The local network when unset.
    pub dlna_probe: Option<std::net::SocketAddr>,
    dlna: servers::DlnaSearch,
    pub section: Section,
    /// Drill-down stack per section, so switching away and back returns
    /// you to where you were.
    pub stacks: HashMap<Section, Vec<Level>>,
    /// Levels backed out of, per section, for a Forward button.
    forward: HashMap<Section, Vec<Source>>,
    pub prefs: ViewPrefs,
    pub config: Config,
    pub settings_info: SettingsInfo,
    /// Sound cards for the bit-perfect picker, looked up when Settings opens.
    devices: Vec<HwDevice>,
    /// Devices the shared output can play to, as last looked up.
    outputs: Vec<crate::audio::output::OutDevice>,
    pub sidebar: Vec<SideEntry>,
    pub sidebar_sel: usize,
    pub view_error: Option<String>,
    pub config_path: PathBuf,
    pub stations: Vec<Station>,
    pub feeds: Vec<String>,
    /// Podcast speed and trimming as last set. The player's snapshot lags a
    /// tick behind, so repeated changes build on these.
    pub podcast_tempo: f64,
    pub podcast_trim: bool,
    /// Station test results by URL.
    pub checks: HashMap<String, String>,
    check_tx: Sender<(String, String)>,
    check_rx: Receiver<(String, String)>,
    notes: Vec<Note>,
    /// Bumped whenever anything a view draws changes, so a frontend that
    /// keeps its own models (the GUI) knows when to rebuild them.
    generation: u64,
    /// When the library roots were last checked for being there.
    roots_checked: Instant,
    /// The playing song's rating as last read, by its path.
    playing_rating: Option<(PathBuf, Option<u8>)>,
    lyrics: Option<marks::LyricsView>,
    lyrics_job: Option<marks::LyricsJob>,
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "Added 12 songs to \"Mix\"", with any left out because they were
/// already there.
pub fn added_note(added: usize, asked: usize, name: &str) -> String {
    let skipped = asked.saturating_sub(added);
    match (added, skipped) {
        (0, 1) => format!("That track is already in \"{name}\""),
        (0, _) => format!("All {} are already in \"{name}\"", count(asked)),
        (_, 0) => format!("Added {} to \"{name}\"", count(added)),
        _ => format!(
            "Added {} to \"{name}\", left out {skipped} already in it",
            count(added)
        ),
    }
}

/// Said when you try to play something whose server is away and that
/// isn't kept here.
const UNREACHABLE: &str = "Its server can't be reached right now, and it isn't kept here. Keep offline saves it for times like this.";

pub fn count(n: usize) -> String {
    format!("{n} track{}", plural(n))
}

impl Browser {
    pub fn new(s: Startup) -> Self {
        let (check_tx, check_rx) = unbounded();
        let mut b = Self {
            player: s.player,
            lib: s.lib,
            service: s.service,
            pod: s.pod,
            remote: s.remote,
            cache_status: s.cache.as_ref().map(|c| c.status()).unwrap_or_default(),
            cache: s.cache,
            mixes: s.mixes,
            level: s.level,
            scrobble: s.scrobble,
            loved_now: None,
            love_by_hand: false,
            radio_song: None,
            eq: eq::EqState::new(
                s.config.eq.clone(),
                crate::eq::presets::Presets::new(
                    s.settings_info
                        .config_path
                        .parent()
                        .map(|d| d.join("presets").join("eq"))
                        .unwrap_or_else(crate::eq::presets::dir),
                ),
            ),
            amp: amp::AmpState::new(
                s.config.amp.clone(),
                crate::amp::Presets::new(
                    s.settings_info
                        .config_path
                        .parent()
                        .map(|d| d.join("presets").join("amp"))
                        .unwrap_or_else(crate::amp::dir),
                ),
            ),
            sent_ahead: Vec::new(),
            mix: None,
            mix_runs: 0,
            genre_marks: Vec::new(),
            sent_upcoming: Vec::new(),
            avail: offline::Availability::default(),
            avail_dirty: false,
            servers: Vec::new(),
            servers_changed: None,
            queue_sync: servers::QueueSync::default(),
            dlna_probe: None,
            dlna: servers::DlnaSearch::default(),
            section: Section::Folders,
            stacks: HashMap::new(),
            forward: HashMap::new(),
            prefs: ViewPrefs::default(),
            sidebar: Vec::new(),
            sidebar_sel: 0,
            view_error: None,
            config_path: s.settings_info.config_path.clone(),
            stations: s.config.stations.clone(),
            feeds: s.config.podcasts.feeds.clone(),
            podcast_tempo: s.config.podcasts.speed,
            podcast_trim: s.config.podcasts.trim_silence,
            config: s.config,
            settings_info: s.settings_info,
            devices: Vec::new(),
            outputs: Vec::new(),
            checks: HashMap::new(),
            check_tx,
            check_rx,
            notes: Vec::new(),
            generation: 0,
            roots_checked: Instant::now(),
            playing_rating: None,
            lyrics: None,
            lyrics_job: None,
        };
        b.rebuild_sidebar();
        b
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn touch(&mut self) {
        self.generation += 1;
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.notes.push(Note::Info(msg.into()));
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.notes.push(Note::Error(msg.into()));
    }

    /// Messages since the last call.
    pub fn take_notes(&mut self) -> Vec<Note> {
        std::mem::take(&mut self.notes)
    }

    /// Handle everything the background threads reported. Returns the
    /// player's events, which frontends may want too (track changes).
    pub fn poll(&mut self) -> Vec<Event> {
        let events: Vec<Event> = self.player.events().try_iter().collect();
        let mut queue_moved = false;
        for ev in &events {
            if let Event::Error(e) = ev {
                self.error(e.clone());
            }
            if matches!(ev, Event::QueueChanged | Event::TrackChanged(_)) {
                self.touch();
                queue_moved = true;
            }
        }
        self.lastfm_events(&events);
        self.poll_lastfm();
        self.poll_mixes();
        if queue_moved {
            self.feed_prefetch();
            self.feed_level();
            self.mix_tick();
        }
        self.poll_cache();
        self.poll_level();
        self.eq_tick();
        self.amp_tick();
        self.poll_lyrics();
        if events.iter().any(|e| matches!(e, Event::Played(_))) {
            for s in Section::STATS {
                self.refresh_section(s);
            }
            // The open list shows play counts too (an album's songs, the
            // Albums and Songs lists): it shows the new one now rather
            // than the next time the app opens.
            if matches!(
                self.section,
                Section::Folders
                    | Section::Artists
                    | Section::Albums
                    | Section::Songs
                    | Section::Genres
                    | Section::Playlists
            ) {
                self.refresh_current();
            }
        }
        let mut changed = false;
        if let Some(svc) = &self.service {
            let evs: Vec<LibEvent> = svc.events().try_iter().collect();
            for ev in evs {
                match ev {
                    LibEvent::Changed(_) => changed = true,
                    LibEvent::Error(e) => self.notes.push(Note::Error(e)),
                    LibEvent::Progress(_) | LibEvent::Ready => self.generation += 1,
                }
            }
        }
        let mut pods_changed = false;
        if let Some(pod) = &self.pod {
            let evs: Vec<PodEvent> = pod.events().try_iter().collect();
            for ev in evs {
                match ev {
                    PodEvent::Updated => pods_changed = true,
                    PodEvent::Error(e) => self.notes.push(Note::Error(e)),
                }
            }
        }
        while let Ok((url, summary)) = self.check_rx.try_recv() {
            self.checks.insert(url, summary.clone());
            self.note(summary);
            self.refresh_section(Section::Radio);
        }
        if self.roots_checked.elapsed() >= Duration::from_secs(5) {
            self.roots_checked = Instant::now();
            let mut flipped = false;
            for e in &mut self.sidebar {
                if let SideEntry::Source {
                    path,
                    present,
                    server: None,
                    ..
                } = e
                {
                    let now = path.is_dir();
                    flipped |= now != *present;
                    *present = now;
                }
            }
            if flipped {
                self.touch();
            }
        }
        let synced = self.poll_servers();
        if synced && let Some(c) = &self.cache {
            // Pins may stand for more songs now.
            c.resync();
        }
        changed |= synced;
        if changed {
            self.avail_dirty = true;
            self.refresh_all();
            self.rebuild_sidebar();
        } else if pods_changed {
            self.refresh_section(Section::Podcasts);
            self.refresh_section(Section::Downloads);
        }
        if self.avail_dirty {
            self.avail_dirty = false;
            self.update_availability();
        }
        events
    }

    // ----- Sidebar -----

    pub fn rebuild_sidebar(&mut self) {
        let mut s: Vec<SideEntry> = Section::NUMBERED
            .iter()
            .map(|&x| SideEntry::Section(x))
            .collect();
        if self.lib.is_some() {
            s.push(SideEntry::Gap);
            s.push(SideEntry::Label("Listening"));
            s.extend(Section::STATS.iter().map(|&x| SideEntry::Section(x)));
            let favs = self.favorites();
            if !favs.is_empty() {
                s.push(SideEntry::Gap);
                s.push(SideEntry::Label("Favorites"));
                s.extend(favs.into_iter().map(|f| SideEntry::Favorite {
                    id: f.id,
                    label: f.label,
                    kind: f.kind,
                }));
            }
            let saved = self.saved_mixes();
            if !saved.is_empty() {
                s.push(SideEntry::Gap);
                s.push(SideEntry::Label("Mixes"));
                s.extend(saved.into_iter().map(|m| SideEntry::Mix {
                    id: m.id,
                    name: m.name,
                }));
            }
        }
        let sources = self
            .lib
            .as_ref()
            .and_then(|l| l.sources().ok())
            .unwrap_or_default();
        let mut entries: Vec<SideEntry> = sources
            .iter()
            .filter(|src| !crate::remote::is_remote(&src.root))
            .map(|src| SideEntry::Source {
                present: src.root.is_dir(),
                path: src.root.clone(),
                tracks: src.tracks,
                server: None,
            })
            .collect();
        // Servers as configured, synced yet or not.
        if self.lib.is_some() {
            for sc in &self.config.servers {
                let root = crate::remote::root(&sc.name);
                entries.push(SideEntry::Source {
                    tracks: sources
                        .iter()
                        .find(|x| x.root == root)
                        .map(|x| x.tracks)
                        .unwrap_or(0),
                    path: root,
                    present: true,
                    server: Some(
                        self.servers
                            .iter()
                            .find(|x| x.name == sc.name)
                            .map(|x| x.state.clone())
                            .unwrap_or(ServerState::Connecting),
                    ),
                });
            }
        }
        if !entries.is_empty() {
            s.push(SideEntry::Gap);
            s.push(SideEntry::Label("Sources"));
            s.extend(entries);
        }
        s.push(SideEntry::Gap);
        s.push(SideEntry::Section(Section::Eq));
        s.push(SideEntry::Section(Section::Amp));
        s.push(SideEntry::Section(Section::Settings));
        self.sidebar = s;
        self.sync_sidebar_sel();
        self.touch();
    }

    fn sync_sidebar_sel(&mut self) {
        if let Some(i) = self
            .sidebar
            .iter()
            .position(|e| *e == SideEntry::Section(self.section))
        {
            self.sidebar_sel = i;
        }
    }

    /// Move the sidebar selection to the next selectable entry, opening it.
    pub fn sidebar_move(&mut self, d: isize) {
        let n = self.sidebar.len() as isize;
        let mut i = self.sidebar_sel as isize;
        loop {
            i += d;
            if !(0..n).contains(&i) {
                return;
            }
            if matches!(
                self.sidebar[i as usize],
                SideEntry::Section(_)
                    | SideEntry::Source { .. }
                    | SideEntry::Mix { .. }
                    | SideEntry::Favorite { .. }
            ) {
                break;
            }
        }
        // Moving onto a saved mix or a favorite selects it; Enter opens
        // it. (A favorite moved by J and K stays put this way.)
        if matches!(
            self.sidebar[i as usize],
            SideEntry::Mix { .. } | SideEntry::Favorite { .. }
        ) {
            self.sidebar_sel = i as usize;
            self.touch();
            return;
        }
        self.activate_sidebar(i as usize);
    }

    /// Open a sidebar entry. A source opens its folder tree under Folders.
    pub fn activate_sidebar(&mut self, index: usize) {
        match self.sidebar.get(index).cloned() {
            Some(SideEntry::Section(s)) => self.switch(s),
            Some(SideEntry::Source { path, .. }) => {
                self.switch(Section::Folders);
                if let Some(stack) = self.stacks.get_mut(&Section::Folders) {
                    stack.truncate(1);
                }
                self.push(Source::Folder(path));
                self.sidebar_sel = index;
            }
            Some(SideEntry::Mix { id, .. }) => {
                self.sidebar_sel = index;
                self.play_saved_mix(id);
            }
            Some(SideEntry::Favorite { id, .. }) => {
                if let Some(f) = self.favorites().into_iter().find(|f| f.id == id) {
                    self.open_favorite(&f);
                }
                self.sidebar_sel = index;
            }
            _ => return,
        }
        self.touch();
    }

    // ----- Levels -----

    pub fn switch(&mut self, s: Section) {
        self.section = s;
        self.view_error = None;
        if s == Section::Settings {
            // Cards come and go (a USB DAC); look again each time.
            self.devices = exclusive::list_devices();
            self.refresh_outputs();
            self.refresh_section(Section::Settings);
        }
        if s != Section::Queue && !self.stacks.contains_key(&s) {
            let level = self.build(&Source::Section(s));
            self.stacks.insert(s, level.into_iter().collect());
        }
        self.sync_sidebar_sel();
        self.touch();
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            lib: self.lib.as_ref(),
            prefs: &self.prefs,
            settings: SettingsState {
                config: &self.config,
                info: &self.settings_info,
                devices: &self.devices,
                podcast_tempo: self.podcast_tempo,
                podcast_trim: self.podcast_trim,
                measuring: self.measuring_library(),
                lastfm: self.scrobble.as_ref().map(|s| s.status()),
                outputs: &self.outputs,
            },
            stations: &self.stations,
            checks: &self.checks,
            servers: &self.servers,
            cache: self.cache.as_ref().map(|_| &self.cache_status),
            eq: &self.eq,
            amp: &self.amp,
        }
    }

    fn build(&mut self, src: &Source) -> Option<Level> {
        match build(&self.ctx(), src) {
            Ok((title, items)) => {
                let mut l = Level {
                    title,
                    source: src.clone(),
                    items,
                    selected: 0,
                    offset: 0,
                };
                // Start on content, not on a button like "Pause downloads"
                // that Enter would press by accident.
                l.selected = l
                    .items
                    .iter()
                    .position(|i| i.selectable() && !matches!(i, Item::Action { .. }))
                    .unwrap_or_else(|| l.snap(0, 1));
                Some(l)
            }
            Err(e) => {
                self.view_error = Some(e);
                None
            }
        }
    }

    pub fn level(&self) -> Option<&Level> {
        self.stacks.get(&self.section).and_then(|s| s.last())
    }

    pub fn level_mut(&mut self) -> Option<&mut Level> {
        self.generation += 1;
        self.stacks
            .get_mut(&self.section)
            .and_then(|s| s.last_mut())
    }

    /// The open level, to move its selection. Unlike [`Self::level_mut`]
    /// this doesn't count as a change: a new selection is not new rows, and
    /// the GUI rebuilding every row on each click made big lists stall.
    /// Frontends show the selection from [`Level::selected`] themselves.
    fn level_sel(&mut self) -> Option<&mut Level> {
        self.stacks
            .get_mut(&self.section)
            .and_then(|s| s.last_mut())
    }

    pub fn depth(&self) -> usize {
        self.stacks.get(&self.section).map(|s| s.len()).unwrap_or(0)
    }

    /// Breadcrumb titles for the open stack.
    pub fn crumbs(&self) -> Vec<String> {
        self.stacks
            .get(&self.section)
            .map(|s| s.iter().map(|l| l.title.clone()).collect())
            .unwrap_or_default()
    }

    pub fn push(&mut self, src: Source) {
        if let Some(l) = self.build(&src) {
            self.stacks.entry(self.section).or_default().push(l);
            self.forward.remove(&self.section);
            self.touch();
        }
    }

    /// Back up one level. False if already at the top.
    pub fn pop(&mut self) -> bool {
        if let Some(stack) = self.stacks.get_mut(&self.section)
            && stack.len() > 1
        {
            let l = stack.pop().unwrap();
            // A search isn't worth going forward to; everything else is.
            if !matches!(l.source, Source::Search(_)) {
                self.forward.entry(self.section).or_default().push(l.source);
            }
            self.touch();
            return true;
        }
        false
    }

    pub fn can_go_forward(&self) -> bool {
        self.forward
            .get(&self.section)
            .is_some_and(|f| !f.is_empty())
    }

    /// Re-open the level last backed out of.
    pub fn go_forward(&mut self) -> bool {
        let Some(src) = self.forward.get_mut(&self.section).and_then(|f| f.pop()) else {
            return false;
        };
        let rest = self.forward.remove(&self.section);
        if let Some(l) = self.build(&src) {
            self.stacks.entry(self.section).or_default().push(l);
            self.touch();
        }
        if let Some(rest) = rest {
            self.forward.insert(self.section, rest);
        }
        true
    }

    /// Back up to a breadcrumb.
    pub fn pop_to(&mut self, depth: usize) {
        if let Some(stack) = self.stacks.get_mut(&self.section)
            && depth >= 1
        {
            stack.truncate(depth);
            self.touch();
        }
    }

    /// Re-run the query behind one level, keeping the selection on the
    /// same row if it's still there.
    fn rebuild_level(&mut self, section: Section, idx: usize) {
        let Some(level) = self.stacks.get(&section).and_then(|s| s.get(idx)) else {
            return;
        };
        let src = level.source.clone();
        let key = level.selected_item().map(Item::key);
        let (old_sel, old_off) = (level.selected, level.offset);
        let built = build(&self.ctx(), &src);
        let Some(level) = self.stacks.get_mut(&section).and_then(|s| s.get_mut(idx)) else {
            return;
        };
        match built {
            Ok((title, items)) => {
                level.title = title;
                level.items = items;
                level.selected = key
                    .and_then(|k| level.items.iter().position(|i| i.key() == k))
                    .unwrap_or(old_sel);
                level.selected = level.snap(level.selected, 1);
                level.offset = old_off;
            }
            Err(e) => self.view_error = Some(e),
        }
        self.touch();
    }

    pub fn refresh_current(&mut self) {
        if let Some(n) = self.stacks.get(&self.section).map(|s| s.len())
            && n > 0
        {
            self.rebuild_level(self.section, n - 1);
        }
    }

    pub fn refresh_section(&mut self, s: Section) {
        if let Some(n) = self.stacks.get(&s).map(|v| v.len()) {
            for i in 0..n {
                self.rebuild_level(s, i);
            }
        }
    }

    pub fn refresh_all(&mut self) {
        let sections: Vec<Section> = self.stacks.keys().copied().collect();
        for s in sections {
            let n = self.stacks[&s].len();
            if n == 0 {
                // A section that failed to open (no library yet) may work now.
                self.stacks.remove(&s);
                continue;
            }
            for i in 0..n {
                self.rebuild_level(s, i);
            }
        }
        if !self.stacks.contains_key(&self.section) {
            self.switch(self.section);
        }
    }

    // ----- Selection -----

    pub fn selected_item(&self) -> Option<Item> {
        self.level().and_then(|l| l.selected_item().cloned())
    }

    /// Select a row by index, if it can be selected.
    pub fn select(&mut self, index: usize) -> bool {
        if let Some(l) = self.level_sel()
            && index < l.items.len()
            && l.items[index].selectable()
        {
            l.selected = index;
            return true;
        }
        false
    }

    pub fn move_selection(&mut self, delta: isize) {
        if let Some(l) = self.level_sel() {
            if l.items.is_empty() {
                return;
            }
            let target =
                (l.selected as isize + delta).clamp(0, l.items.len() as isize - 1) as usize;
            l.selected = l.snap(target, if delta < 0 { -1 } else { 1 });
        }
    }

    fn selection_tracks(&self) -> Vec<TrackRow> {
        match (&self.lib, self.selected_item()) {
            (Some(lib), Some(item)) => tracks_of(lib, &item, &self.prefs),
            _ => Vec::new(),
        }
    }

    /// Enter on the selected row: open it, or play it in context.
    pub fn activate(&mut self) -> Activated {
        let Some(item) = self.selected_item() else {
            return Activated::Nothing;
        };
        if let Some(src) = open(&item) {
            self.push(src);
            return Activated::Opened;
        }
        if matches!(
            item,
            Item::Station { .. } | Item::Episode(_) | Item::Track(_)
        ) && self.section != Section::Queue
        {
            self.end_mix();
        }
        let play = |b: &Self, tracks: Vec<TrackInfo>, start: usize, pos: Duration| {
            b.player
                .send(Command::SetQueueTracks { tracks, start, pos });
        };
        match &item {
            Item::Station { station, .. } => {
                play(self, vec![station_info(station)], 0, Duration::ZERO);
                Activated::Played
            }
            Item::Episode(e) => {
                // Fresh from the database, so the resume point is current.
                let e = self
                    .lib
                    .as_ref()
                    .and_then(|l| l.episode(e.id).ok().flatten())
                    .unwrap_or_else(|| e.clone());
                play(self, vec![e.to_info()], 0, e.resume_at());
                Activated::Played
            }
            Item::Track(t) => {
                if !self.avail.plays(&item, self.prefs.artist_mode) {
                    self.note(UNREACHABLE);
                    return Activated::Nothing;
                }
                // Play the whole list this track sits in, starting here, the
                // way picking a song in an album plays the album from there.
                // Songs that can't play right now (a server away, not kept)
                // are left out.
                let Some(level) = self.level() else {
                    return Activated::Nothing;
                };
                let tracks: Vec<TrackRow> = level
                    .items
                    .iter()
                    .filter(|i| self.avail.plays(i, self.prefs.artist_mode))
                    .filter_map(|i| match i {
                        Item::Track(t) => Some(t.clone()),
                        _ => None,
                    })
                    .collect();
                let start = tracks.iter().position(|x| x.path == t.path).unwrap_or(0);
                play(
                    self,
                    tracks.iter().map(TrackRow::to_info).collect(),
                    start,
                    Duration::ZERO,
                );
                Activated::Played
            }
            Item::Action { action, .. } => {
                self.list_action(*action);
                Activated::Nothing
            }
            Item::Mix(m) => {
                self.play_saved_mix(m.id);
                Activated::Played
            }
            _ => Activated::Nothing,
        }
    }

    // ----- Whole view -----

    /// Whether the open view has a "play all": anything built from the
    /// library, but not stations, podcasts, or lists of lists (genres,
    /// playlists) where the same song would come up more than once.
    pub fn can_play_view(&self) -> bool {
        let Some(level) = self.level() else {
            return false;
        };
        let playable = !matches!(
            level.source,
            Source::Section(
                Section::Radio
                    | Section::Podcasts
                    | Section::Queue
                    | Section::Downloads
                    | Section::Settings
                    | Section::Eq
                    | Section::Amp
                    | Section::Genres
                    | Section::Playlists
            ) | Source::Podcast { .. }
        );
        playable && self.lib.is_some() && level.items.iter().any(Item::selectable)
    }

    /// Everything the open view stands for, in view order: a folder's
    /// whole tree, every album in a list, an artist's albums one after
    /// another. In a grouped list (search, Most listened) only the songs
    /// count, since the albums and artists there overlap them.
    pub fn view_tracks(&self) -> Vec<TrackRow> {
        let (Some(lib), Some(level)) = (&self.lib, self.level()) else {
            return Vec::new();
        };
        if !self.can_play_view() {
            return Vec::new();
        }
        // One query for a folder, however many subfolders it has.
        if let Source::Folder(dir) = &level.source {
            return lib.tracks_under(dir).unwrap_or_default();
        }
        let grouped = level.items.iter().any(|i| matches!(i, Item::Header(_)));
        level
            .items
            .iter()
            .filter(|i| !grouped || matches!(i, Item::Track(_)))
            .flat_map(|i| tracks_of(lib, i, &self.prefs))
            .collect()
    }

    /// Play the whole view from the top, in order or shuffled. Shuffle is
    /// left on or off to match, so the shuffle button flips between the two
    /// afterwards.
    pub fn play_view(&mut self, shuffle: bool) {
        let tracks = self.playable(self.view_tracks().iter().map(TrackRow::to_info).collect());
        if tracks.is_empty() {
            return self.note("Nothing here to play");
        }
        let n = tracks.len();
        self.end_mix();
        self.player.send(Command::PlayAll { tracks, shuffle });
        self.note(format!(
            "{} {}",
            if shuffle { "Shuffling" } else { "Playing" },
            count(n)
        ));
    }

    /// Add the whole view to the end of the queue.
    pub fn queue_view(&mut self) {
        let tracks = self.playable(self.view_tracks().iter().map(TrackRow::to_info).collect());
        if tracks.is_empty() {
            return self.note("Nothing here to add");
        }
        let n = tracks.len();
        self.player.send(Command::AppendTracks(tracks));
        self.note(format!("Added {} to the queue", count(n)));
    }

    /// Play the selected item by itself, even one Enter would open (an
    /// album, a folder, an artist). Songs, stations and episodes play the
    /// way Enter plays them.
    pub fn play_selection(&mut self) -> Activated {
        let Some(item) = self.selected_item() else {
            return Activated::Nothing;
        };
        if matches!(
            item,
            Item::Track(_) | Item::Station { .. } | Item::Episode(_) | Item::Mix(_)
        ) {
            return self.activate();
        }
        let tracks = self.playable(infos_of(self.lib.as_ref(), &item, &self.prefs));
        if tracks.is_empty() {
            if self.avail.any_offline() {
                self.note(UNREACHABLE);
            }
            return Activated::Nothing;
        }
        self.end_mix();
        self.player.send(Command::SetQueueTracks {
            tracks,
            start: 0,
            pos: Duration::ZERO,
        });
        Activated::Played
    }

    /// Add the selection to the queue, at the end or right after the
    /// current track.
    pub fn add_selected_to_queue(&mut self, next: bool) {
        let Some(item) = self.selected_item() else {
            return;
        };
        let infos = self.playable(infos_of(self.lib.as_ref(), &item, &self.prefs));
        if infos.is_empty() {
            if self.avail.any_offline() {
                self.note(UNREACHABLE);
            }
            return;
        }
        let n = infos.len();
        if next {
            self.player.send(Command::PlayNext(infos));
            self.note(format!("Playing next: {}", count(n)));
        } else {
            self.player.send(Command::AppendTracks(infos));
            self.note(format!("Added {} to the queue", count(n)));
        }
    }

    // ----- Playlists -----

    /// Library files the selection stands for, for adding to a playlist.
    pub fn playlist_paths_for_selection(&self) -> Vec<PathBuf> {
        self.selection_tracks()
            .into_iter()
            .map(|t| t.path)
            .collect()
    }

    pub fn playlists(&self) -> Vec<PlaylistRow> {
        self.lib
            .as_ref()
            .and_then(|l| l.playlists().ok())
            .unwrap_or_default()
    }

    /// Add files to an existing playlist. Files already in it are left
    /// out, and the note says so.
    pub fn add_to_playlist(&mut self, id: i64, name: &str, paths: &[PathBuf]) {
        if id == HEARTED {
            return self.note("Heart songs while they play to put them in Hearted songs");
        }
        if let Some(p) = self.playlist_row(id)
            && p.server.is_some()
        {
            return self.add_to_server_playlist(&p, paths);
        }
        let Some(lib) = self.lib.as_mut() else { return };
        match lib.append_to_playlist(id, paths) {
            Ok(added) => {
                let msg = added_note(added, paths.len(), name);
                self.note(msg);
            }
            Err(e) => self.error(e.to_string()),
        }
        self.refresh_section(Section::Playlists);
    }

    pub fn create_playlist(&mut self, name: &str, paths: &[PathBuf]) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(lib) = self.lib.as_mut() else {
            return self.error("Playlists need the library");
        };
        let res = lib
            .create_playlist(name)
            .and_then(|id| lib.append_to_playlist(id, paths));
        match res {
            Ok(_) if paths.is_empty() => {
                self.note(format!("Created \"{name}\""));
                self.refresh_section(Section::Playlists);
            }
            Ok(n) => {
                self.note(format!("Made \"{name}\" with {}", count(n)));
                self.refresh_section(Section::Playlists);
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn rename_playlist(&mut self, id: i64, name: &str) {
        if name.is_empty() {
            return;
        }
        if id == HEARTED {
            return self.note("Hearted songs keeps its name");
        }
        if !self.server_playlist_job(id, |rid| servers::Job::RenamePlaylist {
            id: rid,
            name: name.to_string(),
        }) {
            return;
        }
        if let Some(lib) = &self.lib
            && let Err(e) = lib.rename_playlist(id, name)
        {
            self.error(e.to_string());
        }
        self.refresh_section(Section::Playlists);
    }

    pub fn delete_playlist(&mut self, id: i64) {
        if id == HEARTED {
            return self.note("Hearted songs is always there; unheart songs to empty it");
        }
        if !self.server_playlist_job(id, |rid| servers::Job::DeletePlaylist { id: rid }) {
            return;
        }
        if let Some(lib) = &self.lib
            && let Err(e) = lib.delete_playlist(id)
        {
            self.error(e.to_string());
        }
        if let Some(stack) = self.stacks.get_mut(&Section::Playlists) {
            stack.truncate(1);
        }
        self.refresh_section(Section::Playlists);
    }

    /// The open playlist's id, if a playlist is open.
    pub fn in_playlist(&self) -> Option<i64> {
        match self.level().map(|l| &l.source) {
            Some(Source::Playlist { id, .. }) => Some(*id),
            _ => None,
        }
    }

    /// Remove (`remove`) or move by `delta` the selected item of the open
    /// playlist.
    pub fn edit_playlist(&mut self, remove: bool, delta: isize) {
        let Some(id) = self.in_playlist() else { return };
        if id == HEARTED {
            return self.edit_hearted(remove);
        }
        if self.playlist_row(id).is_some_and(|p| p.readonly) {
            return self.note(
                "That playlist is someone else's on the server; it can't be changed from here",
            );
        }
        let Some(level) = self.level() else { return };
        let mut uris: Vec<Vec<u8>> = level
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Track(t) => Some(t.uri()),
                Item::Missing { uri } => Some(uri.clone()),
                _ => None,
            })
            .collect();
        let sel = level.selected;
        if sel >= uris.len() {
            return;
        }
        let new_sel = if remove {
            uris.remove(sel);
            sel.min(uris.len().saturating_sub(1))
        } else {
            let to = (sel as isize + delta).clamp(0, uris.len() as isize - 1) as usize;
            if to == sel {
                return;
            }
            let u = uris.remove(sel);
            uris.insert(to, u);
            to
        };
        if let Some(lib) = self.lib.as_mut()
            && let Err(e) = lib.set_playlist_items(id, &uris)
        {
            return self.error(e.to_string());
        }
        self.push_server_playlist(id, &uris);
        self.refresh_current();
        if let Some(l) = self.level_mut() {
            l.selected = new_sel;
        }
        // The Playlists list shows item counts.
        if self
            .stacks
            .get(&Section::Playlists)
            .is_some_and(|s| !s.is_empty())
        {
            self.rebuild_level(Section::Playlists, 0);
        }
    }

    // ----- Queue -----

    pub fn play_queue_position(&self, pos: usize) {
        let id = self.player.queue().at(pos).map(|e| e.id);
        if let Some(id) = id {
            self.player.send(Command::PlayEntry(id));
        }
    }

    pub fn remove_queue_position(&self, pos: usize) {
        let id = self.player.queue().at(pos).map(|e| e.id);
        if let Some(id) = id {
            self.player.send(Command::RemoveEntry(id));
        }
    }

    /// Move the entry at `pos` by `delta`. Returns its new position.
    pub fn move_queue_position(&self, pos: usize, delta: isize) -> usize {
        let (id, to) = {
            let q = self.player.queue();
            let Some(e) = q.at(pos) else { return pos };
            let to = (pos as isize + delta).clamp(0, q.len() as isize - 1) as usize;
            (e.id, to)
        };
        self.player.send(Command::MoveEntry { id, to });
        to
    }

    pub fn clear_queue(&mut self) {
        self.end_mix();
        self.player.send(Command::ClearQueue);
    }

    /// The queue's songs in play order, for adding to a playlist. Stations
    /// and podcast episodes can't go in one, so they're left out.
    pub fn queue_playlist_paths(&self) -> Vec<PathBuf> {
        self.player.queue().music_paths_in_order()
    }

    /// Everything the open view stands for, for adding to a playlist.
    pub fn view_playlist_paths(&self) -> Vec<PathBuf> {
        self.view_tracks().into_iter().map(|t| t.path).collect()
    }

    /// The queue entry at `pos` as a playlist addition.
    pub fn queue_paths_at(&self, pos: usize) -> Vec<PathBuf> {
        let q = self.player.queue();
        q.at(pos)
            .filter(|e| !e.info.path.as_os_str().is_empty())
            .map(|e| vec![e.info.path.clone()])
            .unwrap_or_default()
    }

    // ----- Sorting and modes -----

    pub fn cycle_sort(&mut self) {
        match self.section {
            Section::Albums => self.prefs.album_sort = self.prefs.album_sort.next(),
            Section::Songs => self.prefs.song_sort = self.prefs.song_sort.next(),
            _ => return,
        }
        if let Some(stack) = self.stacks.get_mut(&self.section) {
            stack.truncate(1);
        }
        self.refresh_current();
        if let Some(l) = self.level_mut() {
            l.selected = l.snap(0, 1);
            l.offset = 0;
        }
    }

    pub fn reverse_songs(&mut self) {
        if self.section == Section::Songs {
            self.prefs.song_desc = !self.prefs.song_desc;
            self.refresh_current();
        }
    }

    /// Artists: album or track artist. Genre: albums or songs.
    pub fn toggle_mode(&mut self) {
        match self.level().map(|l| l.source.clone()) {
            Some(Source::Section(Section::Artists)) => {
                self.prefs.artist_mode = match self.prefs.artist_mode {
                    ArtistMode::AlbumArtist => ArtistMode::TrackArtist,
                    ArtistMode::TrackArtist => ArtistMode::AlbumArtist,
                };
                self.refresh_current();
                // The stats group artists the same way.
                for s in Section::STATS {
                    self.refresh_section(s);
                }
            }
            Some(Source::Genre { .. }) => {
                self.prefs.genre_songs = !self.prefs.genre_songs;
                self.refresh_current();
                if let Some(l) = self.level_mut() {
                    l.selected = l.snap(0, 1);
                    l.offset = 0;
                }
            }
            _ => {}
        }
    }

    // ----- Search -----

    /// Open (or reuse) a search level. Returns the query already there.
    pub fn start_search(&mut self) -> String {
        if matches!(
            self.section,
            Section::Queue
                | Section::Settings
                | Section::Eq
                | Section::Amp
                | Section::Downloads
                | Section::Radio
                | Section::Podcasts
                | Section::MostListened
                | Section::TopAlbums
                | Section::TopArtists
        ) {
            self.switch(Section::Songs);
        }
        let existing = match self.level().map(|l| &l.source) {
            Some(Source::Search(q)) => Some(q.clone()),
            _ => None,
        };
        if existing.is_none() {
            self.push(Source::Search(String::new()));
        }
        existing.unwrap_or_default()
    }

    pub fn in_search(&self) -> bool {
        matches!(self.level().map(|l| &l.source), Some(Source::Search(_)))
    }

    pub fn set_search(&mut self, q: &str) {
        if let Some(l) = self.level_mut()
            && let Source::Search(_) = l.source
        {
            l.source = Source::Search(q.to_string());
            l.offset = 0;
            l.selected = 0;
        }
        self.refresh_current();
        if let Some(l) = self.level_mut() {
            l.selected = l.snap(0, 1);
        }
    }

    // ----- Library and feeds -----

    pub fn rescan(&mut self) {
        if let Some(svc) = &self.service {
            svc.rescan();
        }
        if let Some(pod) = &self.pod {
            pod.refresh();
        }
        self.note("Checking folders and feeds for changes");
    }

    // ----- Settings -----

    /// The setting a row stands for, in the open Settings view.
    pub fn selected_setting(&self) -> Option<Setting> {
        match self.selected_item() {
            Some(Item::Setting(s)) => Some(s),
            _ => None,
        }
    }

    /// A warning to confirm before this change goes through, if it needs
    /// one: turning bit-perfect on takes the sound card away from other
    /// apps.
    pub fn setting_warning(&self, key: &Key, change: &Change) -> Option<&'static str> {
        if let Key::ServerScrobble(_) = key {
            let s = self.settings_row(key)?;
            return match settings::stepped(&s.control, change)? {
                Control::Choice { options, index } if options[index].0 == "both" => {
                    Some(lastfm::BOTH_WARNING)
                }
                _ => None,
            };
        }
        if *key != Key::BitPerfect || self.config.output.bit_perfect != BitPerfect::Off {
            return None;
        }
        let s = self.settings_row(key)?;
        match settings::stepped(&s.control, change)? {
            Control::Choice { index, .. } if index != 0 => Some(settings::BIT_PERFECT_WARNING),
            _ => None,
        }
    }

    fn settings_row(&self, key: &Key) -> Option<Setting> {
        settings::rows(
            &self.config,
            &self.settings_info,
            self.podcast_tempo,
            self.podcast_trim,
            &self.devices,
            &self.servers,
            settings::Live {
                measuring: self.measuring_library(),
                lastfm: self.scrobble.as_ref().map(|s| s.status()),
                outputs: self.outputs.clone(),
            },
        )
        .into_iter()
        .map(|(_, s)| s)
        .find(|s| &s.key == key)
    }

    /// The button at the bottom of the GUI's sidebar: fold it to icons, or
    /// expand it, for the way the screen is held now. Sticks, like the
    /// setting it changes.
    pub fn set_sidebar(&mut self, landscape: bool, fold: bool) {
        let mode = if fold {
            config::SidebarMode::Folded
        } else {
            config::SidebarMode::Expanded
        };
        if let Err(e) = self.save_sidebar(landscape, mode) {
            self.error(format!("couldn't save the sidebar setting: {e}"));
        }
        self.refresh_section(Section::Settings);
        self.touch();
    }

    fn save_sidebar(&mut self, landscape: bool, mode: config::SidebarMode) -> Result<(), String> {
        let key = if landscape {
            "sidebar_landscape"
        } else {
            "sidebar_portrait"
        };
        config::set_value(&self.config_path, "appearance", key, mode.as_str())
            .map_err(|e| e.to_string())?;
        if landscape {
            self.config.appearance.sidebar_landscape = mode;
        } else {
            self.config.appearance.sidebar_portrait = mode;
        }
        Ok(())
    }

    /// Apply a change from the Settings view: write it to config.toml
    /// (comments kept) and put it into effect now where that's possible.
    pub fn change_setting(&mut self, key: &Key, change: Change) {
        if key.is_eq() {
            return self.change_eq(key, change);
        }
        if key.is_amp() {
            return self.change_amp(key, change);
        }
        let Some(row) = self.settings_row(key) else {
            return;
        };
        if let Some(why) = row.disabled {
            return self.note(why);
        }
        if *key == Key::MeasureLibrary {
            return self.toggle_measure_library();
        }
        if *key == Key::LastFmLink {
            return self.lastfm_link();
        }
        let Some(new) = settings::stepped(&row.control, &change) else {
            return;
        };
        let path = self.config_path.clone();
        let res: Result<(), String> = (|| {
            let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
            match (key, &new) {
                (Key::Watch, Control::Toggle(on)) => {
                    w(config::set_value(&path, "library", "watch", *on))?;
                    self.config.library.watch = *on;
                    self.apply_folders();
                }
                (Key::Theme, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(&path, "appearance", "theme", v.as_str()))?;
                    self.config.appearance.theme = v.parse().unwrap_or_default();
                }
                (
                    Key::SidebarLandscape | Key::SidebarPortrait,
                    Control::Choice { options, index },
                ) => {
                    let mode: config::SidebarMode = options[*index].0.parse().unwrap_or_default();
                    self.save_sidebar(*key == Key::SidebarLandscape, mode)?;
                }
                (Key::ServerStream(name), Control::Choice { options, index }) => {
                    let (stream, kbps) = settings::parse_stream(&options[*index].0);
                    let mut list = self.config.servers.clone();
                    if let Some(sc) = list.iter_mut().find(|s| &s.name == name) {
                        sc.stream = stream;
                        if let Some(k) = kbps {
                            sc.bitrate = k;
                        }
                    }
                    w(config::set_servers(&path, &list))?;
                    self.config.servers = list;
                    if let Some(r) = &self.remote {
                        r.set_servers(self.config.servers.clone());
                    }
                }
                (Key::StartVolume, Control::Number { value, .. }) => {
                    w(config::set_value(&path, "playback", "volume", *value))?;
                    self.config.playback.volume = *value as u8;
                }
                (Key::FadeMs, Control::Number { value, .. }) => {
                    w(config::set_value(&path, "playback", "fade_ms", *value))?;
                    self.config.playback.fade_ms = *value as u32;
                    self.player.send(Command::SetFadeMs(*value as u32));
                }
                (Key::Backend, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(&path, "output", "backend", v.as_str()))?;
                    self.config.output.backend = v.parse().unwrap_or_default();
                    self.refresh_outputs();
                    self.apply_output();
                }
                (Key::Device, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(&path, "output", "device", v.as_str()))?;
                    self.config.output.device = (!v.is_empty()).then(|| v.clone());
                    self.apply_output();
                }
                (Key::OutRate, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    match v.parse::<i64>() {
                        Ok(r) => w(config::set_value(&path, "output", "rate", r))?,
                        Err(_) => w(config::set_value(&path, "output", "rate", "match"))?,
                    }
                    self.config.output.rate = v.parse().unwrap_or(0);
                    self.apply_output_format();
                }
                (Key::Resampler, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(&path, "output", "resampler", v.as_str()))?;
                    self.config.output.resampler = v.parse().unwrap_or_default();
                    self.apply_output_format();
                }
                (Key::OutFormat, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(&path, "output", "format", v.as_str()))?;
                    self.config.output.format = v.parse().unwrap_or_default();
                    self.apply_output_format();
                }
                (Key::Dither, Control::Toggle(on)) => {
                    w(config::set_value(&path, "output", "dither", *on))?;
                    self.config.output.dither = *on;
                    self.apply_output_format();
                }
                (Key::Crossfade, Control::Number { value, .. }) => {
                    let ms = (*value * 100) as u32;
                    w(config::set_value(
                        &path,
                        "output",
                        "crossfade_ms",
                        ms as i64,
                    ))?;
                    self.config.output.crossfade_ms = ms;
                    self.player.send(Command::SetCrossfade(ms));
                }
                (Key::BufferMs, Control::Number { value, .. }) => {
                    w(config::set_value(&path, "output", "buffer_ms", *value))?;
                    self.config.output.buffer_ms = *value as u32;
                    self.apply_output();
                }
                (Key::BitPerfect, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(
                        &path,
                        "output",
                        "bit_perfect",
                        v.as_str(),
                    ))?;
                    self.config.output.bit_perfect = v.parse().unwrap_or_default();
                    self.apply_bit_perfect();
                }
                (Key::BitPerfectDevice, Control::Choice { options, index }) => {
                    let v = &options[*index].0;
                    w(config::set_value(
                        &path,
                        "output",
                        "bit_perfect_device",
                        v.as_str(),
                    ))?;
                    self.config.output.bit_perfect_device = (!v.is_empty()).then(|| v.clone());
                    self.apply_bit_perfect();
                }
                (Key::LyricsOnline, Control::Toggle(on)) => {
                    w(config::set_value(&path, "lyrics", "online", *on))?;
                    self.config.lyrics.online = *on;
                }
                (Key::PodcastSpeed, Control::Number { value, scale, .. }) => {
                    let t = *value as f64 / *scale as f64;
                    self.set_podcast_playback(t, self.podcast_trim);
                }
                (Key::PodcastTrim, Control::Toggle(on)) => {
                    self.set_podcast_playback(self.podcast_tempo, *on);
                }
                (Key::GenreMatch, _) => self.change_mix_setting(key, &new)?,
                (
                    Key::LastFmKey
                    | Key::LastFmSecret
                    | Key::Scrobble
                    | Key::ScrobbleRadio
                    | Key::LoveStars
                    | Key::ServerScrobble(_),
                    _,
                ) => self.change_lastfm_setting(key, &new)?,
                (Key::Limiter | Key::LimiterCeiling, _) => self.change_limiter(key, &new)?,
                (
                    Key::ChannelOn
                    | Key::ChannelMono
                    | Key::ChannelWidth
                    | Key::ChannelCrossfeed
                    | Key::ChannelBalance,
                    _,
                ) => self.change_channel(key, &new)?,
                (Key::Tray, Control::Toggle(on)) => {
                    w(config::set_value(&path, "desktop", "tray", *on))?;
                    self.config.desktop.tray = *on;
                }
                (Key::CloseToTray, Control::Toggle(on)) => {
                    w(config::set_value(&path, "desktop", "close_to_tray", *on))?;
                    self.config.desktop.close_to_tray = *on;
                }
                (
                    Key::LevelMode
                    | Key::LevelTarget
                    | Key::LevelFallback
                    | Key::ClipGuard
                    | Key::LiveMusic
                    | Key::LiveRadio,
                    _,
                ) => self.change_level_setting(key, &new)?,
                (
                    Key::CacheSize
                    | Key::Prefetch
                    | Key::PrefetchAmount
                    | Key::PauseMetered
                    | Key::CacheFormat,
                    _,
                ) => self.change_cache_setting(key, &new)?,
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = res {
            self.error(format!("couldn't save the setting: {e}"));
        }
        self.sync_settings();
    }

    /// Add a music folder to the library: the config, then a scan.
    /// Accepts `~/Music` as well as full paths.
    pub fn add_folder(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let dir = match crate::session::absolute(std::path::Path::new(text)) {
            Ok(d) => d,
            Err(e) => {
                self.error(format!("can't find {text}: {e}"));
                return false;
            }
        };
        if !dir.is_dir() {
            self.error(format!("{} isn't a folder", dir.display()));
            return false;
        }
        if self.config.library.folders.contains(&dir) {
            self.note(format!("{} is already in the library", dir.display()));
            return false;
        }
        match config::edit_library_folder(&self.config_path, &dir, true) {
            Ok(_) => {
                self.config.library.folders.push(dir.clone());
                self.apply_folders();
                self.note(format!("Added {}. Scanning it now.", dir.display()));
                self.sync_settings();
                true
            }
            Err(e) => {
                self.error(format!("couldn't save: {e}"));
                false
            }
        }
    }

    /// Take a folder out of the library. The files stay where they are.
    pub fn remove_folder(&mut self, dir: &std::path::Path) {
        match config::edit_library_folder(&self.config_path, dir, false) {
            Ok(_) => {
                self.config.library.folders.retain(|f| f != dir);
                self.apply_folders();
                self.note(format!("Removed {} from the library", dir.display()));
            }
            Err(e) => self.error(format!("couldn't save: {e}")),
        }
        self.sync_settings();
    }

    fn apply_folders(&mut self) {
        if let Some(svc) = &self.service {
            svc.set_roots(
                self.config.library.folders.clone(),
                self.config.library.watch,
            );
        } else if !self.settings_info.no_library {
            self.note("The library starts with the next launch");
        }
    }

    fn apply_output(&self) {
        let o = &self.config.output;
        self.player.send(Command::SetOutput {
            backend: o.backend,
            device: o.device.clone(),
            buffer_ms: o.buffer_ms,
        });
    }

    fn apply_output_format(&self) {
        let o = &self.config.output;
        self.player.send(Command::SetOutputFormat {
            rate: o.rate,
            resampler: o.resampler,
            format: o.format,
            dither: o.dither,
        });
    }

    /// Look up the devices the output can play to again, for Settings and
    /// the bottom bar's device picker.
    pub fn refresh_outputs(&mut self) {
        self.outputs = crate::audio::output::list_outputs(self.config.output.backend);
    }

    /// The Device row, for a picker on the bottom bar. Looks the devices
    /// up again first, since they come and go.
    pub fn device_setting(&mut self) -> Option<Setting> {
        self.refresh_outputs();
        self.settings_row(&Key::Device)
    }

    fn apply_bit_perfect(&self) {
        let o = &self.config.output;
        self.player.send(Command::SetBitPerfect {
            mode: o.bit_perfect,
            device: o.bit_perfect_device.clone(),
        });
    }

    // ----- Radio -----

    fn save_stations(&mut self) {
        if let Err(e) = config::set_stations(&self.config_path, &self.stations) {
            self.error(format!("couldn't save stations: {e}"));
        }
        self.refresh_section(Section::Radio);
        self.sync_settings();
    }

    /// Settings shows live values; rebuild it after anything it shows
    /// changed.
    fn sync_settings(&mut self) {
        self.refresh_section(Section::Settings);
    }

    /// Add a station, or replace station `edit`. Tests it straight away.
    pub fn save_station(&mut self, edit: Option<usize>, name: &str, url: &str) {
        let (name, url) = (name.trim(), url.trim());
        if name.is_empty() || url.is_empty() {
            return self.error("A station needs a name and a URL");
        }
        let st = Station {
            name: name.to_string(),
            url: url.to_string(),
        };
        match edit {
            Some(i) if i < self.stations.len() => self.stations[i] = st,
            _ => self.stations.push(st),
        }
        self.save_stations();
        self.test_station(url.to_string());
    }

    pub fn delete_station(&mut self, i: usize) {
        if i < self.stations.len() {
            self.stations.remove(i);
            self.save_stations();
        }
    }

    /// Move station `i` by `delta`. Returns its new index.
    pub fn move_station(&mut self, i: usize, delta: isize) -> usize {
        let to = i as isize + delta;
        if to < 0 || to as usize >= self.stations.len() || i >= self.stations.len() {
            return i;
        }
        self.stations.swap(i, to as usize);
        self.save_stations();
        if let Some(l) = self.level_mut() {
            l.selected = to as usize;
        }
        to as usize
    }

    /// Resolve and open a station in the background; the result arrives
    /// through [`poll`](Self::poll) as a note and in `checks`.
    pub fn test_station(&mut self, url: String) {
        self.note(format!("Testing {url}"));
        let tx = self.check_tx.clone();
        std::thread::spawn(move || {
            let summary = match crate::radio::check(&url) {
                Ok(c) => c.summary(),
                Err(e) => format!("Not working: {e}"),
            };
            let _ = tx.send((url, summary));
        });
    }

    // ----- Podcasts -----

    pub fn subscribe(&mut self, url: &str) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        if self.feeds.contains(&url) {
            return self.note("Already subscribed");
        }
        match config::edit_podcast_feed(&self.config_path, &url, true) {
            Ok(_) => {
                self.feeds.push(url);
                if let Some(pod) = &self.pod {
                    pod.set_feeds(self.feeds.clone());
                }
                self.sync_settings();
                self.note("Subscribed. Fetching the feed…");
            }
            Err(e) => self.error(format!("couldn't save the feed: {e}")),
        }
    }

    pub fn unsubscribe(&mut self, url: &str) {
        if url.starts_with(crate::remote::SCHEME) {
            return self.note("That podcast comes from your server; unsubscribe from it there");
        }
        match config::edit_podcast_feed(&self.config_path, url, false) {
            Ok(_) => {
                self.feeds.retain(|f| f != url);
                if let Some(pod) = &self.pod {
                    pod.set_feeds(self.feeds.clone());
                }
                self.sync_settings();
                if let Some(stack) = self.stacks.get_mut(&Section::Podcasts) {
                    stack.truncate(1);
                }
                self.note("Unsubscribed");
            }
            Err(e) => self.error(format!("couldn't save: {e}")),
        }
    }

    /// Start a download, or cancel the one running or queued.
    pub fn toggle_download(&mut self, e: &EpisodeRow) {
        let Some(cache) = &self.cache else {
            return self.error("Downloads need the library");
        };
        if self.cache_status.episodes.remove(&e.id).is_some() {
            cache.cancel_episode(e.id);
            self.note("Download cancelled");
        } else if e.download.is_some() {
            self.note("Already downloaded");
        } else {
            cache.download_episode(e.id);
            self.cache_status
                .episodes
                .insert(e.id, crate::cache::EpisodeState::Queued);
            self.note(format!("Downloading \"{}\"", e.title));
        }
        self.refresh_section(Section::Downloads);
        self.touch();
    }

    /// Where an episode's download stands, if it's queued or running.
    pub fn episode_download(&self, id: i64) -> Option<crate::cache::EpisodeState> {
        self.cache_status.episodes.get(&id).copied()
    }

    pub fn delete_download(&mut self, id: i64) {
        if let Some(lib) = &self.lib {
            match crate::podcast::delete_download(lib, id) {
                Ok(()) => self.note("Download deleted"),
                Err(err) => self.error(err.to_string()),
            }
        }
        self.refresh_current();
        self.refresh_section(Section::Downloads);
    }

    pub fn toggle_played(&mut self, e: &EpisodeRow) {
        if let Some(lib) = &self.lib
            && let Err(err) = lib.set_episode_played(e.id, !e.played)
        {
            self.error(err.to_string());
        }
        self.refresh_current();
        if self
            .stacks
            .get(&Section::Podcasts)
            .is_some_and(|s| !s.is_empty())
        {
            self.rebuild_level(Section::Podcasts, 0);
        }
    }

    pub fn set_podcast_playback(&mut self, tempo: f64, trim: bool) {
        let tempo = ((tempo * 10.0).round() / 10.0).clamp(0.5, 3.0);
        self.player.send(Command::SetPodcastPlayback {
            tempo,
            trim_silence: trim,
        });
        self.podcast_tempo = tempo;
        self.podcast_trim = trim;
        if let Err(e) = config::set_podcast_playback(&self.config_path, tempo, trim) {
            self.error(format!("couldn't save to the config: {e}"));
        }
        self.sync_settings();
        self.touch();
    }

    pub fn nudge_tempo(&mut self, d: f64) {
        let t = self.podcast_tempo + d;
        self.set_podcast_playback(t, self.podcast_trim);
        let t = self.podcast_tempo;
        self.note(format!("Podcast speed {t:.1}x"));
    }

    pub fn toggle_trim(&mut self) {
        let trim = !self.podcast_trim;
        self.set_podcast_playback(self.podcast_tempo, trim);
        self.note(if trim {
            "Silence trimming on (podcasts)"
        } else {
            "Silence trimming off"
        });
    }
}
