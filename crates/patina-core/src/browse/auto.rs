//! What a car (Android Auto) can browse, and playing what's picked there.
//!
//! The car asks for one level at a time by id. [`children`] answers from
//! the library alone, so it can run on the thread Android asks from; the
//! play request it makes comes back through [`Browser::auto_play`], on the
//! app's own thread, so it plays the way a tap in the app would.
//!
//! Ids: `root`, `favorites`, `playlists`, `smart`, `history`, `mixes`,
//! `shuffle`, and for things in them `fav:<id>`, `pl:<id>`, `sp:<id>`,
//! `mix:<id>`, `album:<key>`, `artist:<name>`, `dir:<hex of the path>`.
//! A song is its list's id, a `|`, and its place in the list, so playing
//! it plays the list from there.

use std::path::PathBuf;

use super::{Browser, Item, ViewPrefs, tracks_of};
use crate::library::marks::FavKind;
use crate::library::{AlbumSort, ArtistMode, ArtistRow, Library, TrackRow};

/// A list gets at most this many rows in the car.
pub const MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub browsable: bool,
    pub playable: bool,
}

fn folder(id: &str, title: &str, subtitle: impl Into<String>) -> AutoItem {
    AutoItem {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        browsable: true,
        playable: false,
    }
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
            browsable: false,
            playable: true,
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
        "root" => {
            let mut v = vec![AutoItem {
                id: "shuffle".into(),
                title: "Shuffle everything".into(),
                subtitle: String::new(),
                browsable: false,
                playable: true,
            }];
            if lib.favorites().is_ok_and(|f| !f.is_empty()) {
                v.push(folder("favorites", "Favorites", ""));
            }
            v.push(folder("playlists", "Playlists", ""));
            if lib.smart_playlists().is_ok_and(|s| !s.is_empty()) {
                v.push(folder("smart", "Smart playlists", ""));
            }
            v.push(folder("history", "Recently played", ""));
            if lib.saved_mixes().is_ok_and(|m| !m.is_empty()) {
                v.push(folder("mixes", "Mixes", ""));
            }
            v.push(folder("albums", "Recently added", ""));
            v
        }
        "favorites" => lib
            .favorites()
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                let sub = match f.kind {
                    FavKind::Folder => "Folder",
                    FavKind::Album => "Album",
                    FavKind::Artist => "Artist",
                };
                folder(&format!("fav:{}", f.id), &f.label, sub)
            })
            .collect(),
        "playlists" => {
            let mut rows = lib.hearted_row().ok().into_iter().collect::<Vec<_>>();
            rows.extend(lib.playlists().unwrap_or_default());
            rows.into_iter()
                .filter(|p| p.items > 0)
                .map(|p| {
                    folder(
                        &format!("pl:{}", p.id),
                        &p.name,
                        songs_line(p.items as usize),
                    )
                })
                .collect()
        }
        "smart" => lib
            .smart_playlists()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                folder(
                    &format!("sp:{}", p.id),
                    &p.name,
                    songs_line(p.items as usize),
                )
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
                browsable: false,
                playable: true,
            })
            .collect(),
        "albums" => lib
            .albums(AlbumSort::RecentlyAdded)
            .unwrap_or_default()
            .into_iter()
            .take(MAX)
            .map(|a| folder(&format!("album:{}", a.key), &a.name, a.artist_line()))
            .collect(),
        id => {
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

fn artist_children(lib: &Library, name: &str) -> Vec<AutoItem> {
    lib.artist_albums(name, ArtistMode::AlbumArtist)
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            folder(
                &format!("album:{}", a.key),
                &a.name,
                a.year.map(|y| y.to_string()).unwrap_or_default(),
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
                out.push(folder(&dir_id(&path), &name, ""));
            }
            crate::library::DirEntry::Track(t) => tracks.push(t),
        }
    }
    out.extend(track_items(&dir_id(dir), &tracks));
    out.truncate(MAX);
    out
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
    fn ids_round_trip() {
        let p = std::path::Path::new("/music/Ä b");
        assert_eq!(dir_of(&dir_id(p)).as_deref(), Some(p));
        assert_eq!(unhex("zz"), None);
    }
}
