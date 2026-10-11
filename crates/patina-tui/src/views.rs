//! What each view shows, as plain data: the sidebar sections, a stack of
//! levels you drill into, and the rows in each level. Drawing lives in
//! ui.rs and keys in app.rs; this file only asks the library questions.

use std::path::PathBuf;
use std::time::Duration;

use std::collections::HashMap;

use patina_core::config::Station;
use patina_core::library::podcasts::{EpisodeRow, PodcastRow};
use patina_core::library::{
    AlbumRow, AlbumSort, ArtistMode, DirEntry, GenreRow, Library, PlaylistRow, SongSort, TrackRow,
};
use patina_core::track::{MediaKind, TrackInfo};

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
    Artist {
        name: String,
        albums: i64,
        tracks: i64,
    },
    Album(AlbumRow),
    Track(TrackRow),
    /// A playlist item whose file isn't in the library any more.
    Missing {
        uri: Vec<u8>,
    },
    Genre(GenreRow),
    Playlist(PlaylistRow),
    /// One of your stations, by its place in the config list.
    Station {
        index: usize,
        station: Station,
        check: Option<String>,
    },
    Podcast(PodcastRow),
    Episode(EpisodeRow),
    /// Group label inside search results. Not selectable.
    Header(&'static str),
    /// Label and value, for Settings and Downloads.
    Info {
        label: String,
        value: String,
    },
}

impl Item {
    pub fn selectable(&self) -> bool {
        !matches!(self, Item::Header(_) | Item::Info { .. })
    }

    /// Identity used to keep the selection on the same row after a refresh.
    pub fn key(&self) -> String {
        match self {
            Item::Dir { path, .. } | Item::Root { path, .. } => format!("d:{}", path.display()),
            Item::Artist { name, .. } => format!("a:{name}"),
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

pub struct Settings {
    pub lines: Vec<(String, String)>,
}

/// Everything a view can draw from besides the library itself.
pub struct Ctx<'a> {
    pub lib: Option<&'a Library>,
    pub prefs: &'a ViewPrefs,
    pub settings: &'a Settings,
    pub stations: &'a [Station],
    /// Last test result per station URL.
    pub checks: &'a HashMap<String, String>,
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

/// Build the rows for a source. Queue rows come from the player, not the
/// library, so the Queue section returns nothing here.
pub fn build(ctx: &Ctx, source: &Source) -> Result<(String, Vec<Item>), String> {
    let (lib, prefs, settings) = (ctx.lib, ctx.prefs, ctx.settings);
    let none = || Err("No library folders yet. Add one with --add-folder.".to_string());
    let err = |e: patina_core::library::LibError| e.to_string();
    match source {
        Source::Section(Section::Radio) => Ok((
            "Radio".into(),
            ctx.stations
                .iter()
                .enumerate()
                .map(|(index, s)| Item::Station {
                    index,
                    station: s.clone(),
                    check: ctx.checks.get(&s.url).cloned(),
                })
                .collect(),
        )),
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
        Source::Section(Section::Settings) => Ok((
            "Settings".into(),
            settings
                .lines
                .iter()
                .map(|(l, v)| Item::Info {
                    label: l.clone(),
                    value: v.clone(),
                })
                .collect(),
        )),
        Source::Section(Section::Downloads) => {
            let eps = lib
                .and_then(|l| l.downloaded_episodes().ok())
                .unwrap_or_default();
            let mut items = Vec::new();
            if eps.is_empty() {
                items.push(Item::Info {
                    label: "Podcast episodes".into(),
                    value: "none downloaded. Press w on an episode to keep it.".into(),
                });
                items.push(Item::Info {
                    label: "Track cache".into(),
                    value: "arrives with Subsonic streaming".into(),
                });
            } else {
                items.push(Item::Header("Downloaded episodes"));
                items.extend(eps.into_iter().map(Item::Episode));
            }
            Ok(("Downloads".into(), items))
        }
        Source::Section(Section::Queue) => Ok(("Queue".into(), Vec::new())),
        _ if lib.is_none() => none(),
        Source::Section(Section::Folders) => {
            let lib = lib.unwrap();
            let roots = lib.sources().map_err(err)?;
            if roots.is_empty() {
                return none();
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
            let entries = lib.unwrap().folder(dir).map_err(err)?;
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
            let rows = lib.unwrap().artists(prefs.artist_mode).map_err(err)?;
            let title = match prefs.artist_mode {
                ArtistMode::AlbumArtist => "Artists (album artist)",
                ArtistMode::TrackArtist => "Artists (track artist)",
            };
            Ok((
                title.into(),
                rows.into_iter()
                    .map(|a| Item::Artist {
                        name: a.name,
                        albums: a.albums,
                        tracks: a.tracks,
                    })
                    .collect(),
            ))
        }
        Source::ArtistAlbums(name) => {
            let lib = lib.unwrap();
            let albums = lib.artist_albums(name, prefs.artist_mode).map_err(err)?;
            Ok((name.clone(), albums.into_iter().map(Item::Album).collect()))
        }
        Source::Section(Section::Albums) => {
            let rows = lib.unwrap().albums(prefs.album_sort).map_err(err)?;
            Ok((
                format!("Albums by {}", prefs.album_sort.label()),
                rows.into_iter().map(Item::Album).collect(),
            ))
        }
        Source::Album { key } => {
            let lib = lib.unwrap();
            let title = lib
                .album(key)
                .map_err(err)?
                .map(|a| format!("{} · {}", a.name, a.artist))
                .unwrap_or_else(|| "Album".into());
            let tracks = lib.album_tracks(key).map_err(err)?;
            Ok((title, tracks.into_iter().map(Item::Track).collect()))
        }
        Source::Section(Section::Songs) => {
            let rows = lib
                .unwrap()
                .songs(prefs.song_sort, prefs.song_desc)
                .map_err(err)?;
            let dir = if prefs.song_desc { ", reversed" } else { "" };
            Ok((
                format!("Songs by {}{dir}", prefs.song_sort.label()),
                rows.into_iter().map(Item::Track).collect(),
            ))
        }
        Source::Section(Section::Genres) => {
            let rows = lib.unwrap().genres().map_err(err)?;
            Ok(("Genres".into(), rows.into_iter().map(Item::Genre).collect()))
        }
        Source::Genre { norm, name } => {
            let lib = lib.unwrap();
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
            let rows = lib.unwrap().playlists().map_err(err)?;
            Ok((
                "Playlists".into(),
                rows.into_iter().map(Item::Playlist).collect(),
            ))
        }
        Source::Playlist { id, name } => {
            let items = lib.unwrap().playlist_items(*id).map_err(err)?;
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
            let res = lib.unwrap().search(q, 200).map_err(err)?;
            let mut items = Vec::new();
            if !res.artists.is_empty() {
                items.push(Item::Header("Artists"));
                items.extend(res.artists.into_iter().map(|a| Item::Artist {
                    name: a.name,
                    albums: a.albums,
                    tracks: a.tracks,
                }));
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
    }
}

/// The level you get by pressing Enter on an item, if it opens one.
pub fn open(item: &Item) -> Option<Source> {
    match item {
        Item::Dir { path, .. } | Item::Root { path, .. } => Some(Source::Folder(path.clone())),
        Item::Artist { name, .. } => Some(Source::ArtistAlbums(name.clone())),
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
        Item::Artist { name, .. } => lib.artist_tracks(name, prefs.artist_mode),
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
        _ => Ok(Vec::new()),
    };
    r.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Item::Artist {
                name: "A".into(),
                albums: 1,
                tracks: 1,
            },
            Item::Header("Albums"),
            Item::Artist {
                name: "B".into(),
                albums: 1,
                tracks: 1,
            },
        ]);
        assert_eq!(l.snap(0, 1), 1);
        assert_eq!(l.snap(2, 1), 3);
        assert_eq!(l.snap(2, -1), 1);
        // Nothing selectable below the last header: fall back upward.
        let l = level(vec![
            Item::Artist {
                name: "A".into(),
                albums: 1,
                tracks: 1,
            },
            Item::Header("Songs"),
        ]);
        assert_eq!(l.snap(1, 1), 0);
    }

    #[test]
    fn lengths() {
        assert_eq!(fmt_length(Duration::from_secs(20)), "0:20");
        assert_eq!(fmt_length(Duration::from_secs(42 * 60)), "42 min");
        assert_eq!(fmt_length(Duration::from_secs(72 * 60 + 10)), "1 h 12 min");
    }
}
