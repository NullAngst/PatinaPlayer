//! Keys, mouse and the terminal's own state (focus, prompts, scroll). What
//! each key does to the library, queue or config is the shared Browser's
//! job; this file only maps input to it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use patina_core::browse::{
    Browser, Change, Control, Item, Key as SettingKey, ListAction, Note, Section, SideEntry, Source,
};
use patina_core::library::PlaylistRow;
use patina_core::remote::dlna::Found;
use patina_core::{Command, Event, Snapshot};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::art::ArtView;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
}

/// Adding or editing a server takes four answers, one box at a time.
#[derive(Debug, Clone, Default)]
pub struct ServerForm {
    /// The server being edited, by its old name.
    pub old: Option<String>,
    pub name: String,
    pub url: String,
    pub user: String,
}

#[derive(Debug, Clone)]
pub enum InputPurpose {
    Search,
    ServerName(ServerForm),
    ServerUrl(ServerForm),
    ServerUser(ServerForm),
    /// The password or API key. Drawn as dots, and not trimmed.
    ServerSecret(ServerForm),
    /// A new name for a DLNA server, the one thing it has to edit.
    RenameServer(String),
    /// A new playlist on a server.
    ServerPlaylist {
        server: String,
        add: Vec<PathBuf>,
    },
    NewPlaylist {
        add: Vec<PathBuf>,
    },
    RenameMix {
        id: i64,
    },
    RenamePlaylist {
        id: i64,
    },
    /// Station name, then its URL. `edit` is the station being changed.
    StationName {
        edit: Option<usize>,
    },
    StationUrl {
        name: String,
        edit: Option<usize>,
    },
    FeedUrl,
    AddFolder,
    /// A name to save the EQ under.
    EqPresetName,
    /// An AutoEq or APO file to import.
    EqImport,
    /// A typed value for a setting.
    SetValue {
        key: SettingKey,
        label: String,
    },
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeletePlaylist(i64),
    ClearQueue,
    DeleteStation(usize),
    RemoveFeed(String),
    Setting(SettingKey, Change),
    RemoveFolder(PathBuf),
    RemoveServer(String),
    ClearCache,
    DeleteMix(i64),
}

pub enum Modal {
    None,
    Help,
    Input {
        purpose: InputPurpose,
        buf: String,
    },
    /// "New playlist…", then "New playlist on <server>" for each
    /// connected server, then the playlists you can add to.
    PickPlaylist {
        title: String,
        add: Vec<PathBuf>,
        servers: Vec<String>,
        rows: Vec<PlaylistRow>,
        selected: usize,
    },
    Confirm {
        msg: String,
        action: ConfirmAction,
    },
    /// DLNA servers a search found, to add one.
    PickDlna {
        found: Vec<Found>,
        selected: usize,
    },
}

pub struct App {
    pub b: Browser,
    pub theme: Theme,
    pub snap: Snapshot,
    pub focus: Focus,
    pub modal: Modal,
    /// Message, when it arrived, and whether it's an error.
    pub toast: Option<(String, Instant, bool)>,
    /// Keep the queue selection on the playing track until the user moves it.
    pub follow: bool,
    pub queue_panel: bool,
    pub queue_sel: usize,
    pub queue_offset: usize,
    pub art: ArtView,
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
    pub fn new(b: Browser, art: ArtView) -> Self {
        Self {
            theme: Theme::new(b.config.appearance.theme),
            b,
            snap: Snapshot::default(),
            focus: Focus::Content,
            modal: Modal::None,
            toast: None,
            follow: true,
            queue_panel: false,
            queue_sel: 0,
            queue_offset: 0,
            art,
            sidebar_area: Rect::default(),
            list_area: Rect::default(),
            seek_area: Rect::default(),
            volume_area: Rect::default(),
            last_click: None,
            quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            // A terminal that never answers the art capability query leaves
            // the query thread waiting, and it turns raw mode off when it
            // finally gives up. Keep input working regardless.
            if !ratatui::crossterm::terminal::is_raw_mode_enabled().unwrap_or(true) {
                let _ = ratatui::crossterm::terminal::enable_raw_mode();
            }
            for ev in self.b.poll() {
                if let Event::TrackChanged(Some(id)) = ev
                    && self.follow
                    && let Some(p) = self.b.player.queue().position(id)
                {
                    self.queue_sel = p;
                }
            }
            self.take_notes();
            if matches!(self.modal, Modal::None)
                && let Some(found) = self.b.take_dlna_found()
            {
                self.modal = Modal::PickDlna { found, selected: 0 };
            }
            self.snap = self.b.player.snapshot();
            if self.theme.kind != self.b.config.appearance.theme {
                self.theme = Theme::new(self.b.config.appearance.theme);
            }
            self.art.update(self.snap.track.as_ref());
            if let Some((_, t, err)) = &self.toast
                && t.elapsed() > if *err { ERROR_TIME } else { NOTE_TIME }
            {
                self.toast = None;
            }
            terminal.draw(|f| crate::ui::draw(f, self))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    TermEvent::Key(k) if k.kind != KeyEventKind::Release => self.key(k),
                    TermEvent::Mouse(m) => self.mouse(m),
                    _ => {}
                }
                self.take_notes();
            }
        }
        Ok(())
    }

    fn take_notes(&mut self) {
        for n in self.b.take_notes() {
            self.toast = Some(match n {
                Note::Info(m) => (m, Instant::now(), false),
                Note::Error(m) => (m, Instant::now(), true),
            });
        }
    }

    // ----- Selection -----

    fn move_sel(&mut self, delta: isize) {
        if self.b.section == Section::Queue {
            let n = self.b.player.queue().len();
            if n == 0 {
                return;
            }
            self.queue_sel = (self.queue_sel as isize + delta).clamp(0, n as isize - 1) as usize;
            self.follow = false;
            return;
        }
        self.b.move_selection(delta);
    }

    fn page(&self) -> isize {
        self.list_area.height.saturating_sub(3).max(1) as isize
    }

    fn activate(&mut self) {
        if self.b.section == Section::Queue {
            self.b.play_queue_position(self.queue_sel);
            self.follow = true;
            return;
        }
        if self.b.activate() == patina_core::browse::Activated::Played {
            self.follow = true;
        }
    }

    /// Play everything in the open view, in order or shuffled.
    fn play_view(&mut self, shuffle: bool) {
        self.b.play_view(shuffle);
        self.follow = true;
    }

    /// Add the selection (the selected queue entry, in the queue) to a
    /// playlist.
    fn pick_playlist(&mut self) {
        let add = if self.b.section == Section::Queue {
            self.b.queue_paths_at(self.queue_sel)
        } else {
            self.b.playlist_paths_for_selection()
        };
        self.open_pick("Add to playlist", add);
    }

    /// Choose a playlist, or a new one, for `add`.
    fn open_pick(&mut self, title: &str, add: Vec<PathBuf>) {
        if self.b.lib.is_none() {
            return self.b.error("Playlists need the library");
        }
        if add.is_empty() {
            return self.b.note("Only library songs can go in a playlist");
        }
        self.modal = Modal::PickPlaylist {
            title: format!("{title} ({})", patina_core::browse::count(add.len())),
            add,
            servers: self.b.connected_servers(),
            rows: self
                .b
                .playlists()
                .into_iter()
                .filter(|p| !p.readonly)
                .collect(),
            selected: 0,
        };
    }

    // ----- Keys -----

    fn key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('q') | KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        if ctrl && k.code == KeyCode::Char('e') && matches!(self.modal, Modal::None) {
            self.b.switch(Section::Eq);
            self.focus = Focus::Content;
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::Help => return,
            Modal::Input { purpose, buf } => return self.input_key(k, purpose, buf),
            Modal::PickPlaylist {
                title,
                add,
                servers,
                rows,
                selected,
            } => return self.pick_key(k, title, add, servers, rows, selected),
            Modal::PickDlna {
                found,
                mut selected,
            } => {
                match k.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => self.b.add_dlna_server(&found[selected]),
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = selected.saturating_sub(1);
                        self.modal = Modal::PickDlna { found, selected };
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected = (selected + 1).min(found.len() - 1);
                        self.modal = Modal::PickDlna { found, selected };
                    }
                    _ => self.modal = Modal::PickDlna { found, selected },
                }
                return;
            }
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
        let p = &self.b.player;
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
            KeyCode::Char('l') => self.b.toggle_love(),
            KeyCode::Char('q') => self.queue_panel = !self.queue_panel,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('/') if self.b.lib.is_some() => {
                let q = self.b.start_search();
                self.focus = Focus::Content;
                self.modal = Modal::Input {
                    purpose: InputPurpose::Search,
                    buf: q,
                };
            }
            KeyCode::Char(c @ '0'..='9') => {
                let i = if c == '0' {
                    9
                } else {
                    (c as u8 - b'1') as usize
                };
                self.b.switch(Section::NUMBERED[i]);
                self.focus = Focus::Content;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Content,
                    Focus::Content => Focus::Sidebar,
                };
            }
            KeyCode::F(5) => self.b.rescan(),
            KeyCode::Char('[') => self.b.nudge_tempo(-0.1),
            KeyCode::Char(']') => self.b.nudge_tempo(0.1),
            KeyCode::Char('z') => self.b.toggle_trim(),
            KeyCode::Char('.') => {
                self.follow = true;
                if let Some(cur) = self.snap.current
                    && let Some(p) = self.b.player.queue().position(cur)
                {
                    self.queue_sel = p;
                }
                self.b.switch(Section::Queue);
                self.focus = Focus::Content;
            }
            _ => return false,
        }
        true
    }

    fn sidebar_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.b.sidebar_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.b.sidebar_move(1),
            KeyCode::Enter => {
                self.b.activate_sidebar(self.b.sidebar_sel);
                self.focus = Focus::Content;
            }
            _ => {}
        }
    }

    /// Keys for Radio and Podcasts. True if the key was theirs.
    fn media_key(&mut self, k: KeyEvent) -> bool {
        let src = self.b.level().map(|l| l.source.clone());
        let item = self.b.selected_item();
        match (src, k.code) {
            (Some(Source::Section(Section::Radio)), code) => match code {
                KeyCode::Char('N') => {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::StationName { edit: None },
                        buf: String::new(),
                    }
                }
                KeyCode::Char('E' | 'D' | 'J' | 'K')
                    if matches!(
                        item,
                        Some(Item::Station {
                            server: Some(_),
                            ..
                        })
                    ) =>
                {
                    self.b
                        .note("Stations from a server are changed on the server");
                }
                KeyCode::Char('E') => {
                    if let Some(Item::Station { index, station, .. }) = item {
                        self.modal = Modal::Input {
                            purpose: InputPurpose::StationName { edit: Some(index) },
                            buf: station.name,
                        }
                    }
                }
                KeyCode::Char('D') => {
                    if let Some(Item::Station { index, station, .. }) = item {
                        self.modal = Modal::Confirm {
                            msg: format!("Delete station \"{}\"?", station.name),
                            action: ConfirmAction::DeleteStation(index),
                        }
                    }
                }
                KeyCode::Char('T') => {
                    if let Some(Item::Station { station, .. }) = item {
                        self.b.test_station(station.url);
                    }
                }
                KeyCode::Char(c @ ('J' | 'K')) => {
                    if let Some(Item::Station { index, .. }) = item {
                        self.b.move_station(index, if c == 'J' { 1 } else { -1 });
                    }
                }
                _ => return false,
            },
            (Some(Source::Section(Section::Podcasts)), code) => match code {
                KeyCode::Char('N') => {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::FeedUrl,
                        buf: String::new(),
                    }
                }
                KeyCode::Char('D') => {
                    if let Some(Item::Podcast(p)) = item {
                        self.modal = Modal::Confirm {
                            msg: format!("Unsubscribe from \"{}\"?", p.title),
                            action: ConfirmAction::RemoveFeed(p.feed_url),
                        }
                    }
                }
                _ => return false,
            },
            (Some(Source::Section(Section::Downloads)), KeyCode::Char('W')) => {
                self.b.list_action(if self.b.cache_status.paused {
                    ListAction::ResumeDownloads
                } else {
                    ListAction::PauseDownloads
                });
            }
            (Some(Source::Section(Section::Downloads)), KeyCode::Char('C')) => {
                self.ask_clear_cache()
            }
            (Some(Source::Section(Section::Downloads)), KeyCode::Enter)
                if matches!(
                    item,
                    Some(Item::Action {
                        action: ListAction::ClearCache,
                        ..
                    })
                ) =>
            {
                self.ask_clear_cache();
            }
            (Some(Source::Section(Section::Downloads)), code)
                if matches!(item, Some(Item::Job(_) | Item::Pinned(_))) =>
            {
                match (item, code) {
                    (Some(Item::Job(j)), KeyCode::Char('w')) => self.b.toggle_job_pause(&j),
                    (Some(Item::Job(j)), KeyCode::Char('x') | KeyCode::Delete) => {
                        self.b.cancel_job(&j)
                    }
                    (Some(Item::Pinned(p)), KeyCode::Char('x' | 'd') | KeyCode::Delete) => {
                        self.b.unpin(p.pin.id)
                    }
                    _ => return false,
                }
            }
            (Some(Source::Podcast { .. } | Source::Section(Section::Downloads)), code) => {
                let Some(Item::Episode(e)) = item else {
                    return false;
                };
                match code {
                    KeyCode::Char('w') => self.b.toggle_download(&e),
                    KeyCode::Char('x') => self.b.delete_download(e.id),
                    KeyCode::Char('P') => self.b.toggle_played(&e),
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    fn content_key(&mut self, k: KeyEvent) {
        if self.media_key(k) {
            return;
        }
        if matches!(self.b.section, Section::Settings | Section::Eq) && self.settings_key(k) {
            return;
        }
        let queue = self.b.section == Section::Queue;
        let playlist = self.b.in_playlist().is_some();
        let lists = self.b.section == Section::Playlists && !playlist;
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-self.page()),
            KeyCode::PageDown => self.move_sel(self.page()),
            KeyCode::Home | KeyCode::Char('g') => self.move_sel(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_sel(isize::MAX / 2),
            KeyCode::Enter => self.activate(),
            KeyCode::Backspace | KeyCode::Esc => {
                self.b.pop();
            }
            KeyCode::Char('v') if !queue => self.play_view(false),
            KeyCode::Char('V') if !queue => self.play_view(true),
            KeyCode::Char('e') if !queue => self.b.queue_view(),
            KeyCode::Char('P') if !queue => {
                let add = self.b.view_playlist_paths();
                self.open_pick("Add this view to a playlist", add);
            }
            KeyCode::Char('a') if !queue => self.b.add_selected_to_queue(false),
            KeyCode::Char('A') if !queue => self.b.add_selected_to_queue(true),
            KeyCode::Char('p') => self.pick_playlist(),
            KeyCode::Char('X') if !queue => self.b.mix_selected(),
            KeyCode::Char('M') if queue => self.b.save_mix(),
            KeyCode::Char('M') if self.b.section == Section::Genres => match self.b.selected_item()
            {
                Some(Item::Genre(g)) => self.b.toggle_genre_mark(&g),
                _ => self.b.note("M marks a genre for a mix"),
            },
            KeyCode::Char('R') if lists && matches!(self.b.selected_item(), Some(Item::Mix(_))) => {
                if let Some(Item::Mix(m)) = self.b.selected_item() {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::RenameMix { id: m.id },
                        buf: m.name,
                    }
                }
            }
            KeyCode::Char('D') if lists && matches!(self.b.selected_item(), Some(Item::Mix(_))) => {
                if let Some(Item::Mix(m)) = self.b.selected_item() {
                    self.modal = Modal::Confirm {
                        msg: format!("Delete the saved mix \"{}\"?", m.name),
                        action: ConfirmAction::DeleteMix(m.id),
                    }
                }
            }
            KeyCode::Char('f') if !queue => self.b.toggle_pin_selected(),
            KeyCode::Char('F') if !queue => {
                if self.b.view_pin_state().is_some() {
                    self.b.toggle_pin_view();
                } else {
                    self.b.note(
                        "Open an album, artist, playlist or server folder to keep it offline",
                    );
                }
            }
            KeyCode::Char('o') => self.b.cycle_sort(),
            KeyCode::Char('O') => self.b.reverse_songs(),
            KeyCode::Char('t') => self.b.toggle_mode(),

            // Queue editing.
            KeyCode::Char('d') | KeyCode::Delete if queue => {
                self.b.remove_queue_position(self.queue_sel)
            }
            KeyCode::Char('J') if queue => {
                self.queue_sel = self.b.move_queue_position(self.queue_sel, 1);
                self.follow = false;
            }
            KeyCode::Char('K') if queue => {
                self.queue_sel = self.b.move_queue_position(self.queue_sel, -1);
                self.follow = false;
            }
            KeyCode::Char('C') if queue => {
                self.modal = Modal::Confirm {
                    msg: "Clear the whole queue?".into(),
                    action: ConfirmAction::ClearQueue,
                }
            }
            KeyCode::Char('L') if queue => match self.b.queue_servers().first() {
                Some(s) => {
                    let s = s.clone();
                    self.b.load_server_queue(&s);
                }
                None => self.b.note("No connected server keeps a play queue"),
            },
            KeyCode::Char('S') if queue => {
                let add = self.b.queue_playlist_paths();
                self.open_pick("Add the queue to a playlist", add);
            }

            // Playlist editing.
            KeyCode::Char('d') | KeyCode::Delete if playlist => self.b.edit_playlist(true, 0),
            KeyCode::Char('J') if playlist => self.b.edit_playlist(false, 1),
            KeyCode::Char('K') if playlist => self.b.edit_playlist(false, -1),
            KeyCode::Char('N') if lists => {
                self.modal = Modal::Input {
                    purpose: InputPurpose::NewPlaylist { add: Vec::new() },
                    buf: String::new(),
                }
            }
            KeyCode::Char('R') if lists => {
                if let Some(Item::Playlist(p)) = self.b.selected_item() {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::RenamePlaylist { id: p.id },
                        buf: p.name,
                    }
                }
            }
            KeyCode::Char('D') if lists => {
                if let Some(Item::Playlist(p)) = self.b.selected_item() {
                    self.modal = Modal::Confirm {
                        msg: format!("Delete playlist \"{}\"?", p.name),
                        action: ConfirmAction::DeletePlaylist(p.id),
                    }
                }
            }
            _ => {}
        }
    }

    /// Settings: Enter flips, cycles or edits; h and l step back and
    /// forward; N adds a library folder; d removes one.
    fn settings_key(&mut self, k: KeyEvent) -> bool {
        let st = self.b.selected_setting();
        match (k.code, st) {
            (KeyCode::Char('N'), _) => {
                self.modal = Modal::Input {
                    purpose: InputPurpose::AddFolder,
                    buf: "~/".into(),
                };
            }
            (KeyCode::Enter, Some(st)) if st.disabled.is_some() => {
                self.b.note(st.disabled.unwrap_or_default());
            }
            (KeyCode::Enter, Some(st)) if st.key == SettingKey::MeasureLibrary => {
                self.b.toggle_measure_library();
            }
            (KeyCode::Enter, Some(st)) if st.key == SettingKey::EqSavePreset => {
                let cur = self.b.eq_settings().preset.clone();
                self.modal = Modal::Input {
                    purpose: InputPurpose::EqPresetName,
                    buf: if patina_core::eq::presets::is_builtin(&cur) {
                        String::new()
                    } else {
                        cur
                    },
                };
            }
            (KeyCode::Enter, Some(st)) if st.key == SettingKey::EqImport => {
                self.modal = Modal::Input {
                    purpose: InputPurpose::EqImport,
                    buf: "~/".into(),
                };
            }
            (KeyCode::Enter, Some(st))
                if matches!(
                    st.key,
                    SettingKey::EqDevice
                        | SettingKey::EqAddBand
                        | SettingKey::EqDeletePreset
                        | SettingKey::LastFmLink
                ) =>
            {
                self.b.change_setting(&st.key, Change::Toggle);
            }
            (KeyCode::Char('o'), _) if self.b.section == Section::Eq => self.b.eq_toggle(),
            (KeyCode::Char('H'), Some(st)) if matches!(st.key, SettingKey::EqBand(_)) => {
                if let SettingKey::EqBand(i) = st.key {
                    self.b.eq_nudge_freq(i, -1.0);
                }
            }
            (KeyCode::Char('L'), Some(st)) if matches!(st.key, SettingKey::EqBand(_)) => {
                if let SettingKey::EqBand(i) = st.key {
                    self.b.eq_nudge_freq(i, 1.0);
                }
            }
            (KeyCode::Char('d') | KeyCode::Delete, Some(st))
                if matches!(st.key, SettingKey::EqBand(_)) =>
            {
                if let SettingKey::EqBand(i) = st.key {
                    self.b.eq_remove_band(i);
                }
            }
            (KeyCode::Enter, Some(st)) => match &st.control {
                Control::Toggle(_) => self.change_setting(st.key, Change::Toggle),
                Control::Choice { .. } => self.change_setting(st.key, Change::Step(1)),
                Control::Number { value, scale, .. } => {
                    let buf = if *scale == 1 {
                        value.to_string()
                    } else {
                        format!("{:.1}", *value as f64 / *scale as f64)
                    };
                    self.modal = Modal::Input {
                        purpose: InputPurpose::SetValue {
                            key: st.key,
                            label: st.label,
                        },
                        buf,
                    };
                }
                Control::Text { value, .. } => {
                    // A secret is typed fresh, never shown.
                    let buf = if st.key == SettingKey::LastFmSecret {
                        String::new()
                    } else {
                        value.clone()
                    };
                    self.modal = Modal::Input {
                        purpose: InputPurpose::SetValue {
                            key: st.key,
                            label: st.label,
                        },
                        buf,
                    };
                }
                Control::Action(_) if st.key == SettingKey::FindDlna => self.b.find_dlna(),
                Control::Action(_) if st.key == SettingKey::AddServer => {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::ServerName(ServerForm::default()),
                        buf: String::new(),
                    };
                }
                Control::Action(_) => {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::AddFolder,
                        buf: "~/".into(),
                    };
                }
                Control::Server(_) => {
                    if let SettingKey::Server(name) = &st.key {
                        self.b.sync_server(Some(name));
                    }
                }
                Control::Folder | Control::Info(_) => {}
            },
            (KeyCode::Char('E'), Some(st)) => {
                let SettingKey::Server(name) = &st.key else {
                    return false;
                };
                let Some(sc) = self.b.server(name).cloned() else {
                    return false;
                };
                if !sc.kind.logs_in() {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::RenameServer(sc.name.clone()),
                        buf: sc.name,
                    };
                    return true;
                }
                self.modal = Modal::Input {
                    purpose: InputPurpose::ServerName(ServerForm {
                        old: Some(sc.name.clone()),
                        name: sc.name.clone(),
                        url: sc.url,
                        user: sc.user,
                    }),
                    buf: sc.name,
                };
            }
            (KeyCode::Char('h'), Some(st)) => self.change_setting(st.key, Change::Step(-1)),
            (KeyCode::Char('l'), Some(st)) => self.change_setting(st.key, Change::Step(1)),
            (KeyCode::Char('d') | KeyCode::Delete, Some(st)) => match st.key {
                SettingKey::Folder(p) => {
                    self.modal = Modal::Confirm {
                        msg: format!(
                            "Take {} out of the library? The files stay where they are.",
                            p.display()
                        ),
                        action: ConfirmAction::RemoveFolder(p),
                    };
                }
                SettingKey::Server(name) => {
                    self.modal = Modal::Confirm {
                        msg: format!("Remove {name}? Its songs leave the library here."),
                        action: ConfirmAction::RemoveServer(name),
                    };
                }
                _ => {}
            },
            _ => return false,
        }
        true
    }

    /// Change a setting, asking first if it needs a yes.
    fn change_setting(&mut self, key: SettingKey, change: Change) {
        match self.b.setting_warning(&key, &change) {
            Some(w) => {
                self.modal = Modal::Confirm {
                    msg: w.to_string(),
                    action: ConfirmAction::Setting(key, change),
                }
            }
            None => self.b.change_setting(&key, change),
        }
    }

    fn input_key(&mut self, k: KeyEvent, purpose: InputPurpose, mut buf: String) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let search = matches!(purpose, InputPurpose::Search);
        match k.code {
            KeyCode::Esc => {
                if search {
                    self.b.pop();
                }
                return;
            }
            KeyCode::Enter => {
                // A password is taken exactly as typed.
                let text = if matches!(purpose, InputPurpose::ServerSecret(_)) {
                    buf
                } else {
                    buf.trim().to_string()
                };
                self.submit_input(purpose, text);
                return;
            }
            // Arrow keys move through live search results while typing.
            KeyCode::Up if search => self.move_sel(-1),
            KeyCode::Down if search => self.move_sel(1),
            KeyCode::Backspace => {
                buf.pop();
                if search {
                    self.b.set_search(&buf);
                }
            }
            KeyCode::Char('u') if ctrl => {
                buf.clear();
                if search {
                    self.b.set_search(&buf);
                }
            }
            KeyCode::Char(c) if !ctrl => {
                buf.push(c);
                if search {
                    self.b.set_search(&buf);
                }
            }
            _ => {}
        }
        self.modal = Modal::Input { purpose, buf };
    }

    fn submit_input(&mut self, purpose: InputPurpose, text: String) {
        match purpose {
            InputPurpose::StationName { edit } => {
                if text.is_empty() {
                    return;
                }
                let url = edit
                    .and_then(|i| self.b.stations.get(i))
                    .map(|s| s.url.clone())
                    .unwrap_or_default();
                self.modal = Modal::Input {
                    purpose: InputPurpose::StationUrl { name: text, edit },
                    buf: url,
                };
            }
            InputPurpose::StationUrl { name, edit } => {
                if !text.is_empty() {
                    self.b.save_station(edit, &name, &text);
                }
            }
            InputPurpose::FeedUrl => self.b.subscribe(&text),
            InputPurpose::AddFolder => {
                if !self.b.add_folder(&text) && !text.is_empty() {
                    // Keep the box up to fix a typo.
                    self.modal = Modal::Input {
                        purpose: InputPurpose::AddFolder,
                        buf: text,
                    };
                }
            }
            InputPurpose::SetValue { key, .. } => self.change_setting(key, Change::Set(text)),
            InputPurpose::EqPresetName => self.b.eq_save_preset(&text),
            InputPurpose::EqImport => self
                .b
                .eq_import(&patina_core::config::expand_tilde(text.trim())),
            // Enter keeps the results on screen for browsing.
            InputPurpose::Search => {}
            InputPurpose::NewPlaylist { add } => self.b.create_playlist(&text, &add),
            InputPurpose::ServerPlaylist { server, add } => {
                self.b.create_server_playlist(&server, &text, &add)
            }
            InputPurpose::RenameServer(old) => self.b.edit_server(&old, &text, "", "", None),
            InputPurpose::ServerName(mut f) => {
                if text.is_empty() {
                    return;
                }
                f.name = text;
                let buf = f.url.clone();
                self.modal = Modal::Input {
                    purpose: InputPurpose::ServerUrl(f),
                    buf,
                };
            }
            InputPurpose::ServerUrl(mut f) => {
                if text.is_empty() {
                    return;
                }
                f.url = text;
                let buf = f.user.clone();
                self.modal = Modal::Input {
                    purpose: InputPurpose::ServerUser(f),
                    buf,
                };
            }
            InputPurpose::ServerUser(mut f) => {
                f.user = text;
                self.modal = Modal::Input {
                    purpose: InputPurpose::ServerSecret(f),
                    buf: String::new(),
                };
            }
            InputPurpose::ServerSecret(f) => match &f.old {
                Some(old) => {
                    let secret = (!text.is_empty()).then_some(text.as_str());
                    self.b.edit_server(old, &f.name, &f.url, &f.user, secret);
                }
                None => self.b.add_server(&f.name, &f.url, &f.user, &text),
            },
            InputPurpose::RenamePlaylist { id } => self.b.rename_playlist(id, &text),
            InputPurpose::RenameMix { id } => self.b.rename_mix(id, &text),
        }
    }

    fn pick_key(
        &mut self,
        k: KeyEvent,
        title: String,
        add: Vec<PathBuf>,
        servers: Vec<String>,
        rows: Vec<PlaylistRow>,
        mut selected: usize,
    ) {
        let n = rows.len() + servers.len() + 1;
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
                } else if selected <= servers.len() {
                    self.modal = Modal::Input {
                        purpose: InputPurpose::ServerPlaylist {
                            server: servers[selected - 1].clone(),
                            add,
                        },
                        buf: String::new(),
                    };
                } else {
                    let p = &rows[selected - 1 - servers.len()];
                    self.b.add_to_playlist(p.id, &p.name, &add);
                }
                return;
            }
            _ => {}
        }
        self.modal = Modal::PickPlaylist {
            title,
            add,
            servers,
            rows,
            selected,
        };
    }

    fn confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteStation(i) => self.b.delete_station(i),
            ConfirmAction::RemoveFeed(url) => self.b.unsubscribe(&url),
            ConfirmAction::ClearQueue => self.b.clear_queue(),
            ConfirmAction::DeletePlaylist(id) => self.b.delete_playlist(id),
            ConfirmAction::Setting(key, change) => self.b.change_setting(&key, change),
            ConfirmAction::RemoveFolder(p) => self.b.remove_folder(&p),
            ConfirmAction::RemoveServer(n) => self.b.remove_server(&n),
            ConfirmAction::ClearCache => self.b.list_action(ListAction::ClearCache),
            ConfirmAction::DeleteMix(id) => self.b.delete_mix(id),
        }
    }

    fn ask_clear_cache(&mut self) {
        self.modal = Modal::Confirm {
            msg: "Clear the song cache? Pinned songs stay.".into(),
            action: ConfirmAction::ClearCache,
        };
    }

    fn nudge_volume(&self, d: f64) {
        self.b
            .player
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
                    self.b
                        .player
                        .send(Command::Seek(d.mul_f64(frac.clamp(0.0, 1.0))));
                }
            }
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
                if inside(self.volume_area) =>
            {
                let frac = (x - self.volume_area.x) as f64 / self.volume_area.width.max(1) as f64;
                self.b.player.send(Command::SetVolume(frac.clamp(0.0, 1.0)));
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.sidebar_area) => {
                let row = (y - self.sidebar_area.y) as usize;
                if let Some(e) = self.b.sidebar.get(row)
                    && matches!(
                        e,
                        SideEntry::Section(_) | SideEntry::Source { .. } | SideEntry::Mix { .. }
                    )
                {
                    self.b.activate_sidebar(row);
                    self.focus = Focus::Content;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.list_area) => {
                // Row 0 is the title, row 1 the column header.
                if y < self.list_area.y + 2 {
                    return;
                }
                let row = (y - self.list_area.y - 2) as usize;
                self.focus = Focus::Content;
                if self.b.section == Section::Queue {
                    let pos = self.queue_offset + row;
                    if pos < self.b.player.queue().len() {
                        self.queue_sel = pos;
                        self.follow = false;
                        if double {
                            self.activate();
                        }
                    }
                } else {
                    let pos = self.b.level().map(|l| l.offset + row).unwrap_or(0);
                    if self.b.select(pos) && double {
                        self.activate();
                    }
                }
                self.last_click = if double {
                    None
                } else {
                    Some((Instant::now(), x, y))
                };
            }
            MouseEventKind::Down(MouseButton::Right) if inside(self.list_area) => {
                self.b.pop();
            }
            MouseEventKind::ScrollUp if inside(self.volume_area) => self.nudge_volume(0.05),
            MouseEventKind::ScrollDown if inside(self.volume_area) => self.nudge_volume(-0.05),
            MouseEventKind::ScrollUp if inside(self.sidebar_area) => self.b.sidebar_move(-1),
            MouseEventKind::ScrollDown if inside(self.sidebar_area) => self.b.sidebar_move(1),
            MouseEventKind::ScrollUp => self.move_sel(-3),
            MouseEventKind::ScrollDown => self.move_sel(3),
            _ => {}
        }
    }
}
