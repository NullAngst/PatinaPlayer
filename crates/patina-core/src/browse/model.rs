//! What each view shows, as plain data: the sidebar sections, a stack of
//! levels you drill into, and the rows in each level. Both frontends draw
//! from this, so a view looks and behaves the same in the TUI and the GUI.

use std::path::PathBuf;
use std::time::Duration;

use std::collections::HashMap;

use crate::config::Station;
use crate::library::podcasts::{EpisodeRow, PodcastRow};
use crate::library::{
    AlbumRow, AlbumSort, ArtistMode, ArtistRow, DirEntry, GenreRow, Library, PlaylistRow, SongSort,
    TrackRow,
};
use crate::track::{MediaKind, TrackInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Folders,
    Artists,
    Albums,
    Songs,
    Genres,
    Playlists,
    Radio,
    Podcasts,
    Queue,
    Downloads,
    Settings,
    /// The EQ view.
    Eq,
    /// Your top five songs, albums and artists.
    MostListened,
    TopAlbums,
    TopArtists,
}

impl Section {
    /// The ten with number keys (1 to 9, then 0), in sidebar order.
    pub const NUMBERED: [Section; 10] = [
        Section::Folders,
        Section::Artists,
        Section::Albums,
        Section::Songs,
        Section::Genres,
        Section::Playlists,
        Section::Radio,
        Section::Podcasts,
        Section::Queue,
        Section::Downloads,
    ];

    /// The key that jumps here.
    pub fn key(self) -> Option<char> {
        let i = Self::NUMBERED.iter().position(|&s| s == self)?;
        Some(if i == 9 {
            '0'
        } else {
            (b'1' + i as u8) as char
        })
    }

    /// The listening stats, which sit together in the sidebar.
    pub const STATS: [Section; 3] = [
        Section::MostListened,
        Section::TopAlbums,
        Section::TopArtists,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Folders => "Folders",
            Section::Artists => "Artists",
            Section::Albums => "Albums",
            Section::Songs => "Songs",
            Section::Genres => "Genres",
            Section::Playlists => "Playlists",
            Section::Radio => "Radio",
            Section::Podcasts => "Podcasts",
            Section::Queue => "Queue",
            Section::Downloads => "Downloads",
            Section::Settings => "Settings",
            Section::Eq => "EQ",
            Section::MostListened => "Most listened",
            Section::TopAlbums => "Top albums",
            Section::TopArtists => "Top artists",
        }
    }
}

/// Where a level's rows came from, so it can be rebuilt when the library
/// changes under it.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Section(Section),
    Folder(PathBuf),
    ArtistAlbums(String),
    Album { key: String },
    Genre { norm: String, name: String },
    Playlist { id: i64, name: String },
    Podcast { id: i64, title: String },
    Search(String),
}

#[derive(Debug, Clone)]
pub enum Item {
    Dir {
        path: PathBuf,
        name: String,
    },
    /// A folder root, with its track count.
    Root {
        path: PathBuf,
        tracks: i64,
    },
    Artist(ArtistRow),
    Album(AlbumRow),
    Track(TrackRow),
    /// A playlist item whose file isn't in the library any more.
    Missing {
        uri: Vec<u8>,
    },
    Genre(GenreRow),
    Playlist(PlaylistRow),
    /// One of your stations, by its place in the config list. A server's
    /// station has `server` set instead, and isn't yours to edit.
    Station {
        index: usize,
        station: Station,
        check: Option<String>,
        server: Option<String>,
    },
    Podcast(PodcastRow),
    Episode(EpisodeRow),
    /// Group label inside search results. Not selectable.
    Header(&'static str),
    /// One option in Settings.
    Setting(super::settings::Setting),
    /// Label and value, for Downloads.
    Info {
        label: String,
        value: String,
    },
    /// Something pinned for offline, in Downloads.
    Pinned(crate::cache::PinRow),
    /// A download, running or waiting, in Downloads.
    Job(JobRow),
    /// A saved mix, in Playlists.
    Mix(crate::library::mixes::SavedMixRow),
    /// A button that sits in a list, in Downloads.
    Action {
        action: ListAction,
        label: String,
    },
}

/// Buttons in the Downloads list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListAction {
    PauseDownloads,
    ResumeDownloads,
    ClearCache,
}

/// One download, as Downloads shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub key: crate::cache::JobKey,
    pub kind: crate::cache::JobKind,
    pub label: String,
    /// Downloading right now, rather than waiting.
    pub active: bool,
    pub done: u64,
    pub total: Option<u64>,
    pub paused: bool,
    pub error: Option<String>,
}

impl JobRow {
    /// "41% of 34 MB", "waiting · 34 MB", "paused", "failed: ...".
    pub fn state(&self) -> String {
        if self.active {
            return match self.total {
                Some(t) if t > 0 => {
                    format!("{}% of {}", (self.done * 100 / t).min(100), fmt_bytes(t))
                }
                _ => format!("{} so far", fmt_bytes(self.done)),
            };
        }
        if let Some(e) = &self.error {
            return format!("failed, trying again later: {e}");
        }
        let size = self.total.map(|t| format!(" · {}", fmt_bytes(t)));
        format!(
            "{}{}",
            if self.paused { "paused" } else { "waiting" },
            size.unwrap_or_default()
        )
    }
}

/// "512 KB", "34 MB", "3.2 GB".
pub fn fmt_bytes(b: u64) -> String {
    const K: f64 = 1024.0;
    let f = b as f64;
    if f >= K * K * K {
        format!("{:.1} GB", f / (K * K * K))
    } else if f >= K * K {
        format!("{:.0} MB", f / (K * K))
    } else if b > 0 {
        format!("{:.0} KB", (f / K).max(1.0))
    } else {
        "0 MB".into()
    }
}

fn songs(n: usize) -> String {
    format!("{n} song{}", if n == 1 { "" } else { "s" })
}

impl Item {
    pub fn selectable(&self) -> bool {
        !matches!(self, Item::Header(_) | Item::Info { .. })
    }

    /// The server songs it stands for, as a pin would keep them.
    pub fn pin(&self, mode: ArtistMode) -> Option<crate::cache::Pin> {
        use crate::cache::{Pin, PinKind};
        use std::os::unix::ffi::OsStrExt;
        let path = |p: &std::path::Path| p.as_os_str().as_bytes().to_vec();
        Some(match self {
            Item::Album(a) => Pin::new(PinKind::Album, a.key.clone(), a.name.clone()),
            Item::Artist(a) => Pin::new(
                match mode {
                    ArtistMode::AlbumArtist => PinKind::Artist,
                    ArtistMode::TrackArtist => PinKind::TrackArtist,
                },
                a.name.clone(),
                a.name.clone(),
            ),
            Item::Playlist(p) => Pin::new(PinKind::Playlist, p.id.to_string(), p.name.clone()),
            Item::Dir { path: p, name } => Pin::new(PinKind::Folder, path(p), name.clone()),
            Item::Root { path: p, .. } => Pin::new(
                PinKind::Folder,
                path(p),
                crate::remote::server_of(p)
                    .map(str::to_string)
                    .unwrap_or_else(|| p.display().to_string()),
            ),
            Item::Track(t) => Pin::new(
                PinKind::Song,
                path(&t.path),
                match &t.artist {
                    Some(a) => format!("{} · {a}", t.title),
                    None => t.title.clone(),
                },
            ),
            Item::Pinned(p) => p.pin.clone(),
            Item::Mix(m) => Pin::new(PinKind::Mix, m.id.to_string(), m.name.clone()),
            _ => return None,
        })
    }

    /// Identity used to keep the selection on the same row after a refresh.
    pub fn key(&self) -> String {
        match self {
            Item::Dir { path, .. } | Item::Root { path, .. } => format!("d:{}", path.display()),
            Item::Artist(a) => format!("a:{}", a.name),
            Item::Album(a) => format!("al:{}", a.key),
            Item::Track(t) => format!("t:{}", t.path.display()),
            Item::Missing { uri } => format!("m:{}", String::from_utf8_lossy(uri)),
            Item::Genre(g) => format!("g:{}", g.norm),
            Item::Playlist(p) => format!("p:{}", p.id),
            Item::Station { station, .. } => format!("s:{}", station.url),
            Item::Podcast(p) => format!("pc:{}", p.id),
            Item::Episode(e) => format!("e:{}", e.id),
            Item::Header(h) => format!("h:{h}"),
            Item::Info { label, .. } => format!("i:{label}"),
            Item::Setting(s) => format!("s:{:?}", s.key),
            Item::Pinned(p) => format!("pin:{}", p.pin.id),
            Item::Mix(m) => format!("mix:{}", m.id),
            Item::Job(j) => format!("job:{:?}", j.key),
            Item::Action { action, .. } => format!("act:{action:?}"),
        }
    }
}

pub struct Level {
    pub title: String,
    pub source: Source,
    pub items: Vec<Item>,
    pub selected: usize,
    pub offset: usize,
}

impl Level {
    pub fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    /// First selectable row at or after `from`, searching in direction `dir`.
    pub fn snap(&self, from: usize, dir: isize) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        let mut i = from.min(self.items.len() - 1) as isize;
        while (0..self.items.len() as isize).contains(&i) {
            if self.items[i as usize].selectable() {
                return i as usize;
            }
            i += dir;
        }
        // Nothing selectable that way; try the other.
        let mut i = from.min(self.items.len() - 1) as isize;
        while (0..self.items.len() as isize).contains(&i) {
            if self.items[i as usize].selectable() {
                return i as usize;
            }
            i -= dir;
        }
        from.min(self.items.len() - 1)
    }
}

/// Per-section view settings that survive leaving and coming back.
#[derive(Debug, Clone, Default)]
pub struct ViewPrefs {
    pub artist_mode: ArtistMode,
    pub album_sort: AlbumSort,
    pub song_sort: SongSort,
    pub song_desc: bool,
    /// Genre level shows songs instead of albums.
    pub genre_songs: bool,
}

/// What the Settings view is built from.
pub struct SettingsState<'a> {
    pub config: &'a crate::config::Config,
    pub info: &'a super::settings::SettingsInfo,
    pub devices: &'a [crate::audio::output::exclusive::HwDevice],
    pub podcast_tempo: f64,
    pub podcast_trim: bool,
    /// A whole-library loudness run is going; None without the library.
    pub measuring: Option<bool>,
    pub lastfm: Option<crate::scrobble::ScrobbleStatus>,
    pub outputs: &'a [crate::audio::output::OutDevice],
}

/// Everything a view can draw from besides the library itself.
pub struct Ctx<'a> {
    pub lib: Option<&'a Library>,
    pub prefs: &'a ViewPrefs,
    pub settings: SettingsState<'a>,
    pub stations: &'a [Station],
    /// Last test result per station URL.
    pub checks: &'a HashMap<String, String>,
    /// Music servers as they stand: their stations, their status.
    pub servers: &'a [crate::remote::ServerStatus],
    /// The cache, for Downloads.
    pub cache: Option<&'a crate::cache::Status>,
    pub eq: &'a super::eq::EqState,
}

/// Queue entry for one of your stations.
pub fn station_info(s: &Station) -> TrackInfo {
    TrackInfo {
        kind: MediaKind::Radio,
        url: Some(s.url.clone()),
        title: Some(s.name.clone()),
        ..Default::default()
    }
}

pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Human-readable total length for albums: "42 min", "1 h 12 min".
pub fn fmt_length(d: Duration) -> String {
    if d.as_secs() < 60 {
        return fmt_duration(d);
    }
    let m = (d.as_secs() + 30) / 60;
    if m >= 60 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

const NO_FOLDERS: &str = "No library folders yet. Add one with --add-folder.";

/// Build the rows for a source. Queue rows come from the player, not the
/// library, so the Queue section returns nothing here.
pub fn build(ctx: &Ctx, source: &Source) -> Result<(String, Vec<Item>), String> {
    let (lib, prefs) = (ctx.lib, ctx.prefs);
    let none = || Err(NO_FOLDERS.to_string());
    let err = |e: crate::library::LibError| e.to_string();
    match source {
        Source::Section(Section::Radio) => {
            let mut items: Vec<Item> = ctx
                .stations
                .iter()
                .enumerate()
                .map(|(index, s)| Item::Station {
                    index,
                    station: s.clone(),
                    check: ctx.checks.get(&s.url).cloned(),
                    server: None,
                })
                .collect();
            let theirs: Vec<Item> = ctx
                .servers
                .iter()
                .flat_map(|srv| {
                    srv.stations.iter().map(|st| Item::Station {
                        index: usize::MAX,
                        station: Station {
                            name: st.name.clone(),
                            url: st.url.clone(),
                        },
                        check: ctx
                            .checks
                            .get(&st.url)
                            .cloned()
                            .or_else(|| Some(format!("on {}", srv.name))),
                        server: Some(srv.name.clone()),
                    })
                })
                .collect();
            if !theirs.is_empty() {
                items.push(Item::Header("On your servers"));
                items.extend(theirs);
            }
            Ok(("Radio".into(), items))
        }
        Source::Section(Section::Podcasts) => {
            let Some(lib) = lib else {
                return Err("Podcasts need the library database (--no-library is set).".into());
            };
            let rows = lib.podcasts().map_err(err)?;
            Ok((
                "Podcasts".into(),
                rows.into_iter().map(Item::Podcast).collect(),
            ))
        }
        Source::Podcast { id, title } => {
            let Some(lib) = lib else { return none() };
            let eps = lib.episodes(*id).map_err(err)?;
            Ok((title.clone(), eps.into_iter().map(Item::Episode).collect()))
        }
        Source::Section(Section::Settings) => {
            let st = &ctx.settings;
            let rows = super::settings::rows(
                st.config,
                st.info,
                st.podcast_tempo,
                st.podcast_trim,
                st.devices,
                ctx.servers,
                super::settings::Live {
                    measuring: st.measuring,
                    lastfm: st.lastfm.clone(),
                    outputs: st.outputs.to_vec(),
                },
            );
            let mut items = Vec::new();
            let mut last = None;
            for (group, setting) in rows {
                if group != last
                    && let Some(g) = group
                {
                    items.push(Item::Header(g));
                }
                last = group;
                items.push(Item::Setting(setting));
            }
            Ok(("Settings".into(), items))
        }
        Source::Section(Section::Eq) => {
            let greyed =
                ctx.settings.config.output.bit_perfect == crate::audio::output::BitPerfect::On;
            let mut items = Vec::new();
            if greyed {
                items.push(Item::Info {
                    label: "Bit-perfect is on".into(),
                    value: "the EQ is skipped while it is. Set bit-perfect to Off or Auto in Settings to use it.".into(),
                });
            }
            let mut last = None;
            for (group, setting) in super::eq::eq_rows(ctx.eq, greyed) {
                if group != last
                    && let Some(g) = group
                {
                    items.push(Item::Header(g));
                }
                last = group;
                items.push(Item::Setting(setting));
            }
            Ok(("EQ".into(), items))
        }
        Source::Section(Section::Downloads) => {
            let eps = lib
                .and_then(|l| l.downloaded_episodes().ok())
                .unwrap_or_default();
            let mut items = Vec::new();
            if let Some(st) = ctx.cache {
                downloads(st, &mut items);
            }
            if !eps.is_empty() {
                items.push(Item::Header("Downloaded episodes"));
                items.extend(eps.into_iter().map(Item::Episode));
            } else if ctx.cache.is_none() {
                items.push(Item::Info {
                    label: "Podcast episodes".into(),
                    value: "none downloaded. Press w on an episode to keep it.".into(),
                });
            }
            Ok(("Downloads".into(), items))
        }
        Source::Section(Section::Queue) => Ok(("Queue".into(), Vec::new())),
        _ => {
            let Some(lib) = lib else { return none() };
            build_library(lib, prefs, source)
        }
    }
}

/// The cache's part of Downloads: how full it is, what's downloading,
/// and what's pinned.
fn downloads(st: &crate::cache::Status, items: &mut Vec<Item>) {
    items.push(Item::Header("Storage"));
    items.push(Item::Info {
        label: "Cache".into(),
        value: if st.cap == 0 {
            "off: songs stream and aren't kept".into()
        } else {
            format!(
                "{} of {} · {}",
                fmt_bytes(st.cache_bytes),
                fmt_bytes(st.cap),
                songs(st.cache_files)
            )
        },
    });
    items.push(Item::Info {
        label: "Pinned".into(),
        value: format!(
            "{} · {}",
            fmt_bytes(st.offline_bytes),
            songs(st.offline_files)
        ),
    });
    if st.metered_hold {
        items.push(Item::Info {
            label: "Metered".into(),
            value: "prefetch and pinned songs wait for a connection that isn't metered".into(),
        });
    }
    items.push(if st.paused {
        Item::Action {
            action: ListAction::ResumeDownloads,
            label: "Resume downloads".into(),
        }
    } else {
        Item::Action {
            action: ListAction::PauseDownloads,
            label: "Pause downloads".into(),
        }
    });
    if st.cache_files > 0 {
        items.push(Item::Action {
            action: ListAction::ClearCache,
            label: "Clear the cache".into(),
        });
    }
    let mut jobs: Vec<JobRow> = Vec::new();
    if let Some(a) = &st.active {
        jobs.push(JobRow {
            key: a.key.clone(),
            kind: a.kind,
            label: a.label.clone(),
            active: true,
            done: a.done,
            total: a.total,
            paused: false,
            error: None,
        });
    }
    jobs.extend(st.queue.iter().map(|q| JobRow {
        key: q.key.clone(),
        kind: q.kind,
        label: q.label.clone(),
        active: false,
        done: 0,
        total: q.size,
        paused: q.paused,
        error: q.error.clone(),
    }));
    if !jobs.is_empty() {
        items.push(Item::Header("Downloading"));
        let shown = st.queue.len();
        items.extend(jobs.into_iter().map(Item::Job));
        if st.waiting > shown {
            items.push(Item::Info {
                label: "More".into(),
                value: format!("{} more waiting", st.waiting - shown),
            });
        }
    }
    if !st.pins.is_empty() {
        items.push(Item::Header("Pinned for offline"));
        items.extend(st.pins.iter().cloned().map(Item::Pinned));
    }
}

/// Rows for the views that come straight from the library index.
fn build_library(
    lib: &Library,
    prefs: &ViewPrefs,
    source: &Source,
) -> Result<(String, Vec<Item>), String> {
    let err = |e: crate::library::LibError| e.to_string();
    match source {
        Source::Section(Section::MostListened) => {
            let mut items = Vec::new();
            let songs = lib.top_tracks(5).map_err(err)?;
            if !songs.is_empty() {
                items.push(Item::Header("Songs"));
                items.extend(songs.into_iter().map(Item::Track));
            }
            let albums = lib.top_albums(5).map_err(err)?;
            if !albums.is_empty() {
                items.push(Item::Header("Albums"));
                items.extend(albums.into_iter().map(Item::Album));
            }
            let artists = lib.top_artists(prefs.artist_mode, 5).map_err(err)?;
            if !artists.is_empty() {
                items.push(Item::Header("Artists"));
                items.extend(artists.into_iter().map(Item::Artist));
            }
            Ok(("Most listened".into(), items))
        }
        Source::Section(Section::TopAlbums) => {
            let rows = lib.top_albums(10).map_err(err)?;
            Ok((
                "Top albums".into(),
                rows.into_iter().map(Item::Album).collect(),
            ))
        }
        Source::Section(Section::TopArtists) => {
            let rows = lib.top_artists(prefs.artist_mode, 10).map_err(err)?;
            Ok((
                "Top artists".into(),
                rows.into_iter().map(Item::Artist).collect(),
            ))
        }
        Source::Section(Section::Folders) => {
            let roots = lib.sources().map_err(err)?;
            if roots.is_empty() {
                return Err(NO_FOLDERS.into());
            }
            Ok((
                "Folders".into(),
                roots
                    .into_iter()
                    .map(|s| Item::Root {
                        path: s.root,
                        tracks: s.tracks,
                    })
                    .collect(),
            ))
        }
        Source::Folder(dir) => {
            let entries = lib.folder(dir).map_err(err)?;
            let title = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string());
            Ok((
                title,
                entries
                    .into_iter()
                    .map(|e| match e {
                        DirEntry::Dir { path, name } => Item::Dir { path, name },
                        DirEntry::Track(t) => Item::Track(t),
                    })
                    .collect(),
            ))
        }
        Source::Section(Section::Artists) => {
            let rows = lib.artists(prefs.artist_mode).map_err(err)?;
            let title = match prefs.artist_mode {
                ArtistMode::AlbumArtist => "Artists (album artist)",
                ArtistMode::TrackArtist => "Artists (track artist)",
            };
            Ok((title.into(), rows.into_iter().map(Item::Artist).collect()))
        }
        Source::ArtistAlbums(name) => {
            let albums = lib.artist_albums(name, prefs.artist_mode).map_err(err)?;
            Ok((name.clone(), albums.into_iter().map(Item::Album).collect()))
        }
        Source::Section(Section::Albums) => {
            let rows = lib.albums(prefs.album_sort).map_err(err)?;
            Ok((
                format!("Albums by {}", prefs.album_sort.label()),
                rows.into_iter().map(Item::Album).collect(),
            ))
        }
        Source::Album { key } => {
            let title = lib
                .album(key)
                .map_err(err)?
                .map(|a| format!("{} · {}", a.name, a.artist))
                .unwrap_or_else(|| "Album".into());
            let tracks = lib.album_tracks(key).map_err(err)?;
            Ok((title, tracks.into_iter().map(Item::Track).collect()))
        }
        Source::Section(Section::Songs) => {
            let rows = lib.songs(prefs.song_sort, prefs.song_desc).map_err(err)?;
            let dir = if prefs.song_desc { ", reversed" } else { "" };
            Ok((
                format!("Songs by {}{dir}", prefs.song_sort.label()),
                rows.into_iter().map(Item::Track).collect(),
            ))
        }
        Source::Section(Section::Genres) => {
            let rows = lib.genres().map_err(err)?;
            Ok(("Genres".into(), rows.into_iter().map(Item::Genre).collect()))
        }
        Source::Genre { norm, name } => {
            if prefs.genre_songs {
                let t = lib.genre_tracks(norm).map_err(err)?;
                Ok((
                    format!("{name} · songs"),
                    t.into_iter().map(Item::Track).collect(),
                ))
            } else {
                let a = lib.genre_albums(norm).map_err(err)?;
                Ok((
                    format!("{name} · albums"),
                    a.into_iter().map(Item::Album).collect(),
                ))
            }
        }
        Source::Section(Section::Playlists) => {
            let rows = lib.playlists().map_err(err)?;
            let mixes = lib.saved_mixes().map_err(err)?;
            let mut items = Vec::new();
            if !mixes.is_empty() && !rows.is_empty() {
                items.push(Item::Header("Playlists"));
            }
            items.extend(rows.into_iter().map(Item::Playlist));
            if !mixes.is_empty() {
                items.push(Item::Header("Saved mixes"));
                items.extend(mixes.into_iter().map(Item::Mix));
            }
            Ok(("Playlists".into(), items))
        }
        Source::Playlist { id, name } => {
            let items = lib.playlist_items(*id).map_err(err)?;
            Ok((
                name.clone(),
                items
                    .into_iter()
                    .map(|(uri, t)| match t {
                        Some(t) => Item::Track(t),
                        None => Item::Missing { uri },
                    })
                    .collect(),
            ))
        }
        Source::Search(q) => {
            let res = lib.search(q, 200).map_err(err)?;
            let mut items = Vec::new();
            if !res.artists.is_empty() {
                items.push(Item::Header("Artists"));
                items.extend(res.artists.into_iter().map(Item::Artist));
            }
            if !res.albums.is_empty() {
                items.push(Item::Header("Albums"));
                items.extend(res.albums.into_iter().map(Item::Album));
            }
            if !res.tracks.is_empty() {
                items.push(Item::Header("Songs"));
                items.extend(res.tracks.into_iter().map(Item::Track));
            }
            Ok((format!("Search: {q}"), items))
        }
        // Handled by build() before it gets here.
        Source::Section(
            Section::Radio
            | Section::Podcasts
            | Section::Settings
            | Section::Eq
            | Section::Downloads
            | Section::Queue,
        )
        | Source::Podcast { .. } => Ok((String::new(), Vec::new())),
    }
}

/// The level you get by pressing Enter on an item, if it opens one.
pub fn open(item: &Item) -> Option<Source> {
    match item {
        Item::Dir { path, .. } | Item::Root { path, .. } => Some(Source::Folder(path.clone())),
        Item::Artist(a) => Some(Source::ArtistAlbums(a.name.clone())),
        Item::Album(a) => Some(Source::Album { key: a.key.clone() }),
        Item::Genre(g) => Some(Source::Genre {
            norm: g.norm.clone(),
            name: g.name.clone(),
        }),
        Item::Playlist(p) => Some(Source::Playlist {
            id: p.id,
            name: p.name.clone(),
        }),
        Item::Podcast(p) => Some(Source::Podcast {
            id: p.id,
            title: p.title.clone(),
        }),
        Item::Pinned(p) => {
            use crate::cache::PinKind;
            use std::os::unix::ffi::OsStrExt;
            let path = || PathBuf::from(std::ffi::OsStr::from_bytes(&p.pin.key));
            match p.pin.kind {
                PinKind::Album => Some(Source::Album {
                    key: p.pin.key_str(),
                }),
                PinKind::Artist | PinKind::TrackArtist => {
                    Some(Source::ArtistAlbums(p.pin.key_str()))
                }
                PinKind::Playlist => p.pin.key_str().parse().ok().map(|id| Source::Playlist {
                    id,
                    name: p.pin.name.clone(),
                }),
                PinKind::Folder => Some(Source::Folder(path())),
                PinKind::Song => path().parent().map(|d| Source::Folder(d.to_path_buf())),
                // Saved mixes are listed in Playlists.
                PinKind::Mix => Some(Source::Section(Section::Playlists)),
            }
        }
        _ => None,
    }
}

/// What an item queues as. Covers stations and episodes as well as
/// library tracks.
pub fn infos_of(lib: Option<&Library>, item: &Item, prefs: &ViewPrefs) -> Vec<TrackInfo> {
    match item {
        Item::Station { station, .. } => vec![station_info(station)],
        Item::Episode(e) => vec![e.to_info()],
        Item::Podcast(p) => lib
            .and_then(|l| l.episodes(p.id).ok())
            .map(|eps| {
                // Oldest unplayed first: the order you'd catch up in.
                let mut v: Vec<_> = eps.into_iter().filter(|e| !e.played).collect();
                v.reverse();
                v.iter().map(EpisodeRow::to_info).collect()
            })
            .unwrap_or_default(),
        _ => lib
            .map(|l| {
                tracks_of(l, item, prefs)
                    .iter()
                    .map(TrackRow::to_info)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Every track an item stands for, in play order: a folder's whole tree,
/// an artist's albums, an album, a genre, a playlist.
pub fn tracks_of(lib: &Library, item: &Item, prefs: &ViewPrefs) -> Vec<TrackRow> {
    let r = match item {
        Item::Track(t) => Ok(vec![t.clone()]),
        Item::Dir { path, .. } | Item::Root { path, .. } => lib.tracks_under(path),
        Item::Artist(a) => lib.artist_tracks(&a.name, prefs.artist_mode),
        Item::Album(a) => lib.album_tracks(&a.key),
        Item::Genre(g) => {
            if prefs.genre_songs {
                lib.genre_tracks(&g.norm)
            } else {
                lib.genre_albums(&g.norm).map(|albums| {
                    albums
                        .iter()
                        .flat_map(|a| lib.album_tracks(&a.key).unwrap_or_default())
                        .collect()
                })
            }
        }
        Item::Playlist(p) => lib
            .playlist_items(p.id)
            .map(|v| v.into_iter().filter_map(|(_, t)| t).collect()),
        Item::Pinned(p) => crate::cache::store::resolve(lib, &p.pin),
        Item::Mix(m) => Ok(crate::mix::resolve_saved(lib, m)),
        _ => Ok(Vec::new()),
    };
    r.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(name: &str) -> Item {
        Item::Artist(ArtistRow {
            name: name.into(),
            albums: 1,
            tracks: 1,
            plays: 0,
        })
    }

    fn level(items: Vec<Item>) -> Level {
        Level {
            title: String::new(),
            source: Source::Section(Section::Songs),
            items,
            selected: 0,
            offset: 0,
        }
    }

    #[test]
    fn selection_skips_headers() {
        let l = level(vec![
            Item::Header("Artists"),
            artist("A"),
            Item::Header("Albums"),
            artist("B"),
        ]);
        assert_eq!(l.snap(0, 1), 1);
        assert_eq!(l.snap(2, 1), 3);
        assert_eq!(l.snap(2, -1), 1);
        // Nothing selectable below the last header: fall back upward.
        let l = level(vec![artist("A"), Item::Header("Songs")]);
        assert_eq!(l.snap(1, 1), 0);
    }

    #[test]
    fn lengths() {
        assert_eq!(fmt_length(Duration::from_secs(20)), "0:20");
        assert_eq!(fmt_length(Duration::from_secs(42 * 60)), "42 min");
        assert_eq!(fmt_length(Duration::from_secs(72 * 60 + 10)), "1 h 12 min");
    }
}
