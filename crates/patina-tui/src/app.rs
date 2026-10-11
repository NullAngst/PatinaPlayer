//! Input handling and the frontend's own state: which view is open, what's
//! selected, any prompt on screen. Playback lives in the core and the
//! library in the index; this file only sends commands and asks questions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use patina_core::library::{ArtistMode, LibEvent, Library, LibraryService, PlaylistRow, TrackRow};
use patina_core::{Command, Event, Player, Snapshot, TrackInfo};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::theme::Theme;
use crate::views::{self, Item, Level, Section, Settings, Source, ViewPrefs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
}

#[derive(Debug, Clone)]
pub enum InputPurpose {
    Search,
    NewPlaylist { add: Vec<PathBuf> },
    RenamePlaylist { id: i64 },
    SaveQueue,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeletePlaylist(i64),
    ClearQueue,
}

pub enum Modal {
    None,
    Help,
    Input {
        purpose: InputPurpose,
        buf: String,
    },
    PickPlaylist {
        add: Vec<PathBuf>,
        rows: Vec<PlaylistRow>,
        selected: usize,
    },
    Confirm {
        msg: String,
        action: ConfirmAction,
    },
}

/// A row in the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub enum SideEntry {
    Section(Section),
    Source { path: PathBuf, tracks: i64 },
    Label(&'static str),
    Gap,
}

pub struct App {
    pub player: Player,
    pub theme: Theme,
    pub snap: Snapshot,
    pub lib: Option<Library>,
    pub service: Option<LibraryService>,
    pub section: Section,
    /// Drill-down stack per section, so switching away and back returns
    /// you to where you were.
    pub stacks: HashMap<Section, Vec<Level>>,
    pub prefs: ViewPrefs,
    pub settings: Settings,
    pub focus: Focus,
    pub sidebar: Vec<SideEntry>,
    pub sidebar_sel: usize,
    pub modal: Modal,
    /// Message, when it arrived, and whether it's an error.
    pub toast: Option<(String, Instant, bool)>,
    /// Keep the queue selection on the playing track until the user moves it.
    pub follow: bool,
    pub queue_panel: bool,
    pub queue_sel: usize,
    pub queue_offset: usize,
    pub view_error: Option<String>,
    // Hit areas from the last draw, for the mouse.
    pub sidebar_area: Rect,
    pub list_area: Rect,
    pub seek_area: Rect,
    pub volume_area: Rect,
    last_click: Option<(Instant, u16, u16)>,
    quit: bool,
}

const ERROR_TIME: Duration = Duration::from_secs(8);
const NOTE_TIME: Duration = Duration::from_secs(3);

impl App {
    pub fn new(
        player: Player,
        lib: Option<Library>,
        service: Option<LibraryService>,
        settings: Settings,
    ) -> Self {
        let mut app = Self {
            player,
            theme: Theme::detect(),
            snap: Snapshot::default(),
            lib,
            service,
            section: Section::Folders,
            stacks: HashMap::new(),
            prefs: ViewPrefs::default(),
            settings,
            focus: Focus::Content,
            sidebar: Vec::new(),
            sidebar_sel: 0,
            modal: Modal::None,
            toast: None,
            follow: true,
            queue_panel: false,
            queue_sel: 0,
            queue_offset: 0,
            view_error: None,
            sidebar_area: Rect::default(),
            list_area: Rect::default(),
            seek_area: Rect::default(),
            volume_area: Rect::default(),
            last_click: None,
            quit: false,
        };
        app.rebuild_sidebar();
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.drain_events();
            self.snap = self.player.snapshot();
            if let Some((_, t, err)) = &self.toast
                && t.elapsed() > if *err { ERROR_TIME } else { NOTE_TIME }
            {
                self.toast = None;
            }
            terminal.draw(|f| crate::ui::draw(f, &mut self))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    TermEvent::Key(k) if k.kind != KeyEventKind::Release => self.key(k),
                    TermEvent::Mouse(m) => self.mouse(m),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now(), false));
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now(), true));
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.player.events().try_recv() {
            match ev {
                Event::Error(e) => self.error(e),
                Event::TrackChanged(Some(id)) if self.follow => {
                    if let Some(p) = self.player.queue().position(id) {
                        self.queue_sel = p;
                    }
                }
                _ => {}
            }
        }
        let mut changed = false;
        if let Some(svc) = &self.service {
            let evs: Vec<LibEvent> = svc.events().try_iter().collect();
            for ev in evs {
                match ev {
                    LibEvent::Changed(_) => changed = true,
                    LibEvent::Error(e) => self.error(e),
                    LibEvent::Progress(_) | LibEvent::Ready => {}
                }
            }
        }
        if changed {
            self.refresh_all();
            self.rebuild_sidebar();
        }
    }

    // ----- Sidebar -----

    pub fn rebuild_sidebar(&mut self) {
        let mut s: Vec<SideEntry> = Section::NUMBERED
            .iter()
            .map(|&x| SideEntry::Section(x))
            .collect();
        if let Some(lib) = &self.lib
            && let Ok(sources) = lib.sources()
            && !sources.is_empty()
        {
            s.push(SideEntry::Gap);
            s.push(SideEntry::Label("Sources"));
            for src in sources {
                s.push(SideEntry::Source {
                    path: src.root,
                    tracks: src.tracks,
                });
            }
        }
        s.push(SideEntry::Gap);
        s.push(SideEntry::Section(Section::Settings));
        self.sidebar = s;
        self.sync_sidebar_sel();
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

    fn sidebar_move(&mut self, d: isize) {
        let n = self.sidebar.len() as isize;
        let mut i = self.sidebar_sel as isize;
        loop {
            i += d;
            if !(0..n).contains(&i) {
                return;
            }
            if matches!(
                self.sidebar[i as usize],
                SideEntry::Section(_) | SideEntry::Source { .. }
            ) {
                break;
            }
        }
        self.sidebar_sel = i as usize;
        self.activate_sidebar(false);
    }

    fn activate_sidebar(&mut self, focus_content: bool) {
        let sel = self.sidebar_sel;
        match self.sidebar.get(sel).cloned() {
            Some(SideEntry::Section(s)) => self.switch(s),
            Some(SideEntry::Source { path, .. }) => {
                // A source opens its folder tree under Folders.
                self.switch(Section::Folders);
                if let Some(stack) = self.stacks.get_mut(&Section::Folders) {
                    stack.truncate(1);
                }
                self.push(Source::Folder(path));
                self.sidebar_sel = sel;
            }
            _ => {}
        }
        if focus_content {
            self.focus = Focus::Content;
        }
    }

    // ----- Levels -----

    pub fn switch(&mut self, s: Section) {
        self.section = s;
        self.view_error = None;
        if s != Section::Queue && !self.stacks.contains_key(&s) {
            let level = self.build(&Source::Section(s));
            self.stacks.insert(s, level.into_iter().collect());
        }
        self.sync_sidebar_sel();
    }

    fn build(&mut self, src: &Source) -> Option<Level> {
        match views::build(self.lib.as_ref(), src, &self.prefs, &self.settings) {
            Ok((title, items)) => {
                let mut l = Level {
                    title,
                    source: src.clone(),
                    items,
                    selected: 0,
                    offset: 0,
                };
                l.selected = l.snap(0, 1);
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

    fn level_mut(&mut self) -> Option<&mut Level> {
        self.stacks
            .get_mut(&self.section)
            .and_then(|s| s.last_mut())
    }

    /// Breadcrumb titles for the open stack.
    pub fn crumbs(&self) -> Vec<String> {
        self.stacks
            .get(&self.section)
            .map(|s| s.iter().map(|l| l.title.clone()).collect())
            .unwrap_or_default()
    }

    fn push(&mut self, src: Source) {
        if let Some(l) = self.build(&src) {
            self.stacks.entry(self.section).or_default().push(l);
        }
    }

    fn pop(&mut self) {
        if let Some(stack) = self.stacks.get_mut(&self.section)
            && stack.len() > 1
        {
            stack.pop();
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
        let built = views::build(self.lib.as_ref(), &src, &self.prefs, &self.settings);
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
    }

    fn refresh_current(&mut self) {
        if let Some(n) = self.stacks.get(&self.section).map(|s| s.len())
            && n > 0
        {
            self.rebuild_level(self.section, n - 1);
        }
    }

    fn refresh_all(&mut self) {
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

    fn move_sel(&mut self, delta: isize) {
        if self.section == Section::Queue {
            let n = self.player.queue().len();
            if n == 0 {
                return;
            }
            self.queue_sel = (self.queue_sel as isize + delta).clamp(0, n as isize - 1) as usize;
            self.follow = false;
            return;
        }
        if let Some(l) = self.level_mut() {
            if l.items.is_empty() {
                return;
            }
            let target =
                (l.selected as isize + delta).clamp(0, l.items.len() as isize - 1) as usize;
            l.selected = l.snap(target, if delta < 0 { -1 } else { 1 });
        }
    }

    fn page(&self) -> isize {
        self.list_area.height.saturating_sub(3).max(1) as isize
    }

    fn selected_item(&self) -> Option<Item> {
        self.level().and_then(|l| l.selected_item().cloned())
    }

    /// Tracks the selection stands for.
    fn selection_tracks(&self) -> Vec<TrackRow> {
        match (&self.lib, self.selected_item()) {
            (Some(lib), Some(item)) => views::tracks_of(lib, &item, &self.prefs),
            _ => Vec::new(),
        }
    }

    fn infos(tracks: &[TrackRow]) -> Vec<TrackInfo> {
        tracks.iter().map(TrackRow::to_info).collect()
    }

    /// Enter: open what's selected, or play it in context.
    fn activate(&mut self) {
        if self.section == Section::Queue {
            let id = self.player.queue().at(self.queue_sel).map(|e| e.id);
            if let Some(id) = id {
                self.player.send(Command::PlayEntry(id));
            }
            self.follow = true;
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        if let Some(src) = views::open(&item) {
            self.push(src);
            return;
        }
        if let Item::Track(t) = &item {
            // Play the whole list this track sits in, starting here, the
            // way picking a song in an album plays the album from there.
            let Some(level) = self.level() else { return };
            let tracks: Vec<TrackRow> = level
                .items
                .iter()
                .filter_map(|i| match i {
                    Item::Track(t) => Some(t.clone()),
                    _ => None,
                })
                .collect();
            let start = tracks.iter().position(|x| x.path == t.path).unwrap_or(0);
            self.player.send(Command::SetQueueTracks {
                tracks: Self::infos(&tracks),
                start,
            });
            self.follow = true;
        }
    }

    fn add_to_queue(&mut self, next: bool) {
        let tracks = self.selection_tracks();
        if tracks.is_empty() {
            return;
        }
        let n = tracks.len();
        let infos = Self::infos(&tracks);
        if next {
            self.player.send(Command::PlayNext(infos));
            self.note(format!("Playing next: {}", count(n)));
        } else {
            self.player.send(Command::AppendTracks(infos));
            self.note(format!("Added {} to the queue", count(n)));
        }
    }

    fn pick_playlist(&mut self) {
        if self.lib.is_none() {
            return self.error("Playlists need the library");
        }
        let add: Vec<PathBuf> = if self.section == Section::Queue {
            let q = self.player.queue();
            q.at(self.queue_sel)
                .map(|e| vec![e.info.path.clone()])
                .unwrap_or_default()
        } else {
            self.selection_tracks()
                .into_iter()
                .map(|t| t.path)
                .collect()
        };
        if add.is_empty() {
            return;
        }
        let rows = self
            .lib
            .as_ref()
            .and_then(|l| l.playlists().ok())
            .unwrap_or_default();
        self.modal = Modal::PickPlaylist {
            add,
            rows,
            selected: 0,
        };
    }

    fn in_playlist(&self) -> Option<i64> {
        match self.level().map(|l| &l.source) {
            Some(Source::Playlist { id, .. }) => Some(*id),
            _ => None,
        }
    }

    /// Remove or move the selected item in an open playlist.
    fn edit_playlist(&mut self, id: i64, remove: bool, delta: isize) {
        let Some(level) = self.level() else { return };
        let mut uris: Vec<Vec<u8>> = level
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Track(t) => Some(patina_core::library::file_uri(&t.path)),
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

    // ----- Keys -----

    fn key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('q') | KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::Help => return,
            Modal::Input { purpose, buf } => return self.input_key(k, purpose, buf),
            Modal::PickPlaylist {
                add,
                rows,
                selected,
            } => return self.pick_key(k, add, rows, selected),
            Modal::Confirm { msg, action } => {
                match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm(action),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {}
                    _ => self.modal = Modal::Confirm { msg, action },
                }
                return;
            }
        }
        if self.global_key(k) {
            return;
        }
        match self.focus {
            Focus::Sidebar => self.sidebar_key(k),
            Focus::Content => self.content_key(k),
        }
    }

    /// Keys that work the same everywhere.
    fn global_key(&mut self, k: KeyEvent) -> bool {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let p = &self.player;
        match k.code {
            KeyCode::Char(' ') => p.send(Command::TogglePause),
            KeyCode::Char('n') => p.send(Command::Next),
            KeyCode::Char('b') => p.send(Command::Back),
            KeyCode::Left => p.send(Command::SeekBy(if shift { -30.0 } else { -5.0 })),
            KeyCode::Right => p.send(Command::SeekBy(if shift { 30.0 } else { 5.0 })),
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_volume(0.05),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_volume(-0.05),
            KeyCode::Char('m') => p.send(Command::ToggleMute),
            KeyCode::Char('s') => p.send(Command::ToggleShuffle),
            KeyCode::Char('r') => p.send(Command::CycleRepeat),
            KeyCode::Char('q') => self.queue_panel = !self.queue_panel,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('/') if self.lib.is_some() => self.start_search(),
            KeyCode::Char(c @ '1'..='8') => {
                let s = Section::NUMBERED[(c as u8 - b'1') as usize];
                self.switch(s);
                self.focus = Focus::Content;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Content,
                    Focus::Content => Focus::Sidebar,
                };
            }
            KeyCode::F(5) => {
                if let Some(svc) = &self.service {
                    svc.rescan();
                    self.note("Checking library folders for changes");
                }
            }
            KeyCode::Char('.') => {
                self.follow = true;
                if let Some(cur) = self.snap.current
                    && let Some(p) = self.player.queue().position(cur)
                {
                    self.queue_sel = p;
                }
                self.switch(Section::Queue);
                self.focus = Focus::Content;
            }
            _ => return false,
        }
        true
    }

    fn sidebar_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.sidebar_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.sidebar_move(1),
            KeyCode::Enter => self.activate_sidebar(true),
            _ => {}
        }
    }

    fn content_key(&mut self, k: KeyEvent) {
        let queue = self.section == Section::Queue;
        let playlist = self.in_playlist();
        let lists = self.section == Section::Playlists && playlist.is_none();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-self.page()),
            KeyCode::PageDown => self.move_sel(self.page()),
            KeyCode::Home | KeyCode::Char('g') => self.move_sel(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_sel(isize::MAX / 2),
            KeyCode::Enter => self.activate(),
            KeyCode::Backspace | KeyCode::Esc => self.pop(),
            KeyCode::Char('a') if !queue => self.add_to_queue(false),
            KeyCode::Char('A') if !queue => self.add_to_queue(true),
            KeyCode::Char('p') => self.pick_playlist(),
            KeyCode::Char('o') => self.cycle_sort(),
            KeyCode::Char('O') if self.section == Section::Songs => {
                self.prefs.song_desc = !self.prefs.song_desc;
                self.refresh_current();
            }
            KeyCode::Char('t') => self.toggle_mode(),

            // Queue editing.
            KeyCode::Char('d') | KeyCode::Delete if queue => {
                let id = self.player.queue().at(self.queue_sel).map(|e| e.id);
                if let Some(id) = id {
                    self.player.send(Command::RemoveEntry(id));
                }
            }
            KeyCode::Char('J') if queue => self.move_queue(1),
            KeyCode::Char('K') if queue => self.move_queue(-1),
            KeyCode::Char('C') if queue => {
                self.modal = Modal::Confirm {
                    msg: "Clear the whole queue?".into(),
                    action: ConfirmAction::ClearQueue,
                }
            }
            KeyCode::Char('S') if queue => {
                self.modal = Modal::Input {
                    purpose: InputPurpose::SaveQueue,
                    buf: String::new(),
                }
            }

            // Playlist editing.
            KeyCode::Char('d') | KeyCode::Delete if playlist.is_some() => {
                self.edit_playlist(playlist.unwrap(), true, 0)
            }
            KeyCode::Char('J') if playlist.is_some() => {
                self.edit_playlist(playlist.unwrap(), false, 1)
            }
            KeyCode::Char('K') if playlist.is_some() => {
                self.edit_playlist(playlist.unwrap(), false, -1)
            }
            KeyCode::Char('N') if lists => {
                self.modal = Modal::Input {
                    purpose: InputPurpose::NewPlaylist { add: Vec::new() },
                    buf: String::new(),
                }
            }
            KeyCode::Char('R') if lists => {
                if let Some(Item::Playlist(p)) = self.selected_item() {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::RenamePlaylist { id: p.id },
                        buf: p.name,
                    }
                }
            }
            KeyCode::Char('D') if lists => {
                if let Some(Item::Playlist(p)) = self.selected_item() {
                    self.modal = Modal::Confirm {
                        msg: format!("Delete playlist \"{}\"?", p.name),
                        action: ConfirmAction::DeletePlaylist(p.id),
                    }
                }
            }
            _ => {}
        }
    }

    fn move_queue(&mut self, d: isize) {
        let (id, to) = {
            let q = self.player.queue();
            let Some(e) = q.at(self.queue_sel) else {
                return;
            };
            let to = (self.queue_sel as isize + d).clamp(0, q.len() as isize - 1) as usize;
            (e.id, to)
        };
        self.player.send(Command::MoveEntry { id, to });
        self.queue_sel = to;
        self.follow = false;
    }

    fn cycle_sort(&mut self) {
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

    fn toggle_mode(&mut self) {
        match self.level().map(|l| l.source.clone()) {
            Some(Source::Section(Section::Artists)) => {
                self.prefs.artist_mode = match self.prefs.artist_mode {
                    ArtistMode::AlbumArtist => ArtistMode::TrackArtist,
                    ArtistMode::TrackArtist => ArtistMode::AlbumArtist,
                };
                self.refresh_current();
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

    fn start_search(&mut self) {
        if matches!(
            self.section,
            Section::Queue | Section::Settings | Section::Downloads
        ) {
            self.switch(Section::Songs);
        }
        // Reuse a search level already on top; otherwise push a fresh one.
        let existing = match self.level().map(|l| &l.source) {
            Some(Source::Search(q)) => Some(q.clone()),
            _ => None,
        };
        if existing.is_none() {
            self.push(Source::Search(String::new()));
        }
        self.focus = Focus::Content;
        self.modal = Modal::Input {
            purpose: InputPurpose::Search,
            buf: existing.unwrap_or_default(),
        };
    }

    fn set_search(&mut self, q: &str) {
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

    fn input_key(&mut self, k: KeyEvent, purpose: InputPurpose, mut buf: String) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let search = matches!(purpose, InputPurpose::Search);
        match k.code {
            KeyCode::Esc => {
                if search {
                    self.pop();
                }
                return;
            }
            KeyCode::Enter => {
                self.submit_input(purpose, buf.trim().to_string());
                return;
            }
            // Arrow keys move through live search results while typing.
            KeyCode::Up if search => self.move_sel(-1),
            KeyCode::Down if search => self.move_sel(1),
            KeyCode::Backspace => {
                buf.pop();
                if search {
                    self.set_search(&buf);
                }
            }
            KeyCode::Char('u') if ctrl => {
                buf.clear();
                if search {
                    self.set_search(&buf);
                }
            }
            KeyCode::Char(c) if !ctrl => {
                buf.push(c);
                if search {
                    self.set_search(&buf);
                }
            }
            _ => {}
        }
        self.modal = Modal::Input { purpose, buf };
    }

    fn submit_input(&mut self, purpose: InputPurpose, text: String) {
        match purpose {
            // Enter keeps the results on screen for browsing.
            InputPurpose::Search => {}
            InputPurpose::NewPlaylist { add } => {
                if text.is_empty() {
                    return;
                }
                let Some(lib) = self.lib.as_mut() else { return };
                let res = lib
                    .create_playlist(&text)
                    .and_then(|id| lib.append_to_playlist(id, &add));
                match res {
                    Ok(()) if add.is_empty() => self.note(format!("Created \"{text}\"")),
                    Ok(()) => self.note(format!("Added {} to \"{text}\"", count(add.len()))),
                    Err(e) => self.error(e.to_string()),
                }
                self.refresh_playlists();
            }
            InputPurpose::RenamePlaylist { id } => {
                if text.is_empty() {
                    return;
                }
                if let Some(lib) = &self.lib
                    && let Err(e) = lib.rename_playlist(id, &text)
                {
                    self.error(e.to_string());
                }
                self.refresh_playlists();
            }
            InputPurpose::SaveQueue => {
                if text.is_empty() {
                    return;
                }
                let paths = self.player.queue().paths_in_order();
                let Some(lib) = self.lib.as_mut() else {
                    return self.error("Saving playlists needs the library");
                };
                let res = lib
                    .create_playlist(&text)
                    .and_then(|id| lib.append_to_playlist(id, &paths));
                match res {
                    Ok(()) => self.note(format!("Saved the queue as \"{text}\"")),
                    Err(e) => self.error(e.to_string()),
                }
                self.refresh_playlists();
            }
        }
    }

    fn refresh_playlists(&mut self) {
        if let Some(n) = self.stacks.get(&Section::Playlists).map(|s| s.len()) {
            for i in 0..n {
                self.rebuild_level(Section::Playlists, i);
            }
        }
    }

    fn pick_key(
        &mut self,
        k: KeyEvent,
        add: Vec<PathBuf>,
        rows: Vec<PlaylistRow>,
        mut selected: usize,
    ) {
        // Row 0 is "New playlist", then the playlists.
        let n = rows.len() + 1;
        match k.code {
            KeyCode::Esc => return,
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(n - 1),
            KeyCode::Enter => {
                if selected == 0 {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::NewPlaylist { add },
                        buf: String::new(),
                    };
                } else {
                    let p = &rows[selected - 1];
                    if let Some(lib) = self.lib.as_mut() {
                        match lib.append_to_playlist(p.id, &add) {
                            Ok(()) => {
                                self.note(format!("Added {} to \"{}\"", count(add.len()), p.name))
                            }
                            Err(e) => self.error(e.to_string()),
                        }
                    }
                    self.refresh_playlists();
                }
                return;
            }
            _ => {}
        }
        self.modal = Modal::PickPlaylist {
            add,
            rows,
            selected,
        };
    }

    fn confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::ClearQueue => self.player.send(Command::ClearQueue),
            ConfirmAction::DeletePlaylist(id) => {
                if let Some(lib) = &self.lib
                    && let Err(e) = lib.delete_playlist(id)
                {
                    self.error(e.to_string());
                }
                if let Some(stack) = self.stacks.get_mut(&Section::Playlists) {
                    stack.truncate(1);
                }
                self.refresh_playlists();
            }
        }
    }

    fn nudge_volume(&self, d: f64) {
        self.player
            .send(Command::SetVolume((self.snap.volume + d).clamp(0.0, 1.0)));
    }

    // ----- Mouse -----

    fn mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let double = self.last_click.is_some_and(|(t, lx, ly)| {
            ly == y && lx.abs_diff(x) < 3 && t.elapsed() < Duration::from_millis(400)
        });
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
                if inside(self.seek_area) =>
            {
                if let Some(d) = self.snap.duration {
                    let frac = (x - self.seek_area.x) as f64 / self.seek_area.width.max(1) as f64;
                    self.player
                        .send(Command::Seek(d.mul_f64(frac.clamp(0.0, 1.0))));
                }
            }
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
                if inside(self.volume_area) =>
            {
                let frac = (x - self.volume_area.x) as f64 / self.volume_area.width.max(1) as f64;
                self.player.send(Command::SetVolume(frac.clamp(0.0, 1.0)));
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.sidebar_area) => {
                let row = (y - self.sidebar_area.y) as usize;
                if let Some(e) = self.sidebar.get(row)
                    && matches!(e, SideEntry::Section(_) | SideEntry::Source { .. })
                {
                    self.sidebar_sel = row;
                    self.activate_sidebar(true);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.list_area) => {
                // Row 0 is the title, row 1 the column header.
                if y < self.list_area.y + 2 {
                    return;
                }
                let row = (y - self.list_area.y - 2) as usize;
                self.focus = Focus::Content;
                if self.section == Section::Queue {
                    let pos = self.queue_offset + row;
                    if pos < self.player.queue().len() {
                        self.queue_sel = pos;
                        self.follow = false;
                        if double {
                            self.activate();
                        }
                    }
                } else if let Some(l) = self.level_mut() {
                    let pos = l.offset + row;
                    if pos < l.items.len() && l.items[pos].selectable() {
                        l.selected = pos;
                        if double {
                            self.activate();
                        }
                    }
                }
                self.last_click = if double {
                    None
                } else {
                    Some((Instant::now(), x, y))
                };
            }
            MouseEventKind::Down(MouseButton::Right) if inside(self.list_area) => self.pop(),
            MouseEventKind::ScrollUp if inside(self.volume_area) => self.nudge_volume(0.05),
            MouseEventKind::ScrollDown if inside(self.volume_area) => self.nudge_volume(-0.05),
            MouseEventKind::ScrollUp if inside(self.sidebar_area) => self.sidebar_move(-1),
            MouseEventKind::ScrollDown if inside(self.sidebar_area) => self.sidebar_move(1),
            MouseEventKind::ScrollUp => self.move_sel(-3),
            MouseEventKind::ScrollDown => self.move_sel(3),
            _ => {}
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn count(n: usize) -> String {
    format!("{n} track{}", plural(n))
}
