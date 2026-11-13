//! What a car (Android Auto) can browse, and playing what's picked there.
//!
//! The car asks for one level at a time by id. [`children`] answers from
//! the library alone, so it can run on the thread Android asks from; the
//! play request it makes comes back through [`Browser::auto_play`], on the
//! app's own thread, so it plays the way a tap in the app would.
//!
//! The top level is the car's tabs: Home, Artists, Albums, Playlists and
//! Folders. A car that shows only four tabs asks for `root4`, which folds
//! Home into the top of Playlists. Long lists of artists or albums come
//! as one folder per first letter, since a car shows only so many rows.
//!
//! Ids: `root`, `root4`, `home`, `artists`, `albums`, `playlists`,
//! `lists` (Home and Playlists together), `folders`, `favorites`,
//! `smart`, `history`, `mixes`, `recent`, `shuffle`, `artists:<letter>`,
//! `albums:<letter>`, and for things in them `fav:<id>`, `pl:<id>`,
//! `sp:<id>`, `mix:<id>`, `album:<key>`, `artist:<name>`,
//! `dir:<hex of the path>`. A song is its list's id, a `|`, and its place
//! in the list, so playing it plays the list from there.

use std::path::PathBuf;

use super::{Browser, Item, ViewPrefs, tracks_of};
use crate::library::marks::FavKind;
use crate::library::{AlbumSort, ArtistMode, ArtistRow, Library, TrackRow};

/// A list gets at most this many rows in the car.
pub const MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutoItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub browsable: bool,
    pub playable: bool,
    /// A heading the car shows it under, with the rows after it that
    /// share it. Empty for none.
    pub group: String,
    /// Show what's in it as a grid of covers rather than a list.
    pub grid: bool,
    /// The picture beside it, or on its tile in a grid.
    pub art: Option<CarArt>,
}

/// A row's picture in the car.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarArt {
    /// One of the app's own icons, by its drawable name.
    Icon(&'static str),
    /// A song's cover, which stands for its album.
    Cover(PathBuf),
}

fn icon(mut it: AutoItem, name: &'static str) -> AutoItem {
    it.art = Some(CarArt::Icon(name));
    it
}

fn folder(id: &str, title: &str, subtitle: impl Into<String>) -> AutoItem {
    AutoItem {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        browsable: true,
        ..Default::default()
    }
}

fn grouped(mut v: Vec<AutoItem>, group: &str) -> Vec<AutoItem> {
    for it in &mut v {
        it.group = group.to_string();
    }
    v
}

/// The letter a name files under in a long list: A to Z, or # for the rest.
fn letter(name: &str) -> char {
    match name.trim().chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        Some(c) if c.is_alphabetic() => c.to_uppercase().next().unwrap_or('#'),
        _ => '#',
    }
}

/// One folder per first letter, with how many are in it.
fn letter_folders<'a>(
    prefix: &str,
    names: impl Iterator<Item = &'a str>,
    noun: &str,
) -> Vec<AutoItem> {
    let mut counts: Vec<(char, usize)> = Vec::new();
    for n in names {
        let l = letter(n);
        match counts.iter_mut().find(|(c, _)| *c == l) {
            Some((_, k)) => *k += 1,
            None => counts.push((l, 1)),
        }
    }
    counts.sort_by_key(|(c, _)| (*c == '#', *c));
    counts
        .into_iter()
        .map(|(c, k)| {
            folder(
                &format!("{prefix}:{c}"),
                &c.to_string(),
                format!("{k} {noun}{}", if k == 1 { "" } else { "s" }),
            )
        })
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn dir_id(p: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!("dir:{}", hex(p.as_os_str().as_bytes()))
}

fn dir_of(id: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let b = unhex(id.strip_prefix("dir:")?)?;
    Some(PathBuf::from(std::ffi::OsString::from_vec(b)))
}

fn songs_line(n: usize) -> String {
    format!("{n} song{}", if n == 1 { "" } else { "s" })
}

fn track_items(list: &str, tracks: &[TrackRow]) -> Vec<AutoItem> {
    tracks
        .iter()
        .take(MAX)
        .enumerate()
        .map(|(i, t)| AutoItem {
            id: format!("{list}|{i}"),
            title: t.title.clone(),
            subtitle: [t.artist.as_deref(), t.album.as_deref()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            playable: true,
            ..Default::default()
        })
        .collect()
}

/// What a favorite stands for, as a row.
fn favorite_item(lib: &Library, kind: FavKind, key: &[u8], label: &str) -> Option<Item> {
    let key_str = String::from_utf8_lossy(key).into_owned();
    Some(match kind {
        FavKind::Folder => {
            use std::os::unix::ffi::OsStringExt;
            Item::Dir {
                path: PathBuf::from(std::ffi::OsString::from_vec(key.to_vec())),
                name: label.to_string(),
            }
        }
        FavKind::Album => Item::Album(lib.album(&key_str).ok().flatten()?),
        FavKind::Artist => Item::Artist(ArtistRow {
            name: key_str,
            albums: 0,
            tracks: 0,
            plays: 0,
        }),
    })
}

/// The songs a list id stands for, in play order.
fn list_tracks(lib: &Library, id: &str) -> Vec<TrackRow> {
    let prefs = ViewPrefs::default();
    let num = |p: &str| id.strip_prefix(p).and_then(|n| n.parse::<i64>().ok());
    if id == "history" {
        return lib
            .recent_plays(MAX)
            .map(|v| v.into_iter().map(|p| p.track).collect())
            .unwrap_or_default();
    }
    if let Some(n) = num("pl:") {
        return lib
            .playlist_items(n)
            .map(|v| v.into_iter().filter_map(|(_, t)| t).collect())
            .unwrap_or_default();
    }
    if let Some(n) = num("sp:") {
        return lib.smart_tracks(n).unwrap_or_default();
    }
    if let Some(key) = id.strip_prefix("album:") {
        return lib.album_tracks(key).unwrap_or_default();
    }
    if let Some(name) = id.strip_prefix("artist:") {
        return lib
            .artist_tracks(name, ArtistMode::AlbumArtist)
            .unwrap_or_default();
    }
    if let Some(dir) = dir_of(id) {
        // The folder's own songs, the way it's listed; its subfolders
        // play from their own rows.
        return lib
            .folder(&dir)
            .map(|v| {
                v.into_iter()
                    .filter_map(|e| match e {
                        crate::library::DirEntry::Track(t) => Some(t),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    if let Some(n) = num("fav:")
        && let Some(f) = lib
            .favorites()
            .ok()
            .and_then(|v| v.into_iter().find(|f| f.id == n))
        && let Some(item) = favorite_item(lib, f.kind, &f.key, &f.label)
    {
        return tracks_of(lib, &item, &prefs);
    }
    Vec::new()
}

/// The rows under `parent`.
pub fn children(lib: &Library, parent: &str) -> Vec<AutoItem> {
    match parent {
        "root" => vec![
            folder("home", "Home", ""),
            folder("artists", "Artists", ""),
            folder("albums", "Albums", ""),
            folder("playlists", "Playlists", ""),
            folder("folders", "Folders", ""),
        ],
        // A car with room for four tabs: Home goes at the top of Playlists.
        "root4" => vec![
            folder("artists", "Artists", ""),
            folder("albums", "Albums", ""),
            folder("lists", "Playlists", ""),
            folder("folders", "Folders", ""),
        ],
        "home" => home(lib),
        "lists" => {
            let mut v = grouped(home(lib), "Home");
            v.extend(grouped(all_playlists(lib), "Playlists"));
            v
        }
        "artists" => {
            let rows = lib.artists(ArtistMode::AlbumArtist).unwrap_or_default();
            if rows.len() <= MAX {
                rows.iter().map(artist_item).collect()
            } else {
                letter_folders("artists", rows.iter().map(|a| a.name.as_str()), "artist")
            }
        }
        "albums" => {
            let rows = lib.albums(AlbumSort::Name).unwrap_or_default();
            // A list, with each album's cover beside it, or with a folder
            // per letter; covers come as a grid once a letter's open.
            let mut v = vec![icon(
                AutoItem {
                    grid: true,
                    ..folder("recent", "Recently added", "")
                },
                "ic_car_recent",
            )];
            if rows.len() <= MAX {
                v.extend(rows.iter().map(album_item));
            } else {
                v.extend(
                    letter_folders("albums", rows.iter().map(|a| a.name.as_str()), "album")
                        .into_iter()
                        .map(|f| AutoItem { grid: true, ..f }),
                );
            }
            v
        }
        "recent" => lib
            .albums(AlbumSort::RecentlyAdded)
            .unwrap_or_default()
            .iter()
            .take(MAX)
            .map(album_item)
            .collect(),
        "folders" => {
            let roots = lib.sources().unwrap_or_default();
            // One folder: straight into it, rather than a list of one.
            if let [only] = roots.as_slice() {
                return dir_children(lib, &only.root);
            }
            roots
                .iter()
                .map(|r| {
                    let name = match crate::remote::server_of(&r.root) {
                        Some(s) => s.to_string(),
                        None => r
                            .root
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| r.root.display().to_string()),
                    };
                    icon(
                        folder(&dir_id(&r.root), &name, songs_line(r.tracks as usize)),
                        "ic_car_folder",
                    )
                })
                .collect()
        }
        "playlists" => all_playlists(lib),
        "smart" => smart_rows(lib),
        "favorites" => lib
            .favorites()
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                let (sub, art) = match f.kind {
                    FavKind::Folder => ("Folder", "ic_car_folder"),
                    FavKind::Album => ("Album", "ic_car_album"),
                    FavKind::Artist => ("Artist", "ic_car_artist"),
                };
                icon(folder(&format!("fav:{}", f.id), &f.label, sub), art)
            })
            .collect(),
        "mixes" => lib
            .saved_mixes()
            .unwrap_or_default()
            .into_iter()
            .map(|m| AutoItem {
                id: format!("mix:{}", m.id),
                title: m.name,
                subtitle: match m.kind.as_str() {
                    "genre" => "Genre mix".into(),
                    "artist" => "Artist mix".into(),
                    _ => "Track mix".into(),
                },
                playable: true,
                ..Default::default()
            })
            .collect(),
        id => {
            if let Some(l) = id.strip_prefix("artists:").and_then(|l| l.chars().next()) {
                return lib
                    .artists(ArtistMode::AlbumArtist)
                    .unwrap_or_default()
                    .iter()
                    .filter(|a| letter(&a.name) == l)
                    .take(MAX)
                    .map(artist_item)
                    .collect();
            }
            if let Some(l) = id.strip_prefix("albums:").and_then(|l| l.chars().next()) {
                return lib
                    .albums(AlbumSort::Name)
                    .unwrap_or_default()
                    .iter()
                    .filter(|a| letter(&a.name) == l)
                    .take(MAX)
                    .map(album_item)
                    .collect();
            }

            // A favorite artist or folder lists what's in it; everything
            // else is a list of songs.
            if let Some(n) = id.strip_prefix("fav:").and_then(|n| n.parse::<i64>().ok())
                && let Some(f) = lib
                    .favorites()
                    .ok()
                    .and_then(|v| v.into_iter().find(|f| f.id == n))
            {
                match f.kind {
                    FavKind::Artist => return artist_children(lib, &f.key_str()),
                    FavKind::Folder => return dir_children(lib, &f.path()),
                    FavKind::Album => {}
                }
            }
            if let Some(name) = id.strip_prefix("artist:") {
                return artist_children(lib, name);
            }
            if let Some(dir) = dir_of(id) {
                return dir_children(lib, &dir);
            }
            track_items(id, &list_tracks(lib, id))
        }
    }
}

/// What the Home tab holds: shuffle, then the short lists.
fn home(lib: &Library) -> Vec<AutoItem> {
    let mut v = vec![icon(
        AutoItem {
            id: "shuffle".into(),
            title: "Shuffle everything".into(),
            playable: true,
            ..Default::default()
        },
        "ic_car_shuffle",
    )];
    if lib.favorites().is_ok_and(|f| !f.is_empty()) {
        v.push(icon(folder("favorites", "Favorites", ""), "ic_car_star"));
    }
    v.push(icon(
        folder("history", "Recently played", ""),
        "ic_car_history",
    ));
    v.push(icon(
        AutoItem {
            grid: true,
            ..folder("recent", "Recently added", "")
        },
        "ic_car_recent",
    ));
    if lib.saved_mixes().is_ok_and(|m| !m.is_empty()) {
        v.push(icon(folder("mixes", "Mixes", ""), "ic_car_mix"));
    }
    v
}

/// Hearted songs, your playlists, then smart playlists.
fn all_playlists(lib: &Library) -> Vec<AutoItem> {
    let mut rows = lib.hearted_row().ok().into_iter().collect::<Vec<_>>();
    rows.extend(lib.playlists().unwrap_or_default());
    let mut v: Vec<AutoItem> = rows
        .into_iter()
        .filter(|p| p.items > 0)
        .map(|p| {
            icon(
                folder(
                    &format!("pl:{}", p.id),
                    &p.name,
                    songs_line(p.items as usize),
                ),
                "ic_car_playlist",
            )
        })
        .collect();
    v.extend(smart_rows(lib));
    v
}

fn smart_rows(lib: &Library) -> Vec<AutoItem> {
    lib.smart_playlists()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            icon(
                folder(
                    &format!("sp:{}", p.id),
                    &p.name,
                    format!("Smart · {}", songs_line(p.items as usize)),
                ),
                "ic_car_smart",
            )
        })
        .collect()
}

fn artist_item(a: &ArtistRow) -> AutoItem {
    AutoItem {
        grid: true,
        art: Some(CarArt::Icon("ic_car_artist")),
        ..folder(
            &format!("artist:{}", a.name),
            &a.name,
            format!("{} album{}", a.albums, if a.albums == 1 { "" } else { "s" }),
        )
    }
}

fn album_item(a: &crate::library::AlbumRow) -> AutoItem {
    with_cover(
        folder(&format!("album:{}", a.key), &a.name, a.artist_line()),
        a,
    )
}

/// An album's cover, or the album icon when it has no song to take one
/// from. The cover itself may turn out missing; the car is sent a plain
/// tile then (see [`car_art_file`]).
fn with_cover(mut it: AutoItem, a: &crate::library::AlbumRow) -> AutoItem {
    it.art = Some(match &a.cover_track {
        Some(p) => CarArt::Cover(p.clone()),
        None => CarArt::Icon("ic_car_album"),
    });
    it
}

fn artist_children(lib: &Library, name: &str) -> Vec<AutoItem> {
    lib.artist_albums(name, ArtistMode::AlbumArtist)
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            with_cover(
                folder(
                    &format!("album:{}", a.key),
                    &a.name,
                    a.year.map(|y| y.to_string()).unwrap_or_default(),
                ),
                &a,
            )
        })
        .collect()
}

fn dir_children(lib: &Library, dir: &std::path::Path) -> Vec<AutoItem> {
    let entries = lib.folder(dir).unwrap_or_default();
    let mut out = Vec::new();
    let mut tracks = Vec::new();
    for e in entries {
        match e {
            crate::library::DirEntry::Dir { path, name } => {
                out.push(icon(folder(&dir_id(&path), &name, ""), "ic_car_folder"));
            }
            crate::library::DirEntry::Track(t) => tracks.push(t),
        }
    }
    out.extend(track_items(&dir_id(dir), &tracks));
    out.truncate(MAX);
    out
}

/// Where covers for the car are kept, as files the app's content
/// provider hands it.
pub fn car_art_dir() -> PathBuf {
    crate::config::cache_dir().join("car-art")
}

/// A PNG of a song's cover for the car, `key` naming the file. Made the
/// first time it's asked for, from the same caches the window uses. A
/// cover that can't be found gets a plain tile instead, so the car doesn't
/// draw its broken-picture sign.
pub fn car_art_file(track: &std::path::Path, key: &str) -> Option<PathBuf> {
    let dir = car_art_dir();
    let out = dir.join(format!("{key}.png"));
    if out.is_file() {
        return Some(out);
    }
    std::fs::create_dir_all(&dir).ok()?;
    match crate::art::load(track, CAR_ART) {
        Some(img) => {
            let tmp = dir.join(format!(".{key}.part"));
            img.save_with_format(&tmp, image::ImageFormat::Png).ok()?;
            std::fs::rename(&tmp, &out).ok()?;
            Some(out)
        }
        None => blank_tile(&dir),
    }
}

/// Covers in the car are drawn this big.
const CAR_ART: u32 = 320;

/// The tile shown for an album with no cover: the Runout ground with a
/// record on it.
fn blank_tile(dir: &std::path::Path) -> Option<PathBuf> {
    let out = dir.join("no-cover.png");
    if out.is_file() {
        return Some(out);
    }
    let n = CAR_ART;
    let c = n as f32 / 2.0;
    let img = image::RgbaImage::from_fn(n, n, |x, y| {
        let r = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt() / c;
        if r < 0.12 {
            image::Rgba([0xb8, 0x73, 0x33, 0xff])
        } else if r < 0.62 {
            image::Rgba([0x1c, 0x24, 0x22, 0xff])
        } else {
            image::Rgba([0x2e, 0x4a, 0x45, 0xff])
        }
    });
    let tmp = dir.join(".no-cover.part");
    img.save_with_format(&tmp, image::ImageFormat::Png).ok()?;
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// What playing an id means.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoPlay {
    Tracks { tracks: Vec<TrackRow>, start: usize },
    Shuffle(Vec<TrackRow>),
    Mix(i64),
    Nothing,
}

pub fn resolve(lib: &Library, id: &str) -> AutoPlay {
    if id == "shuffle" {
        return AutoPlay::Shuffle(
            lib.songs(crate::library::SongSort::Title, false)
                .unwrap_or_default(),
        );
    }
    if let Some(n) = id.strip_prefix("mix:").and_then(|n| n.parse::<i64>().ok()) {
        return AutoPlay::Mix(n);
    }
    if let Some(q) = id.strip_prefix("search:") {
        return search(lib, q);
    }
    let (list, start) = match id.rsplit_once('|') {
        Some((l, i)) => (l, i.parse::<usize>().unwrap_or(0)),
        None => (id, 0),
    };
    let tracks = list_tracks(lib, list);
    if tracks.is_empty() {
        return AutoPlay::Nothing;
    }
    AutoPlay::Tracks {
        start: start.min(tracks.len() - 1),
        tracks,
    }
}

/// What to play for something said to the car ("play Nirvana on
/// PatinaPlayer"): an artist or album named exactly, else the songs that
/// match, else the first album or artist that does. Nothing said
/// shuffles everything.
fn search(lib: &Library, q: &str) -> AutoPlay {
    let q = q.trim();
    if q.is_empty() {
        return resolve(lib, "shuffle");
    }
    let Ok(r) = lib.search(q, MAX) else {
        return AutoPlay::Nothing;
    };
    let tracks = if let Some(a) = r.artists.iter().find(|a| a.name.eq_ignore_ascii_case(q)) {
        lib.artist_tracks(&a.name, ArtistMode::AlbumArtist)
            .unwrap_or_default()
    } else if let Some(a) = r.albums.iter().find(|a| a.name.eq_ignore_ascii_case(q)) {
        lib.album_tracks(&a.key).unwrap_or_default()
    } else if !r.tracks.is_empty() {
        r.tracks
    } else if let Some(a) = r.albums.first() {
        lib.album_tracks(&a.key).unwrap_or_default()
    } else if let Some(a) = r.artists.first() {
        lib.artist_tracks(&a.name, ArtistMode::AlbumArtist)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if tracks.is_empty() {
        AutoPlay::Nothing
    } else {
        AutoPlay::Tracks { tracks, start: 0 }
    }
}

impl Browser {
    /// Play what the car picked.
    pub fn auto_play(&mut self, id: &str) {
        let Some(lib) = &self.lib else { return };
        match resolve(lib, id) {
            AutoPlay::Mix(m) => self.play_saved_mix(m),
            AutoPlay::Shuffle(tracks) => {
                let tracks = self.playable(tracks.iter().map(TrackRow::to_info).collect());
                if tracks.is_empty() {
                    return;
                }
                self.end_mix();
                self.player.send(crate::Command::PlayAll {
                    tracks,
                    shuffle: true,
                });
            }
            AutoPlay::Tracks { tracks, start } => {
                let picked = tracks[start].path.clone();
                let infos = self.playable(tracks.iter().map(TrackRow::to_info).collect());
                if infos.is_empty() {
                    return;
                }
                let start = infos.iter().position(|t| t.path == picked).unwrap_or(0);
                self.end_mix();
                self.player.send(crate::Command::SetQueueTracks {
                    tracks: infos,
                    start,
                    pos: std::time::Duration::ZERO,
                });
            }
            AutoPlay::Nothing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(letter("abba"), 'A');
        assert_eq!(letter("  Émile"), 'É');
        assert_eq!(letter("2Pac"), '#');
        let f = letter_folders(
            "artists",
            ["Beck", "abba", "ABC", "311"].into_iter(),
            "artist",
        );
        let got: Vec<(&str, &str)> = f
            .iter()
            .map(|i| (i.id.as_str(), i.subtitle.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("artists:A", "2 artists"),
                ("artists:B", "1 artist"),
                ("artists:#", "1 artist")
            ]
        );
    }

    #[test]
    fn tabs_hold_only_folders() {
        let lib = Library::open_in_memory().unwrap();
        let root = children(&lib, "root");
        assert_eq!(
            root.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["Home", "Artists", "Albums", "Playlists", "Folders"]
        );
        assert!(root.iter().all(|i| i.browsable && !i.playable));
        assert_eq!(children(&lib, "root4").len(), 4);
        // Four tabs: Home's rows lead Playlists, under their own heading.
        let lists = children(&lib, "lists");
        assert_eq!(lists[0].id, "shuffle");
        assert_eq!(lists[0].group, "Home");
        assert_eq!(resolve(&lib, "search:nothing here"), AutoPlay::Nothing);
    }

    #[test]
    fn a_missing_cover_is_a_tile() {
        let d = tempfile::tempdir().unwrap();
        let p = blank_tile(d.path()).unwrap();
        let img = image::open(&p).unwrap();
        assert_eq!(img.width(), CAR_ART);
    }

    #[test]
    fn ids_round_trip() {
        let p = std::path::Path::new("/music/Ä b");
        assert_eq!(dir_of(&dir_id(p)).as_deref(), Some(p));
        assert_eq!(unhex("zz"), None);
    }
}
