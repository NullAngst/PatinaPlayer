//! The play queue: what's in it, what order it plays in, and what comes
//! next. Shared between the core thread and the decoder thread behind a
//! mutex. Neither of those is the real-time thread, so a lock is fine.
//!
//! Everything outside this file refers to entries by [`EntryId`], never by
//! position. Positions shift when you reorder or remove; ids don't, so the
//! decoder can be halfway through the next track while you drag things
//! around and nothing gets confused.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::track::{MediaKind, TrackInfo};

pub type EntryId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: EntryId,
    pub info: TrackInfo,
    /// Tags are known (from the library, or read from the file).
    pub probed: bool,
    /// Last attempt to open this file failed.
    pub failed: bool,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// Entries in the order they were queued. This is the play order
    /// whenever shuffle is off.
    entries: Vec<Entry>,
    /// id -> index into `entries`.
    index: HashMap<EntryId, usize>,
    /// Play order as ids.
    order: Vec<EntryId>,
    current: Option<EntryId>,
    pub repeat: Repeat,
    shuffle: bool,
    next_id: EntryId,
    /// Bumped on every change, so frontends know when to redraw.
    pub version: u64,
    rng: u64,
    /// The queue was cleared while this entry played. It's all that's
    /// left, nothing follows it (Repeat All doesn't bring it round again),
    /// and it goes too once it has played out. Anything queued after the
    /// clear lifts this.
    ending: Option<EntryId>,
}

impl Queue {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self {
            rng: seed | 1,
            next_id: 1,
            ..Default::default()
        }
    }

    fn reindex(&mut self) {
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();
    }

    fn touch(&mut self) {
        self.version += 1;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current(&self) -> Option<EntryId> {
        self.current
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn get(&self, id: EntryId) -> Option<&Entry> {
        self.index.get(&id).map(|&i| &self.entries[i])
    }

    pub fn contains(&self, id: EntryId) -> bool {
        self.index.contains_key(&id)
    }

    pub fn path(&self, id: EntryId) -> Option<PathBuf> {
        self.get(id).map(|e| e.info.path.clone())
    }

    /// Entries in play order, for drawing.
    pub fn play_order(&self) -> &[EntryId] {
        &self.order
    }

    /// Position of an entry in play order.
    pub fn position(&self, id: EntryId) -> Option<usize> {
        self.order.iter().position(|&i| i == id)
    }

    /// Entry at a play-order position.
    pub fn at(&self, pos: usize) -> Option<&Entry> {
        self.order.get(pos).and_then(|&id| self.get(id))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.order.clear();
        self.current = None;
        self.ending = None;
        self.touch();
    }

    /// Take everything out but `keep`, which stays current and is the last
    /// to play. Clearing the queue while a song plays: what was coming up
    /// goes, the song doesn't.
    pub fn clear_except(&mut self, keep: EntryId) {
        let Some(&i) = self.index.get(&keep) else {
            self.clear();
            return;
        };
        let e = self.entries.swap_remove(i);
        self.entries.clear();
        self.entries.push(e);
        self.order = vec![keep];
        self.current = Some(keep);
        self.ending = Some(keep);
        self.reindex();
        self.touch();
    }

    /// The entry left playing after a clear, if it's still the only one.
    pub fn ending(&self) -> Option<EntryId> {
        self.ending
    }

    fn push_entry(&mut self, info: TrackInfo, probed: bool) -> EntryId {
        let id = self.next_id;
        self.next_id += 1;
        self.index.insert(id, self.entries.len());
        self.entries.push(Entry {
            id,
            info,
            probed,
            failed: false,
        });
        id
    }

    /// Where new entries go in play order when shuffle is on: somewhere
    /// after the current track, so they're still coming up.
    fn shuffled_slot(&mut self) -> usize {
        let cur = self
            .current
            .and_then(|c| self.position(c))
            .map(|p| p + 1)
            .unwrap_or(0);
        let span = self.order.len() - cur + 1;
        cur + (self.rand() as usize % span)
    }

    /// Append files that haven't been probed. Returns (id, path) pairs for
    /// the tag prober.
    pub fn append(&mut self, paths: Vec<PathBuf>) -> Vec<(EntryId, PathBuf)> {
        self.ending = None;
        let mut added = Vec::with_capacity(paths.len());
        for p in paths {
            let id = self.push_entry(TrackInfo::unprobed(&p), false);
            self.place(id);
            added.push((id, p));
        }
        self.touch();
        added
    }

    /// Append tracks whose tags are already known (from the library).
    pub fn append_tracks(&mut self, tracks: Vec<TrackInfo>) -> Vec<EntryId> {
        self.ending = None;
        let ids: Vec<EntryId> = tracks
            .into_iter()
            .map(|t| {
                let id = self.push_entry(t, true);
                self.place(id);
                id
            })
            .collect();
        self.touch();
        ids
    }

    fn place(&mut self, id: EntryId) {
        if self.shuffle {
            let at = self.shuffled_slot();
            self.order.insert(at, id);
        } else {
            self.order.push(id);
        }
    }

    /// Insert tracks right after the current one, in order. "Play next".
    pub fn insert_next(&mut self, tracks: Vec<TrackInfo>) -> Vec<EntryId> {
        self.ending = None;
        let cur_base = self.current.and_then(|c| self.index.get(&c).copied());
        let cur_play = self.current.and_then(|c| self.position(c));
        let mut ids = Vec::with_capacity(tracks.len());
        for (k, t) in tracks.into_iter().enumerate() {
            let id = self.next_id;
            self.next_id += 1;
            let at = cur_base
                .map(|i| i + 1 + k)
                .unwrap_or(k)
                .min(self.entries.len());
            self.entries.insert(
                at,
                Entry {
                    id,
                    info: t,
                    probed: true,
                    failed: false,
                },
            );
            let pat = cur_play
                .map(|i| i + 1 + k)
                .unwrap_or(k)
                .min(self.order.len());
            self.order.insert(pat, id);
            ids.push(id);
        }
        self.reindex();
        self.touch();
        ids
    }

    pub fn set_info(&mut self, id: EntryId, info: TrackInfo) {
        if let Some(&i) = self.index.get(&id) {
            let e = &mut self.entries[i];
            if e.info != info || !e.probed {
                e.info = info;
                e.probed = true;
                e.failed = false;
                self.touch();
            }
        }
    }

    pub fn mark_failed(&mut self, id: EntryId) {
        if let Some(&i) = self.index.get(&id) {
            self.entries[i].failed = true;
            self.entries[i].probed = true;
            self.touch();
        }
    }

    pub fn set_current(&mut self, id: Option<EntryId>) {
        if self.current != id {
            self.current = id;
            self.touch();
        }
    }

    /// The entry that plays after `id` on its own (end of track).
    /// Repeat One plays the same track again; Repeat All wraps.
    pub fn upcoming(&self, id: EntryId) -> Option<EntryId> {
        if self.repeat == Repeat::One && self.contains(id) {
            return Some(id);
        }
        if self.ending == Some(id) {
            return None;
        }
        self.step(id, 1)
    }

    /// The entry `delta` steps away in play order, for next and back.
    /// Ignores Repeat One, since pressing next should move on. Wraps only
    /// with Repeat All.
    pub fn step(&self, id: EntryId, delta: isize) -> Option<EntryId> {
        let n = self.order.len() as isize;
        if n == 0 {
            return None;
        }
        let pos = self.position(id)? as isize + delta;
        if (0..n).contains(&pos) {
            Some(self.order[pos as usize])
        } else if self.repeat == Repeat::All {
            Some(self.order[pos.rem_euclid(n) as usize])
        } else {
            None
        }
    }

    /// Up to `n` entries after the current one, in the order they'll
    /// play: wrapping around with Repeat All, the current one over and
    /// over with Repeat One. From the top when nothing is current.
    pub fn ahead(&self, n: usize) -> Vec<EntryId> {
        let len = self.order.len();
        if len == 0 || n == 0 {
            return Vec::new();
        }
        let start = match self.current.and_then(|c| self.position(c)) {
            Some(p) if self.repeat == Repeat::One => return vec![self.order[p]],
            Some(p) => p + 1,
            None => 0,
        };
        let take = if self.repeat == Repeat::All {
            // Everything but the current one, once round.
            len.saturating_sub(usize::from(start > 0))
        } else {
            len.saturating_sub(start)
        };
        (0..take.min(n))
            .map(|i| self.order[(start + i) % len])
            .collect()
    }

    pub fn first(&self) -> Option<EntryId> {
        self.order.first().copied()
    }

    /// Move an entry to a new play-order position. With shuffle off this is
    /// also its queued position, so the move survives toggling shuffle.
    pub fn move_to(&mut self, id: EntryId, to: usize) -> bool {
        let Some(from) = self.position(id) else {
            return false;
        };
        let to = to.min(self.order.len() - 1);
        if from == to {
            return false;
        }
        let v = self.order.remove(from);
        self.order.insert(to, v);
        if !self.shuffle {
            let i = self.index[&id];
            let e = self.entries.remove(i);
            self.entries.insert(to, e);
            self.reindex();
        }
        self.touch();
        true
    }

    pub fn remove(&mut self, id: EntryId) -> bool {
        let Some(&i) = self.index.get(&id) else {
            return false;
        };
        self.entries.remove(i);
        self.order.retain(|&x| x != id);
        if self.current == Some(id) {
            self.current = None;
        }
        if self.ending == Some(id) {
            self.ending = None;
        }
        self.reindex();
        self.touch();
        true
    }

    /// Turn shuffle on or off. The current track stays put and becomes the
    /// first in the new order, so what's playing never jumps.
    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        let mut order: Vec<EntryId> = self.entries.iter().map(|e| e.id).collect();
        if on {
            // Fisher-Yates.
            for i in (1..order.len()).rev() {
                let j = (self.rand() % (i as u64 + 1)) as usize;
                order.swap(i, j);
            }
            if let Some(cur) = self.current
                && let Some(p) = order.iter().position(|&i| i == cur)
            {
                order.remove(p);
                order.insert(0, cur);
            }
        }
        self.order = order;
        self.touch();
    }

    /// Every entry's path, in play order. For "save queue as playlist".
    pub fn paths_in_order(&self) -> Vec<PathBuf> {
        self.order.iter().filter_map(|&id| self.path(id)).collect()
    }

    /// Local music files in play order, for adding the queue to a
    /// playlist. Stations and podcast episodes are left out.
    pub fn music_paths_in_order(&self) -> Vec<PathBuf> {
        self.order
            .iter()
            .filter_map(|&id| self.get(id))
            .filter(|e| e.info.kind == MediaKind::File && !e.info.path.as_os_str().is_empty())
            .map(|e| e.info.path.clone())
            .collect()
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*. Good enough for shuffling a queue.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: usize) -> (Queue, Vec<EntryId>) {
        let mut q = Queue::new();
        let ids = q
            .append((0..n).map(|i| PathBuf::from(format!("{i}.flac"))).collect())
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        (q, ids)
    }

    #[test]
    fn clearing_keeps_the_current_one() {
        let (mut q, ids) = q(4);
        q.set_current(Some(ids[2]));
        q.repeat = Repeat::All;
        q.clear_except(ids[2]);
        assert_eq!(q.len(), 1);
        assert_eq!(q.play_order(), &[ids[2]]);
        assert_eq!(q.current(), Some(ids[2]));
        // Repeat All doesn't bring the last one round again.
        assert_eq!(q.upcoming(ids[2]), None);
        assert_eq!(q.ending(), Some(ids[2]));
        // Something queued after: it plays on into that.
        let more = q.append_tracks(vec![TrackInfo::unprobed(std::path::Path::new("x.flac"))]);
        assert_eq!(q.ending(), None);
        assert_eq!(q.upcoming(ids[2]), Some(more[0]));
        // One that isn't there: everything goes.
        q.clear_except(999);
        assert!(q.is_empty());
        assert_eq!(q.current(), None);
    }

    #[test]
    fn upcoming_and_repeat() {
        let (mut q, ids) = q(3);
        assert_eq!(q.upcoming(ids[0]), Some(ids[1]));
        assert_eq!(q.upcoming(ids[2]), None);
        q.repeat = Repeat::All;
        assert_eq!(q.upcoming(ids[2]), Some(ids[0]));
        q.repeat = Repeat::One;
        assert_eq!(q.upcoming(ids[1]), Some(ids[1]));
        assert_eq!(q.step(ids[1], 1), Some(ids[2]), "next ignores repeat one");
    }

    #[test]
    fn shuffle_keeps_current_first_and_all_entries() {
        let (mut q, ids) = q(50);
        q.set_current(Some(ids[17]));
        q.set_shuffle(true);
        assert_eq!(q.play_order()[0], ids[17]);
        let mut sorted = q.play_order().to_vec();
        sorted.sort();
        assert_eq!(sorted, ids);
        q.set_shuffle(false);
        assert_eq!(q.play_order(), ids.as_slice());
    }

    #[test]
    fn move_and_remove_keep_ids_stable() {
        let (mut q, ids) = q(5);
        q.set_current(Some(ids[2]));
        assert!(q.move_to(ids[4], 0));
        assert_eq!(q.play_order(), &[ids[4], ids[0], ids[1], ids[2], ids[3]]);
        assert_eq!(q.upcoming(ids[2]), Some(ids[3]));
        assert!(q.remove(ids[3]));
        assert_eq!(q.upcoming(ids[2]), None);
        assert_eq!(q.path(ids[4]).unwrap(), PathBuf::from("4.flac"));
        // The move holds with shuffle toggled off and back.
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(q.play_order(), &[ids[4], ids[0], ids[1], ids[2]]);
    }

    #[test]
    fn ahead_follows_play_order_and_repeat() {
        let (mut q, ids) = q(4);
        assert_eq!(
            q.ahead(2),
            vec![ids[0], ids[1]],
            "nothing playing: from the top"
        );
        q.set_current(Some(ids[2]));
        assert_eq!(q.ahead(10), vec![ids[3]]);
        q.repeat = Repeat::All;
        assert_eq!(q.ahead(10), vec![ids[3], ids[0], ids[1]]);
        q.repeat = Repeat::One;
        assert_eq!(q.ahead(10), vec![ids[2]]);
    }

    #[test]
    fn insert_next_goes_after_current() {
        let (mut q, ids) = q(3);
        q.set_current(Some(ids[0]));
        let new = q.insert_next(vec![TrackInfo::unprobed(&PathBuf::from("x.flac"))]);
        assert_eq!(q.play_order(), &[ids[0], new[0], ids[1], ids[2]]);
        assert_eq!(q.upcoming(ids[0]), Some(new[0]));
    }
}
