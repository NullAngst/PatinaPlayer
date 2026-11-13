//! Which album a "Find cover art" or "Use an image" is for: a row, the
//! open album, or the song playing. The finding and saving is in
//! [`crate::cover`]; frontends run it off their own thread, since it waits
//! on the network.

use std::path::{Path, PathBuf};

use super::{Browser, Item, Source};
use crate::library::tags::album_dir;

/// The album a cover is being chosen for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverTarget {
    /// Its folder: where `album_art.jpg` goes, and what the app's own copy
    /// is filed under.
    pub dir: PathBuf,
    /// What to look for online.
    pub artist: String,
    pub album: String,
    /// What to call it on screen.
    pub label: String,
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
                    dir: path.clone(),
                    artist,
                    album,
                    label: name.clone(),
                })
            }
            _ => None,
        }
    }

    /// The selected row's album.
    pub fn selected_cover_target(&self) -> Option<CoverTarget> {
        self.cover_target(&self.selected_item()?)
    }

    /// The open album's, for its header.
    pub fn level_cover_target(&self) -> Option<CoverTarget> {
        let Some(Source::Album { key }) = self.level().map(|l| &l.source) else {
            return None;
        };
        let a = self.lib.as_ref()?.album(key).ok().flatten()?;
        self.cover_target(&Item::Album(a))
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
