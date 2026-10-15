//! Between the Slint window and the shared Browser: turns Browser state
//! into the models the window draws, and every click and key into a
//! Browser or player call. Nothing here decides what an action means for
//! the library or queue; that's patina-core's job, shared with the TUI.

use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use patina_core::browse::{
    Activated, Browser, Change, Control, Item, Key as SettingKey, ListAction, Note, Section,
    SideEntry, Source, fmt_duration, fmt_length,
};
use patina_core::config::Theme;
use patina_core::library::PlaylistRow;
use patina_core::remote::ServerState;
use patina_core::track::MediaKind;
use patina_core::{Command, Event, PlayState, Repeat, Snapshot, TrackInfo};
use slint::platform::Key;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::art::{ArtCache, LARGE, Req, THUMB};
use crate::{AppWindow, Hero, Icons, P, QueueRow, Row, SideItem};

enum Dlg {
    None,
    /// Picking one of a setting's choices.
    Choose(SettingKey),
    /// Typing a setting's value.
    SetValue(SettingKey),
    AddFolder,
    NewPlaylist(Vec<PathBuf>),
    RenamePlaylist(i64),
    Station(Option<usize>),
    Feed,
    /// Adding a server, or editing the one with this name.
    Server(Option<String>),
    /// Renaming a DLNA server, which has nothing else to edit.
    RenameServer(String),
    /// Picking one of the DLNA servers a search found.
    Dlna(Vec<patina_core::remote::dlna::Found>),
    /// The paths, the servers offering "New playlist on…", the playlists.
    Pick(Vec<PathBuf>, Vec<String>, Vec<PlaylistRow>),
    /// Naming a new playlist on a server.
    ServerPlaylist(String, Vec<PathBuf>),
    Confirm(Confirm),
    Help,
}

enum Confirm {
    /// A settings change that needs a yes first (bit-perfect).
    Setting(SettingKey, Change),
    RemoveFolder(PathBuf),
    DeletePlaylist(i64),
    ClearQueue,
    DeleteStation(usize),
    Unsubscribe(String),
    RemoveServer(String),
    ClearCache,
}

pub struct State {
    b: Browser,
    art: ArtCache,
    snap: Snapshot,
    rows: Rc<VecModel<Row>>,
    side: Rc<VecModel<SideItem>>,
    queue: Rc<VecModel<QueueRow>>,
    crumbs: Rc<VecModel<SharedString>>,
    columns: Rc<VecModel<SharedString>>,
    actions: Rc<VecModel<SharedString>>,
    action_labels: Rc<VecModel<SharedString>>,
    dlg_items: Rc<VecModel<SharedString>>,
    queue_servers: Rc<VecModel<SharedString>>,
    last_gen: u64,
    last_queue_ver: u64,
    /// Identity of the open level, to scroll back to the top on change.
    level_key: String,
    queue_sel: usize,
    follow: bool,
    dlg: Dlg,
    toast_until: Option<Instant>,
    /// Art each row and queue entry would show. Only the rows on screen,
    /// plus a margin, actually get theirs, so a 5000-track queue or a
    /// 2000-album grid doesn't load (and keep reloading) every cover.
    row_art: Vec<Option<Req>>,
    queue_art: Vec<Option<Req>>,
    /// Row ranges that have art set right now.
    row_shown: Vec<Range<usize>>,
    queue_shown: Vec<Range<usize>>,
    /// A native folder picker running on its own thread, and when it was
    /// opened.
    folder_pick: Option<(crossbeam_channel::Receiver<Option<PathBuf>>, Instant)>,
    /// The theme on screen, so a change in Settings redraws.
    theme: Option<Theme>,
}

const HELP: &str = "Space        play / pause
n / b        next / back
Left Right   seek 5 s (Shift: 30 s)
+ / -  m     volume, mute
s  r         shuffle, cycle repeat
1 to 9, 0    Folders, Artists, Albums, Songs, Genres,
             Playlists, Radio, Podcasts, Queue, Downloads
/  Ctrl+F    search
Enter        open, or play from here
v / V        play this whole view in order / shuffled
Backspace    back a level (Alt+Left / Alt+Right: back, forward)
a / A        add to queue / play next
p            add to a playlist
f / F        keep the selection / this whole view offline, or stop
Delete       remove from the queue, the open playlist, or Settings;
             in Downloads: cancel a download or unpin
Enter        in Settings: flip, pick or edit the option
[ ]  z       podcast speed, silence trimming
q            queue panel
F5           check folders and feeds for changes
Ctrl+Q       quit

Right-click anything for everything else you can do with it.";

/// What a row's columns show, which depends on the list it's in.
#[derive(Clone, Copy, PartialEq)]
enum RowCtx {
    Plain,
    /// Inside an album: track numbers, and plays instead of the album.
    Album,
    /// A listening-stats list: plays on the right.
    Ranked,
}

/// The kawaii themes' Japanese names for the sidebar, in kana so they stay
/// in the bundled rounded font.
fn jp_section(s: Section) -> &'static str {
    match s {
        Section::Folders => "フォルダ",
        Section::Artists => "アーティスト",
        Section::Albums => "アルバム",
        Section::Songs => "きょく",
        Section::Genres => "ジャンル",
        Section::Playlists => "プレイリスト",
        Section::Radio => "ラジオ",
        Section::Podcasts => "ポッドキャスト",
        Section::Queue => "キュー",
        Section::Downloads => "ダウンロード",
        Section::Settings => "せってい",
        Section::MostListened => "いちばん",
        Section::TopAlbums => "ベスト",
        Section::TopArtists => "スター",
    }
}

fn jp_label(l: &str) -> &'static str {
    match l {
        "Listening" => "きいた",
        "Sources" => "ソース",
        _ => "",
    }
}

fn plays(n: i64) -> String {
    format!("{n} play{}", s_(n))
}

fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

fn key_str(k: Key) -> SharedString {
    k.into()
}

fn s_(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Civil date (UTC) from Unix seconds, for episode rows.
fn date(ts: i64) -> String {
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

impl State {
    fn new(b: Browser) -> Self {
        Self {
            b,
            art: ArtCache::new(),
            snap: Snapshot::default(),
            rows: Rc::new(VecModel::default()),
            side: Rc::new(VecModel::default()),
            queue: Rc::new(VecModel::default()),
            crumbs: Rc::new(VecModel::default()),
            columns: Rc::new(VecModel::default()),
            actions: Rc::new(VecModel::default()),
            action_labels: Rc::new(VecModel::default()),
            dlg_items: Rc::new(VecModel::default()),
            queue_servers: Rc::new(VecModel::default()),
            last_gen: u64::MAX,
            last_queue_ver: u64::MAX,
            level_key: String::new(),
            queue_sel: 0,
            follow: true,
            dlg: Dlg::None,
            toast_until: None,
            row_art: Vec::new(),
            queue_art: Vec::new(),
            row_shown: Vec::new(),
            queue_shown: Vec::new(),
            folder_pick: None,
            theme: None,
        }
    }

    fn art_req(item: &Item) -> Option<Req> {
        match item {
            Item::Track(t) => Some(Req::Track(t.path.clone())),
            Item::Album(a) => a.cover_track.clone().map(Req::Track),
            Item::Dir { path, .. } => Some(Req::Dir(path.clone())),
            _ => None,
        }
    }

    fn playing_path(&self) -> Option<PathBuf> {
        self.snap
            .track
            .as_ref()
            .filter(|t| !t.path.as_os_str().is_empty())
            .map(|t| t.path.clone())
    }

    fn row(&mut self, item: &Item, i: usize, ctx: RowCtx) -> Row {
        let mut r = Row {
            selectable: item.selectable(),
            num: ss((i + 1).to_string()),
            ..Default::default()
        };
        let playing = self.playing_path();
        match item {
            Item::Track(t) => {
                r.kind = 0;
                if ctx == RowCtx::Album {
                    r.num = ss(t.track_no.map(|n| n.to_string()).unwrap_or_default());
                    r.meta = ss(match t.play_count {
                        0 => String::new(),
                        n => plays(n),
                    });
                } else {
                    r.meta = ss(t.album.clone().unwrap_or_default());
                }
                r.title = ss(&t.title);
                r.sub = ss(t.artist.clone().unwrap_or_default());
                r.right = ss(if ctx == RowCtx::Ranked {
                    plays(t.play_count)
                } else {
                    t.duration.map(fmt_duration).unwrap_or_default()
                });
                r.playing = playing.as_deref() == Some(t.path.as_path());
            }
            Item::Missing { uri } => {
                r.kind = 11;
                let name =
                    String::from_utf8_lossy(uri.strip_prefix(b"file:").unwrap_or(uri)).into_owned();
                r.title = ss(std::path::Path::new(&name)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(name));
                r.warn = true;
            }
            Item::Album(a) => {
                r.kind = 1;
                r.title = ss(&a.name);
                r.sub = ss(a.artist_line());
                r.meta = ss(a.year.map(|y| y.to_string()).unwrap_or_default());
                r.right = ss(if ctx == RowCtx::Ranked {
                    plays(a.play_count)
                } else {
                    format!("{} · {}", a.track_count, fmt_length(a.duration))
                });
            }
            Item::Artist(a) => {
                r.kind = 2;
                r.title = ss(&a.name);
                r.sub = ss(format!("{} album{}", a.albums, s_(a.albums)));
                r.right = ss(if ctx == RowCtx::Ranked {
                    plays(a.plays)
                } else {
                    format!("{} track{}", a.tracks, s_(a.tracks))
                });
            }
            Item::Dir { name, .. } => {
                r.kind = 3;
                r.title = ss(name);
            }
            Item::Root { path, tracks } => {
                r.kind = 3;
                r.title = ss(match patina_core::remote::server_of(path) {
                    Some(name) => name.to_string(),
                    None => path.display().to_string(),
                });
                if patina_core::remote::is_remote(path) {
                    r.sub = ss("server");
                }
                r.right = ss(format!("{tracks} track{}", s_(*tracks)));
            }
            Item::Genre(g) => {
                r.kind = 4;
                r.title = ss(&g.name);
                r.sub = ss(format!("{} album{}", g.albums, s_(g.albums)));
                r.right = ss(format!("{} track{}", g.tracks, s_(g.tracks)));
            }
            Item::Playlist(p) => {
                r.kind = 5;
                r.title = ss(&p.name);
                r.sub = ss(match (&p.server, p.readonly) {
                    (Some(s), true) => format!("on {s}, read-only"),
                    (Some(s), false) => format!("on {s}"),
                    _ => String::new(),
                });
                r.right = ss(format!("{} track{}", p.items, s_(p.items)));
            }
            Item::Station { station, check, .. } => {
                r.kind = 6;
                r.title = ss(&station.name);
                match check {
                    Some(c) => {
                        r.warn = c.starts_with("Not working") || c.contains("no stream answered");
                        r.sub = ss(c);
                    }
                    None => r.sub = ss(&station.url),
                }
                r.playing =
                    self.snap.track.as_ref().is_some_and(|t| {
                        t.is_live() && t.url.as_deref() == Some(station.url.as_str())
                    });
            }
            Item::Podcast(p) => {
                r.kind = 7;
                r.title = ss(&p.title);
                r.sub = ss(p.author.clone().unwrap_or_default());
                r.right = ss(match (&p.last_error, p.episodes) {
                    (Some(_), 0) => "feed error".to_string(),
                    (_, 0) => "not fetched".to_string(),
                    (_, n) => format!("{} new of {n}", p.unplayed),
                });
                r.warn = p.last_error.is_some() && p.episodes == 0;
            }
            Item::Episode(e) => {
                r.kind = 8;
                r.title = ss(&e.title);
                r.sub = ss(e.published.map(date).unwrap_or_default());
                use patina_core::cache::EpisodeState;
                let dl = self.b.episode_download(e.id);
                r.meta = ss(
                    if let Some(EpisodeState::Downloading { done, total }) = dl {
                        match total {
                            Some(t) if t > 0 => format!("downloading {}%", done * 100 / t),
                            _ => format!("downloading {} MB", done / 1_000_000),
                        }
                    } else if dl == Some(EpisodeState::Paused) {
                        "download paused".into()
                    } else if dl.is_some() {
                        "waiting to download".into()
                    } else if e.played {
                        "played".into()
                    } else if e.position > Duration::from_secs(10) {
                        match e.duration {
                            Some(d) if d > e.position => {
                                format!("{} left", fmt_duration(d - e.position))
                            }
                            _ => format!("at {}", fmt_duration(e.position)),
                        }
                    } else if e.download.is_some() {
                        "new · downloaded".into()
                    } else {
                        "new".into()
                    },
                );
                r.right = ss(e.duration.map(fmt_duration).unwrap_or_default());
                r.dim = e.played;
                r.downloaded = e.download.is_some();
                r.playing = self.snap.track.as_ref().and_then(|t| t.episode) == Some(e.id);
            }
            Item::Header(h) => {
                r.kind = 9;
                r.title = ss(h.to_uppercase());
            }
            Item::Info { label, value } => {
                r.kind = 10;
                r.num = SharedString::new();
                r.title = ss(label);
                r.sub = ss(value);
            }
            Item::Setting(st) => {
                r.kind = 12;
                r.num = SharedString::new();
                r.title = ss(&st.label);
                r.sub = ss(st.control.display());
                r.meta = ss(st.help);
                (r.ctl, r.on) = match &st.control {
                    Control::Toggle(on) => (1, *on),
                    Control::Choice { .. } => (2, false),
                    Control::Number { .. } => (3, false),
                    Control::Text { value, .. } => (4, !value.is_empty()),
                    Control::Folder => (5, false),
                    Control::Action(_) if st.key == SettingKey::AddServer => (9, false),
                    Control::Action(_) if st.key == SettingKey::FindDlna => (10, false),
                    Control::Action(_) => (6, false),
                    Control::Info(_) => (7, false),
                    Control::Server(_) => (8, false),
                };
                if matches!(st.control, Control::Folder) {
                    r.meta = SharedString::new();
                }
                // A server's row: its status line under its name.
                if matches!(st.control, Control::Server(_)) {
                    r.meta = r.sub.clone();
                }
            }
            Item::Pinned(p) => {
                r.kind = 13;
                r.num = SharedString::new();
                r.title = ss(&p.pin.name);
                r.sub = ss(p.pin.kind.label());
                r.meta = ss(format!(
                    "{} of {} song{}",
                    p.stored,
                    p.songs,
                    s_(p.songs as i64)
                ));
                r.right = ss(patina_core::browse::fmt_bytes(p.bytes));
                r.downloaded = p.stored >= p.songs;
            }
            Item::Job(j) => {
                r.kind = 14;
                r.num = SharedString::new();
                r.title = ss(&j.label);
                r.sub = ss(j.kind.label());
                r.meta = ss(j.state());
                r.playing = j.active;
                r.warn = j.error.is_some();
                r.on = j.paused;
            }
            Item::Action { label, .. } => {
                r.kind = 15;
                r.num = SharedString::new();
                r.title = ss(label);
            }
        }
        r.off = !self.b.plays(item);
        r.can_pin = self.b.can_pin(item);
        r.pinned = r.can_pin && self.b.pinned(item).is_some();
        r
    }

    /// Columns, title actions and layout for the open level. Every view
    /// that can be played as a whole gets Play and Shuffle up front.
    fn chrome(&self) -> (i32, Vec<&'static str>, Vec<(&'static str, String)>) {
        let (layout, cols, mut acts) = self.chrome_base();
        // An album, artist, playlist or folder with server songs in it can
        // be kept offline whole.
        if let Some(pinned) = self.b.view_pin_state() {
            acts.push((
                "pin",
                if pinned {
                    "Kept offline".into()
                } else {
                    "Keep offline".into()
                },
            ));
        }
        if self.b.can_play_view() && !acts.iter().any(|(a, _)| *a == "play") {
            acts.splice(
                0..0,
                [
                    ("play", "Play".into()),
                    ("shuffle", "Shuffle".into()),
                    ("queue", "Add to queue".into()),
                    ("playlist", "Add to playlist".into()),
                ],
            );
        }
        (layout, cols, acts)
    }

    fn chrome_base(&self) -> (i32, Vec<&'static str>, Vec<(&'static str, String)>) {
        let src = self.b.level().map(|l| l.source.clone());
        let prefs = &self.b.prefs;
        let tracks_cols = vec!["#", "Title", "Artist", "Album", "Time"];
        match (self.b.section, src) {
            (Section::Queue, _) => (2, vec![], vec![]),
            (_, Some(Source::Album { .. })) => (
                0,
                vec!["#", "Title", "Artist", "Plays", "Time"],
                vec![
                    ("play", "Play".into()),
                    ("shuffle", "Shuffle".into()),
                    ("queue", "Add to queue".into()),
                    ("playlist", "Add to playlist".into()),
                ],
            ),
            (Section::Albums, Some(Source::Section(_))) => (
                1,
                vec![],
                vec![("sort", format!("Sort: {}", prefs.album_sort.label()))],
            ),
            (_, Some(Source::ArtistAlbums(_))) => (1, vec![], vec![]),
            (_, Some(Source::Genre { .. })) if !prefs.genre_songs => {
                (1, vec![], vec![("mode", "Show songs".into())])
            }
            (_, Some(Source::Genre { .. })) => {
                (0, tracks_cols, vec![("mode", "Show albums".into())])
            }
            (Section::MostListened, Some(Source::Section(_))) => (0, vec![], vec![]),
            (Section::TopAlbums, Some(Source::Section(_))) => {
                (0, vec!["#", "Album", "Artist", "Year", "Plays"], vec![])
            }
            (Section::TopArtists, Some(Source::Section(_))) => {
                (0, vec!["#", "Artist", "Albums", "", "Plays"], vec![])
            }
            (Section::Songs, Some(Source::Section(_))) => (
                0,
                tracks_cols,
                vec![
                    ("sort", format!("Sort: {}", prefs.song_sort.label())),
                    (
                        "reverse",
                        if prefs.song_desc {
                            "Reversed".into()
                        } else {
                            "Reverse".into()
                        },
                    ),
                ],
            ),
            (Section::Artists, Some(Source::Section(_))) => (
                0,
                vec!["", "Artist", "Albums", "", "Tracks"],
                vec![(
                    "mode",
                    match prefs.artist_mode {
                        patina_core::library::ArtistMode::AlbumArtist => "By album artist".into(),
                        patina_core::library::ArtistMode::TrackArtist => "By track artist".into(),
                    },
                )],
            ),
            (Section::Playlists, Some(Source::Section(_))) => (
                0,
                vec!["", "Playlist", "", "", "Tracks"],
                vec![("new", "New playlist".into())],
            ),
            (_, Some(Source::Playlist { .. })) => (0, tracks_cols, vec![]),
            (Section::Radio, _) => (
                0,
                vec!["", "Station", "Last test", "", ""],
                vec![("new", "New station".into())],
            ),
            (Section::Podcasts, Some(Source::Section(_))) => (
                0,
                vec!["", "Podcast", "Author", "", "Episodes"],
                vec![
                    ("subscribe", "Subscribe".into()),
                    ("refresh", "Refresh".into()),
                ],
            ),
            (_, Some(Source::Podcast { .. })) => (
                0,
                vec!["", "Episode", "Published", "Progress", "Length"],
                vec![("refresh", "Refresh".into())],
            ),
            (Section::Downloads, _) => (0, vec![], vec![]),
            (Section::Settings, _) => (0, vec![], vec![]),
            // Mixed artists, albums and songs: the group headings label them.
            (_, Some(Source::Search(_))) => (0, vec![], vec![]),
            (_, Some(Source::Folder(_))) => (0, tracks_cols, vec![]),
            (Section::Folders, Some(Source::Section(_))) => {
                (0, vec!["", "Folder", "", "", "Tracks"], vec![])
            }
            _ => (0, tracks_cols, vec![]),
        }
    }

    fn empty_text(&self) -> String {
        if let Some(e) = &self.b.view_error {
            return e.clone();
        }
        match self.b.level().map(|l| &l.source) {
            Some(Source::Search(q)) if q.is_empty() => "Type to search the library.".into(),
            Some(Source::Search(_)) => "No matches.".into(),
            Some(Source::Section(Section::Playlists)) => "No playlists yet. Make one with New playlist.".into(),
            Some(Source::Playlist { .. }) => "Empty. Right-click anything and choose Add to playlist.".into(),
            Some(Source::Section(Section::Radio)) => "No stations yet. Add one with New station: a name, then its stream or playlist URL.".into(),
            Some(Source::Section(Section::Podcasts)) => "No podcasts yet. Subscribe with a feed URL.".into(),
            Some(Source::Section(Section::MostListened | Section::TopAlbums | Section::TopArtists)) => {
                "Nothing played yet. A song counts once you've heard half of it, or four minutes of it, whichever comes first.".into()
            }
            Some(Source::Podcast { .. }) => {
                if self.b.pod.as_ref().is_some_and(|p| p.status().refreshing) {
                    "Fetching the feed…".into()
                } else {
                    "No episodes with audio in this feed.".into()
                }
            }
            _ => {
                if self.b.service.as_ref().is_some_and(|s| !s.status().ready) {
                    "Scanning your folders…".into()
                } else {
                    "Nothing here.".into()
                }
            }
        }
    }

    fn sync_view(&mut self, ui: &AppWindow) {
        let (layout, cols, acts) = self.chrome();
        ui.set_layout(layout);
        set_strings(&self.columns, cols.iter().map(ss));
        set_strings(&self.actions, acts.iter().map(|(a, _)| ss(a)));
        set_strings(&self.action_labels, acts.iter().map(|(_, l)| ss(l)));
        set_strings(&self.crumbs, self.b.crumbs().iter().map(ss));
        ui.set_can_back(self.b.depth() > 1);
        ui.set_can_forward(self.b.can_go_forward());
        let mut empty = self.empty_text();
        if self.b.config.appearance.theme.is_kawaii() {
            empty.push_str("  (´・ω・`)");
        }
        ui.set_empty_text(ss(empty));
        ui.set_album_tracks(self.b.in_playlist().is_none());

        let key = format!(
            "{:?}/{}/{:?}",
            self.b.section,
            self.b.depth(),
            self.b.level().map(|l| &l.source)
        );
        let level_changed = key != self.level_key;
        self.level_key = key;

        let (items, selected, title) = match self.b.level() {
            Some(l) => (l.items.clone(), l.selected as i32, l.title.clone()),
            None => (Vec::new(), -1, self.b.section.label().to_string()),
        };
        let ctx = match self.b.level().map(|l| &l.source) {
            Some(Source::Album { .. }) => RowCtx::Album,
            Some(Source::Section(s)) if Section::STATS.contains(s) => RowCtx::Ranked,
            _ => RowCtx::Plain,
        };
        // Numbers count within each headed group, so search results read
        // 1, 2, 3 under Songs instead of carrying on from Albums.
        let mut n = 0;
        let rows: Vec<Row> = items
            .iter()
            .map(|it| {
                if matches!(it, Item::Header(_)) {
                    n = 0;
                } else {
                    n += 1;
                }
                self.row(it, n.max(1) - 1, ctx)
            })
            .collect();
        // Songs in a list don't show covers; everything else that has one does.
        self.row_art = items
            .iter()
            .map(|it| match it {
                Item::Track(_) => None,
                other => Self::art_req(other),
            })
            .collect();
        ui.set_view_title(ss(match self.b.level().map(|l| &l.source) {
            Some(Source::Search(_)) => "Search".to_string(),
            // Top levels are named after their section; the sort and mode
            // show on the chips beside the title.
            Some(Source::Section(s)) => s.label().to_string(),
            _ => title.split(" · ").next().unwrap_or(&title).to_string(),
        }));
        ui.set_view_sub(ss(match self.b.level().map(|l| &l.source) {
            Some(Source::Section(Section::Settings)) => {
                "Changes are saved to config.toml as you make them".to_string()
            }
            _ if items.is_empty() => String::new(),
            _ => count_label(&items),
        }));
        ui.set_covers(
            items
                .iter()
                .any(|i| matches!(i, Item::Album(_) | Item::Dir { .. } | Item::Root { .. })),
        );
        let mut rows = rows;
        let shown = self.row_ranges(ui, rows.len());
        for r in &shown {
            for i in r.clone() {
                if let Some(req) = self.row_art[i].clone()
                    && let Some(img) = self.art.get(req, THUMB)
                {
                    rows[i].art = img;
                    rows[i].has_art = true;
                }
            }
        }
        self.row_shown = shown;
        update_rows(&self.rows, rows);
        let sel_ok = items
            .get(selected.max(0) as usize)
            .is_some_and(Item::selectable);
        ui.set_selected(if sel_ok { selected } else { -1 });

        // Album header.
        let hero = match self.b.level().map(|l| l.source.clone()) {
            Some(Source::Album { key }) => self
                .b
                .lib
                .as_ref()
                .and_then(|l| l.album(&key).ok().flatten())
                .map(|a| {
                    let mut meta = Vec::new();
                    if let Some(y) = a.year {
                        meta.push(y.to_string());
                    }
                    if let Some(g) = &a.genre {
                        meta.push(g.clone());
                    }
                    meta.push(format!("{} track{}", a.track_count, s_(a.track_count)));
                    meta.push(fmt_length(a.duration));
                    if a.lossless {
                        meta.push("lossless".into());
                    }
                    let art = a
                        .cover_track
                        .clone()
                        .and_then(|p| self.art.get(Req::Track(p), LARGE));
                    Hero {
                        show: true,
                        kicker: ss(if self.b.config.appearance.theme.is_kawaii() {
                            "ALBUM · アルバム"
                        } else {
                            "ALBUM"
                        }),
                        title: ss(&a.name),
                        sub: ss(a.artist_line()),
                        meta: ss(meta.join(" · ")),
                        has_art: art.is_some(),
                        art: art.unwrap_or_default(),
                    }
                }),
            _ => None,
        };
        ui.set_hero(hero.unwrap_or_default());
        if level_changed {
            ui.invoke_scroll_top();
        }
    }

    fn sync_side(&mut self, ui: &AppWindow) {
        let icons = ui.global::<Icons>();
        let kawaii = self.b.config.appearance.theme.is_kawaii();
        let status = self.b.service.as_ref().map(|s| s.status());
        let open_folder = match self.b.level().map(|l| &l.source) {
            Some(Source::Folder(p)) if self.b.section == Section::Folders => Some(p.clone()),
            _ => None,
        };
        let items: Vec<SideItem> = self
            .b
            .sidebar
            .iter()
            .map(|e| match e {
                SideEntry::Section(s) => SideItem {
                    kind: 0,
                    label: ss(s.label()),
                    sub: ss(if kawaii { jp_section(*s) } else { "" }),
                    icon: match s {
                        Section::Folders => icons.get_folders(),
                        Section::Artists => icons.get_artists(),
                        Section::Albums => icons.get_albums(),
                        Section::Songs => icons.get_songs(),
                        Section::Genres => icons.get_genres(),
                        Section::Playlists => icons.get_playlists(),
                        Section::Radio => icons.get_radio(),
                        Section::Podcasts => icons.get_podcasts(),
                        Section::Queue => icons.get_queue(),
                        Section::Downloads => icons.get_downloads(),
                        Section::Settings => icons.get_settings(),
                        Section::MostListened => icons.get_most_listened(),
                        Section::TopAlbums => icons.get_top_albums(),
                        Section::TopArtists => icons.get_top_artists(),
                    },
                    active: *s == self.b.section,
                    count: ss(if *s == Section::Queue {
                        let n = self.b.player.queue().len();
                        if n > 0 { n.to_string() } else { String::new() }
                    } else {
                        String::new()
                    }),
                    dot: 0,
                },
                SideEntry::Source {
                    path,
                    tracks,
                    server: Some(state),
                    ..
                } => SideItem {
                    kind: 1,
                    sub: SharedString::new(),
                    label: ss(patina_core::remote::server_of(path).unwrap_or("server")),
                    count: ss(match state {
                        ServerState::Syncing { done, total } if *total > 0 => {
                            format!("{done}/{total}")
                        }
                        ServerState::Offline(_) => "offline".into(),
                        ServerState::NoPassword | ServerState::BadLogin => "login".into(),
                        _ => tracks.to_string(),
                    }),
                    icon: SharedString::new(),
                    active: open_folder.as_ref() == Some(path),
                    dot: match state {
                        ServerState::Ready => 0,
                        ServerState::Syncing { .. } | ServerState::Connecting => 1,
                        _ => 2,
                    },
                },
                SideEntry::Source {
                    path,
                    tracks,
                    present,
                    server: None,
                } => {
                    let scanning = status
                        .as_ref()
                        .and_then(|s| s.scanning.as_ref())
                        .filter(|sc| sc.root == *path && sc.total > 0);
                    SideItem {
                        kind: 1,
                        sub: SharedString::new(),
                        label: ss(path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string())),
                        count: ss(match scanning {
                            Some(sc) => format!("{}/{}", sc.done, sc.total),
                            None if !present => "missing".into(),
                            None => tracks.to_string(),
                        }),
                        icon: SharedString::new(),
                        active: open_folder.as_ref() == Some(path),
                        dot: if !present {
                            2
                        } else if scanning.is_some() {
                            1
                        } else if status.as_ref().is_some_and(|s| s.watching) {
                            0
                        } else {
                            3
                        },
                    }
                }
                SideEntry::Label(l) => SideItem {
                    kind: 2,
                    label: ss(l.to_uppercase()),
                    sub: ss(if kawaii { jp_label(l) } else { "" }),
                    ..Default::default()
                },
                SideEntry::Gap => SideItem {
                    kind: 3,
                    ..Default::default()
                },
            })
            .collect();
        if self.side.row_count() == items.len() {
            for (i, it) in items.into_iter().enumerate() {
                if self.side.row_data(i).as_ref() != Some(&it) {
                    self.side.set_row_data(i, it);
                }
            }
        } else {
            self.side.set_vec(items);
        }
    }

    fn sync_queue(&mut self, ui: &AppWindow) {
        let (rows, current, total) = {
            let q = self.b.player.queue();
            let cur = q.current();
            let mut total = Duration::ZERO;
            let mut current = -1;
            let mut infos = Vec::new();
            for (pos, &id) in q.play_order().iter().enumerate() {
                let Some(e) = q.get(id) else { continue };
                total += e.info.duration.unwrap_or_default();
                if cur == Some(id) {
                    current = pos as i32;
                }
                infos.push((e.info.clone(), cur == Some(id), e.failed));
            }
            (infos, current, total)
        };
        let rows_art: Vec<Option<Req>> = rows
            .iter()
            .map(|(info, _, _)| {
                (info.kind == MediaKind::File && !info.path.as_os_str().is_empty())
                    .then(|| Req::Track(info.path.clone()))
            })
            .collect();
        let n = rows.len();
        let models: Vec<QueueRow> = rows
            .into_iter()
            .map(|(info, playing, failed)| QueueRow {
                off: !self.b.avail.info_plays(&info),
                title: ss(info.display_title()),
                sub: ss(sub_line(&info)),
                playing,
                failed,
                ..Default::default()
            })
            .collect();
        self.queue_art = rows_art;
        let mut models = models;
        let shown = self.queue_ranges(ui, models.len());
        for r in &shown {
            for i in r.clone() {
                if let Some(req) = self.queue_art[i].clone()
                    && let Some(img) = self.art.get(req, THUMB)
                {
                    models[i].art = img;
                    models[i].has_art = true;
                }
            }
        }
        self.queue_shown = shown;
        if self.queue.row_count() == models.len() {
            for (i, m) in models.into_iter().enumerate() {
                if self.queue.row_data(i).as_ref() != Some(&m) {
                    self.queue.set_row_data(i, m);
                }
            }
        } else {
            self.queue.set_vec(models);
        }
        if self.follow && current >= 0 {
            self.queue_sel = current as usize;
        }
        self.queue_sel = self.queue_sel.min(n.saturating_sub(1));
        ui.set_queue_sel(if n == 0 { -1 } else { self.queue_sel as i32 });
        ui.set_queue_current(current);
        ui.set_queue_summary(ss(if n == 0 {
            String::new()
        } else {
            format!("{n} track{} · {}", s_(n as i64), fmt_length(total))
        }));
    }

    fn sync_playback(&mut self, ui: &AppWindow) {
        let s = &self.snap;
        let track = s.track.clone();
        ui.set_has_track(track.is_some());
        ui.set_playing(s.state == PlayState::Playing);
        ui.set_live(s.live);
        match &track {
            Some(t) => {
                ui.set_np_title(ss(t.display_title()));
                ui.set_np_sub(ss(if t.is_live() {
                    s.live_title.clone().unwrap_or_else(|| "Live radio".into())
                } else {
                    sub_line(t)
                }));
                let art = (!t.path.as_os_str().is_empty())
                    .then(|| self.art.get(Req::Track(t.path.clone()), LARGE))
                    .flatten();
                ui.set_np_has_art(art.is_some());
                ui.set_np_art(art.unwrap_or_default());
                ui.set_badge(ss(t.badge()));
                let (bp, tone, tip) = match (&s.exclusive, &s.bit_perfect_refused) {
                    (Some(e), _) if e.exact => (
                        "BIT-PERFECT",
                        0,
                        format!("Bit-perfect to {} as {}", e.device, e.format),
                    ),
                    (Some(e), _) => (
                        "EXCLUSIVE",
                        1,
                        format!(
                            "Exclusive and unprocessed to {}, but the source isn't integer PCM",
                            e.device
                        ),
                    ),
                    (None, Some(r)) => ("NOT BIT-PERFECT", 2, r.clone()),
                    (None, None) => ("", 0, String::new()),
                };
                ui.set_bp_badge(ss(bp));
                ui.set_bp_tone(tone);
                ui.set_bp_tip(ss(tip));
            }
            None => {
                ui.set_np_title(SharedString::new());
                ui.set_np_sub(SharedString::new());
                ui.set_np_has_art(false);
                ui.set_badge(SharedString::new());
                ui.set_bp_badge(SharedString::new());
            }
        }
        let s = &self.snap;
        ui.set_position(match s.duration {
            Some(d) if !d.is_zero() => {
                (s.position.as_secs_f64() / d.as_secs_f64()).clamp(0.0, 1.0) as f32
            }
            _ => 0.0,
        });
        ui.set_elapsed(ss(if s.track.is_some() {
            fmt_duration(s.position)
        } else {
            String::new()
        }));
        ui.set_remaining(ss(match s.duration {
            Some(d) => format!("-{}", fmt_duration(d.saturating_sub(s.position))),
            None => String::new(),
        }));
        ui.set_shuffle(s.shuffle);
        ui.set_repeat(match s.repeat {
            Repeat::Off => 0,
            Repeat::All => 1,
            Repeat::One => 2,
        });
        ui.set_volume(s.volume as f32);
        ui.set_volume_locked(s.volume_locked);
        ui.set_muted(s.muted);
        ui.set_output_label(ss(s.output.clone().unwrap_or_default()));
        let podcast = s
            .track
            .as_ref()
            .is_some_and(|t| t.kind == MediaKind::Podcast);
        ui.set_speed_label(ss(if podcast {
            let mut l = format!("{:.1}x", s.podcast_tempo);
            if s.podcast_trim {
                l.push_str(" trim");
            }
            l
        } else {
            String::new()
        }));
        let (line, warn) = if let Some(r) = &s.reconnect {
            let secs = r.at.saturating_duration_since(Instant::now()).as_secs();
            (
                format!(
                    "Connection lost, retrying in {secs} s (attempt {})",
                    r.attempt
                ),
                true,
            )
        } else if s.buffering {
            ("Buffering".to_string(), true)
        } else {
            (String::new(), false)
        };
        ui.set_status_line(ss(line));
        ui.set_status_warn(warn);
    }

    fn notes(&mut self, ui: &AppWindow) {
        for n in self.b.take_notes() {
            let (msg, err) = match n {
                Note::Info(m) => (m, false),
                Note::Error(m) => (m, true),
            };
            ui.set_toast(ss(msg));
            ui.set_toast_error(err);
            self.toast_until = Some(
                Instant::now()
                    + if err {
                        Duration::from_secs(8)
                    } else {
                        Duration::from_secs(3)
                    },
            );
        }
        if self.toast_until.is_some_and(|t| Instant::now() > t) {
            self.toast_until = None;
            ui.set_toast(SharedString::new());
        }
    }

    fn tick(&mut self, ui: &AppWindow) {
        for ev in self.b.poll() {
            if let Event::TrackChanged(_) = ev {
                self.follow = true;
            }
        }
        self.snap = self.b.player.snapshot();
        self.poll_folder_pick();
        if let Some(found) = self.b.take_dlna_found() {
            self.open_dlna(ui, found);
        }
        let theme = self.b.config.appearance.theme;
        if self.theme != Some(theme) {
            self.theme = Some(theme);
            ui.global::<P>().set_theme(theme.index());
            // The sidebar's Japanese names and the album kicker change too.
            self.last_gen = u64::MAX;
        }
        let art_arrived = self.art.collect();
        let qv = self.b.player.queue().version;
        let gen_changed = self.b.generation() != self.last_gen;
        if gen_changed || qv != self.last_queue_ver {
            self.last_gen = self.b.generation();
            self.sync_side(ui);
            self.sync_view(ui);
        }
        if qv != self.last_queue_ver || gen_changed {
            self.last_queue_ver = qv;
            self.sync_queue(ui);
        }
        if gen_changed {
            let servers = self.b.queue_servers();
            if self
                .queue_servers
                .iter()
                .map(|x| x.to_string())
                .ne(servers.iter().cloned())
            {
                set_strings(&self.queue_servers, servers.iter().map(ss));
            }
        }
        // Scrolling, or covers finishing loading: touch only the rows that
        // come into view or leave it.
        self.update_row_art(ui, art_arrived);
        self.update_queue_art(ui, art_arrived);
        self.sync_playback(ui);
        self.notes(ui);
        // The queue panel needs room; fold it away on a narrow window.
        let w = ui.window().size().width as f32 / ui.window().scale_factor();
        if ui.get_queue_panel() && w < 1100.0 {
            ui.set_queue_panel(false);
        }
        // Below this the bottom bar drops the format badge and output chip.
        let compact = w < 1180.0;
        if ui.get_compact() != compact {
            ui.set_compact(compact);
        }
    }

    // ----- Cover art on screen -----

    /// Rows of the main view on screen now, with a margin either side so
    /// covers are ready a little before they scroll in.
    fn row_ranges(&self, ui: &AppWindow, n: usize) -> Vec<Range<usize>> {
        let (first, count, margin) = match ui.get_layout() {
            0 => (ui.get_list_first(), ui.get_list_count(), 20),
            1 => {
                let cols = ui.get_grid_cols().max(1);
                (ui.get_grid_first(), ui.get_grid_count(), cols * 2)
            }
            _ => return Vec::new(),
        };
        vec![window(first, count, margin, n)]
    }

    /// Queue entries on screen: the Queue view, the queue panel, or both.
    fn queue_ranges(&self, ui: &AppWindow, n: usize) -> Vec<Range<usize>> {
        let mut v = Vec::new();
        if ui.get_layout() == 2 {
            v.push(window(ui.get_qlist_first(), ui.get_qlist_count(), 10, n));
        }
        if ui.get_queue_panel() {
            v.push(window(ui.get_panel_first(), ui.get_panel_count(), 10, n));
        }
        v
    }

    fn update_row_art(&mut self, ui: &AppWindow, arrived: bool) {
        let shown = self.row_ranges(ui, self.row_art.len());
        if !arrived && shown == self.row_shown {
            return;
        }
        let old = std::mem::replace(&mut self.row_shown, shown.clone());
        for i in touched(&old, &shown) {
            let want = shown.iter().any(|r| r.contains(&i));
            let img = match (want, self.row_art.get(i).cloned().flatten()) {
                (true, Some(req)) => self.art.get(req, THUMB),
                _ => None,
            };
            let Some(mut row) = self.rows.row_data(i) else {
                continue;
            };
            if row.has_art != img.is_some() || (img.is_some() && img.as_ref() != Some(&row.art)) {
                row.has_art = img.is_some();
                row.art = img.unwrap_or_default();
                self.rows.set_row_data(i, row);
            }
        }
    }

    fn update_queue_art(&mut self, ui: &AppWindow, arrived: bool) {
        let shown = self.queue_ranges(ui, self.queue_art.len());
        if !arrived && shown == self.queue_shown {
            return;
        }
        let old = std::mem::replace(&mut self.queue_shown, shown.clone());
        for i in touched(&old, &shown) {
            let want = shown.iter().any(|r| r.contains(&i));
            let img = match (want, self.queue_art.get(i).cloned().flatten()) {
                (true, Some(req)) => self.art.get(req, THUMB),
                _ => None,
            };
            let Some(mut row) = self.queue.row_data(i) else {
                continue;
            };
            if row.has_art != img.is_some() || (img.is_some() && img.as_ref() != Some(&row.art)) {
                row.has_art = img.is_some();
                row.art = img.unwrap_or_default();
                self.queue.set_row_data(i, row);
            }
        }
    }

    // ----- Actions -----

    fn selected_index(&self) -> Option<usize> {
        self.b.level().map(|l| l.selected)
    }

    fn move_sel(&mut self, ui: &AppWindow, delta: isize) {
        if self.b.section == Section::Queue {
            let n = self.b.player.queue().len();
            if n == 0 {
                return;
            }
            self.queue_sel = (self.queue_sel as isize + delta).clamp(0, n as isize - 1) as usize;
            self.follow = false;
            ui.set_queue_sel(self.queue_sel as i32);
            ui.invoke_scroll_to(self.queue_sel as i32);
            return;
        }
        self.b.move_selection(delta);
        if let Some(i) = self.selected_index() {
            ui.invoke_scroll_to(i as i32);
        }
    }

    /// Enter or a double-click. Clearing the cache asks first.
    fn activate_in(&mut self, ui: &AppWindow) {
        if let Some(Item::Action {
            action: ListAction::ClearCache,
            ..
        }) = self.b.selected_item()
        {
            return self.ask_clear_cache(ui);
        }
        self.activate();
    }

    fn ask_clear_cache(&mut self, ui: &AppWindow) {
        self.confirm(
            ui,
            "Clear the song cache? Pinned songs stay, and songs come back as you play them.".into(),
            "Clear",
            Confirm::ClearCache,
        );
    }

    fn activate(&mut self) {
        if self.b.section == Section::Queue {
            self.b.play_queue_position(self.queue_sel);
            self.follow = true;
            return;
        }
        if self.b.selected_setting().is_some() {
            return; // handled by key/act with the window at hand
        }
        if self.b.activate() == Activated::Played {
            self.follow = true;
        }
    }

    /// Play the item itself (from a context menu), even ones Enter would
    /// open, like an album or a folder.
    fn play_selected(&mut self) {
        if self.b.play_selection() == Activated::Played {
            self.follow = true;
        }
    }

    fn play_view(&mut self, shuffle: bool) {
        self.b.play_view(shuffle);
        self.follow = true;
    }

    fn view_action(&mut self, ui: &AppWindow, a: &str) {
        match a {
            "play" => self.play_view(false),
            "shuffle" => self.play_view(true),
            "queue" => self.b.queue_view(),
            "playlist" => {
                let paths = self.b.view_playlist_paths();
                self.open_pick(ui, "", paths);
            }
            "sort" => self.b.cycle_sort(),
            "reverse" => self.b.reverse_songs(),
            "mode" => self.b.toggle_mode(),
            "new" if self.b.section == Section::Radio => self.open_station(ui, None),
            "new" => {
                self.dlg = Dlg::NewPlaylist(Vec::new());
                open_text(ui, "New playlist", "Name", "", None, "Create");
            }
            "subscribe" => {
                self.dlg = Dlg::Feed;
                open_text(
                    ui,
                    "Subscribe to a podcast",
                    "Feed URL",
                    "",
                    None,
                    "Subscribe",
                );
            }
            "refresh" => self.b.rescan(),
            "pin" => self.b.toggle_pin_view(),
            _ => {}
        }
    }

    /// Choose a playlist, or a new one, for `paths`. `what` names them in
    /// the title ("the queue"); empty counts them instead.
    fn open_pick(&mut self, ui: &AppWindow, what: &str, paths: Vec<PathBuf>) {
        if self.b.lib.is_none() {
            return self.b.error("Playlists need the library");
        }
        if paths.is_empty() {
            return self.b.note("Only library songs can go in a playlist");
        }
        let rows: Vec<PlaylistRow> = self
            .b
            .playlists()
            .into_iter()
            .filter(|p| !p.readonly)
            .collect();
        let servers = self.b.connected_servers();
        let mut items = vec![ss("New playlist…")];
        items.extend(servers.iter().map(|s| ss(format!("New playlist on {s}…"))));
        items.extend(rows.iter().map(|r| {
            ss(match &r.server {
                Some(s) => format!("{}  ({}, on {s})", r.name, r.items),
                None => format!("{}  ({})", r.name, r.items),
            })
        }));
        set_strings(&self.dlg_items, items.into_iter());
        let n = patina_core::browse::count(paths.len());
        ui.set_dlg_title(ss(if what.is_empty() {
            format!("Add {n} to a playlist")
        } else {
            format!("Add {what} ({n}) to a playlist")
        }));
        ui.set_dlg_new_first(true);
        ui.set_dialog(2);
        self.dlg = Dlg::Pick(paths, servers, rows);
    }

    /// The add or edit form for a server.
    fn open_server(&mut self, ui: &AppWindow, edit: Option<String>) {
        let cur = edit.as_ref().and_then(|n| self.b.server(n).cloned());
        if let Some(c) = cur.as_ref().filter(|c| !c.kind.logs_in()) {
            open_text(
                ui,
                "Rename DLNA server",
                "Name, as the sidebar shows it",
                &c.name,
                None,
                "Rename",
            );
            self.dlg = Dlg::RenameServer(c.name.clone());
            return;
        }
        let (name, url, user) = cur.map(|c| (c.name, c.url, c.user)).unwrap_or_default();
        open_text(
            ui,
            if edit.is_some() {
                "Edit server"
            } else {
                "Add a music server"
            },
            "Name, as the sidebar shows it",
            &name,
            Some(("Address, like https://music.example.com", &url)),
            if edit.is_some() { "Save" } else { "Connect" },
        );
        ui.set_dlg_label3(ss("User name (leave it empty for an API key)"));
        ui.set_dlg_text3(ss(&user));
        ui.set_dlg_label4(ss(if edit.is_some() {
            "Password or API key (empty keeps the saved one)"
        } else {
            "Password or API key"
        }));
        ui.set_dlg_text4(SharedString::new());
        ui.set_dlg_four_fields(true);
        self.dlg = Dlg::Server(edit);
    }

    fn open_station(&mut self, ui: &AppWindow, edit: Option<usize>) {
        let (name, url) = edit
            .and_then(|i| self.b.stations.get(i))
            .map(|s| (s.name.clone(), s.url.clone()))
            .unwrap_or_default();
        self.dlg = Dlg::Station(edit);
        open_text(
            ui,
            if edit.is_some() {
                "Edit station"
            } else {
                "New station"
            },
            "Name",
            &name,
            Some((
                "Stream or playlist URL (.pls, .m3u, .m3u8 or a direct stream)",
                &url,
            )),
            "Save and test",
        );
    }

    /// The servers a DLNA search found, to pick one to add.
    fn open_dlna(&mut self, ui: &AppWindow, found: Vec<patina_core::remote::dlna::Found>) {
        let items = found.iter().map(|f| ss(f.label()));
        set_strings(&self.dlg_items, items);
        ui.set_dlg_title(ss(if found.len() == 1 {
            "Found a DLNA server. Add it?"
        } else {
            "Found DLNA servers. Add which?"
        }));
        ui.set_dlg_new_first(false);
        ui.set_dialog(2);
        self.dlg = Dlg::Dlna(found);
    }

    fn confirm(&mut self, ui: &AppWindow, msg: String, ok: &str, c: Confirm) {
        ui.set_dlg_title(ss("Are you sure?"));
        ui.set_dlg_message(ss(msg));
        ui.set_dlg_ok(ss(ok));
        ui.set_dialog(3);
        self.dlg = Dlg::Confirm(c);
    }

    /// Enter on a settings row: the obvious thing for its kind.
    fn setting_default(&mut self, ui: &AppWindow) {
        let Some(st) = self.b.selected_setting() else {
            return;
        };
        let a = match st.control {
            Control::Toggle(_) => "toggle",
            Control::Choice { .. } => "choose",
            Control::Number { .. } | Control::Text { .. } => "edit",
            Control::Action(_) if st.key == SettingKey::AddServer => "add-server",
            Control::Action(_) if st.key == SettingKey::FindDlna => "find-dlna",
            Control::Action(_) => "browse",
            Control::Server(_) => "sync",
            Control::Folder | Control::Info(_) => return,
        };
        self.setting_act(ui, a);
    }

    fn setting_act(&mut self, ui: &AppWindow, a: &str) {
        let Some(st) = self.b.selected_setting() else {
            return;
        };
        let key = st.key.clone();
        match a {
            "toggle" => self.change_setting(ui, key, Change::Toggle),
            "dec" => self.change_setting(ui, key, Change::Step(-1)),
            "inc" => self.change_setting(ui, key, Change::Step(1)),
            "choose" => {
                if let Control::Choice { options, index } = &st.control {
                    let items: Vec<SharedString> = options
                        .iter()
                        .enumerate()
                        .map(|(i, (_, l))| {
                            ss(if i == *index {
                                format!("{l}   (current)")
                            } else {
                                l.clone()
                            })
                        })
                        .collect();
                    set_strings(&self.dlg_items, items.into_iter());
                    ui.set_dlg_title(ss(&st.label));
                    ui.set_dlg_new_first(false);
                    ui.set_dialog(2);
                    self.dlg = Dlg::Choose(key);
                }
            }
            "add-server" => self.open_server(ui, None),
            "find-dlna" => self.b.find_dlna(),
            "sync" => {
                if let SettingKey::Server(n) = &key {
                    self.b.sync_server(Some(n));
                }
            }
            "srv-edit" => {
                if let SettingKey::Server(n) = &key {
                    self.open_server(ui, Some(n.clone()));
                }
            }
            "srv-remove" => {
                if let SettingKey::Server(n) = key {
                    let login = self.b.server(&n).is_some_and(|c| c.kind.logs_in());
                    self.confirm(
                        ui,
                        if login {
                            format!("Remove {n}? Its songs and playlists leave the library here, and its password leaves the keyring. Nothing changes on the server.")
                        } else {
                            format!("Remove {n}? Its songs leave the library here. Nothing changes on the server, and Find servers can add it again.")
                        },
                        "Remove",
                        Confirm::RemoveServer(n),
                    );
                }
            }
            "edit" if key == SettingKey::AddFolder => {
                self.dlg = Dlg::AddFolder;
                open_text(ui, "Add a music folder", "Folder", "~/", None, "Add");
            }
            "edit" => {
                let current = match &st.control {
                    Control::Number { value, scale, .. } if *scale == 1 => value.to_string(),
                    Control::Number { value, scale, .. } => {
                        format!("{:.1}", *value as f64 / *scale as f64)
                    }
                    Control::Text { value, .. } => value.clone(),
                    _ => return,
                };
                self.dlg = Dlg::SetValue(key);
                open_text(ui, &st.label, st.help, &current, None, "Save");
            }
            "remove" => {
                if let SettingKey::Folder(p) = key {
                    self.confirm(
                        ui,
                        format!(
                            "Take {} out of the library? Its tracks leave the index; the files stay where they are.",
                            p.display()
                        ),
                        "Remove",
                        Confirm::RemoveFolder(p),
                    );
                }
            }
            "browse" => self.pick_folder(),
            _ => {}
        }
    }

    /// Change a setting, asking first if it needs a yes.
    fn change_setting(&mut self, ui: &AppWindow, key: SettingKey, change: Change) {
        match self.b.setting_warning(&key, &change) {
            Some(w) => {
                self.confirm(
                    ui,
                    w.to_string(),
                    "Turn it on",
                    Confirm::Setting(key, change),
                );
                ui.set_dlg_title(ss("Turn on bit-perfect?"));
            }
            None => self.b.change_setting(&key, change),
        }
    }

    /// The desktop's own folder picker (through the XDG portal, so it's
    /// KDE's dialog on Plasma), on a thread so the window keeps drawing.
    fn pick_folder(&mut self) {
        if self.folder_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Add a music folder")
                .pick_folder();
            let _ = tx.send(picked);
        });
        self.folder_pick = Some((rx, Instant::now()));
    }

    fn poll_folder_pick(&mut self) {
        let Some((rx, opened)) = &self.folder_pick else {
            return;
        };
        let Ok(picked) = rx.try_recv() else { return };
        let quick = opened.elapsed() < Duration::from_millis(700);
        self.folder_pick = None;
        match picked {
            Some(p) => {
                self.b.add_folder(&p.to_string_lossy());
            }
            // Back that fast means no dialog ever showed up.
            None if quick => self
                .b
                .note("No folder picker is available here. Use \"Type a path\" instead."),
            None => {}
        }
    }

    fn row_act(&mut self, ui: &AppWindow, a: &str) {
        if self.b.selected_setting().is_some() {
            return self.setting_act(ui, a);
        }
        let item = self.b.selected_item();
        if matches!(a, "st-edit" | "st-up" | "st-down" | "st-delete")
            && matches!(
                item,
                Some(Item::Station {
                    server: Some(_),
                    ..
                })
            )
        {
            return self
                .b
                .note("Stations from a server are changed on the server");
        }
        match a {
            "play" => self.play_selected(),
            "next" => self.b.add_selected_to_queue(true),
            "queue" => self.b.add_selected_to_queue(false),
            "playlist" => {
                let paths = self.b.playlist_paths_for_selection();
                self.open_pick(ui, "", paths);
            }
            "pl-remove" => self.b.edit_playlist(true, 0),
            "st-edit" => {
                if let Some(Item::Station { index, .. }) = item {
                    self.open_station(ui, Some(index));
                }
            }
            "st-test" => {
                if let Some(Item::Station { station, .. }) = item {
                    self.b.test_station(station.url);
                }
            }
            "st-up" | "st-down" => {
                if let Some(Item::Station { index, .. }) = item {
                    self.b
                        .move_station(index, if a == "st-up" { -1 } else { 1 });
                }
            }
            "st-delete" => {
                if let Some(Item::Station { index, station, .. }) = item {
                    self.confirm(
                        ui,
                        format!("Delete the station \"{}\"?", station.name),
                        "Delete",
                        Confirm::DeleteStation(index),
                    );
                }
            }
            "unsubscribe" => {
                if let Some(Item::Podcast(p)) = item {
                    self.confirm(
                        ui,
                        format!("Unsubscribe from \"{}\"? Its episodes and your place in them are forgotten.", p.title),
                        "Unsubscribe",
                        Confirm::Unsubscribe(p.feed_url),
                    );
                }
            }
            "download" => {
                if let Some(Item::Episode(e)) = item {
                    self.b.toggle_download(&e);
                }
            }
            "dl-delete" => {
                if let Some(Item::Episode(e)) = item {
                    self.b.delete_download(e.id);
                }
            }
            "played" => {
                if let Some(Item::Episode(e)) = item {
                    self.b.toggle_played(&e);
                }
            }
            "pin" => self.b.toggle_pin_selected(),
            "unpin" => {
                if let Some(Item::Pinned(p)) = item {
                    self.b.unpin(p.pin.id);
                }
            }
            "job-pause" => {
                if let Some(Item::Job(j)) = item {
                    self.b.toggle_job_pause(&j);
                }
            }
            "job-cancel" => {
                if let Some(Item::Job(j)) = item {
                    self.b.cancel_job(&j);
                }
            }
            "pl-rename" => {
                if let Some(Item::Playlist(p)) = item {
                    self.dlg = Dlg::RenamePlaylist(p.id);
                    open_text(ui, "Rename playlist", "Name", &p.name, None, "Rename");
                }
            }
            "pl-delete" => {
                if let Some(Item::Playlist(p)) = item {
                    self.confirm(
                        ui,
                        format!("Delete the playlist \"{}\"?", p.name),
                        "Delete",
                        Confirm::DeletePlaylist(p.id),
                    );
                }
            }
            _ => {}
        }
    }

    fn queue_act(&mut self, ui: &AppWindow, i: usize, a: &str) {
        self.queue_sel = i;
        self.follow = false;
        match a {
            "play" => {
                self.b.play_queue_position(i);
                self.follow = true;
            }
            "up" => self.queue_sel = self.b.move_queue_position(i, -1),
            "down" => self.queue_sel = self.b.move_queue_position(i, 1),
            "remove" => self.b.remove_queue_position(i),
            "playlist" => {
                let paths = self.b.queue_paths_at(i);
                self.open_pick(ui, "", paths);
            }
            _ => {}
        }
    }

    fn dlg_accept(&mut self, ui: &AppWindow) {
        let t1 = ui.get_dlg_text1().trim().to_string();
        let t2 = ui.get_dlg_text2().trim().to_string();
        match std::mem::replace(&mut self.dlg, Dlg::None) {
            Dlg::AddFolder => {
                if !self.b.add_folder(&t1) {
                    // Leave the dialog up to fix the path.
                    self.dlg = Dlg::AddFolder;
                    return;
                }
            }
            Dlg::SetValue(key) => self.change_setting(ui, key, Change::Set(t1)),
            Dlg::NewPlaylist(paths) => self.b.create_playlist(&t1, &paths),
            Dlg::RenamePlaylist(id) => self.b.rename_playlist(id, &t1),
            Dlg::Station(edit) => {
                if t1.is_empty() || t2.is_empty() {
                    self.b.error("A station needs a name and a URL");
                    self.dlg = Dlg::Station(edit);
                    return;
                }
                self.b.save_station(edit, &t1, &t2);
            }
            Dlg::Feed => self.b.subscribe(&t1),
            Dlg::Server(old) => {
                let t3 = ui.get_dlg_text3().trim().to_string();
                // The password exactly as typed.
                let t4 = ui.get_dlg_text4().to_string();
                if t1.is_empty() || t2.is_empty() {
                    self.b.error("A server needs a name and an address");
                    self.dlg = Dlg::Server(old);
                    return;
                }
                match &old {
                    Some(o) => {
                        let secret = (!t4.is_empty()).then_some(t4.as_str());
                        self.b.edit_server(o, &t1, &t2, &t3, secret);
                    }
                    None => self.b.add_server(&t1, &t2, &t3, &t4),
                }
            }
            Dlg::RenameServer(old) => self.b.edit_server(&old, &t1, "", "", None),
            Dlg::ServerPlaylist(server, paths) => {
                self.b.create_server_playlist(&server, &t1, &paths)
            }
            Dlg::Confirm(c) => match c {
                Confirm::Setting(key, change) => self.b.change_setting(&key, change),
                Confirm::RemoveFolder(p) => self.b.remove_folder(&p),
                Confirm::DeletePlaylist(id) => self.b.delete_playlist(id),
                Confirm::ClearQueue => self.b.clear_queue(),
                Confirm::DeleteStation(i) => self.b.delete_station(i),
                Confirm::Unsubscribe(url) => self.b.unsubscribe(&url),
                Confirm::RemoveServer(n) => self.b.remove_server(&n),
                Confirm::ClearCache => self.b.list_action(ListAction::ClearCache),
            },
            Dlg::Pick(..) | Dlg::Choose(_) | Dlg::Dlna(_) | Dlg::Help | Dlg::None => {}
        }
        ui.set_dialog(0);
        ui.invoke_focus_root();
    }

    fn dlg_pick(&mut self, ui: &AppWindow, i: usize) {
        if let Dlg::Choose(key) = &self.dlg {
            let key = key.clone();
            self.dlg = Dlg::None;
            ui.set_dialog(0);
            ui.invoke_focus_root();
            // A confirm may open in place of the picker.
            self.change_setting(ui, key, Change::Choose(i));
            return;
        }
        if let Dlg::Dlna(found) = &self.dlg {
            if let Some(f) = found.get(i) {
                let f = f.clone();
                self.b.add_dlna_server(&f);
            }
            self.dlg = Dlg::None;
            ui.set_dialog(0);
            ui.invoke_focus_root();
            return;
        }
        if let Dlg::Pick(paths, servers, rows) = std::mem::replace(&mut self.dlg, Dlg::None) {
            if i == 0 {
                self.dlg = Dlg::NewPlaylist(paths);
                open_text(ui, "New playlist", "Name", "", None, "Create");
                return;
            }
            if let Some(server) = servers.get(i - 1) {
                let title = format!("New playlist on {server}");
                self.dlg = Dlg::ServerPlaylist(server.clone(), paths);
                open_text(ui, &title, "Name", "", None, "Create");
                return;
            }
            if let Some(p) = rows.get(i - 1 - servers.len()) {
                self.b.add_to_playlist(p.id, &p.name, &paths);
            }
        }
        ui.set_dialog(0);
        ui.invoke_focus_root();
    }

    fn key(&mut self, ui: &AppWindow, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        let is = |k: Key| text == key_str(k).as_str();
        let p = self.b.player.clone();
        if ctrl {
            match text {
                "q" | "Q" => {
                    let _ = slint::quit_event_loop();
                }
                "f" | "F" => {
                    let q = self.b.start_search();
                    ui.set_search_text(ss(q));
                    ui.invoke_focus_search();
                }
                _ => return false,
            }
            return true;
        }
        if alt {
            if is(Key::LeftArrow) {
                self.b.pop();
            } else if is(Key::RightArrow) {
                self.b.go_forward();
            } else {
                return false;
            }
            return true;
        }
        let queue = self.b.section == Section::Queue;
        if is(Key::UpArrow) {
            self.move_sel(ui, -1);
        } else if is(Key::DownArrow) {
            self.move_sel(ui, 1);
        } else if is(Key::PageUp) {
            self.move_sel(ui, -12);
        } else if is(Key::PageDown) {
            self.move_sel(ui, 12);
        } else if is(Key::Home) {
            self.move_sel(ui, isize::MIN / 2);
        } else if is(Key::End) {
            self.move_sel(ui, isize::MAX / 2);
        } else if is(Key::Return) {
            if self.b.selected_setting().is_some() {
                self.setting_default(ui);
            } else {
                self.activate_in(ui);
            }
        } else if is(Key::Backspace) {
            self.b.pop();
        } else if is(Key::LeftArrow) {
            p.send(Command::SeekBy(if shift { -30.0 } else { -5.0 }));
        } else if is(Key::RightArrow) {
            p.send(Command::SeekBy(if shift { 30.0 } else { 5.0 }));
        } else if is(Key::Delete) {
            if matches!(
                self.b.selected_setting().map(|s| s.key),
                Some(SettingKey::Folder(_))
            ) {
                self.setting_act(ui, "remove");
            } else if queue {
                self.b.remove_queue_position(self.queue_sel);
            } else if self.b.in_playlist().is_some() {
                self.b.edit_playlist(true, 0);
            } else {
                match self.b.selected_item() {
                    Some(Item::Job(j)) => self.b.cancel_job(&j),
                    Some(Item::Pinned(p)) => self.b.unpin(p.pin.id),
                    _ => return false,
                }
            }
        } else if is(Key::F5) {
            self.b.rescan();
        } else {
            match text {
                " " => p.send(Command::TogglePause),
                "n" => p.send(Command::Next),
                "b" => p.send(Command::Back),
                "+" | "=" => p.send(Command::SetVolume((self.snap.volume + 0.05).min(1.0))),
                "-" | "_" => p.send(Command::SetVolume((self.snap.volume - 0.05).max(0.0))),
                "m" => p.send(Command::ToggleMute),
                "s" => p.send(Command::ToggleShuffle),
                "r" => p.send(Command::CycleRepeat),
                "q" => ui.set_queue_panel(!ui.get_queue_panel()),
                "/" => {
                    let q = self.b.start_search();
                    ui.set_search_text(ss(q));
                    ui.invoke_focus_search();
                }
                "?" => {
                    self.dlg = Dlg::Help;
                    ui.set_dlg_title(ss("Keys"));
                    ui.set_dlg_message(ss(HELP));
                    ui.set_dialog(4);
                }
                "[" => self.b.nudge_tempo(-0.1),
                "]" => self.b.nudge_tempo(0.1),
                "z" => self.b.toggle_trim(),
                "v" if !queue => self.play_view(false),
                "V" if !queue => self.play_view(true),
                "f" if !queue => self.b.toggle_pin_selected(),
                "F" if !queue => self.b.toggle_pin_view(),
                "a" if !queue => self.b.add_selected_to_queue(false),
                "A" if !queue => self.b.add_selected_to_queue(true),
                "p" => {
                    let paths = if queue {
                        self.b.queue_paths_at(self.queue_sel)
                    } else {
                        self.b.playlist_paths_for_selection()
                    };
                    self.open_pick(ui, "", paths);
                }
                c if c.len() == 1 && c.as_bytes()[0].is_ascii_digit() => {
                    let d = c.as_bytes()[0] - b'0';
                    let i = if d == 0 { 9 } else { (d - 1) as usize };
                    self.b.switch(Section::NUMBERED[i]);
                }
                _ => return false,
            }
        }
        true
    }
}

fn sub_line(t: &TrackInfo) -> String {
    let parts: Vec<&str> = [t.artist.as_deref(), t.album.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    // An episode's artist is usually the show; don't say it twice.
    let mut out: Vec<&str> = Vec::new();
    for p in parts {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.join(" · ")
}

/// `count` rows from `first`, widened by `margin` each way, kept inside `n`.
fn window(first: i32, count: i32, margin: i32, n: usize) -> Range<usize> {
    let start = (first - margin).max(0) as usize;
    let end = (first.max(0) + count.max(0) + margin).max(0) as usize;
    start.min(n)..end.min(n)
}

/// Every index in any of the old or new ranges, once each.
fn touched(old: &[Range<usize>], new: &[Range<usize>]) -> Vec<usize> {
    let mut v: Vec<usize> = old.iter().chain(new).flat_map(Clone::clone).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn set_strings(model: &VecModel<SharedString>, items: impl Iterator<Item = SharedString>) {
    let v: Vec<SharedString> = items.collect();
    if model.iter().ne(v.iter().cloned()) {
        model.set_vec(v);
    }
}

/// Replace the rows, in place when the list is the same length so the
/// view keeps its scroll position and nothing flickers.
fn update_rows(model: &VecModel<Row>, rows: Vec<Row>) {
    if model.row_count() == rows.len() {
        for (i, r) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&r) {
                model.set_row_data(i, r);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

fn open_text(
    ui: &AppWindow,
    title: &str,
    label1: &str,
    text1: &str,
    second: Option<(&str, &str)>,
    ok: &str,
) {
    ui.set_dlg_title(ss(title));
    ui.set_dlg_label1(ss(label1));
    ui.set_dlg_text1(ss(text1));
    ui.set_dlg_two_fields(second.is_some());
    ui.set_dlg_four_fields(false);
    let (l2, t2) = second.unwrap_or(("", ""));
    ui.set_dlg_label2(ss(l2));
    ui.set_dlg_text2(ss(t2));
    ui.set_dlg_ok(ss(ok));
    ui.set_dialog(1);
}

/// Keeps the timer (and so the bridge) alive for the window's lifetime.
pub struct Bridge {
    _timer: Timer,
    _state: Rc<RefCell<State>>,
}

impl Bridge {
    /// Save the queue to its server if it's a server's, then stop the
    /// player with a fade.
    pub fn shutdown(&self, fade_ms: u32) {
        patina_core::session::shutdown(&mut self._state.borrow_mut().b, fade_ms);
    }
}

pub fn install(ui: &AppWindow, mut b: Browser, playing_files: bool) -> Bridge {
    b.switch(if playing_files {
        Section::Queue
    } else {
        Section::Folders
    });
    let st = Rc::new(RefCell::new(State::new(b)));
    {
        let s = st.borrow();
        ui.set_side(ModelRc::from(s.side.clone()));
        ui.set_rows(ModelRc::from(s.rows.clone()));
        ui.set_queue(ModelRc::from(s.queue.clone()));
        ui.set_crumbs(ModelRc::from(s.crumbs.clone()));
        ui.set_columns(ModelRc::from(s.columns.clone()));
        ui.set_actions(ModelRc::from(s.actions.clone()));
        ui.set_action_labels(ModelRc::from(s.action_labels.clone()));
        ui.set_dlg_items(ModelRc::from(s.dlg_items.clone()));
        ui.set_queue_servers(ModelRc::from(s.queue_servers.clone()));
    }

    // Wire a callback to a closure that gets the state and the window.
    macro_rules! on {
        ($setter:ident, |$s:ident, $u:ident $(, $arg:ident)*| $body:expr) => {{
            let st = st.clone();
            let weak = ui.as_weak();
            ui.$setter(move |$($arg),*| {
                let Some($u) = weak.upgrade() else { return Default::default() };
                let mut guard = st.borrow_mut();
                let $s = &mut *guard;
                let r = $body;
                $s.tick(&$u);
                r
            });
        }};
    }

    on!(on_side_clicked, |s, u, i| s.b.activate_sidebar(i as usize));
    on!(on_back, |s, u| {
        s.b.pop();
    });
    on!(on_forward, |s, u| {
        s.b.go_forward();
    });
    on!(on_search_edited, |s, u, text| {
        let t = text.to_string();
        if t.trim().is_empty() {
            if s.b.in_search() {
                s.b.set_search("");
            }
        } else {
            if !s.b.in_search() {
                s.b.start_search();
            }
            s.b.set_search(&t);
        }
    });
    on!(on_search_cancel, |s, u| {
        u.set_search_text(SharedString::new());
        if s.b.in_search() {
            s.b.pop();
        }
    });
    on!(on_crumb_clicked, |s, u, i| s.b.pop_to(i as usize + 1));
    on!(on_row_select, |s, u, i| {
        s.b.select(i as usize);
    });
    on!(on_row_activate, |s, u, i| {
        if s.b.select(i as usize) {
            s.activate_in(&u);
        }
    });
    on!(on_bp_clicked, |s, u| {
        let tip = u.get_bp_tip().to_string();
        if !tip.is_empty() {
            s.b.note(tip);
        }
    });
    on!(on_row_act, |s, u, i, a| {
        s.b.select(i as usize);
        s.row_act(&u, a.as_str());
    });
    on!(on_view_action, |s, u, a| s.view_action(&u, a.as_str()));
    on!(on_queue_select, |s, u, i| {
        s.queue_sel = i as usize;
        s.follow = false;
        u.set_queue_sel(i);
    });
    on!(on_queue_activate, |s, u, i| {
        s.b.play_queue_position(i as usize);
        s.follow = true;
    });
    on!(on_queue_act, |s, u, i, a| s.queue_act(
        &u,
        i as usize,
        a.as_str()
    ));
    on!(on_queue_save, |s, u| {
        if s.b.player.queue().is_empty() {
            return;
        }
        let paths = s.b.queue_playlist_paths();
        s.open_pick(&u, "the queue", paths);
    });
    on!(on_queue_load, |s, u, name| {
        let _ = &u;
        s.b.load_server_queue(name.as_str());
    });
    on!(on_queue_clear, |s, u| {
        if !s.b.player.queue().is_empty() {
            s.confirm(
                &u,
                "Clear the whole queue?".into(),
                "Clear",
                Confirm::ClearQueue,
            );
        }
    });
    on!(on_play_pause, |s, u| s.b.player.send(Command::TogglePause));
    on!(on_prev, |s, u| s.b.player.send(Command::Back));
    on!(on_next, |s, u| s.b.player.send(Command::Next));
    on!(on_toggle_shuffle, |s, u| s
        .b
        .player
        .send(Command::ToggleShuffle));
    on!(on_cycle_repeat, |s, u| s
        .b
        .player
        .send(Command::CycleRepeat));
    on!(on_seek, |s, u, f| {
        if let Some(d) = s.snap.duration {
            s.b.player
                .send(Command::Seek(d.mul_f64(f.clamp(0.0, 1.0) as f64)));
        }
    });
    on!(on_set_volume, |s, u, f| s
        .b
        .player
        .send(Command::SetVolume(f.clamp(0.0, 1.0) as f64)));
    on!(on_toggle_mute, |s, u| s.b.player.send(Command::ToggleMute));
    on!(on_toggle_queue, |s, u| u
        .set_queue_panel(!u.get_queue_panel()));
    on!(on_speed_clicked, |s, u| {
        // Click cycles the common speeds.
        let next = match s.b.podcast_tempo {
            t if t < 1.15 => 1.2,
            t if t < 1.45 => 1.5,
            t if t < 1.95 => 2.0,
            _ => 1.0,
        };
        s.b.set_podcast_playback(next, s.b.podcast_trim);
    });
    on!(on_dlg_accept, |s, u| s.dlg_accept(&u));
    on!(on_dlg_pick, |s, u, i| s.dlg_pick(&u, i as usize));
    on!(on_dlg_cancel, |s, u| {
        s.dlg = Dlg::None;
        u.set_dialog(0);
        u.invoke_focus_root();
    });
    on!(on_key, |s, u, text, ctrl, shift, alt| s.key(
        &u,
        text.as_str(),
        ctrl,
        shift,
        alt
    ));

    let timer = Timer::default();
    {
        let st = st.clone();
        let weak = ui.as_weak();
        timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            if let Some(u) = weak.upgrade() {
                st.borrow_mut().tick(&u);
            }
        });
    }
    st.borrow_mut().tick(ui);
    ui.invoke_focus_root();
    Bridge {
        _timer: timer,
        _state: st,
    }
}

/// "12 tracks", "3 stations", or "4 artists, 2 albums, 9 tracks" for a mix.
fn count_label(items: &[Item]) -> String {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for it in items {
        let noun = match it {
            Item::Track(_) | Item::Missing { .. } => "track",
            Item::Album(_) => "album",
            Item::Artist { .. } => "artist",
            Item::Dir { .. } | Item::Root { .. } => "folder",
            Item::Genre(_) => "genre",
            Item::Playlist(_) => "playlist",
            Item::Station { .. } => "station",
            Item::Podcast(_) => "podcast",
            Item::Episode(_) => "episode",
            Item::Pinned(_) => "pin",
            Item::Job(_) => "download",
            Item::Header(_) | Item::Info { .. } | Item::Setting(_) | Item::Action { .. } => {
                continue;
            }
        };
        match counts.iter_mut().find(|(n, _)| *n == noun) {
            Some((_, c)) => *c += 1,
            None => counts.push((noun, 1)),
        }
    }
    counts
        .iter()
        .map(|(n, c)| format!("{c} {n}{}", if *c == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ")
}
