//! Which album a "Find cover art" or "Use an image" is for: a row, the
//! open album, or the song playing. Or which artist, for their picture.
//! The finding and saving is in [`crate::cover`] and
//! [`crate::artist_art`]; frontends run it off their own thread, since it
//! waits on the network.

use std::path::{Path, PathBuf};

use super::{Browser, Item, Source};
use crate::library::tags::album_dir;

/// What a picture is being chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoverKind {
    /// An album's cover.
    #[default]
    Album,
    /// An artist's picture.
    Artist,
}

/// The album a cover is being chosen for, or the artist a picture is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverTarget {
    pub kind: CoverKind,
    /// Its folder: where `album_art.jpg` goes, and what the app's own copy
    /// is filed under. For an artist, their own folder (where
    /// `artist.jpg` goes), or empty when they have none.
    pub dir: PathBuf,
    /// What to look for online. An artist's picture uses only `artist`.
    pub artist: String,
    pub album: String,
    /// The year it came out, where known: a better match online.
    pub year: Option<i32>,
    /// What to call it on screen.
    pub label: String,
}

impl CoverTarget {
    /// Look online. Waits on the network, so call it off the UI thread.
    pub fn search(&self) -> Vec<crate::cover::Found> {
        match self.kind {
            CoverKind::Album => crate::cover::search_year(&self.artist, &self.album, self.year),
            CoverKind::Artist => crate::artist_art::search(&self.artist),
        }
    }

    /// Save a picture's bytes for it.
    pub fn save(&self, bytes: &[u8]) -> Result<crate::cover::Saved, String> {
        match self.kind {
            CoverKind::Album => crate::cover::save(&self.dir, bytes),
            CoverKind::Artist => crate::artist_art::save(&self.artist, self.folder(), bytes),
        }
    }

    /// Save an image file for it.
    pub fn save_file(&self, file: &Path) -> Result<crate::cover::Saved, String> {
        match self.kind {
            CoverKind::Album => crate::cover::save_file(&self.dir, file),
            CoverKind::Artist => crate::artist_art::save_file(&self.artist, self.folder(), file),
        }
    }

    fn folder(&self) -> Option<&Path> {
        (!self.dir.as_os_str().is_empty()).then_some(self.dir.as_path())
    }

    /// "\"Bleach\" by Nirvana", or "Nirvana" for an artist.
    pub fn what(&self) -> String {
        match (self.kind, self.album.is_empty(), self.artist.is_empty()) {
            (CoverKind::Artist, _, _) => self.artist.clone(),
            (_, false, false) => format!("\"{}\" by {}", self.album, self.artist),
            (_, false, true) => format!("\"{}\"", self.album),
            _ => self.label.clone(),
        }
    }
}

fn of_track_path(p: &Path) -> PathBuf {
    p.parent().map(album_dir).unwrap_or(p).to_path_buf()
}

impl Browser {
    /// The album a row stands for. Songs give their album, folders
    /// themselves.
    pub fn cover_target(&self, item: &Item) -> Option<CoverTarget> {
        match item {
            Item::Album(a) => Some(CoverTarget {
                kind: CoverKind::Album,
                year: a.year,
                dir: a
                    .cover_track
                    .as_deref()
                    .map(of_track_path)
                    .unwrap_or_else(|| a.dir.clone()),
                artist: a.artist.clone(),
                album: a.name.clone(),
                label: a.name.clone(),
            }),
            Item::Track(t) => Some(CoverTarget {
                kind: CoverKind::Album,
                year: t.year,
                dir: of_track_path(&t.path),
                artist: t
                    .album_artist
                    .clone()
                    .or_else(|| t.artist.clone())
                    .unwrap_or_default(),
                album: t.album.clone().unwrap_or_default(),
                label: t.album.clone().unwrap_or_else(|| t.title.clone()),
            }),
            Item::Dir { path, name } => {
                // Its songs' tags say what to look for; the folder's name
                // when it has none of its own (an artist's folder).
                let first = self
                    .lib
                    .as_ref()
                    .and_then(|l| l.tracks_under(path).ok())
                    .and_then(|v| v.into_iter().next());
                let (artist, album) = match &first {
                    Some(t) => (
                        t.album_artist
                            .clone()
                            .or_else(|| t.artist.clone())
                            .unwrap_or_default(),
                        t.album.clone().unwrap_or_else(|| name.clone()),
                    ),
                    None => (String::new(), name.clone()),
                };
                Some(CoverTarget {
                    kind: CoverKind::Album,
                    year: first.as_ref().and_then(|t| t.year),
                    dir: path.clone(),
                    artist,
                    album,
                    label: name.clone(),
                })
            }
            Item::Artist(a) => Some(self.artist_target(&a.name)),
            _ => None,
        }
    }

    /// An artist's picture.
    fn artist_target(&self, name: &str) -> CoverTarget {
        CoverTarget {
            kind: CoverKind::Artist,
            dir: self
                .lib
                .as_ref()
                .and_then(|l| crate::artist_art::artist_dir(l, name))
                .unwrap_or_default(),
            artist: name.to_string(),
            album: String::new(),
            year: None,
            label: name.to_string(),
        }
    }

    /// The selected row's album.
    pub fn selected_cover_target(&self) -> Option<CoverTarget> {
        self.cover_target(&self.selected_item()?)
    }

    /// The open album's, for its header; the open artist's picture.
    pub fn level_cover_target(&self) -> Option<CoverTarget> {
        match self.level().map(|l| &l.source)? {
            Source::Album { key } => {
                let a = self.lib.as_ref()?.album(key).ok().flatten()?;
                self.cover_target(&Item::Album(a))
            }
            Source::ArtistAlbums(name) => Some(self.artist_target(name)),
            _ => None,
        }
    }

    /// The album of the song playing.
    pub fn playing_cover_target(&self) -> Option<CoverTarget> {
        let t = self.player.snapshot().track?;
        if t.kind != crate::track::MediaKind::File || t.path.as_os_str().is_empty() {
            return None;
        }
        if let Some(row) = self
            .lib
            .as_ref()
            .and_then(|l| l.track_by_path(&t.path).ok().flatten())
        {
            return self.cover_target(&Item::Track(row));
        }
        Some(CoverTarget {
            kind: CoverKind::Album,
            year: None,
            dir: of_track_path(&t.path),
            artist: t.artist.clone().unwrap_or_default(),
            album: t.album.clone().unwrap_or_default(),
            label: t.album.clone().unwrap_or_else(|| t.display_title()),
        })
    }

    /// A cover was saved: say where, and show it.
    pub fn cover_saved(&mut self, saved: &crate::cover::Saved) {
        self.note(saved.describe());
        // Rows show covers from their folders; nothing in them changed,
        // but they draw again with the new one.
        self.touch();
    }
}
