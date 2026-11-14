//! Between the Slint window and the shared Browser: turns Browser state
//! into the models the window draws, and every click and key into a
//! Browser or player call. Nothing here decides what an action means for
//! the library or queue; that's patina-core's job, shared with the TUI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use patina_core::browse::covers::CoverTarget;
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
use crate::desktop::Action;
use crate::platform::{self, Picked, Prompt, PromptEvent, PromptField};
use crate::{
    AmpKnob, AmpUnit, AppWindow, ChainRow, CoverPick, EqBand, Hero, Icons, P, QueueRow, Row,
    SideItem, Tint, Ui,
};
use patina_core::dsp::amp::Kind as AmpKind;
use patina_core::eq::Mode as EqMode;
use patina_core::library::HEARTED;

enum Dlg {
    None,
    /// Picking one of a setting's choices.
    Choose(SettingKey),
    /// Typing a setting's value.
    SetValue(SettingKey),
    /// Naming an EQ preset.
    EqSave,
    /// Naming an amp stack preset.
    AmpSave,
    /// Typing the path of an EQ file, when there's no file picker.
    EqImportPath,
    AddFolder,
    NewPlaylist(Vec<PathBuf>),
    RenamePlaylist(i64),
    RenameMix(i64),
    Station(Option<usize>),
    Feed,
    /// Adding a server, or editing the one with this name.
    Server(Option<String>),
    /// Renaming a DLNA server, which has nothing else to edit.
    RenameServer(String),
    /// Picking one of the DLNA servers a search found.
    Dlna(Vec<patina_core::remote::dlna::Found>),
    /// A new smart playlist: its name and its query.
    SmartNew,
    /// Changing one.
    SmartEdit(i64),
    /// Where to write a playlist file: the selection's (false) or the
    /// queue's (true).
    Export(bool),
    /// The path of a playlist file to read, when there's no file picker.
    ImportPath,
    /// The sleep timer's own time.
    Sleep,
    /// The paths, the servers offering "New playlist on…", the playlists.
    Pick(Vec<PathBuf>, Vec<String>, Vec<PlaylistRow>),
    /// Naming a new playlist on a server.
    ServerPlaylist(String, Vec<PathBuf>),
    Confirm(Confirm),
    Help,
    /// Picking a cover found online.
    Covers,
    /// Where to play: this device, or one found on the network.
    Cast(Vec<CastRow>),
    /// Typing a snapserver's address.
    CastSnapcast,
    /// Picking a player to control.
    Players(Vec<PlayerRow>),
    /// The code for the player at this address, with its name.
    PlayerCode(String, String),
    /// Typing a player's address and code.
    PlayerAddr,
    /// The controlled player's queue, by entry id.
    RemoteQueue(Vec<u64>),
}

/// A row of the Play on picker.
#[derive(Clone)]
enum CastRow {
    Here,
    To(patina_core::cast::Target),
    Looking,
    Snapcast,
}

/// A row of the player picker.
#[derive(Clone)]
enum PlayerRow {
    Found(patina_core::control::Found),
    Looking,
    Type,
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
    DeleteMix(i64),
    DeleteSmart(i64),
    IpodRemove(patina_core::browse::ipod::IpodRow),
}

pub struct State {
    b: Browser,
    art: ArtCache,
    snap: Snapshot,
    rows: Rc<VecModel<Row>>,
    /// The EQ panel's band handles. Kept and updated in place, so a handle
    /// being dragged isn't torn down under the pointer.
    eq_bands: Rc<VecModel<EqBand>>,
    /// What's in Favorites, read once per rebuild of the rows.
    fav_keys: std::collections::HashSet<(patina_core::library::marks::FavKind, Vec<u8>)>,
    /// The lyrics as last shown, so the list is only replaced on change.
    lyrics_shown: (Vec<String>, i32),
    /// The rack: a row per unit, and each unit's knobs in a model of its
    /// own, updated in place so a knob being dragged keeps its grip.
    amp_units: Rc<VecModel<AmpUnit>>,
    amp_knobs: Vec<Rc<VecModel<AmpKnob>>>,
    amp_kinds: Vec<AmpKind>,
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
    /// Whether the sidebar was last drawn with Now Playing open, so it's
    /// redrawn when the page opens or closes from the bottom bar or Esc.
    np_shown: bool,
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
    folder_pick: Option<(crossbeam_channel::Receiver<Picked>, Instant)>,
    /// An EQ file being picked to import.
    eq_pick: Option<(crossbeam_channel::Receiver<Picked>, Instant)>,
    /// The system's text dialog standing in for the window's (Android),
    /// by its id, and the next id to hand out.
    prompt_open: Option<u64>,
    prompt_seq: u64,
    /// The last one was answered but its text didn't take (a bad path, a
    /// station with no URL): the next one says why.
    prompt_retry: bool,
    /// The system's search box, while it's up.
    search_prompt: Option<u64>,
    /// When a row was last opened or played. A click this soon after is
    /// the second half of a double-click, from the habit of double-clicking:
    /// it would otherwise open whatever row the new list put under it.
    last_activate: Option<Instant>,
    /// The theme on screen, so a change in Settings redraws.
    theme: Option<Theme>,
    /// The System theme: when the system's colors were last asked for,
    /// and what was put on screen from them.
    system_checked: Option<Instant>,
    system_shown: Option<Option<crate::system_theme::SystemColors>>,
    /// The colors last sent to the home screen widget.
    widget_sent: Option<platform::WidgetColors>,
    /// MPRIS and the tray icon.
    desktop: crate::desktop::Desktop,
    /// What Back does with nowhere left to go back to (Android: send the
    /// app to the background, still playing). None leaves it unhandled.
    back_out: Option<Box<dyn Fn()>>,
    /// Closing the window hides it (there's a tray icon to come back
    /// from, and the setting says so). Read by the close handler.
    hide_on_close: Rc<std::cell::Cell<bool>>,
    /// Taps pick rows instead of opening them: a phone, after Select.
    marking: bool,
    queue_marking: bool,
    /// A playlist row, or a queue entry, lifted to move by a tap on its
    /// handle (a phone), waiting for a tap on where it goes.
    lifted: Option<usize>,
    queue_lifted: Option<usize>,
    /// The Now Playing page's sound chain.
    np_chain: Rc<VecModel<ChainRow>>,
    /// A playlist file being picked to import.
    import_pick: Option<(crossbeam_channel::Receiver<Picked>, Instant)>,
    /// Rows were picked or let go of: their marks need redrawing.
    marks_dirty: bool,
    /// The album grid's tiles near the screen, a slice of `rows` from
    /// `grid_base` on. The grid builds only these.
    grid_rows: Rc<VecModel<Row>>,
    grid_base: usize,
    /// The sidebar as last drawn, by the browser's sidebar generation.
    last_side_gen: u64,
    /// What the rows were last built from: the level, its items' revision
    /// and everything else a row shows. While it's the same, a new song
    /// only moves the playing mark instead of every row being built again.
    rows_sig: Option<(String, u64, u64)>,
    /// The song the rows last marked as playing.
    rows_playing: Option<PathBuf>,
    /// The last two big lists' rows, to show again without building them.
    rows_cache: Vec<RowsCache>,
    /// Each queue entry's row as last drawn, by id and revision, to reuse
    /// while the entry hasn't changed, and what greyed out then.
    queue_rows: HashMap<patina_core::queue::EntryId, (u64, QueueRow)>,
    queue_decor: u64,
    /// The cover picker: what it's for and what's been found.
    covers: CoverPicker,
    covers_model: Rc<VecModel<CoverPick>>,
    /// An image being picked for a cover, and the album it's for.
    cover_file_pick: Option<(crossbeam_channel::Receiver<Picked>, CoverTarget)>,
    /// The blurred cover behind Now Playing, by the song and whether its
    /// cover had arrived.
    blur_key: Option<(PathBuf, bool)>,
    /// What the backdrop and the cover accent were made for: whether each
    /// is on, and the theme's ground color.
    look_flags: (bool, bool, u32, u8),
    /// Placeholder rows went up for a section still to be read.
    loading_shown: bool,
    /// The cast and player pickers, as last filled: redrawn when a search
    /// finishes while they're open.
    cast_shown: u64,
    players_shown: u64,
    /// Another player being steered from here.
    remote: Option<crate::remote_view::RemoteView>,
    /// Now Playing's visualizer: the analyzer and its bars' model.
    vis: patina_core::visual::Analyzer,
    vis_bars: Rc<VecModel<f32>>,
}

/// Lists at least this long keep their built rows for when they're opened
/// again.
const CACHE_ROWS: usize = 2000;

/// A big list's rows as built, by what they were built from.
struct RowsCache {
    sig: (String, u64, u64),
    rows: Vec<Row>,
    art: Vec<Option<Req>>,
    height: f32,
}

/// The cover picker's search, running off the window's thread.
#[derive(Default)]
struct CoverPicker {
    target: Option<CoverTarget>,
    found: Vec<patina_core::cover::Found>,
    rx: Option<crossbeam_channel::Receiver<CoverMsg>>,
    /// Bumped per search, so answers from an earlier one are dropped.
    seq: u64,
}

enum CoverMsg {
    /// What was found, without pictures yet.
    Found(u64, Vec<patina_core::cover::Found>),
    /// One thumbnail, by its place in the list; None if it wouldn't load.
    Thumb(u64, usize, Option<image::RgbaImage>),
    /// Every thumbnail is in.
    Done(u64),
    /// The picked cover is saved, or why not.
    Saved(Result<(patina_core::cover::Saved, PathBuf), String>),
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
X            mix from the selection: artist, album, song or genres
M            mark a genre for a mix; in the Queue, save the mix
Delete       remove from the queue, the open playlist, or Settings;
             in Downloads: cancel a download or unpin
Enter        in Settings: flip, pick or edit the option
[ ]  z       podcast speed, silence trimming
q            queue panel
l            love the song playing
Ctrl+E       the EQ (o in it: on or bypassed)
Ctrl+A       the amp stack: drag a dial up or down to turn it
Ctrl+O       pick the output device (or click the output on the bottom bar)
Ctrl+L       lyrics for the song playing (looked up only when asked)
Right-click a song or album to rate it, a folder, album or artist to
             add it to Favorites; the album header and the bottom bar
             have stars too
Right-click an album, a folder or a cover to find cover art online,
             or to use an image file as the cover
F5           check folders and feeds for changes
Ctrl+Q       quit. Closing the window only hides it to the tray
             while there's a tray icon (Settings, Desktop)

Media keys and the desktop's media controls work through MPRIS.
Mono, width, crossfeed and balance are in Settings, Channel tools.
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
        Section::Eq => "イコライザー",
        Section::Amp => "アンプ",
        Section::MostListened => "いちばん",
        Section::TopAlbums => "ベスト",
        Section::TopArtists => "スター",
        Section::History => "さいきん",
        Section::Ipod => "アイポッド",
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

/// Rows the GUI puts at the top of the sidebar ahead of the core's (Now
/// Playing), so a sidebar index minus this is the core's index.
const SIDE_EXTRA: i32 = 1;

impl State {
    fn new(b: Browser, hide_on_close: Rc<std::cell::Cell<bool>>) -> Self {
        let desktop = crate::desktop::Desktop::start(b.player.clone(), b.config.desktop.tray);
        Self {
            b,
            art: ArtCache::new(),
            snap: Snapshot::default(),
            rows: Rc::new(VecModel::default()),
            eq_bands: Rc::new(VecModel::default()),
            fav_keys: Default::default(),
            lyrics_shown: (Vec::new(), -1),
            amp_units: Rc::new(VecModel::default()),
            amp_knobs: Vec::new(),
            amp_kinds: Vec::new(),
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
            np_shown: false,
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
            eq_pick: None,
            prompt_open: None,
            prompt_seq: 0,
            prompt_retry: false,
            search_prompt: None,
            last_activate: None,
            theme: None,
            system_checked: None,
            system_shown: None,
            widget_sent: None,
            desktop,
            back_out: None,
            hide_on_close,
            marking: false,
            queue_marking: false,
            lifted: None,
            queue_lifted: None,
            np_chain: Rc::new(VecModel::default()),
            import_pick: None,
            marks_dirty: false,
            grid_rows: Rc::new(VecModel::default()),
            grid_base: 0,
            last_side_gen: u64::MAX,
            rows_sig: None,
            rows_playing: None,
            rows_cache: Vec::new(),
            queue_rows: HashMap::new(),
            queue_decor: 0,
            covers: CoverPicker::default(),
            covers_model: Rc::new(VecModel::default()),
            cover_file_pick: None,
            blur_key: None,
            look_flags: (false, false, 0, 0),
            loading_shown: false,
            cast_shown: u64::MAX,
            players_shown: u64::MAX,
            remote: None,
            vis: patina_core::visual::Analyzer::new(VIS_BARS),
            vis_bars: Rc::new(VecModel::default()),
        }
    }

    fn art_req(item: &Item) -> Option<Req> {
        match item {
            Item::Track(t) => Some(Req::Track(t.path.clone())),
            Item::Album(a) => a.cover_track.clone().map(Req::Track),
            Item::Dir { path, .. } => Some(Req::Dir(path.clone())),
            Item::Artist(a) => Some(Req::Artist(a.name.clone())),
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

    fn row(&self, item: &Item, i: usize, ctx: RowCtx, playing: Option<&std::path::Path>) -> Row {
        let mut r = Row {
            selectable: item.selectable(),
            num: ss((i + 1).to_string()),
            rating: -1,
            fav: -1,
            ..Default::default()
        };
        if let Some(rt) = Browser::item_rating(item) {
            r.rating = rt as i32;
            r.stars = ss("★".repeat(rt as usize));
        }
        if let Some(k) = patina_core::browse::marks::fav_key(item) {
            r.fav = i32::from(self.fav_keys.contains(&k));
        }
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
                r.playing = playing == Some(t.path.as_path());
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
                r.on = self.b.genre_marked(&g.norm);
                r.title = ss(&g.name);
                r.sub = ss(format!("{} album{}", g.albums, s_(g.albums)));
                r.right = ss(format!("{} track{}", g.tracks, s_(g.tracks)));
            }
            Item::Playlist(p) => {
                r.kind = 5;
                r.fixed = p.id == HEARTED;
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
                    Control::Action(_) if st.key == SettingKey::MeasureLibrary => (11, false),
                    Control::Action(_) if st.key.is_eq() || st.key.is_amp() => (11, false),
                    Control::Action(_) if st.key == SettingKey::LastFmLink => (11, false),
                    Control::Action(_)
                        if matches!(
                            st.key,
                            SettingKey::RemoteCode | SettingKey::ControlAnother
                        ) =>
                    {
                        (11, false)
                    }
                    Control::Action(_) => (6, false),
                    Control::Info(_) => (7, false),
                    Control::Server(_) => (8, false),
                };
                if matches!(st.control, Control::Folder) {
                    r.meta = SharedString::new();
                }
                if let Some(why) = st.disabled {
                    r.off = true;
                    r.meta = ss(why);
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
            Item::Mix(m) => {
                r.kind = 16;
                r.title = ss(&m.name);
                r.sub = ss(match m.kind.as_str() {
                    "genre" => "Genre mix",
                    "artist" => "Artist mix",
                    _ => "Track mix",
                });
                r.right = ss(if m.kind == "genre" { "" } else { "endless" });
                r.playing = self.b.mix.as_ref().and_then(|x| x.saved) == Some(m.id);
            }
            Item::Smart(p) => {
                r.kind = 17;
                r.title = ss(&p.name);
                r.sub = ss(p.rules());
                r.dim = false;
                r.right = ss(format!("{} track{}", p.items, s_(p.items)));
            }
            Item::Ipod(row) => {
                r.kind = 18;
                r.num = SharedString::new();
                r.title = ss(&row.label);
                r.sub = ss(&row.detail);
            }
        }
        r.off = r.off || !self.b.plays(item);
        r.can_mix = self.b.can_mix(item) && !matches!(item, Item::Mix(_));
        r.can_pin = self.b.can_pin(item);
        r.pinned = r.can_pin && self.b.pinned(item).is_some();
        r
    }

    /// Columns, title actions and layout for the open level. Every view
    /// that can be played as a whole gets Play and Shuffle up front.
    fn chrome(&self) -> (i32, Vec<&'static str>, Vec<(&'static str, String)>) {
        let (layout, cols, mut acts) = self.chrome_base();
        let src = self.b.level().map(|l| l.source.clone());
        match (self.b.section, &src) {
            (Section::Queue, _) if self.b.mix.as_ref().is_some_and(|m| m.saved.is_none()) => {
                acts.push(("save-mix", "Save this mix".into()));
            }
            (Section::Genres, Some(Source::Section(_))) if !self.b.genre_marks.is_empty() => {
                let n = self.b.genre_marks.len();
                acts.push((
                    "genre-mix",
                    if n == 1 {
                        "Play mix".into()
                    } else {
                        format!("Play mix of {n} genres")
                    },
                ));
                acts.push(("clear-marks", "Clear marks".into()));
            }
            (_, Some(Source::Genre { .. } | Source::ArtistAlbums(_) | Source::Album { .. }))
                if self.b.mixes.is_some() =>
            {
                acts.push(("mix", "Mix".into()));
            }
            _ => {}
        }
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
        if self.b.ipod_connected() && self.b.can_play_view() {
            acts.push(("ipod", "Send to iPod".into()));
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
            (Section::Genres, Some(Source::Section(_))) => {
                (0, vec!["", "Genre", "Albums", "", "Tracks"], vec![])
            }
            (Section::Playlists, Some(Source::Section(_))) => (
                0,
                vec!["", "Playlist", "", "", "Tracks"],
                vec![
                    ("new", "New playlist".into()),
                    ("new-smart", "New smart playlist".into()),
                    ("import", "Import…".into()),
                ],
            ),
            (_, Some(Source::Playlist { .. })) => {
                (0, tracks_cols, vec![("export", "Export…".into())])
            }
            (_, Some(Source::Smart { .. })) => (
                0,
                tracks_cols,
                vec![("smart-edit", "Edit".into()), ("export", "Export…".into())],
            ),
            (Section::History, _) => (0, tracks_cols, vec![]),
            (Section::Ipod, _) => (0, vec![], vec![]),
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
            (Section::Settings | Section::Eq | Section::Amp, _) => (0, vec![], vec![]),
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
            Some(Source::Section(Section::History)) => {
                "Nothing played yet. Songs land here once they count as played: half of one, or four minutes of it.".into()
            }
            Some(Source::Smart { .. }) => {
                "Nothing in the library matches this smart playlist right now. Edit changes what it looks for.".into()
            }
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
        ui.set_loading(self.b.loading());
        ui.set_loading_thumbs(matches!(
            self.b.section,
            Section::Folders | Section::Artists | Section::Albums
        ));
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
        ui.set_hearted_open(self.b.in_playlist() == Some(HEARTED));
        self.sync_eq_panel(ui);
        self.sync_amp_panel(ui);
        ui.set_queue_mix_unsaved(
            self.b
                .mix
                .as_ref()
                .is_some_and(|m| m.started && m.saved.is_none()),
        );

        let key = format!(
            "{:?}/{}/{:?}",
            self.b.section,
            self.b.depth(),
            self.b.level().map(|l| &l.source)
        );
        let level_changed = key != self.level_key;
        self.level_key = key;

        let empty = Vec::new();
        let (items, selected, title) = match self.b.level() {
            Some(l) => (&l.items, l.selected as i32, l.title.clone()),
            None => (&empty, -1, self.b.section.label().to_string()),
        };
        let ctx = match self.b.level().map(|l| &l.source) {
            Some(Source::Album { .. }) => RowCtx::Album,
            Some(Source::Section(s)) if Section::STATS.contains(s) && *s != Section::History => {
                RowCtx::Ranked
            }
            _ => RowCtx::Plain,
        };
        // Numbers count within each headed group, so search results read
        // 1, 2, 3 under Songs instead of carrying on from Albums.
        let mut n = 0;
        let fav_keys = self
            .b
            .favorites()
            .into_iter()
            .map(|f| (f.kind, f.key))
            .collect();
        self.fav_keys = fav_keys;
        let playing = self.playing_path();
        // The same rows as last time, apart from which song plays: move
        // the playing mark and keep the rest. Building every row again for
        // a new song made 85,000 songs stall the window.
        let library_view = matches!(
            self.b.section,
            Section::Folders
                | Section::Artists
                | Section::Albums
                | Section::Songs
                | Section::Genres
                | Section::Playlists
                | Section::History
                | Section::MostListened
                | Section::TopAlbums
                | Section::TopArtists
        );
        let sig = (
            self.level_key.clone(),
            self.b.level().map(|l| l.rev).unwrap_or(0),
            self.decor_stamp(),
        );
        if library_view
            && self.rows_sig.as_ref() == Some(&sig)
            && self.rows.row_count() == items.len()
        {
            if playing != self.rows_playing {
                for (i, it) in items.iter().enumerate() {
                    let Item::Track(t) = it else { continue };
                    let now = playing.as_deref() == Some(t.path.as_path());
                    let was = self.rows_playing.as_deref() == Some(t.path.as_path());
                    if now != was
                        && let Some(mut r) = self.rows.row_data(i)
                    {
                        r.playing = now;
                        self.rows.set_row_data(i, r);
                    }
                }
                self.rows_playing = playing;
            }
            let sel_ok = items
                .get(selected.max(0) as usize)
                .is_some_and(Item::selectable);
            ui.set_selected(if sel_ok { selected } else { -1 });
            let had_marks = ui.get_marked_count() > 0;
            ui.set_marked_count(self.b.marked_count() as i32);
            if had_marks || self.b.marked_count() > 0 {
                self.refresh_marks(ui);
            }
            if self.b.marked_count() == 0 && !is_touch(ui) {
                self.marking = false;
            }
            ui.set_marking(self.marking);
            ui.set_ipod(self.b.ipod_connected());
            return;
        }
        self.rows_sig = Some(sig.clone());
        self.rows_playing = playing.clone();
        let marked: std::collections::HashSet<String> =
            self.b.level().map(|l| l.marked.clone()).unwrap_or_default();
        // A big list opened again (back to Songs from Albums) takes the
        // rows built last time, with the playing mark and picks redone.
        let cached = self
            .rows_cache
            .iter()
            .position(|c| c.sig == sig && c.rows.len() == items.len())
            .map(|i| self.rows_cache.remove(i));
        let (rows, row_art, rows_h) = match cached {
            Some(c) => {
                let mut rows = c.rows.clone();
                for (r, it) in rows.iter_mut().zip(items) {
                    if let Item::Track(t) = it {
                        r.playing = playing.as_deref() == Some(t.path.as_path());
                    }
                    r.marked = !marked.is_empty() && marked.contains(&it.key());
                }
                let out = (rows, c.art.clone(), c.height);
                self.rows_cache.push(c);
                out
            }
            None => {
                let rows: Vec<Row> = items
                    .iter()
                    .map(|it| {
                        if matches!(it, Item::Header(_)) {
                            n = 0;
                        } else {
                            n += 1;
                        }
                        let mut r = self.row(it, n.max(1) - 1, ctx, playing.as_deref());
                        r.marked = !marked.is_empty() && marked.contains(&it.key());
                        r
                    })
                    .collect();
                // Songs in a list don't show covers; everything else that
                // has one does.
                let row_art: Vec<Option<Req>> = items
                    .iter()
                    .map(|it| match it {
                        Item::Track(_) => None,
                        other => Self::art_req(other),
                    })
                    .collect();
                // The rows' whole height, for views where the page scrolls
                // as one (the Amp and EQ panels on a phone). Matches
                // ListRow's heights.
                let rows_h: f32 = rows
                    .iter()
                    .map(|r| match r.kind {
                        9 => 34.0,
                        12 => 54.0,
                        _ => 40.0,
                    })
                    .sum();
                if library_view && rows.len() >= CACHE_ROWS {
                    self.rows_cache.push(RowsCache {
                        sig,
                        rows: rows.clone(),
                        art: row_art.clone(),
                        height: rows_h,
                    });
                    if self.rows_cache.len() > 2 {
                        self.rows_cache.remove(0);
                    }
                }
                (rows, row_art, rows_h)
            }
        };
        ui.set_marked_count(self.b.marked_count() as i32);
        if self.b.marked_count() == 0 && !is_touch(ui) {
            self.marking = false;
        }
        ui.set_marking(self.marking);
        let draggable = self.b.in_playlist().is_some_and(|id| {
            id != HEARTED && !self.b.playlists().iter().any(|p| p.id == id && p.readonly)
        });
        ui.set_draggable(draggable);
        if !draggable {
            self.lifted = None;
        }
        ui.set_lifted(self.lifted.map(|i| i as i32).unwrap_or(-1));
        ui.set_ipod(self.b.ipod_connected());
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
            _ => count_label(items),
        }));
        ui.set_covers(items.iter().any(|i| {
            matches!(
                i,
                Item::Album(_) | Item::Dir { .. } | Item::Root { .. } | Item::Artist(_)
            )
        }));
        let mut rows = rows;
        let shown = self.row_ranges(ui, rows.len());
        for r in &shown {
            for i in r.clone() {
                if let Some(req) = row_art[i].clone() {
                    match self.art.get(req.clone(), THUMB) {
                        Some(img) => {
                            rows[i].art = img;
                            rows[i].has_art = true;
                        }
                        None => rows[i].wait = self.art.loading(&req, THUMB),
                    }
                }
            }
        }
        self.row_shown = shown;
        self.row_art = row_art;
        ui.set_rows_height(rows_h);
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
                        rating: self.b.level_rating().map(i32::from).unwrap_or(-1),
                        fav: self
                            .b
                            .level_fav_target()
                            .map(|t| i32::from(self.b.is_favorite(&t)))
                            .unwrap_or(-1),
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
            // Browsing somewhere (search, back, Go to album, a link in a
            // menu) shows where it went, rather than leaving Now Playing
            // over it.
            ui.set_np_open(false);
        }
    }

    /// Everything a row shows that isn't in its item: Favorites, genres
    /// marked for a mix, what greys out with a server away, what's pinned,
    /// the mix playing, an iPod. A change in any of these builds the rows
    /// again.
    fn decor_stamp(&self) -> u64 {
        let one = |x: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            x(&mut h);
            h.finish()
        };
        // Favorites in any order: their hashes added up.
        let favs = self
            .fav_keys
            .iter()
            .map(|k| one(&|h| k.hash(h)))
            .fold(0u64, u64::wrapping_add);
        one(&|h| {
            favs.hash(h);
            for g in &self.b.genre_marks {
                g.norm.hash(h);
            }
            if self.b.avail.any_offline() {
                self.b.avail.stamp.hash(h);
            }
            for p in &self.b.cache_status.pins {
                p.pin.id.hash(h);
            }
            self.b.mix.as_ref().and_then(|m| m.saved).hash(h);
            self.b.ipod_connected().hash(h);
            self.b.config.appearance.theme.is_kawaii().hash(h);
        })
    }

    /// Send the theme's colors to the home screen widget when they change:
    /// a new theme, or for System, a new wallpaper or dark mode. Read from
    /// the window's own palette, so the widget matches it exactly.
    fn sync_widget_colors(&mut self, ui: &AppWindow) {
        let p = ui.global::<P>();
        let c = platform::WidgetColors {
            surface: p.get_surface().as_argb_encoded(),
            border: p.get_border().as_argb_encoded(),
            text: p.get_text().as_argb_encoded(),
            muted: p.get_muted().as_argb_encoded(),
            accent: p.get_verdigris().as_argb_encoded(),
            on_accent: p.get_on_accent().as_argb_encoded(),
        };
        if self.widget_sent != Some(c) {
            self.widget_sent = Some(c);
            platform::widget_colors(&c);
        }
    }

    fn sync_side(&mut self, ui: &AppWindow) {
        let icons = ui.global::<Icons>();
        let kawaii = self.b.config.appearance.theme.is_kawaii();
        let np = ui.get_np_open();
        self.np_shown = np;
        let status = self.b.service.as_ref().map(|s| s.status());
        let open_folder = match self.b.level().map(|l| &l.source) {
            Some(Source::Folder(p)) if self.b.section == Section::Folders => Some(p.clone()),
            _ => None,
        };
        // Now Playing first. It isn't one of the core's sections (the
        // terminal app opens it as a popup), so it's added here and the
        // sidebar's indexes are one past the core's: see SIDE_EXTRA.
        let now_playing = SideItem {
            kind: 0,
            label: ss("Now Playing"),
            sub: ss(if kawaii { "いま きいてる" } else { "" }),
            icon: icons.get_now_playing(),
            active: np,
            count: SharedString::new(),
            dot: 0,
        };
        let items: Vec<SideItem> = std::iter::once(now_playing)
            .chain(self.b.sidebar.iter().map(|e| match e {
                SideEntry::Section(s) => SideItem {
                    kind: 0,
                    label: ss(match s {
                        Section::Ipod => self.b.ipod_name().unwrap_or_else(|| "iPod".into()),
                        _ => s.label().to_string(),
                    }),
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
                        Section::Eq => icons.get_eq(),
                        Section::Amp => icons.get_amp(),
                        Section::MostListened => icons.get_most_listened(),
                        Section::TopAlbums => icons.get_top_albums(),
                        Section::TopArtists => icons.get_top_artists(),
                        Section::History => icons.get_history(),
                        Section::Ipod => icons.get_ipod(),
                    },
                    active: *s == self.b.section && !np,
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
                    active: open_folder.as_ref() == Some(path) && !np,
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
                        active: open_folder.as_ref() == Some(path) && !np,
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
                SideEntry::Favorite { label, .. } => SideItem {
                    kind: 4,
                    label: ss(label),
                    sub: SharedString::new(),
                    icon: icons.get_star(),
                    active: false,
                    count: SharedString::new(),
                    dot: 0,
                },
                SideEntry::Mix { id, name } => SideItem {
                    kind: 0,
                    label: ss(name),
                    sub: SharedString::new(),
                    icon: icons.get_radio(),
                    active: self.b.mix.as_ref().and_then(|m| m.saved) == Some(*id) && !np,
                    count: SharedString::new(),
                    dot: 0,
                },
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
            }))
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
        // Each entry's row is kept by its id and revision, and only built
        // again when its tags change. A shuffled library of 85,000 songs
        // used to be built whole on every new song.
        let marks: std::collections::HashSet<patina_core::queue::EntryId> =
            self.b.queue_marks.iter().copied().collect();
        let decor = if self.b.avail.any_offline() {
            self.b.avail.stamp
        } else {
            0
        };
        if decor != self.queue_decor {
            self.queue_rows.clear();
            self.queue_decor = decor;
        }
        let mut ids: Vec<patina_core::queue::EntryId> = Vec::new();
        let mut models: Vec<QueueRow> = Vec::new();
        let mut rows_art: Vec<Option<Req>> = Vec::new();
        let (current, total) = {
            let q = self.b.player.queue();
            let cur = q.current();
            let order = q.play_order();
            ids.reserve(order.len());
            models.reserve(order.len());
            rows_art.reserve(order.len());
            let mut total = Duration::ZERO;
            let mut current = -1;
            for (pos, &id) in order.iter().enumerate() {
                let Some(e) = q.get(id) else { continue };
                total += e.info.duration.unwrap_or_default();
                if cur == Some(id) {
                    current = pos as i32;
                }
                let mut row = match self.queue_rows.get(&id) {
                    Some((rev, row)) if *rev == e.rev => row.clone(),
                    _ => {
                        let row = QueueRow {
                            off: !self.b.avail.info_plays(&e.info),
                            title: ss(e.info.display_title()),
                            sub: ss(sub_line(&e.info)),
                            failed: e.failed,
                            ..Default::default()
                        };
                        self.queue_rows.insert(id, (e.rev, row.clone()));
                        row
                    }
                };
                row.playing = cur == Some(id);
                row.marked = marks.contains(&id);
                models.push(row);
                rows_art.push(
                    (e.info.kind == MediaKind::File && !e.info.path.as_os_str().is_empty())
                        .then(|| Req::Track(e.info.path.clone())),
                );
                ids.push(id);
            }
            (current, total)
        };
        if self.queue_rows.len() > ids.len() * 2 + 64 {
            let keep: std::collections::HashSet<_> = ids.iter().copied().collect();
            self.queue_rows.retain(|id, _| keep.contains(id));
        }
        let n = models.len();
        self.queue_art = rows_art;
        let shown = self.queue_ranges(ui, models.len());
        for r in &shown {
            for i in r.clone() {
                if let Some(req) = self.queue_art[i].clone() {
                    match self.art.get(req.clone(), THUMB) {
                        Some(img) => {
                            models[i].art = img;
                            models[i].has_art = true;
                        }
                        None => models[i].wait = self.art.loading(&req, THUMB),
                    }
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
        ui.set_queue_marked_count(self.b.queue_marks.len() as i32);
        if self.b.queue_marks.is_empty() && !is_touch(ui) {
            self.queue_marking = false;
        }
        ui.set_queue_marking(self.queue_marking);
        if self.queue_lifted.is_some_and(|i| i >= n) {
            self.queue_lifted = None;
        }
        ui.set_queue_lifted(self.queue_lifted.map(|i| i as i32).unwrap_or(-1));
        ui.set_queue_summary(ss(match (n, self.b.mix_title()) {
            (0, _) => String::new(),
            (_, Some(mix)) => format!("{mix} · {n} track{} so far", s_(n as i64)),
            _ => format!("{n} track{} · {}", s_(n as i64), fmt_length(total)),
        }));
    }

    fn sync_playback(&mut self, ui: &AppWindow) {
        // Every tick: the stars follow the song, and synced lyrics the
        // position.
        ui.set_np_rating(self.b.playing_rating().map(i32::from).unwrap_or(-1));
        self.sync_lyrics(ui);
        if ui.get_np_open() {
            self.sync_now_playing(ui);
        }
        let s = &self.snap;
        let track = s.track.clone();
        ui.set_has_track(track.is_some());
        ui.set_playing(s.state == PlayState::Playing);
        ui.set_live(s.live);
        ui.set_sleep_label(ss(s.sleep.map(|t| t.short()).unwrap_or_default()));
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
                ui.set_limit_label(ss(if s.limiting_db >= 0.1 {
                    format!("LIM -{:.1} dB", s.limiting_db.min(99.9))
                } else {
                    String::new()
                }));
                match s.level {
                    Some(l) => {
                        ui.set_level_label(ss(l.short()));
                        ui.set_level_tip(ss(format!(
                            "Leveling {}. {}",
                            l.text(),
                            l.source.detail()
                        )));
                    }
                    None => {
                        ui.set_level_label(SharedString::new());
                        ui.set_level_tip(SharedString::new());
                    }
                }
            }
            None => {
                ui.set_np_title(SharedString::new());
                ui.set_np_sub(SharedString::new());
                ui.set_np_has_art(false);
                ui.set_badge(SharedString::new());
                ui.set_bp_badge(SharedString::new());
                ui.set_limit_label(SharedString::new());
                ui.set_level_label(SharedString::new());
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
        ui.set_casting(s.cast.is_some());
        ui.set_cast_name(ss(s
            .cast
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default()));
        ui.set_resample_label(ss(s
            .resample
            .map(patina_core::browse::settings::resample_badge)
            .unwrap_or_default()));
        let podcast = s
            .track
            .as_ref()
            .is_some_and(|t| t.kind == MediaKind::Podcast);
        ui.set_speed_tempo(s.podcast_tempo as f32);
        ui.set_speed_trim(s.podcast_trim);
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
        // A section opened last tick: its placeholder rows have been drawn,
        // so read it now.
        if self.loading_shown {
            self.loading_shown = false;
            self.b.finish_pending();
        }
        for ev in self.b.poll() {
            if let Event::TrackChanged(_) = ev {
                self.follow = true;
            }
        }
        self.snap = self.b.player.snapshot();
        // Answers from the system's text dialogs first, so what they change
        // shows in this same tick.
        self.native_prompts(ui);
        self.desktop_tick(ui);
        self.poll_folder_pick();
        self.poll_eq_pick(ui);
        self.poll_import_pick(ui);
        for id in platform::take_media_ids() {
            self.b.auto_play(&id);
            self.follow = true;
        }
        for a in platform::take_outside_actions() {
            if a == "love" {
                self.b.toggle_love();
            }
        }
        platform::set_now_loved(self.b.loved_now);
        if let Some(found) = self.b.take_dlna_found() {
            self.open_dlna(ui, found);
        }
        self.sync_window_size(ui);
        let look = &self.b.config.appearance;
        ui.set_side_land(look.sidebar_landscape.index());
        ui.set_side_port(look.sidebar_portrait.index());
        let theme = look.theme;
        if theme == Theme::System
            && self
                .system_checked
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
        {
            self.system_checked = Some(Instant::now());
            let colors = platform::system_colors();
            if self.system_shown.as_ref() != Some(&colors) {
                let (pal, light) = crate::system_theme::palette(colors.as_ref());
                ui.global::<P>().set_system(pal);
                ui.global::<P>().set_system_light(light);
                self.system_shown = Some(colors);
            }
        }
        if self.theme != Some(theme) {
            self.theme = Some(theme);
            ui.global::<P>().set_theme(theme.index());
            // The sidebar's Japanese names and the album kicker change too.
            self.last_gen = u64::MAX;
        }
        self.sync_widget_colors(ui);
        self.other_devices_tick(ui);
        let art_arrived = self.art.collect();
        let qv = self.b.player.queue().version;
        let gen_changed = self.b.generation() != self.last_gen;
        let side_changed = self.b.side_generation() != self.last_side_gen;
        if gen_changed || qv != self.last_queue_ver {
            self.last_gen = self.b.generation();
            self.sync_view(ui);
        }
        self.loading_shown = self.b.loading();
        if gen_changed || side_changed || qv != self.last_queue_ver {
            self.last_side_gen = self.b.side_generation();
            self.sync_side(ui);
        }
        if qv != self.last_queue_ver || gen_changed {
            self.last_queue_ver = qv;
            self.sync_queue(ui);
        }
        if ui.get_np_open() != self.np_shown {
            self.sync_side(ui);
        }
        if std::mem::take(&mut self.marks_dirty) && !gen_changed {
            self.refresh_marks(ui);
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
        self.sync_grid(ui);
        self.sync_playback(ui);
        self.sync_backdrop(ui);
        self.poll_covers(ui);
        self.notes(ui);
        // Both side panels only fit on a wide window: shrinking the window
        // with both open folds the queue away.
        if ui.get_queue_panel() && ui.get_lyrics_open() && narrow_window(ui) {
            ui.set_queue_panel(false);
        }
        // The drawer only exists on a tiny window (with the sidebar not
        // set to stay folded).
        if ui.get_drawer_open() && !ui.get_drawer() {
            ui.set_drawer_open(false);
        }
    }

    /// The window's size tiers follow its real size. The UI copies its
    /// size over when it changes, which misses a window that opens at its
    /// final size and never changes (a phone, a tablet).
    fn sync_window_size(&self, ui: &AppWindow) {
        let w = ui.window();
        let size = w.size().to_logical(w.scale_factor());
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        if (ui.get_win_w() - size.width).abs() > 0.5 {
            ui.set_win_w(size.width);
        }
        if (ui.get_win_h() - size.height).abs() > 0.5 {
            ui.set_win_h(size.height);
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
            let (img, wait) = match (want, self.row_art.get(i).cloned().flatten()) {
                (true, Some(req)) => {
                    let img = self.art.get(req.clone(), THUMB);
                    let wait = img.is_none() && self.art.loading(&req, THUMB);
                    (img, wait)
                }
                _ => (None, false),
            };
            let Some(mut row) = self.rows.row_data(i) else {
                continue;
            };
            if row.has_art != img.is_some()
                || row.wait != wait
                || (img.is_some() && img.as_ref() != Some(&row.art))
            {
                row.has_art = img.is_some();
                row.wait = wait;
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
            let (img, wait) = match (want, self.queue_art.get(i).cloned().flatten()) {
                (true, Some(req)) => {
                    let img = self.art.get(req.clone(), THUMB);
                    let wait = img.is_none() && self.art.loading(&req, THUMB);
                    (img, wait)
                }
                _ => (None, false),
            };
            let Some(mut row) = self.queue.row_data(i) else {
                continue;
            };
            if row.has_art != img.is_some()
                || row.wait != wait
                || (img.is_some() && img.as_ref() != Some(&row.art))
            {
                row.has_art = img.is_some();
                row.wait = wait;
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
            ui.set_selected(i as i32);
            ui.invoke_scroll_to(i as i32);
        }
    }

    /// Select a row the pointer pressed. Only the highlight moves: the rows
    /// themselves aren't rebuilt, which on a long list took long enough to
    /// swallow the next click.
    fn select_row(&mut self, ui: &AppWindow, i: usize) -> bool {
        if self.b.select(i) {
            ui.set_selected(i as i32);
            true
        } else {
            false
        }
    }

    /// A row clicked or tapped: open it or play it.
    fn activate_row(&mut self, ui: &AppWindow, i: usize) {
        let now = Instant::now();
        if self
            .last_activate
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400))
        {
            return;
        }
        if self.select_row(ui, i) {
            self.last_activate = Some(now);
            if self.b.selected_setting().is_none() {
                self.activate_in(ui);
            }
        }
    }

    /// The search box's text changed.
    fn search_typed(&mut self, ui: &AppWindow, t: &str) {
        if ui.get_search_text().as_str() != t {
            ui.set_search_text(ss(t));
        }
        if t.trim().is_empty() {
            if self.b.in_search() {
                self.b.set_search("");
            }
        } else {
            if !self.b.in_search() {
                self.b.start_search();
            }
            self.b.set_search(t);
        }
    }

    /// Search dropped: the box emptied and the results closed.
    fn search_cancel(&mut self, ui: &AppWindow) {
        ui.set_search_text(SharedString::new());
        if self.b.in_search() {
            self.b.pop();
        }
    }

    /// Android: search in the system's own text box, the results updating
    /// below it as you type.
    fn open_search_prompt(&mut self, ui: &AppWindow) {
        let Some(h) = platform::hooks() else { return };
        if self.search_prompt.is_some() {
            return;
        }
        self.prompt_seq += 1;
        let id = self.prompt_seq;
        let p = Prompt {
            title: "Search".into(),
            fields: vec![PromptField {
                label: "Artists, albums, songs".into(),
                value: ui.get_search_text().to_string(),
                secret: false,
            }],
            ok: "Done".into(),
            live: true,
            light: ui.global::<P>().get_light(),
            ..Default::default()
        };
        if h.prompt(id, &p) {
            self.search_prompt = Some(id);
        }
    }

    /// Android: answers from the system's text dialogs, and the window's
    /// text dialog shown as one of them whenever it opens.
    fn native_prompts(&mut self, ui: &AppWindow) {
        if !ui.get_native_text() {
            return;
        }
        let Some(h) = platform::hooks() else { return };
        for (id, ev) in platform::take_prompt_events() {
            if Some(id) == self.search_prompt {
                match ev {
                    PromptEvent::Edited { text, .. } => self.search_typed(ui, &text),
                    PromptEvent::Done(Some(v)) => {
                        self.search_prompt = None;
                        let t = v.into_iter().next().unwrap_or_default();
                        self.search_typed(ui, &t);
                    }
                    PromptEvent::Done(None) => {
                        self.search_prompt = None;
                        self.search_cancel(ui);
                    }
                }
                continue;
            }
            if Some(id) != self.prompt_open {
                continue;
            }
            match ev {
                PromptEvent::Edited { .. } => {}
                PromptEvent::Done(Some(v)) => {
                    self.prompt_open = None;
                    let mut v = v.into_iter();
                    ui.set_dlg_text1(ss(v.next().unwrap_or_default()));
                    ui.set_dlg_text2(ss(v.next().unwrap_or_default()));
                    ui.set_dlg_text3(ss(v.next().unwrap_or_default()));
                    ui.set_dlg_text4(ss(v.next().unwrap_or_default()));
                    if ui.get_dialog() == 1 {
                        self.dlg_accept(ui);
                        // Still open: what was typed didn't take.
                        self.prompt_retry = ui.get_dialog() == 1;
                    }
                }
                PromptEvent::Done(None) => {
                    self.prompt_open = None;
                    if ui.get_dialog() == 1 {
                        self.dlg = Dlg::None;
                        ui.set_dialog(0);
                        ui.invoke_focus_root();
                    }
                }
            }
        }
        if ui.get_dialog() != 1 || self.prompt_open.is_some() {
            return;
        }
        // Whatever an answer just set off is on the toast before the next
        // dialog reads it.
        self.notes(ui);
        let mut fields = vec![PromptField {
            label: ui.get_dlg_label1().to_string(),
            value: ui.get_dlg_text1().to_string(),
            secret: false,
        }];
        if ui.get_dlg_two_fields() || ui.get_dlg_four_fields() {
            fields.push(PromptField {
                label: ui.get_dlg_label2().to_string(),
                value: ui.get_dlg_text2().to_string(),
                secret: false,
            });
        }
        if ui.get_dlg_four_fields() {
            fields.push(PromptField {
                label: ui.get_dlg_label3().to_string(),
                value: ui.get_dlg_text3().to_string(),
                secret: false,
            });
            fields.push(PromptField {
                label: ui.get_dlg_label4().to_string(),
                value: ui.get_dlg_text4().to_string(),
                secret: true,
            });
        }
        let message = if std::mem::take(&mut self.prompt_retry) && ui.get_toast_error() {
            ui.get_toast().to_string()
        } else {
            ui.get_dlg_note().to_string()
        };
        self.prompt_seq += 1;
        let id = self.prompt_seq;
        let p = Prompt {
            title: ui.get_dlg_title().to_string(),
            message,
            fields,
            ok: ui.get_dlg_ok().to_string(),
            live: false,
            light: ui.global::<P>().get_light(),
        };
        if h.prompt(id, &p) {
            self.prompt_open = Some(id);
        } else {
            // The system couldn't show it: the window's own dialog does.
            ui.set_native_text(false);
        }
    }

    /// Enter or a click. Clearing the cache asks first.
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

    /// The Mix button at the top of a genre, artist or album page.
    fn mix_view(&mut self) {
        let lib = self.b.lib.as_ref();
        let item = match self.b.level().map(|l| l.source.clone()) {
            // A genre mix only needs the genre's name.
            Some(Source::Genre { norm, name }) => {
                Some(Item::Genre(patina_core::library::GenreRow {
                    norm,
                    name,
                    albums: 0,
                    tracks: 0,
                }))
            }
            Some(Source::Album { key }) => lib
                .and_then(|l| l.album(&key).ok().flatten())
                .map(Item::Album),
            Some(Source::ArtistAlbums(_)) => self.b.view_item(),
            _ => None,
        };
        if let Some(it) = item {
            self.b.mix_item(&it);
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
            "eq-toggle" => self.b.eq_toggle(),
            "eq-flat" => self.b.eq_flat(),
            "eq-import" => self.pick_eq_file(),
            "eq-save" => self.open_eq_save(ui),
            "amp-toggle" => self.b.amp_toggle(),
            "amp-save" => self.open_amp_save(ui),
            "pin" => self.b.toggle_pin_view(),
            "save-mix" => self.b.save_mix(),
            "genre-mix" => self.b.play_genre_mix(),
            "clear-marks" => self.b.clear_genre_marks(),
            "mix" => self.mix_view(),
            "new-smart" => self.open_smart(ui, None),
            "smart-edit" => {
                let t = self.b.smart_target();
                if let Some(p) = t {
                    self.open_smart(ui, Some(p));
                }
            }
            "import" => self.import_playlist(),
            "export" => self.open_export(ui, false),
            "queue-export" => self.open_export(ui, true),
            "ipod" => self.b.send_view_to_ipod(),
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

    // ----- The visualizer -----

    /// Every frame (30 a second): while Now Playing shows a visualizer,
    /// read what's going out and draw it. Otherwise make sure the renderer
    /// isn't copying samples for nobody.
    fn vis_tick(&mut self, ui: &AppWindow) {
        let style = self.b.config.appearance.visualizer;
        ui.set_vis_style(style.index());
        let scope = self.b.player.scope();
        let showing = style != patina_core::config::Visualizer::Off
            && ui.get_np_open()
            && self.remote.is_none()
            && self.snap.track.is_some();
        if scope.is_on() != showing {
            scope.set_on(showing);
        }
        if !showing {
            return;
        }
        let f = self.vis.frame(scope, self.snap.state == PlayState::Playing);
        if ui.get_vis_bars().row_count() != f.bars.len() {
            ui.set_vis_bars(ModelRc::from(self.vis_bars.clone()));
        }
        self.vis_bars.set_vec(f.bars.clone());
        ui.set_vis_wave(ss(wave_path(&f.wave)));
        ui.set_vis_ring(ss(ring_path(&f.bars)));
    }

    // ----- Other devices -----

    /// Play on: this device or one on the network. Looks again each time,
    /// listing what was found before until the new answer's in.
    fn open_cast(&mut self, ui: &AppWindow) {
        self.b.look_for_cast_targets();
        ui.set_dlg_title(ss("Play on"));
        ui.set_dlg_new_first(false);
        self.dlg = Dlg::Cast(Vec::new());
        self.cast_shown = u64::MAX;
        self.fill_cast();
        ui.set_dialog(2);
    }

    fn fill_cast(&mut self) {
        if !matches!(self.dlg, Dlg::Cast(_)) || self.cast_shown == self.b.cast_generation {
            return;
        }
        self.cast_shown = self.b.cast_generation;
        let on = self.b.casting();
        let mut rows = vec![CastRow::Here];
        rows.extend(self.b.cast_targets().into_iter().map(CastRow::To));
        if self.b.cast_looking() {
            rows.push(CastRow::Looking);
        }
        rows.push(CastRow::Snapcast);
        let items = rows.iter().map(|r| {
            ss(match r {
                CastRow::Here if on.is_none() => "This device   (playing)".to_string(),
                CastRow::Here => "This device".to_string(),
                CastRow::To(t) if on.as_ref() == Some(t) => format!("{}   (playing)", t.label()),
                CastRow::To(t) => t.label(),
                CastRow::Looking => "Looking for devices…".to_string(),
                CastRow::Snapcast => "A Snapcast server, by address…".to_string(),
            })
        });
        set_strings(&self.dlg_items, items);
        self.dlg = Dlg::Cast(rows);
    }

    fn cast_pick(&mut self, ui: &AppWindow, row: CastRow) {
        match row {
            CastRow::Here => self.b.cast_to(None),
            CastRow::To(t) => self.b.cast_to(Some(t)),
            // Still looking: leave the list up.
            CastRow::Looking => {
                self.dlg = Dlg::Cast(Vec::new());
                self.cast_shown = u64::MAX;
                self.fill_cast();
                return;
            }
            CastRow::Snapcast => {
                self.dlg = Dlg::CastSnapcast;
                let now = self.b.config.cast.snapcast.clone().unwrap_or_default();
                open_text(
                    ui,
                    "A Snapcast server",
                    "Address: host or host:port (its TCP source, 4953 unless you changed it)",
                    &now,
                    None,
                    "Play there",
                );
                return;
            }
        }
        ui.set_dialog(0);
        ui.invoke_focus_root();
    }

    /// Control another player: the ones that answer on the network.
    fn open_players(&mut self, ui: &AppWindow) {
        self.b.look_for_players();
        ui.set_dlg_title(ss("Control another player"));
        ui.set_dlg_new_first(false);
        self.dlg = Dlg::Players(Vec::new());
        self.players_shown = u64::MAX;
        self.fill_players();
        ui.set_dialog(2);
    }

    fn fill_players(&mut self) {
        if !matches!(self.dlg, Dlg::Players(_)) || self.players_shown == self.b.control_generation {
            return;
        }
        self.players_shown = self.b.control_generation;
        let mut rows: Vec<PlayerRow> = self
            .b
            .players_found()
            .into_iter()
            .map(PlayerRow::Found)
            .collect();
        if self.b.players_looking() {
            rows.push(PlayerRow::Looking);
        }
        rows.push(PlayerRow::Type);
        let items = rows.iter().map(|r| {
            ss(match r {
                PlayerRow::Found(f) => format!("{}  ·  {}", f.name, f.addr),
                PlayerRow::Looking => "Looking for players…".to_string(),
                PlayerRow::Type => "By address…".to_string(),
            })
        });
        set_strings(&self.dlg_items, items);
        self.dlg = Dlg::Players(rows);
    }

    fn player_pick(&mut self, ui: &AppWindow, row: PlayerRow) {
        match row {
            PlayerRow::Found(f) => match self.b.player_code(&f.addr) {
                Some(code) => {
                    ui.set_dialog(0);
                    self.connect_player(&f.addr, &code, &f.name);
                }
                None => {
                    self.dlg = Dlg::PlayerCode(f.addr.clone(), f.name.clone());
                    open_text(
                        ui,
                        &format!("Control {}", f.name),
                        "Its code (Settings, Other devices, on that one)",
                        "",
                        None,
                        "Connect",
                    );
                }
            },
            PlayerRow::Looking => {
                self.dlg = Dlg::Players(Vec::new());
                self.players_shown = u64::MAX;
                self.fill_players();
            }
            PlayerRow::Type => {
                self.dlg = Dlg::PlayerAddr;
                let last = self.b.config.remote.control.clone().unwrap_or_default();
                open_text(
                    ui,
                    "Control another player",
                    "Address: host or host:port",
                    &last,
                    Some(("Its code", "")),
                    "Connect",
                );
            }
        }
    }

    fn connect_player(&mut self, addr: &str, code: &str, name: &str) {
        match patina_core::control::Client::new(addr, code) {
            Ok(c) => {
                self.remote = Some(crate::remote_view::RemoteView::start(
                    c,
                    name.trim_end_matches(" (last time)").to_string(),
                    code.to_string(),
                ));
            }
            Err(e) => self.b.error(format!("Can't control {addr}: {e}")),
        }
    }

    /// Every tick: pickers whose search finished, and the player being
    /// steered.
    fn other_devices_tick(&mut self, ui: &AppWindow) {
        self.fill_cast();
        self.fill_players();
        let Some(r) = &mut self.remote else {
            ui.set_rm_open(false);
            return;
        };
        for h in r.poll() {
            match h {
                crate::remote_view::Heard::Connected => {
                    let (addr, code, name) = (r.addr.clone(), r.code.clone(), r.name.clone());
                    self.b.remember_player(&addr, &code);
                    self.b.note(format!("Controlling {name}"));
                }
                crate::remote_view::Heard::Error(e) if e == "wrong code" => {
                    let (addr, name) = (r.addr.clone(), r.name.clone());
                    self.remote = None;
                    ui.set_rm_open(false);
                    self.b.error(format!("{name} says that's the wrong code"));
                    self.dlg = Dlg::PlayerCode(addr, name.clone());
                    open_text(
                        ui,
                        &format!("Control {name}"),
                        "Its code (Settings, Other devices, on that one)",
                        "",
                        None,
                        "Connect",
                    );
                    return;
                }
                crate::remote_view::Heard::Error(e) => self.b.error(e),
            }
        }
        if let Some(r) = &self.remote {
            r.show(ui);
            ui.set_rm_open(true);
        }
    }

    fn remote_cmd(&mut self, name: &str) {
        use patina_core::control::Cmd;
        let c = match name {
            "toggle" => Cmd::Toggle,
            "next" => Cmd::Next,
            "prev" => Cmd::Prev,
            "shuffle" => Cmd::Shuffle,
            "repeat" => Cmd::Repeat,
            "mute" => Cmd::Mute,
            "love" => Cmd::Love,
            _ => return,
        };
        if let Some(r) = &mut self.remote {
            r.send(c);
        }
    }

    fn open_remote_queue(&mut self, ui: &AppWindow) {
        let Some(r) = &self.remote else { return };
        let q = &r.queue;
        let items = q.items.iter().map(|i| {
            let mark = if Some(i.id) == q.current { "▶  " } else { "" };
            ss(if i.artist.is_empty() {
                format!("{mark}{}", i.title)
            } else {
                format!("{mark}{}  ·  {}", i.title, i.artist)
            })
        });
        set_strings(&self.dlg_items, items);
        ui.set_dlg_title(ss(format!("Queue on {}", r.name)));
        ui.set_dlg_new_first(false);
        self.dlg = Dlg::RemoteQueue(q.items.iter().map(|i| i.id).collect());
        ui.set_dialog(2);
    }

    /// Where the sound goes, from the output chip: the devices there are
    /// now, the current one marked.
    fn pick_device(&mut self, ui: &AppWindow) {
        let Some(st) = self.b.device_setting() else {
            return;
        };
        if let Some(why) = st.disabled {
            return self.b.note(why);
        }
        if let Control::Choice { options, index } = &st.control {
            let items = options
                .iter()
                .enumerate()
                .map(|(i, (_, l))| {
                    ss(if i == *index && self.snap.cast.is_none() {
                        format!("{l}   (current)")
                    } else {
                        l.clone()
                    })
                })
                .chain(std::iter::once(ss(
                    "Another device: Chromecast, DLNA, Snapcast…",
                )));
            set_strings(&self.dlg_items, items);
            ui.set_dlg_title(ss("Output device"));
            ui.set_dlg_new_first(false);
            ui.set_dialog(2);
            self.dlg = Dlg::Choose(st.key.clone());
        }
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
            Control::Action(_) if st.key == SettingKey::MeasureLibrary => "measure",
            Control::Action(_) if st.key.is_eq() || st.key.is_amp() => "measure",
            Control::Action(_) if st.key == SettingKey::LastFmLink => "measure",
            Control::Action(_)
                if matches!(st.key, SettingKey::RemoteCode | SettingKey::ControlAnother) =>
            {
                "measure"
            }
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
        if let Some(why) = st.disabled {
            return self.b.note(why);
        }
        let key = st.key.clone();
        if key == SettingKey::MeasureLibrary {
            return self.b.toggle_measure_library();
        }
        if key == SettingKey::LastFmLink {
            return self.b.lastfm_link();
        }
        if key == SettingKey::RemoteCode {
            return self.b.new_remote_code();
        }
        if key == SettingKey::ControlAnother {
            return self.open_players(ui);
        }
        match key {
            SettingKey::EqSavePreset => return self.open_eq_save(ui),
            SettingKey::EqImport => return self.pick_eq_file(),
            SettingKey::EqDevice | SettingKey::EqAddBand | SettingKey::EqDeletePreset => {
                return self.b.change_setting(&key, Change::Toggle);
            }
            SettingKey::AmpSavePreset => return self.open_amp_save(ui),
            SettingKey::AmpDevice
            | SettingKey::AmpDeletePreset
            | SettingKey::AmpMoveUp(_)
            | SettingKey::AmpMoveDown(_)
            | SettingKey::AmpRemove(_) => {
                return self.b.change_setting(&key, Change::Toggle);
            }
            _ => {}
        }
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
                    // A secret is typed fresh, never shown.
                    Control::Text { .. } if key == SettingKey::LastFmSecret => String::new(),
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
            let _ = tx.send(match platform::hooks() {
                Some(h) => h.pick_folder(),
                None => pick_folder_dialog(),
            });
        });
        self.folder_pick = Some((rx, Instant::now()));
    }

    /// A band handle dragged on the EQ panel: `x` is 0 to 1 across the
    /// graph (20 Hz to 20 kHz), `gain` in dB.
    fn eq_point(&mut self, i: usize, x: f64, gain: f64) {
        // Close to 0 dB snaps to it, so flat is easy to find again.
        let gain = if gain.abs() < 0.25 {
            0.0
        } else {
            (gain * 10.0).round() / 10.0
        };
        let s = self.b.eq_settings();
        match s.mode {
            EqMode::Graphic => self.b.eq_set_gain(i, gain),
            EqMode::Parametric => {
                if let Some(mut b) = s.parametric.get(i).copied() {
                    b.freq = (20.0 * 1000f64.powf(x.clamp(0.0, 1.0))).round();
                    if b.kind.has_gain() {
                        b.gain = gain;
                    }
                    self.b.eq_set_band(i, b);
                }
            }
        }
    }

    /// The EQ panel: curve, band handles, the line above them.
    fn sync_eq_panel(&self, ui: &AppWindow) {
        let eq = self.b.eq_settings();
        ui.set_eq_lit(eq.on && !eq.is_flat());
        ui.set_love_state(match self.b.loved_now {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        });
        let show = matches!(
            self.b.level().map(|l| &l.source),
            Some(Source::Section(Section::Eq))
        );
        ui.set_eq_panel(show);
        if !show {
            return;
        }
        let eq = self.b.eq_settings();
        let bands = eq.bands();
        let curve = eq.curve(240);
        let range = match eq.mode {
            EqMode::Graphic => 12.0,
            EqMode::Parametric => {
                let m = curve
                    .iter()
                    .map(|p| p.1.abs())
                    .chain(
                        bands
                            .iter()
                            .filter(|b| b.kind.has_gain())
                            .map(|b| b.gain.abs()),
                    )
                    .fold(12.0f64, f64::max);
                (m / 6.0).ceil() * 6.0
            }
        };
        let xf = |f: f64| ((f.max(20.0).log10() - 20f64.log10()) / 3.0).clamp(0.0, 1.0);
        let mut cmd = String::new();
        for (k, (f, db)) in curve.iter().enumerate() {
            let x = xf(*f) * 1000.0;
            let y = ((0.5 - db / (2.0 * range)) * 200.0).clamp(-5.0, 205.0);
            cmd.push_str(&format!("{}{x:.1} {y:.1} ", if k == 0 { "M" } else { "L" }));
        }
        ui.set_eq_curve(ss(cmd));
        ui.set_eq_range(range as f32);
        ui.set_eq_on(eq.on);
        ui.set_eq_graphic(eq.mode == EqMode::Graphic);
        let views: Vec<EqBand> = bands
            .iter()
            .map(|b| EqBand {
                x: xf(b.freq) as f32,
                gain: if b.kind.has_gain() {
                    b.gain as f32
                } else {
                    0.0
                },
                label: ss(patina_core::eq::freq_label(b.freq)),
                has_gain: b.kind.has_gain(),
            })
            .collect();
        if self.eq_bands.row_count() == views.len() {
            for (i, v) in views.into_iter().enumerate() {
                if self.eq_bands.row_data(i).as_ref() != Some(&v) {
                    self.eq_bands.set_row_data(i, v);
                }
            }
        } else {
            self.eq_bands.set_vec(views);
        }
        let mut info = format!(
            "{} · preamp {:+.1} dB",
            self.b.eq.preset_label(),
            eq.total_preamp()
        );
        if let Some(d) = &self.b.eq.device {
            info.push_str(&format!(" · {}", d.name));
            if self.b.eq.linked.is_some() {
                info.push_str(" (its own EQ)");
            }
        }
        ui.set_eq_info(ss(info));
    }

    /// The lyrics panel. Synced lyrics show from a couple of lines before
    /// the one playing, so it stays in view without scrolling.
    fn sync_lyrics(&mut self, ui: &AppWindow) {
        use patina_core::browse::marks::LyricsStatus;
        let stale = self.b.lyrics_stale();
        let pos = self.snap.position;
        let Some(v) = self.b.lyrics() else {
            ui.set_lyrics_open(false);
            return;
        };
        ui.set_lyrics_open(true);
        ui.set_lyrics_title(ss(&v.title));
        let (note, warn, lines, cur, synced): (String, bool, Vec<String>, i32, bool) = match &v
            .status
        {
            LyricsStatus::Looking => ("Looking…".into(), false, Vec::new(), -1, false),
            LyricsStatus::Nothing(why) => (why.clone(), false, Vec::new(), -1, false),
            LyricsStatus::Failed(e) => (format!("Couldn't look: {e}"), true, Vec::new(), -1, false),
            LyricsStatus::Found(l) => {
                let all: Vec<String> = l.lines.iter().map(|x| x.1.clone()).collect();
                let now = if stale { None } else { l.line_at(pos) };
                if l.synced {
                    let first = now.map(|c| c.saturating_sub(2)).unwrap_or(0);
                    let cur = now.map(|c| (c - first) as i32).unwrap_or(-1);
                    (
                        l.origin.label().into(),
                        false,
                        all[first.min(all.len())..].to_vec(),
                        cur,
                        true,
                    )
                } else {
                    (l.origin.label().into(), false, all, -1, false)
                }
            }
        };
        let note = if stale {
            "Another song is playing. Press Lyrics to look it up.".to_string()
        } else {
            note
        };
        ui.set_lyrics_note(ss(note));
        ui.set_lyrics_warn(warn || stale);
        ui.set_lyrics_synced(synced);
        if self.lyrics_shown.0 != lines || self.lyrics_shown.1 != cur {
            ui.set_lyrics_lines(ModelRc::from(Rc::new(VecModel::from(
                lines.iter().map(ss).collect::<Vec<_>>(),
            ))));
            ui.set_lyrics_current(cur);
            self.lyrics_shown = (lines, cur);
        }
    }

    /// The rack above the Amp view's rows, and the AMP badge.
    fn sync_amp_panel(&mut self, ui: &AppWindow) {
        let a = self.b.amp_settings().clone();
        ui.set_amp_lit(a.changes_sound());
        let show = matches!(
            self.b.level().map(|l| &l.source),
            Some(Source::Section(Section::Amp))
        );
        ui.set_amp_panel(show);
        if !show {
            return;
        }
        ui.set_amp_on(a.on);
        let knobs_of = |u: &patina_core::dsp::amp::Unit| -> Vec<AmpKnob> {
            u.kind
                .params()
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let pos = patina_core::browse::amp::knob_pos(s, u.p[i]);
                    AmpKnob {
                        label: ss(s.label),
                        text: ss(patina_core::browse::amp::knob_text(s, u.p[i])),
                        pos: pos as f32,
                        arc: ss(dial_arc(pos)),
                        choice: !s.choices.is_empty(),
                    }
                })
                .collect()
        };
        let kinds: Vec<AmpKind> = a.units.iter().map(|u| u.kind).collect();
        if kinds != self.amp_kinds {
            self.amp_knobs = a
                .units
                .iter()
                .map(|u| Rc::new(VecModel::from(knobs_of(u))))
                .collect();
            let rows: Vec<AmpUnit> = a
                .units
                .iter()
                .zip(&self.amp_knobs)
                .map(|(u, k)| AmpUnit {
                    name: ss(u.kind.label()),
                    on: u.on,
                    knobs: ModelRc::from(k.clone()),
                })
                .collect();
            self.amp_units.set_vec(rows);
            self.amp_kinds = kinds;
        } else {
            for (i, u) in a.units.iter().enumerate() {
                if let Some(mut row) = self.amp_units.row_data(i)
                    && row.on != u.on
                {
                    row.on = u.on;
                    self.amp_units.set_row_data(i, row);
                }
                let m = &self.amp_knobs[i];
                for (j, k) in knobs_of(u).into_iter().enumerate() {
                    if m.row_data(j).as_ref() != Some(&k) {
                        m.set_row_data(j, k);
                    }
                }
            }
        }
        let st = self.b.amp_state();
        let mut info = format!("{} · {}x oversampling", st.preset_label(), a.oversample);
        if let Some(d) = &st.device {
            info.push_str(&format!(" · {}", d.name));
            if st.linked.is_some() {
                info.push_str(" (its own stack)");
            }
        }
        ui.set_amp_info(ss(info));
    }

    /// A dial turned on the rack: its place 0 to 1, to the knob's value.
    fn amp_knob(&mut self, i: usize, j: usize, pos: f64) {
        let Some(u) = self.b.amp_settings().units.get(i).copied() else {
            return;
        };
        let Some(s) = u.kind.params().get(j) else {
            return;
        };
        let v = s.min + pos * (s.max - s.min);
        if (v - u.p[j]).abs() >= s.step / 2.0 {
            self.b.amp_set_param(u.kind, j, v);
        }
    }

    /// A choice on the rack clicked: the next one.
    fn amp_step(&mut self, i: usize, j: usize) {
        let Some(u) = self.b.amp_settings().units.get(i).copied() else {
            return;
        };
        let Some(s) = u.kind.params().get(j) else {
            return;
        };
        let n = s.choices.len().max(1) as f64;
        self.b.amp_set_param(u.kind, j, (u.p[j] + 1.0) % n);
    }

    /// Lyrics: up for the song playing, or closed if they already are.
    fn toggle_lyrics(&mut self, ui: &AppWindow) {
        if self.b.lyrics().is_some() && !self.b.lyrics_stale() {
            self.b.lyrics_hide();
        } else {
            self.b.lyrics_show();
            // No room for both panels on a narrow window.
            if narrow_window(ui) {
                ui.set_queue_panel(false);
            }
        }
    }

    /// MPRIS and the tray: publish the state, and do what they asked.
    fn desktop_tick(&mut self, ui: &AppWindow) {
        let d = &self.b.config.desktop;
        self.desktop.set_tray(d.tray);
        self.hide_on_close
            .set(d.close_to_tray && self.desktop.has_tray());
        for a in self.desktop.tick(&self.snap) {
            match a {
                Action::Show => show_window(ui),
                Action::Toggle => {
                    if ui.window().is_visible() {
                        let _ = ui.window().hide();
                    } else {
                        show_window(ui);
                    }
                }
                Action::Quit => {
                    let _ = slint::quit_event_loop();
                }
            }
        }
    }

    fn toggle_queue_panel(&mut self, ui: &AppWindow) {
        let open = !ui.get_queue_panel();
        if open && narrow_window(ui) && self.b.lyrics().is_some() {
            self.b.lyrics_hide();
        }
        ui.set_queue_panel(open);
    }

    fn open_amp_save(&mut self, ui: &AppWindow) {
        let cur = self.b.amp_settings().preset.clone();
        let name = if patina_core::amp::is_builtin(&cur) {
            String::new()
        } else {
            cur
        };
        self.dlg = Dlg::AmpSave;
        open_text(
            ui,
            "Save the amp stack as a preset",
            "Name",
            &name,
            None,
            "Save",
        );
    }

    fn open_eq_save(&mut self, ui: &AppWindow) {
        let cur = self.b.eq_settings().preset.clone();
        let name = if patina_core::eq::presets::is_builtin(&cur) {
            String::new()
        } else {
            cur
        };
        self.dlg = Dlg::EqSave;
        open_text(ui, "Save the EQ as a preset", "Name", &name, None, "Save");
    }

    fn pick_eq_file(&mut self) {
        if self.eq_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(match platform::hooks() {
                Some(h) => h.pick_eq_file(),
                None => pick_eq_dialog(),
            });
        });
        self.eq_pick = Some((rx, Instant::now()));
    }

    fn poll_eq_pick(&mut self, ui: &AppWindow) {
        let Some((rx, opened)) = &self.eq_pick else {
            return;
        };
        let Ok(picked) = rx.try_recv() else { return };
        // Back that fast from the desktop's picker means none showed up.
        let quick = opened.elapsed() < Duration::from_millis(700) && platform::hooks().is_none();
        self.eq_pick = None;
        match picked {
            Picked::Path(p) => self.b.eq_import(&p),
            Picked::Unavailable(why) => {
                self.b.note(why);
                self.dlg = Dlg::EqImportPath;
                open_text(
                    ui,
                    "Import an AutoEq or APO file",
                    "Path to ParametricEQ.txt or GraphicEQ.txt",
                    "~/",
                    None,
                    "Import",
                );
            }
            // No picker showed up: type the path.
            Picked::Cancelled if quick => {
                self.dlg = Dlg::EqImportPath;
                open_text(
                    ui,
                    "Import an AutoEq or APO file",
                    "Path to ParametricEQ.txt or GraphicEQ.txt",
                    "~/",
                    None,
                    "Import",
                );
            }
            Picked::Cancelled => {}
        }
    }

    fn poll_folder_pick(&mut self) {
        let Some((rx, opened)) = &self.folder_pick else {
            return;
        };
        let Ok(picked) = rx.try_recv() else { return };
        let quick = opened.elapsed() < Duration::from_millis(700) && platform::hooks().is_none();
        self.folder_pick = None;
        match picked {
            Picked::Path(p) => {
                self.b.add_folder(&p.to_string_lossy());
            }
            Picked::Unavailable(why) => self.b.error(why),
            // Back that fast means no dialog ever showed up.
            Picked::Cancelled if quick => self
                .b
                .note("No folder picker is available here. Use \"Type a path\" instead."),
            Picked::Cancelled => {}
        }
    }

    // ----- The album grid -----

    /// Hand the grid the tiles near the screen: two rows of them either
    /// side of what shows. Updated in place, so scrolling reuses tiles.
    fn sync_grid(&mut self, ui: &AppWindow) {
        if ui.get_layout() != 1 {
            if self.grid_rows.row_count() > 0 {
                self.grid_rows.set_vec(Vec::new());
            }
            return;
        }
        let n = self.rows.row_count();
        let cols = ui.get_grid_cols().max(1);
        let r = window(ui.get_grid_first(), ui.get_grid_count(), cols * 2, n);
        // Start on a row's first tile, so the base keeps tiles in columns.
        let start = r.start - r.start % cols as usize;
        let want = r.end.saturating_sub(start);
        if start != self.grid_base {
            self.grid_base = start;
            ui.set_grid_base(start as i32);
        }
        while self.grid_rows.row_count() > want {
            self.grid_rows.remove(self.grid_rows.row_count() - 1);
        }
        for j in 0..want {
            let Some(row) = self.rows.row_data(start + j) else {
                break;
            };
            if j < self.grid_rows.row_count() {
                if self.grid_rows.row_data(j).as_ref() != Some(&row) {
                    self.grid_rows.set_row_data(j, row);
                }
            } else {
                self.grid_rows.push(row);
            }
        }
    }

    // ----- The cover backdrop -----

    /// The playing song's cover, blurred, for the backdrop, made again
    /// when the song or its cover changes.
    fn sync_backdrop(&mut self, ui: &AppWindow) {
        let look = &self.b.config.appearance;
        ui.set_minimal(look.minimal);
        ui.set_backdrop(look.backdrop.index());
        ui.set_backdrop_strength(f32::from(look.backdrop_strength) / 100.0);
        let want_blur = look.backdrop != patina_core::config::Backdrop::Off;
        let want_accent = look.cover_accent;
        let bg = ui.global::<P>().get_bg();
        let flags = (
            want_blur,
            want_accent,
            bg.as_argb_encoded(),
            look.backdrop_strength,
        );
        let everywhere = look.backdrop == patina_core::config::Backdrop::Everywhere;
        ui.global::<P>()
            .set_wash(everywhere && ui.get_np_has_blur());
        let has = ui.get_np_has_art();
        let key = self.playing_path().map(|p| (p, has));
        if key == self.blur_key && flags == self.look_flags {
            return;
        }
        self.blur_key = key;
        self.look_flags = flags;
        let art = has.then(|| ui.get_np_art());
        let img = art.as_ref().filter(|_| want_blur).and_then(blurred);
        let veil = img.as_ref().map_or(0.0, |i| {
            wash_veil(
                i,
                f32::from(look.backdrop_strength) / 100.0,
                bg,
                ui.global::<P>().get_muted(),
            )
        });
        ui.global::<P>().set_wash_veil(veil);
        ui.set_np_has_blur(img.is_some());
        ui.set_np_blur(img.unwrap_or_default());
        ui.global::<P>()
            .set_wash(everywhere && ui.get_np_has_blur());
        let tint = ui.global::<Tint>();
        match art
            .as_ref()
            .filter(|_| want_accent)
            .and_then(|a| accent_of(a, luminance(bg) < 0.5))
        {
            Some((c, on)) => {
                tint.set_c(c);
                tint.set_on_c(on);
                tint.set_on(true);
            }
            None => tint.set_on(false),
        }
    }

    // ----- Choosing a cover -----

    /// A menu's cover action, for the album it was picked on.
    fn cover_action(&mut self, ui: &AppWindow, a: &str, target: Option<CoverTarget>) {
        let Some(t) = target else {
            return self.b.note("That has no album to find a cover for");
        };
        match a {
            "cover-find" => self.open_cover_find(ui, t),
            "cover-file" => self.pick_cover_file(t),
            _ => {}
        }
    }

    fn cover_channel(&mut self) -> crossbeam_channel::Sender<CoverMsg> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.covers.rx = Some(rx);
        tx
    }

    /// Look online for covers, and show them as they come in.
    fn open_cover_find(&mut self, ui: &AppWindow, t: CoverTarget) {
        self.covers.seq += 1;
        let seq = self.covers.seq;
        self.covers.found.clear();
        self.covers_model.set_vec(Vec::new());
        let what = match (t.album.is_empty(), t.artist.is_empty()) {
            (false, false) => format!("\"{}\" by {}", t.album, t.artist),
            (false, true) => format!("\"{}\"", t.album),
            _ => t.label.clone(),
        };
        ui.set_covers_note(ss(format!(
            "Looking for {what} on iTunes, Deezer and MusicBrainz…"
        )));
        ui.set_dlg_title(ss(format!("Cover for {}", t.label)));
        let (artist, album) = (t.artist.clone(), t.album.clone());
        self.covers.target = Some(t);
        self.dlg = Dlg::Covers;
        ui.set_dialog(5);
        let tx = self.cover_channel();
        std::thread::Builder::new()
            .name("patina-covers".into())
            .spawn(move || {
                let found = patina_core::cover::search(&artist, &album);
                let thumbs: Vec<String> = found.iter().map(|f| f.thumb.clone()).collect();
                if tx.send(CoverMsg::Found(seq, found)).is_err() {
                    return;
                }
                // A few at a time: quick, without hammering one server.
                let next = std::sync::atomic::AtomicUsize::new(0);
                std::thread::scope(|sc| {
                    for _ in 0..4 {
                        sc.spawn(|| {
                            loop {
                                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let Some(url) = thumbs.get(i) else { return };
                                let img = patina_core::cover::fetch(url)
                                    .ok()
                                    .and_then(|b| patina_core::cover::thumbnail(&b, THUMB));
                                if tx.send(CoverMsg::Thumb(seq, i, img)).is_err() {
                                    return;
                                }
                            }
                        });
                    }
                });
                let _ = tx.send(CoverMsg::Done(seq));
            })
            .ok();
    }

    /// One of the covers found was picked: fetch it whole and save it.
    fn cover_pick(&mut self, ui: &AppWindow, i: usize) {
        let (Some(t), Some(f)) = (
            self.covers.target.clone(),
            self.covers.found.get(i).cloned(),
        ) else {
            return;
        };
        self.dlg = Dlg::None;
        ui.set_dialog(0);
        self.b.note(format!("Saving the cover from {}…", f.source));
        let tx = self.cover_channel();
        std::thread::spawn(move || {
            // The big one, or the small one if the big one won't come.
            let bytes =
                patina_core::cover::fetch(&f.full).or_else(|_| patina_core::cover::fetch(&f.thumb));
            let r = bytes
                .map_err(|e| format!("Couldn't download that cover: {e}"))
                .and_then(|b| patina_core::cover::save(&t.dir, &b))
                .map(|s| (s, t.dir.clone()));
            let _ = tx.send(CoverMsg::Saved(r));
        });
    }

    /// Pick an image file for an album's cover.
    fn pick_cover_file(&mut self, t: CoverTarget) {
        if self.cover_file_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(match platform::hooks() {
                Some(h) => h.pick_image(),
                None => pick_image_dialog(),
            });
        });
        self.cover_file_pick = Some((rx, t));
    }

    fn poll_covers(&mut self, ui: &AppWindow) {
        if let Some((rx, _)) = &self.cover_file_pick
            && let Ok(picked) = rx.try_recv()
        {
            let (_, t) = self.cover_file_pick.take().unwrap();
            match picked {
                Picked::Path(p) => {
                    match patina_core::cover::save_file(&t.dir, &p) {
                        Ok(saved) => self.cover_done(ui, &saved, &t.dir),
                        Err(e) => self.b.error(e),
                    }
                    // Android hands over a copy in the app's cache.
                    if cfg!(target_os = "android") {
                        let _ = std::fs::remove_file(&p);
                    }
                }
                Picked::Unavailable(why) => self.b.error(why),
                Picked::Cancelled => {}
            }
        }
        let Some(rx) = &self.covers.rx else { return };
        let msgs: Vec<CoverMsg> = rx.try_iter().collect();
        for m in msgs {
            match m {
                CoverMsg::Found(seq, found) if seq == self.covers.seq => {
                    self.covers_model.set_vec(
                        found
                            .iter()
                            .map(|f| CoverPick {
                                title: ss(&f.title),
                                source: ss(f.source),
                                ..Default::default()
                            })
                            .collect::<Vec<_>>(),
                    );
                    ui.set_covers_note(ss(if found.is_empty() {
                        "Nothing found. Use an image file instead, or fix the album's tags and look again."
                            .to_string()
                    } else {
                        "Loading the pictures… Tap one to use it.".to_string()
                    }));
                    self.covers.found = found;
                }
                CoverMsg::Thumb(seq, i, img) if seq == self.covers.seq => {
                    if let (Some(img), Some(mut row)) = (img, self.covers_model.row_data(i)) {
                        row.art = slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                            slint::Rgba8Pixel,
                        >::clone_from_slice(
                            img.as_raw(),
                            img.width(),
                            img.height(),
                        ));
                        row.has_art = true;
                        self.covers_model.set_row_data(i, row);
                    }
                }
                CoverMsg::Done(seq) if seq == self.covers.seq => {
                    // Those with no picture after all (MusicBrainz knows the
                    // album, but nobody gave it a cover) aren't worth showing.
                    let keep: Vec<usize> = (0..self.covers_model.row_count())
                        .filter(|&i| self.covers_model.row_data(i).is_some_and(|r| r.has_art))
                        .collect();
                    let rows: Vec<CoverPick> = keep
                        .iter()
                        .filter_map(|&i| self.covers_model.row_data(i))
                        .collect();
                    self.covers.found = keep
                        .iter()
                        .filter_map(|&i| self.covers.found.get(i).cloned())
                        .collect();
                    self.covers_model.set_vec(rows);
                    ui.set_covers_note(ss(match self.covers.found.len() {
                        0 => "None of what was found has a picture. Use an image file instead."
                            .to_string(),
                        n => format!(
                            "{n} found. Tap one to use it; it's saved as album_art.jpg in the album's folder."
                        ),
                    }));
                }
                CoverMsg::Saved(Ok((saved, dir))) => self.cover_done(ui, &saved, &dir),
                CoverMsg::Saved(Err(e)) => self.b.error(e),
                _ => {}
            }
        }
    }

    /// A cover is saved: show it everywhere it shows.
    fn cover_done(
        &mut self,
        ui: &AppWindow,
        saved: &patina_core::cover::Saved,
        dir: &std::path::Path,
    ) {
        self.art.forget_under(dir);
        self.blur_key = None;
        // The album header and any list kept for later show it too.
        self.rows_sig = None;
        self.rows_cache.clear();
        // The rows draw again with it.
        self.row_shown.clear();
        self.queue_shown.clear();
        self.b.cover_saved(saved);
        if matches!(self.dlg, Dlg::Covers) {
            self.dlg = Dlg::None;
            ui.set_dialog(0);
        }
    }

    /// Now Playing's own menu.
    fn np_act(&mut self, ui: &AppWindow, a: &str) {
        match a {
            "cover-find" | "cover-file" => {
                let t = self.b.playing_cover_target();
                self.cover_action(ui, a, t);
            }
            "go-album" | "go-artist" => {
                let Some(t) = self.snap.track.clone() else {
                    return;
                };
                let row = self
                    .b
                    .lib
                    .as_ref()
                    .and_then(|l| l.track_by_path(&t.path).ok().flatten());
                match row {
                    Some(r) => {
                        self.b.go_to_track(&r, a == "go-artist");
                        ui.set_np_open(false);
                        self.after_go_to(ui);
                    }
                    None => self.b.note("That song isn't in the library"),
                }
            }
            "chain" => {
                let np = self.b.now_playing(&self.snap);
                let mut text = String::new();
                if !np.format.is_empty() {
                    text.push_str(&format!("{}\n", np.format));
                }
                if !np.output.is_empty() {
                    text.push_str(&format!("Output: {}\n", np.output));
                }
                if !np.path.is_empty() {
                    text.push_str(&format!("{}\n", np.path));
                }
                if !np.chain.is_empty() {
                    text.push('\n');
                    for (name, state) in &np.chain {
                        text.push_str(&format!("{name:<14} {state}\n"));
                    }
                }
                self.dlg = Dlg::Help;
                ui.set_dlg_title(ss("Sound chain"));
                ui.set_dlg_message(ss(text.trim_end()));
                ui.set_dialog(4);
            }
            _ => {}
        }
    }

    /// The Now Playing page, while it's open.
    fn sync_now_playing(&mut self, ui: &AppWindow) {
        let np = self.b.now_playing(&self.snap);
        ui.set_np_artist(ss(&np.artist));
        ui.set_np_album(ss(&np.album));
        ui.set_np_format(ss(&np.format));
        ui.set_np_path(ss(&np.path));
        ui.set_np_output(ss(&np.output));
        let rows: Vec<ChainRow> = np
            .chain
            .iter()
            .map(|(name, state)| ChainRow {
                name: ss(name),
                state: ss(state),
                on: !(state == "off"
                    || state.starts_with("skipped")
                    || state.contains("not touching")),
            })
            .collect();
        if self.np_chain.row_count() != rows.len() {
            self.np_chain.set_vec(rows);
        } else {
            for (i, r) in rows.into_iter().enumerate() {
                if self.np_chain.row_data(i).as_ref() != Some(&r) {
                    self.np_chain.set_row_data(i, r);
                }
            }
        }
    }

    /// A row picked or let go of, or a range picked to it.
    fn row_mark(&mut self, ui: &AppWindow, i: usize, range: bool) {
        if range {
            self.b.mark_range(i);
        } else {
            self.b.toggle_mark(i);
        }
        if let Some(sel) = self.selected_index() {
            ui.set_selected(sel as i32);
        }
        self.marks_dirty = true;
    }

    /// Redraw which rows are picked, touching only the ones that changed.
    fn refresh_marks(&mut self, ui: &AppWindow) {
        let Some(l) = self.b.level() else { return };
        for i in 0..self.rows.row_count() {
            let want = l.is_marked(i);
            if let Some(mut r) = self.rows.row_data(i)
                && r.marked != want
            {
                r.marked = want;
                self.rows.set_row_data(i, r);
            }
        }
        let n = self.b.marked_count();
        ui.set_marked_count(n as i32);
        if n == 0 && !is_touch(ui) {
            self.marking = false;
        }
        ui.set_marking(self.marking);
    }

    /// A button on the picked-rows bar.
    fn sel_action(&mut self, ui: &AppWindow, a: &str) {
        if let Some(q) = a.strip_prefix("q-") {
            match q {
                "remove" => self.b.remove_queue_selection(self.queue_sel),
                "playlist" => {
                    let paths = self.b.queue_selection_paths(self.queue_sel);
                    self.open_pick(ui, "", paths);
                }
                "all" => {
                    let n = self.b.player.queue().len();
                    if n > 0 {
                        self.b.clear_queue_marks();
                        self.b.toggle_queue_mark(0);
                        self.b.mark_queue_range(n - 1);
                    }
                }
                "clear" => {
                    self.b.clear_queue_marks();
                    self.queue_marking = false;
                }
                _ => {}
            }
            return;
        }
        match a {
            "play" => self.play_selected(),
            "next" => self.b.add_selected_to_queue(true),
            "queue" => self.b.add_selected_to_queue(false),
            "playlist" => {
                let paths = self.b.playlist_paths_for_selection();
                self.open_pick(ui, "", paths);
            }
            "ipod" => self.b.send_selection_to_ipod(),
            "remove" => self.b.edit_playlist(true, 0),
            "all" => self.b.mark_all(),
            "clear" => {
                self.b.clear_marks();
                self.marking = false;
            }
            _ => {}
        }
        self.marks_dirty = true;
    }

    /// A playlist row dragged (or lifted and dropped) to `to`.
    fn row_drag(&mut self, from: usize, to: usize) {
        self.lifted = None;
        self.b.move_playlist_item(from, to);
    }

    fn queue_drag(&mut self, ui: &AppWindow, from: usize, to: usize) {
        self.queue_lifted = None;
        let n = self.b.player.queue().len();
        let to = to.min(n.saturating_sub(1));
        self.b.move_queue_to(from, to);
        self.queue_sel = to;
        self.follow = false;
        ui.set_queue_sel(to as i32);
    }

    /// The smart playlist form, new or to change one.
    fn open_smart(&mut self, ui: &AppWindow, edit: Option<patina_core::library::smart::SmartRow>) {
        let (title, name, query, exclude, ok) = match &edit {
            Some(p) => (
                "Edit smart playlist",
                p.name.clone(),
                p.query.clone(),
                p.exclude.clone(),
                "Save",
            ),
            None => (
                "New smart playlist",
                String::new(),
                String::new(),
                String::new(),
                "Create",
            ),
        };
        self.dlg = match &edit {
            Some(p) => Dlg::SmartEdit(p.id),
            None => Dlg::SmartNew,
        };
        open_text(ui, title, "Name", &name, Some(("Songs where", &query)), ok);
        // The songs to leave out, under the songs to take.
        ui.set_dlg_label3(ss("Leave out songs where (optional)"));
        ui.set_dlg_text3(ss(&exclude));
        ui.set_dlg_three_fields(true);
        self.smart_note(ui, &query, &exclude);
    }

    /// Under the rules: what they match and how many the second box
    /// leaves out, or why they don't work.
    fn smart_note(&self, ui: &AppWindow, query: &str, exclude: &str) {
        if query.trim().is_empty() && exclude.trim().is_empty() {
            ui.set_dlg_note(ss(format!(
                "Like: {}. Empty matches every song. {}",
                patina_core::library::smart::EXAMPLES[0].0,
                patina_core::library::smart::HELP
            )));
            ui.set_dlg_note_warn(false);
            return;
        }
        let c = self.b.check_smart(query, exclude);
        ui.set_dlg_note(ss(c.message));
        ui.set_dlg_note_warn(!c.ok);
    }

    /// Write the selected playlist, or the open list, as a file.
    fn open_export(&mut self, ui: &AppWindow, queue: bool) {
        let name = if queue {
            Some("Queue".to_string())
        } else {
            self.b.export_subject().map(|(n, _)| n)
        };
        let Some(name) = name else {
            return self
                .b
                .note("Pick a playlist, or open a list of songs, to export");
        };
        let path = self.b.default_export_path(&name);
        self.dlg = Dlg::Export(queue);
        open_text(
            ui,
            &format!("Export \"{name}\""),
            "Save as (M3U8)",
            &path.display().to_string(),
            None,
            "Export",
        );
        ui.set_dlg_note(ss(
            "Local songs go in as their full paths. Songs on a server go in as server:// paths, which only PatinaPlayer reads back.",
        ));
    }

    /// Read an M3U, M3U8 or PLS file into a new playlist: the desktop's
    /// file picker, or a typed path where there isn't one.
    fn import_playlist(&mut self) {
        if self.import_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(pick_playlist_dialog());
        });
        self.import_pick = Some((rx, Instant::now()));
    }

    /// The sleep timer's menu: a few lengths, the end of the song, a time
    /// of your own, or off.
    fn sleep_act(&mut self, ui: &AppWindow, a: &str) {
        use patina_core::sleep::SleepTimer;
        match a {
            "off" => self.b.set_sleep(None),
            "end" => self.b.set_sleep(Some(SleepTimer::EndOfSong)),
            "custom" => {
                self.dlg = Dlg::Sleep;
                open_text(ui, "Sleep timer", "Pause after, or at", "", None, "Start");
                self.sleep_note(ui, "");
            }
            mins => {
                if let Ok(m) = mins.parse::<u64>() {
                    self.b
                        .set_sleep(Some(SleepTimer::after(Duration::from_secs(m * 60))));
                }
            }
        }
    }

    /// Under the sleep timer's box, as it's typed: when it would pause, or
    /// what's wrong.
    fn sleep_note(&self, ui: &AppWindow, text: &str) {
        if text.trim().is_empty() {
            ui.set_dlg_note(ss(patina_core::sleep::HELP));
            ui.set_dlg_note_warn(false);
            return;
        }
        match patina_core::sleep::parse(text) {
            Ok(t) => {
                ui.set_dlg_note(ss(t.describe()));
                ui.set_dlg_note_warn(false);
            }
            Err(e) => {
                ui.set_dlg_note(ss(e));
                ui.set_dlg_note_warn(true);
            }
        }
    }

    fn import_path_dialog(&mut self, ui: &AppWindow) {
        self.dlg = Dlg::ImportPath;
        open_text(
            ui,
            "Import a playlist",
            "Path to an .m3u, .m3u8 or .pls file",
            "~/",
            None,
            "Import",
        );
    }

    fn poll_import_pick(&mut self, ui: &AppWindow) {
        let Some((rx, opened)) = &self.import_pick else {
            return;
        };
        let Ok(picked) = rx.try_recv() else { return };
        let quick = opened.elapsed() < Duration::from_millis(700);
        self.import_pick = None;
        match picked {
            Picked::Path(p) => self.b.import_playlist_file(&p),
            Picked::Unavailable(why) => {
                self.b.note(why);
                self.import_path_dialog(ui);
            }
            Picked::Cancelled if quick => self.import_path_dialog(ui),
            Picked::Cancelled => {}
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
        if let Some(n) = a.strip_prefix("rate-").and_then(|n| n.parse::<u8>().ok()) {
            return self.b.rate_selected(n);
        }
        match a {
            "cover-find" | "cover-file" => {
                let t = self.b.selected_cover_target();
                self.cover_action(ui, a, t);
            }
            "select" => {
                if let Some(i) = self.selected_index() {
                    self.b.toggle_mark(i);
                }
                self.marking = is_touch(ui);
                self.marks_dirty = true;
            }
            "ipod" => self.b.send_selection_to_ipod(),
            "export" => self.open_export(ui, false),
            "smart-edit" => {
                if let Some(Item::Smart(p)) = item {
                    self.open_smart(ui, Some(p));
                }
            }
            "smart-delete" => {
                if let Some(Item::Smart(p)) = item {
                    self.confirm(
                        ui,
                        format!(
                            "Delete the smart playlist \"{}\"? The songs stay in the library.",
                            p.name
                        ),
                        "Delete",
                        Confirm::DeleteSmart(p.id),
                    );
                }
            }
            "ipod-remove" => {
                if let Some(Item::Ipod(row)) = item {
                    use patina_core::browse::ipod::IpodKind;
                    let what = match &row.kind {
                        IpodKind::Playlist(n) => {
                            format!("Take the playlist \"{n}\" off the iPod? Its songs stay on it.")
                        }
                        IpodKind::Album { album, .. } => {
                            format!(
                                "Take {album} off the iPod? Its songs are deleted from the iPod."
                            )
                        }
                    };
                    self.confirm(ui, what, "Remove", Confirm::IpodRemove(row));
                }
            }
            "lift" => self.lifted = self.selected_index(),
            "lift-cancel" => self.lifted = None,
            "drop" => {
                if let (Some(from), Some(to)) = (self.lifted, self.selected_index()) {
                    self.row_drag(from, to);
                }
            }
            "fav" => self.b.toggle_favorite_selected(),
            "play" => self.play_selected(),
            "next" => self.b.add_selected_to_queue(true),
            "queue" => self.b.add_selected_to_queue(false),
            "playlist" => {
                let paths = self.b.playlist_paths_for_selection();
                self.open_pick(ui, "", paths);
            }
            "pl-remove" => self.b.edit_playlist(true, 0),
            "go-album" | "go-artist" => {
                self.b.go_to_selected(a == "go-artist");
                self.after_go_to(ui);
            }
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
            "mix" => {
                if let Some(it) = item {
                    self.b.mix_item(&it);
                    self.follow = true;
                }
            }
            "mark" => {
                if let Some(Item::Genre(g)) = item {
                    self.b.toggle_genre_mark(&g);
                }
            }
            "mix-rename" => {
                if let Some(Item::Mix(m)) = item {
                    self.dlg = Dlg::RenameMix(m.id);
                    open_text(ui, "Rename mix", "Name", &m.name, None, "Rename");
                }
            }
            "mix-delete" => {
                if let Some(Item::Mix(m)) = item {
                    self.confirm(
                        ui,
                        format!("Delete the saved mix \"{}\"?", m.name),
                        "Delete",
                        Confirm::DeleteMix(m.id),
                    );
                }
            }
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
            "select" => {
                self.b.toggle_queue_mark(i);
                self.queue_marking = is_touch(ui);
            }
            "lift" => self.queue_lifted = Some(i),
            "lift-cancel" => self.queue_lifted = None,
            "drop" => {
                if let Some(from) = self.queue_lifted {
                    self.queue_drag(ui, from, i);
                }
            }
            "play" => {
                self.b.play_queue_position(i);
                self.follow = true;
            }
            "up" => self.queue_sel = self.b.move_queue_position(i, -1),
            "down" => self.queue_sel = self.b.move_queue_position(i, 1),
            "remove" => self.b.remove_queue_selection(i),
            "playlist" => {
                let paths = self.b.queue_selection_paths(i);
                self.open_pick(ui, "", paths);
            }
            "go-album" | "go-artist" => {
                self.b.go_to_queue_position(i, a == "go-artist");
                self.after_go_to(ui);
            }
            _ => {}
        }
    }

    /// After Go to album or artist: on a phone the queue panel covers the
    /// view it opened, so it folds away.
    fn after_go_to(&mut self, ui: &AppWindow) {
        if ui.get_tiny() && ui.get_queue_panel() {
            ui.set_queue_panel(false);
        }
        ui.set_drawer_open(false);
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
            Dlg::CastSnapcast => match patina_core::cast::Target::snapcast(&t1) {
                Some(t) => {
                    self.b
                        .change_setting(&SettingKey::CastSnapcast, Change::Set(t1));
                    self.b.cast_to(Some(t));
                }
                None => {
                    self.dlg = Dlg::CastSnapcast;
                    ui.set_dlg_note(ss("Type the snapserver's address"));
                    ui.set_dlg_note_warn(true);
                    return;
                }
            },
            Dlg::PlayerCode(addr, name) => self.connect_player(&addr, &t1, &name),
            Dlg::PlayerAddr => {
                if t1.is_empty() {
                    self.dlg = Dlg::PlayerAddr;
                    ui.set_dlg_note(ss("Type the other player's address"));
                    ui.set_dlg_note_warn(true);
                    return;
                }
                self.connect_player(&t1, &t2, &t1);
            }
            Dlg::EqSave => self.b.eq_save_preset(&t1),
            Dlg::AmpSave => self.b.amp_save_preset(&t1),
            Dlg::EqImportPath => self
                .b
                .eq_import(&patina_core::config::expand_tilde(t1.trim())),
            Dlg::NewPlaylist(paths) => self.b.create_playlist(&t1, &paths),
            Dlg::RenamePlaylist(id) => self.b.rename_playlist(id, &t1),
            Dlg::RenameMix(id) => self.b.rename_mix(id, &t1),
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
            Dlg::SmartNew => {
                let t3 = ui.get_dlg_text3().trim().to_string();
                if !self.b.create_smart(&t1, &t2, &t3) {
                    self.dlg = Dlg::SmartNew;
                    self.smart_note(ui, &t2, &t3);
                    return;
                }
            }
            Dlg::SmartEdit(id) => {
                let t3 = ui.get_dlg_text3().trim().to_string();
                if !self.b.update_smart(id, &t1, &t2, &t3) {
                    self.dlg = Dlg::SmartEdit(id);
                    self.smart_note(ui, &t2, &t3);
                    return;
                }
            }
            Dlg::Export(queue) => {
                let path = std::path::PathBuf::from(&t1);
                if queue {
                    self.b.export_queue_to(&path);
                } else {
                    self.b.export_to(&path);
                }
            }
            Dlg::ImportPath => self.b.import_playlist_file(std::path::Path::new(&t1)),
            Dlg::Sleep => {
                if let Err(e) = self.b.set_sleep_text(&t1) {
                    // Leave it up to fix.
                    self.dlg = Dlg::Sleep;
                    ui.set_dlg_note(ss(e));
                    ui.set_dlg_note_warn(true);
                    return;
                }
            }
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
                Confirm::DeleteMix(id) => self.b.delete_mix(id),
                Confirm::DeleteSmart(id) => self.b.delete_smart(id),
                Confirm::IpodRemove(row) => self.b.remove_from_ipod(&row),
            },
            Dlg::Pick(..)
            | Dlg::Choose(_)
            | Dlg::Dlna(_)
            | Dlg::Help
            | Dlg::Covers
            | Dlg::Cast(_)
            | Dlg::Players(_)
            | Dlg::RemoteQueue(_)
            | Dlg::None => {}
        }
        ui.set_dlg_note(SharedString::new());
        ui.set_dialog(0);
        ui.invoke_focus_root();
    }

    fn dlg_pick(&mut self, ui: &AppWindow, i: usize) {
        // The output picker's last row: another device.
        if let Dlg::Choose(SettingKey::Device) = &self.dlg
            && self.b.device_setting().is_some_and(
                |st| matches!(&st.control, Control::Choice { options, .. } if i >= options.len()),
            )
        {
            return self.open_cast(ui);
        }
        // A device picked here plays here again, rather than on the
        // other device.
        if let Dlg::Choose(SettingKey::Device) = &self.dlg {
            self.b.cast_to(None);
        }
        match std::mem::replace(&mut self.dlg, Dlg::None) {
            Dlg::Cast(rows) => {
                if let Some(r) = rows.get(i).cloned() {
                    self.dlg = Dlg::Cast(rows);
                    return self.cast_pick(ui, r);
                }
                ui.set_dialog(0);
                return;
            }
            Dlg::Players(rows) => {
                if let Some(r) = rows.get(i).cloned() {
                    self.dlg = Dlg::Players(rows);
                    return self.player_pick(ui, r);
                }
                ui.set_dialog(0);
                return;
            }
            Dlg::RemoteQueue(ids) => {
                if let (Some(&id), Some(r)) = (ids.get(i), self.remote.as_mut()) {
                    r.send(patina_core::control::Cmd::PlayEntry { id });
                }
                ui.set_dialog(0);
                return;
            }
            other => self.dlg = other,
        }
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
                "e" | "E" => self.b.switch(Section::Eq),
                "a" | "A" => self.b.switch(Section::Amp),
                "o" | "O" => self.pick_device(ui),
                "l" | "L" => self.toggle_lyrics(ui),
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
        // Steering another player: Escape and Back come back here.
        if self.remote.is_some() && (is(Key::Escape) || is(Key::Back)) {
            self.remote = None;
            ui.set_rm_open(false);
            return true;
        }
        let queue = self.b.section == Section::Queue;
        if is(Key::Escape) {
            // Picked rows, or a lifted one, let go first.
            if self.lifted.take().is_some() || self.queue_lifted.take().is_some() {
                self.last_gen = u64::MAX;
                return true;
            }
            let had = if queue {
                self.b.clear_queue_marks()
            } else {
                self.b.clear_marks()
            };
            self.marking = false;
            self.queue_marking = false;
            self.marks_dirty = true;
            return had;
        }
        if shift && (is(Key::UpArrow) || is(Key::DownArrow)) {
            // Shift and the arrows pick rows as the selection moves.
            let d = if is(Key::UpArrow) { -1 } else { 1 };
            if queue {
                let from = self.queue_sel;
                if self.b.queue_marks.is_empty() {
                    self.b.toggle_queue_mark(from);
                }
                self.move_sel(ui, d);
                self.b.mark_queue_range(self.queue_sel);
            } else if let Some(from) = self.selected_index() {
                if self.b.marked_count() == 0 {
                    self.b.toggle_mark(from);
                }
                self.move_sel(ui, d);
                if let Some(to) = self.selected_index() {
                    self.b.mark_range(to);
                }
                self.marks_dirty = true;
            }
            return true;
        }
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
        } else if is(Key::Back) {
            // Android's Back: a level up while there is one, then out of
            // the app without stopping the music, where the app says how.
            if ui.get_can_back() {
                self.b.pop();
            } else if let Some(out) = &self.back_out {
                out();
            } else {
                return false;
            }
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
                self.b.remove_queue_selection(self.queue_sel);
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
                "l" => self.b.toggle_love(),
                "q" => self.toggle_queue_panel(ui),
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
                "X" if !queue => {
                    self.b.mix_selected();
                    self.follow = true;
                }
                "M" if queue => self.b.save_mix(),
                "M" => {
                    if let Some(Item::Genre(g)) = self.b.selected_item() {
                        self.b.toggle_genre_mark(&g);
                    }
                }
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
                    self.b.switch_soon(Section::NUMBERED[i]);
                }
                _ => return false,
            }
        }
        true
    }
}

fn is_touch(ui: &AppWindow) -> bool {
    ui.global::<Ui>().get_touch()
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
/// A cover blurred for the backdrop: shrunk to a small square and blurred
/// there, cheap at any size. Drawn stretched over the window, which
/// softens it further.
fn blurred(img: &slint::Image) -> Option<slint::Image> {
    let buf = img.to_rgba8()?;
    let rgba = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())?;
    let small = image::imageops::resize(&rgba, 48, 48, image::imageops::FilterType::Triangle);
    let soft = image::imageops::blur(&small, 4.0);
    Some(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::clone_from_slice(
        soft.as_raw(),
        soft.width(),
        soft.height(),
    )))
}

/// How light a color is, 0 to 1 (relative luminance, roughly).
fn luminance(c: slint::Color) -> f32 {
    (0.2126 * f32::from(c.red()) + 0.7152 * f32::from(c.green()) + 0.0722 * f32::from(c.blue()))
        / 255.0
}

/// How much of the theme's ground has to lie over the blurred cover for
/// the theme's text to stay readable on it: 0 when the cover is already
/// dark enough (or light enough, on a light theme), up to nearly all of it
/// for a white cover under light text. Judged on the cover's brightest
/// tenth (darkest, on a light theme), since that's where text gets lost,
/// as blended on screen: the cover at `strength` over the ground. Measured
/// against the theme's muted text, the faintest that has to stay readable
/// (placeholders, second lines).
fn wash_veil(blur: &slint::Image, strength: f32, bg: slint::Color, text: slint::Color) -> f32 {
    let Some(buf) = blur.to_rgba8() else {
        return 0.0;
    };
    let mut lumas: Vec<f32> = buf
        .as_slice()
        .iter()
        .map(|p| luminance(slint::Color::from_rgb_u8(p.r, p.g, p.b)))
        .collect();
    if lumas.is_empty() {
        return 0.0;
    }
    lumas.sort_by(f32::total_cmp);
    let (lb, lt) = (luminance(bg), luminance(text));
    let dark = lt > lb;
    let at = if dark {
        lumas.len() * 9 / 10
    } else {
        lumas.len() / 10
    };
    let art = lumas[at.min(lumas.len() - 1)];
    let shown = strength * art + (1.0 - strength) * lb;
    let v = if dark {
        // Light text: keep what's behind it at most this bright.
        let most = (lt - 0.32).clamp(0.12, 0.35);
        if shown > most {
            (shown - most) / (shown - lb).max(0.01)
        } else {
            0.0
        }
    } else {
        // Dark text: at least this bright.
        let least = (lt + 0.35).clamp(0.65, 0.88);
        if shown < least {
            (least - shown) / (lb - shown).max(0.01)
        } else {
            0.0
        }
    };
    v.clamp(0.0, 0.92)
}

/// A strong color from a cover, for the transport and Now Playing, and
/// what reads on top of it. The most common vivid hue wins, then it's
/// lightened or darkened to stand out from the theme's ground. None for a
/// cover with no strong color (black and white, washed out), which keeps
/// the theme's own.
fn accent_of(img: &slint::Image, dark: bool) -> Option<(slint::Color, slint::Color)> {
    let buf = img.to_rgba8()?;
    let rgba = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())?;
    let small = image::imageops::resize(&rgba, 32, 32, image::imageops::FilterType::Triangle);
    let (h, s, l) = vivid_hsl(small.pixels().map(|p| [p[0], p[1], p[2]]))?;
    let s = s.max(0.45);
    let l = if dark {
        l.clamp(0.55, 0.7)
    } else {
        l.clamp(0.32, 0.45)
    };
    let [r, g, b] = hsl_to_rgb(h, s, l);
    let c = slint::Color::from_rgb_u8(r, g, b);
    let on = if luminance(c) > 0.55 {
        slint::Color::from_rgb_u8(0x10, 0x12, 0x12)
    } else {
        slint::Color::from_rgb_u8(0xff, 0xff, 0xff)
    };
    Some((c, on))
}

/// The hue most of a picture's vivid pixels share, and their average
/// saturation and lightness. Twelve hue buckets; each pixel counts by how
/// vivid it is.
fn vivid_hsl(pixels: impl Iterator<Item = [u8; 3]>) -> Option<(f32, f32, f32)> {
    let mut weight = [0f32; 12];
    let mut sums = [[0f32; 3]; 12];
    let mut total = 0usize;
    for [r, g, b] in pixels {
        total += 1;
        let (h, s, l) = rgb_to_hsl(r, g, b);
        if s < 0.3 || !(0.15..=0.88).contains(&l) {
            continue;
        }
        let w = s * (1.0 - (l - 0.5).abs() * 1.4);
        let k = ((h / 30.0) as usize).min(11);
        weight[k] += w;
        // Hue as a point on a circle, so red near 0 and 360 averages right.
        let rad = h.to_radians();
        sums[k][0] += rad.cos() * w;
        sums[k][1] += rad.sin() * w;
        sums[k][2] += l * w;
    }
    let (k, &w) = weight
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    // Under a twentieth of the picture vivid: no strong color to take.
    if total == 0 || w < total as f32 * 0.05 {
        return None;
    }
    let h = sums[k][1].atan2(sums[k][0]).to_degrees().rem_euclid(360.0);
    Some((h, 0.65, sums[k][2] / w))
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [f(r), f(g), f(b)]
}

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
    ui.set_dlg_three_fields(false);
    let (l2, t2) = second.unwrap_or(("", ""));
    ui.set_dlg_label2(ss(l2));
    ui.set_dlg_text2(ss(t2));
    ui.set_dlg_ok(ss(ok));
    ui.set_dlg_note(SharedString::new());
    ui.set_dlg_note_warn(false);
    ui.set_dialog(1);
}

/// Keeps the timer (and so the bridge) alive for the window's lifetime.
pub struct Bridge {
    _timer: Timer,
    _vis_timer: Timer,
    _state: Rc<RefCell<State>>,
}

impl Bridge {
    /// What Back does once there's no level left to go back to.
    pub fn on_back_out(&self, f: impl Fn() + 'static) {
        self._state.borrow_mut().back_out = Some(Box::new(f));
    }

    /// The player, for the platform glue around the window (Android's media
    /// session).
    pub fn player(&self) -> patina_core::Player {
        self._state.borrow().b.player.clone()
    }

    /// Look through the library folders and feeds again, as the Rescan
    /// action does (Android, once the app may read music).
    pub fn rescan(&self) {
        self._state.borrow_mut().b.rescan();
    }

    /// Save the queue to its server if it's a server's, then stop the
    /// player with a fade.
    pub fn shutdown(&self, fade_ms: u32) {
        patina_core::session::shutdown(&mut self._state.borrow_mut().b, fade_ms);
    }
}

/// Wire the window to the core. `hide_on_close` is kept up to date with
/// whether closing the window should hide it to the tray.
pub fn install(
    ui: &AppWindow,
    mut b: Browser,
    playing_files: bool,
    hide_on_close: Rc<std::cell::Cell<bool>>,
) -> Bridge {
    // Sections read on the frame after they're opened, so placeholder
    // rows show first. The window comes up sooner this way too.
    b.defer_builds = true;
    b.switch_soon(if playing_files {
        Section::Queue
    } else {
        Section::Folders
    });
    // A phone: long presses for menus, sliders for the amp's dials.
    ui.global::<Ui>().set_touch(cfg!(target_os = "android"));
    // The system's text dialogs, where the platform offers them.
    ui.set_native_text(platform::hooks().is_some());
    let st = Rc::new(RefCell::new(State::new(b, hide_on_close)));
    {
        let s = st.borrow();
        ui.set_side(ModelRc::from(s.side.clone()));
        ui.set_rows(ModelRc::from(s.rows.clone()));
        ui.set_eq_bands(ModelRc::from(s.eq_bands.clone()));
        ui.set_amp_units(ModelRc::from(s.amp_units.clone()));
        ui.set_queue(ModelRc::from(s.queue.clone()));
        ui.set_crumbs(ModelRc::from(s.crumbs.clone()));
        ui.set_columns(ModelRc::from(s.columns.clone()));
        ui.set_actions(ModelRc::from(s.actions.clone()));
        ui.set_action_labels(ModelRc::from(s.action_labels.clone()));
        ui.set_dlg_items(ModelRc::from(s.dlg_items.clone()));
        ui.set_queue_servers(ModelRc::from(s.queue_servers.clone()));
        ui.set_np_chain(ModelRc::from(s.np_chain.clone()));
        ui.set_grid_rows(ModelRc::from(s.grid_rows.clone()));
        ui.set_covers_found(ModelRc::from(s.covers_model.clone()));
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

    // A tap in the sidebar always shows the top of the view, even when the
    // view is the one already open: that's how you get the breadcrumbs back
    // after the page scrolled them away.
    on!(on_side_clicked, |s, u, i| {
        if i < SIDE_EXTRA {
            u.set_np_open(true);
        } else {
            // Somewhere else: Now Playing gets out of the way, even when
            // it's the view already open underneath.
            u.set_np_open(false);
            s.b.activate_sidebar((i - SIDE_EXTRA) as usize);
            u.invoke_scroll_top();
        }
    });
    on!(on_side_fold, |s, _u, landscape, fold| s
        .b
        .set_sidebar(landscape, fold));
    on!(on_back, |s, u| {
        s.b.pop();
    });
    on!(on_forward, |s, u| {
        s.b.go_forward();
    });
    on!(on_search_edited, |s, u, text| s
        .search_typed(&u, text.as_str()));
    on!(on_search_cancel, |s, u| s.search_cancel(&u));
    on!(on_search_tapped, |s, u| s.open_search_prompt(&u));
    on!(on_crumb_clicked, |s, u, i| s.b.pop_to(i as usize + 1));
    on!(on_row_select, |s, u, i| {
        s.select_row(&u, i as usize);
    });
    on!(on_row_activate, |s, u, i| s.activate_row(&u, i as usize));
    on!(on_eq_point, |s, _u, i, x, g| s
        .eq_point(i as usize, x as f64, g as f64));
    on!(on_eq_chip, |s, _u| s.b.switch(Section::Eq));
    on!(on_np_rate, |s, _u, n| s.b.rate_playing(n.clamp(0, 5) as u8));
    on!(on_hero_rate, |s, _u, n| s.b.rate_level(n.clamp(0, 5) as u8));
    on!(on_hero_fav, |s, _u| s.b.toggle_favorite_level());
    on!(on_lyrics_clicked, |s, u| s.toggle_lyrics(&u));
    on!(on_side_act, |s, _u, i, a| {
        let core = (i - SIDE_EXTRA).max(0) as usize;
        if let Some(SideEntry::Favorite { id, .. }) = s.b.sidebar.get(core).cloned() {
            match a.as_str() {
                "up" => s.b.favorite_move(id, -1),
                "down" => s.b.favorite_move(id, 1),
                _ => s.b.unfavorite(id),
            }
        }
    });
    on!(on_amp_chip, |s, _u| s.b.switch(Section::Amp));
    on!(on_amp_knob, |s, _u, i, j, p| s
        .amp_knob(i as usize, j as usize, p as f64));
    on!(on_amp_step, |s, _u, i, j| s
        .amp_step(i as usize, j as usize));
    on!(on_amp_unit_toggle, |s, _u, i| {
        if let Some(u) = s.b.amp_settings().units.get(i as usize).copied() {
            s.b.amp_set_unit_on(u.kind, !u.on);
        }
    });
    on!(on_output_clicked, |s, u| s.pick_device(&u));
    on!(on_love_clicked, |s, _u| s.b.toggle_love());
    on!(on_np_act, |s, u, a| s.np_act(&u, &a));
    on!(on_hero_cover, |s, u, a| {
        let t = s.b.level_cover_target();
        s.cover_action(&u, &a, t)
    });
    on!(on_cover_pick, |s, u, i| s.cover_pick(&u, i.max(0) as usize));
    on!(on_cover_file, |s, u| {
        if let Some(t) = s.covers.target.clone() {
            s.dlg = Dlg::None;
            u.set_dialog(0);
            s.pick_cover_file(t);
        }
    });
    on!(on_level_clicked, |s, u| {
        let tip = u.get_level_tip().to_string();
        if !tip.is_empty() {
            s.b.note(tip);
        }
    });
    on!(on_bp_clicked, |s, u| {
        let tip = u.get_bp_tip().to_string();
        if !tip.is_empty() {
            s.b.note(tip);
        }
    });
    on!(on_row_act, |s, u, i, a| {
        s.select_row(&u, i as usize);
        s.row_act(&u, a.as_str());
    });
    on!(on_view_action, |s, u, a| s.view_action(&u, a.as_str()));
    on!(on_row_mark, |s, u, i, range| s
        .row_mark(&u, i as usize, range));
    on!(on_row_drag, |s, _u, from, to| s
        .row_drag(from as usize, to.max(0) as usize));
    on!(on_sel_action, |s, u, a| s.sel_action(&u, a.as_str()));
    on!(on_queue_mark, |s, _u, i, range| {
        if range {
            s.b.mark_queue_range(i as usize);
        } else {
            s.b.toggle_queue_mark(i as usize);
        }
    });
    on!(on_queue_drag, |s, u, from, to| s.queue_drag(
        &u,
        from as usize,
        to.max(0) as usize
    ));
    on!(on_dlg_edited, |s, u, field, text| {
        // Either box changes what the rules match.
        if matches!(field, 2 | 3) && matches!(s.dlg, Dlg::SmartNew | Dlg::SmartEdit(_)) {
            let (q, x) = if field == 2 {
                (text.to_string(), u.get_dlg_text3().to_string())
            } else {
                (u.get_dlg_text2().to_string(), text.to_string())
            };
            s.smart_note(&u, &q, &x);
        }
        if field == 1 && matches!(s.dlg, Dlg::Sleep) {
            s.sleep_note(&u, text.as_str());
        }
    });
    on!(on_queue_select, |s, u, i| {
        s.queue_sel = i as usize;
        s.follow = false;
        u.set_queue_sel(i);
    });
    on!(on_queue_activate, |s, u, i| {
        let now = Instant::now();
        if s.last_activate
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400))
        {
            return;
        }
        s.last_activate = Some(now);
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
    on!(on_toggle_queue, |s, u| s.toggle_queue_panel(&u));
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
    on!(on_speed_set, |s, _u, v| s
        .b
        .set_podcast_playback(v as f64, s.b.podcast_trim));
    on!(on_trim_toggle, |s, _u| s.b.toggle_trim());
    on!(on_dlg_accept, |s, u| s.dlg_accept(&u));
    on!(on_cast_clicked, |s, u| s.open_cast(&u));
    on!(on_vis_next, |s, u| {
        s.b.next_visualizer();
        s.vis_tick(&u);
        let v = s.b.config.appearance.visualizer;
        s.b.note(format!("Visualizer: {}", v.label()));
    });
    on!(on_rm_cmd, |s, _u, c| s.remote_cmd(c.as_str()));
    on!(on_rm_seek, |s, _u, v| {
        if let Some(r) = &mut s.remote {
            let ms = (v.clamp(0.0, 1.0) as f64 * r.duration_ms() as f64) as u64;
            r.send(patina_core::control::Cmd::Seek { ms });
        }
    });
    on!(on_rm_set_volume, |s, _u, v| {
        if let Some(r) = &mut s.remote {
            r.send(patina_core::control::Cmd::Volume {
                value: v.clamp(0.0, 1.0) as f64,
            });
        }
    });
    on!(on_rm_close, |s, u| {
        s.remote = None;
        u.set_rm_open(false);
    });
    on!(on_rm_queue, |s, u| s.open_remote_queue(&u));
    on!(on_sleep_act, |s, u, a| s.sleep_act(&u, a.as_str()));
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
    // The visualizer, smoother than the main tick. Does nothing unless
    // Now Playing shows one.
    let vis_timer = Timer::default();
    {
        let st = st.clone();
        let weak = ui.as_weak();
        vis_timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            if let Some(u) = weak.upgrade()
                && let Ok(mut s) = st.try_borrow_mut()
            {
                s.vis_tick(&u);
            }
        });
    }
    st.borrow_mut().tick(ui);
    ui.invoke_focus_root();
    Bridge {
        _timer: timer,
        _vis_timer: vis_timer,
        _state: st,
    }
}

/// Bars in Now Playing's visualizer.
const VIS_BARS: usize = 40;

/// The oscilloscope's line, in a 100 by 100 box.
fn wave_path(wave: &[f32]) -> String {
    use std::fmt::Write;
    let mut p = String::with_capacity(wave.len() * 14);
    let last = wave.len().saturating_sub(1).max(1) as f32;
    for (i, v) in wave.iter().enumerate() {
        let x = i as f32 * 100.0 / last;
        let y = 50.0 - v * 40.0;
        let _ = write!(p, "{}{x:.1} {y:.1} ", if i == 0 { "M " } else { "L " });
    }
    p
}

/// The ring: a ray for each bar, mirrored left and right so it's
/// symmetrical, lows at the top.
fn ring_path(bars: &[f32]) -> String {
    use std::fmt::Write;
    let mut p = String::with_capacity(bars.len() * 60);
    let n = bars.len();
    for (i, v) in bars.iter().enumerate() {
        for side in [1.0f32, -1.0] {
            let a = side * std::f32::consts::PI * (i as f32 + 0.5) / n as f32;
            let (sin, cos) = a.sin_cos();
            let (r0, r1) = (24.0, 26.0 + v * 22.0);
            let _ = write!(
                p,
                "M {:.1} {:.1} L {:.1} {:.1} ",
                50.0 + sin * r0,
                50.0 - cos * r0,
                50.0 + sin * r1,
                50.0 - cos * r1
            );
        }
    }
    if p.is_empty() {
        p.push_str("M 50 50");
    }
    p
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
            Item::Mix(_) => "mix",
            Item::Smart(_) => "smart playlist",
            Item::Ipod(r) => match r.kind {
                patina_core::browse::ipod::IpodKind::Playlist(_) => "playlist",
                patina_core::browse::ipod::IpodKind::Album { .. } => "album",
            },
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

/// The lit part of a rack dial: an arc from the bottom left, clockwise
/// through the top, `pos` of the way to the bottom right, in a 100 x 100
/// box. Matches the track drawn under it.
fn dial_arc(pos: f64) -> String {
    let pos = pos.clamp(0.0, 1.0);
    let at = |deg: f64| {
        let r = deg.to_radians();
        (50.0 + 38.0 * r.cos(), 50.0 + 38.0 * r.sin())
    };
    let (x0, y0) = at(135.0);
    if pos < 0.004 {
        return format!("M {x0:.2} {y0:.2} L {x0:.2} {y0:.2}");
    }
    let (x, y) = at(135.0 + 270.0 * pos);
    let large = if 270.0 * pos > 180.0 { 1 } else { 0 };
    format!("M {x0:.2} {y0:.2} A 38 38 0 {large} 1 {x:.2} {y:.2}")
}

/// The desktop's folder picker. Cancelled when there isn't one too (rfd
/// can't tell them apart); the app then asks for a typed path.
#[cfg(not(target_os = "android"))]
fn pick_folder_dialog() -> Picked {
    match rfd::FileDialog::new()
        .set_title("Add a music folder")
        .pick_folder()
    {
        Some(p) => Picked::Path(p),
        None => Picked::Cancelled,
    }
}

/// Android picks through the hooks patina-android sets; without them
/// there's nothing to pick with.
#[cfg(target_os = "android")]
fn pick_folder_dialog() -> Picked {
    Picked::Unavailable("No folder picker is available here. Use \"Type a path\" instead.".into())
}

#[cfg(not(target_os = "android"))]
fn pick_eq_dialog() -> Picked {
    match rfd::FileDialog::new()
        .set_title("Import an AutoEq or Equalizer APO file")
        .add_filter("EQ settings", &["txt"])
        .pick_file()
    {
        Some(p) => Picked::Path(p),
        None => Picked::Cancelled,
    }
}

#[cfg(target_os = "android")]
fn pick_eq_dialog() -> Picked {
    Picked::Unavailable("No file picker is available here.".into())
}

#[cfg(not(target_os = "android"))]
fn pick_image_dialog() -> Picked {
    match rfd::FileDialog::new()
        .set_title("Pick a cover")
        .add_filter("Images", &["jpg", "jpeg", "png", "webp", "gif", "bmp"])
        .pick_file()
    {
        Some(p) => Picked::Path(p),
        None => Picked::Cancelled,
    }
}

#[cfg(target_os = "android")]
fn pick_image_dialog() -> Picked {
    Picked::Unavailable("No picker for images is available here.".into())
}

#[cfg(not(target_os = "android"))]
fn pick_playlist_dialog() -> Picked {
    match rfd::FileDialog::new()
        .set_title("Import a playlist")
        .add_filter("Playlists", &["m3u", "m3u8", "pls"])
        .pick_file()
    {
        Some(p) => Picked::Path(p),
        None => Picked::Cancelled,
    }
}

#[cfg(target_os = "android")]
fn pick_playlist_dialog() -> Picked {
    Picked::Unavailable("Type the playlist file's path.".into())
}

/// Bring the window back from the tray, or forward from behind others.
fn show_window(ui: &AppWindow) {
    let _ = ui.window().show();
    ui.window().set_minimized(false);
    ui.invoke_focus_root();
}

/// Too narrow for both side panels at once (the sidebar is folded, or a drawer).
fn narrow_window(ui: &AppWindow) -> bool {
    ui.get_narrow()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trips() {
        for c in [
            [200u8, 40, 30],
            [20, 160, 90],
            [60, 90, 220],
            [128, 128, 128],
        ] {
            let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
            let back = hsl_to_rgb(h, s, l);
            for k in 0..3 {
                assert!(
                    (i16::from(back[k]) - i16::from(c[k])).abs() <= 1,
                    "{c:?} {back:?}"
                );
            }
        }
    }

    #[test]
    fn the_vivid_hue_wins_over_grey() {
        // Mostly grey, with a red patch: red is the accent.
        let mut px = vec![[120u8, 120, 120]; 900];
        px.extend(vec![[210u8, 30, 40]; 124]);
        let (h, _, _) = vivid_hsl(px.into_iter()).unwrap();
        assert!(!(15.0..=345.0).contains(&h), "{h}");
        // All grey: none.
        assert_eq!(vivid_hsl(vec![[90u8, 90, 90]; 100].into_iter()), None);
    }

    fn flat(c: [u8; 3]) -> slint::Image {
        let px: Vec<u8> = (0..16).flat_map(|_| [c[0], c[1], c[2], 255]).collect();
        slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&px, 4, 4))
    }

    fn rgb(c: [u8; 3]) -> slint::Color {
        slint::Color::from_rgb_u8(c[0], c[1], c[2])
    }

    /// The backdrop as it lands on screen, veil included.
    fn shown(cover: [u8; 3], strength: f32, bg: [u8; 3], veil: f32) -> f32 {
        let (a, b) = (luminance(rgb(cover)), luminance(rgb(bg)));
        let img = strength * a + (1.0 - strength) * b;
        veil * b + (1.0 - veil) * img
    }

    #[test]
    fn a_white_cover_is_veiled_under_light_text() {
        // Runout's ground and muted text.
        let (bg, text) = ([0x0e, 0x19, 0x17], [0x8f, 0xa3, 0x9d]);
        let v = wash_veil(&flat([250, 250, 250]), 0.8, rgb(bg), rgb(text));
        assert!(v > 0.5, "{v}");
        assert!(shown([250, 250, 250], 0.8, bg, v) <= 0.31);
        // A dark cover needs nothing.
        assert_eq!(wash_veil(&flat([20, 30, 40]), 0.8, rgb(bg), rgb(text)), 0.0);
    }

    #[test]
    fn a_black_cover_is_veiled_under_dark_text() {
        // Light's ground and muted text.
        let (bg, text) = ([0xff, 0xff, 0xff], [0x68, 0x6d, 0x76]);
        let v = wash_veil(&flat([5, 5, 5]), 0.6, rgb(bg), rgb(text));
        assert!(v > 0.3, "{v}");
        assert!(shown([5, 5, 5], 0.6, bg, v) >= 0.77);
        assert_eq!(
            wash_veil(&flat([240, 235, 230]), 0.6, rgb(bg), rgb(text)),
            0.0
        );
    }
}
