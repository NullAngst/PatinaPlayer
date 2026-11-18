//! Undo, for the things that used to ask "Are you sure?" first: deleting
//! a playlist, a smart playlist, a mix or a station, clearing the queue,
//! and taking songs out of a playlist or the queue. They happen at once,
//! and for a few seconds a note offers to put them back. Only the last
//! one can be undone.
//!
//! Server playlists are the exception for deleting: the server deletes
//! them for good, so those still ask first.

use std::time::{Duration, Instant};

use super::{Browser, Section};
use crate::Command;
use crate::config::Station;
use crate::track::TrackInfo;

/// How long the offer stands.
pub const UNDO_FOR: Duration = Duration::from_secs(10);

/// What to put back.
#[derive(Debug, Clone)]
pub(super) enum Undo {
    /// A playlist's songs as they were (some were taken out). A server
    /// playlist's go back to the server too.
    PlaylistItems {
        id: i64,
        uris: Vec<Vec<u8>>,
    },
    /// A playlist on this device, deleted: made again, with its songs.
    Playlist {
        name: String,
        uris: Vec<Vec<u8>>,
    },
    Smart {
        name: String,
        query: String,
        exclude: String,
    },
    Mix {
        kind: String,
        name: String,
        seed: String,
        similar: String,
    },
    Station {
        index: usize,
        station: Station,
    },
    /// Songs taken out of the queue, or what was coming up when it was
    /// cleared: they go back at the end, in their order.
    Queue(Vec<TrackInfo>),
}

impl Browser {
    /// Offer to undo something just done, with a note saying what was done.
    pub(super) fn offer_undo(&mut self, done: impl Into<String>, u: Undo) {
        self.undo = Some((u, Instant::now() + UNDO_FOR));
        self.undo_label = done.into();
        self.note(self.undo_label.clone());
    }

    /// What can be undone right now, as the note said it: None when
    /// nothing can, or the offer has run out.
    pub fn undo_offer(&self) -> Option<&str> {
        match &self.undo {
            Some((_, until)) if Instant::now() < *until => Some(&self.undo_label),
            _ => None,
        }
    }

    /// Put back the last thing done that can be.
    pub fn undo(&mut self) {
        let Some((u, until)) = self.undo.take() else {
            return self.note("Nothing to undo");
        };
        if Instant::now() >= until {
            return self.note("Too late to undo that");
        }
        match u {
            Undo::PlaylistItems { id, uris } => {
                let n = uris.len();
                self.store_playlist(id, uris, 0, None);
                self.note(format!(
                    "Put the playlist back as it was ({})",
                    super::count(n)
                ));
            }
            Undo::Playlist { name, uris } => {
                let Some(lib) = self.lib.as_mut() else { return };
                let made = lib
                    .create_playlist(&name)
                    .and_then(|id| lib.set_playlist_items(id, &uris));
                match made {
                    Ok(()) => self.note(format!("Put back the playlist \"{name}\"")),
                    Err(e) => self.error(e.to_string()),
                }
                self.refresh_section(Section::Playlists);
            }
            Undo::Smart {
                name,
                query,
                exclude,
            } => {
                let Some(lib) = self.lib.as_ref() else { return };
                match lib.create_smart(&name, &query, &exclude) {
                    Ok(_) => self.note(format!("Put back the smart playlist \"{name}\"")),
                    Err(e) => self.error(e.to_string()),
                }
                self.refresh_section(Section::Playlists);
            }
            Undo::Mix {
                kind,
                name,
                seed,
                similar,
            } => {
                let Some(lib) = self.lib.as_ref() else { return };
                match lib.save_mix(&kind, &name, &seed, &similar) {
                    Ok(_) => self.note(format!(
                        "Put back the mix \"{name}\" (keep it offline again if it was)"
                    )),
                    Err(e) => self.error(e.to_string()),
                }
                self.refresh_section(Section::Playlists);
                self.rebuild_sidebar();
            }
            Undo::Station { index, station } => {
                let name = station.name.clone();
                let at = index.min(self.stations.len());
                self.stations.insert(at, station);
                self.save_stations();
                self.refresh_section(Section::Radio);
                self.note(format!("Put back the station \"{name}\""));
            }
            Undo::Queue(tracks) => {
                let n = tracks.len();
                self.player.send(Command::AppendTracks(tracks));
                self.note(format!(
                    "Put {} back at the end of the queue",
                    super::count(n)
                ));
            }
        }
        self.touch();
    }

    /// What's in a playlist now, for putting it back.
    pub(super) fn playlist_uris(&self, id: i64) -> Option<Vec<Vec<u8>>> {
        let items = self.lib.as_ref()?.playlist_items(id).ok()?;
        Some(items.into_iter().map(|(u, _)| u).collect())
    }

    /// The queue's entries at these places in play order, for putting
    /// them back.
    pub(super) fn queue_infos(&self, ids: &[crate::queue::EntryId]) -> Vec<TrackInfo> {
        let q = self.player.queue();
        q.play_order()
            .iter()
            .filter(|id| ids.contains(id))
            .filter_map(|&id| q.get(id).map(|e| e.info.clone()))
            .collect()
    }

    /// What's coming up after the song playing, for undoing a clear.
    pub(super) fn queue_upcoming(&self) -> Vec<TrackInfo> {
        let q = self.player.queue();
        let order = q.play_order();
        let from = q
            .current()
            .and_then(|c| order.iter().position(|&id| id == c))
            .map_or(0, |p| p + 1);
        order[from..]
            .iter()
            .filter_map(|&id| q.get(id).map(|e| e.info.clone()))
            .collect()
    }

    /// Whether deleting this playlist can be undone: it lives on this
    /// device only. A server's are gone for good.
    pub fn playlist_undoable(&self, id: i64) -> bool {
        id != super::HEARTED
            && self
                .playlist_row(id)
                .is_some_and(|p| p.server.is_none() && !p.readonly)
    }
}
