//! The play queue: what's in it, what order it plays in, and what comes
//! next. Shared between the core thread and the decoder thread behind a
//! mutex. Neither of those is the real-time thread, so a lock is fine.

use std::path::PathBuf;

use crate::track::TrackInfo;

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
    /// Stable id, so background work (tag probing) can find the entry
    /// again after the queue was reordered.
    pub id: u64,
    pub info: TrackInfo,
    /// Tags have been read (successfully or not).
    pub probed: bool,
    /// Last attempt to open this file failed.
    pub failed: bool,
}

#[derive(Debug, Default)]
pub struct Queue {
    entries: Vec<Entry>,
    /// Play order as indices into `entries`. Identity unless shuffled.
    order: Vec<usize>,
    /// Entry currently playing (or last played), as an index into `entries`.
    current: Option<usize>,
    pub repeat: Repeat,
    shuffle: bool,
    next_id: u64,
    /// Bumped on every change, so frontends know when to redraw.
    pub version: u64,
    rng: u64,
}

impl Queue {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self {
            rng: seed | 1,
            ..Default::default()
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn get(&self, idx: usize) -> Option<&Entry> {
        self.entries.get(idx)
    }

    pub fn path(&self, idx: usize) -> Option<PathBuf> {
        self.entries.get(idx).map(|e| e.info.path.clone())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.current = None;
        self.version += 1;
    }

    /// Append files. Returns (id, path) pairs for the tag prober.
    pub fn append(&mut self, paths: Vec<PathBuf>) -> Vec<(u64, PathBuf)> {
        let mut added = Vec::with_capacity(paths.len());
        for p in paths {
            let id = self.next_id;
            self.next_id += 1;
            let idx = self.entries.len();
            self.entries.push(Entry {
                id,
                info: TrackInfo::unprobed(&p),
                probed: false,
                failed: false,
            });
            if self.shuffle {
                // Drop new tracks somewhere after the current one.
                let cur = self.order_pos(self.current).map(|p| p + 1).unwrap_or(0);
                let span = self.order.len() - cur + 1;
                let at = cur + (self.rand() as usize % span);
                self.order.insert(at, idx);
            } else {
                self.order.push(idx);
            }
            added.push((id, p));
        }
        self.version += 1;
        added
    }

    pub fn set_info(&mut self, id: u64, info: TrackInfo) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.info = info;
            e.probed = true;
            self.version += 1;
        }
    }

    pub fn set_info_at(&mut self, idx: usize, info: TrackInfo) {
        if let Some(e) = self.entries.get_mut(idx)
            && (e.info != info || !e.probed)
        {
            e.info = info;
            e.probed = true;
            e.failed = false;
            self.version += 1;
        }
    }

    pub fn mark_failed(&mut self, idx: usize) {
        if let Some(e) = self.entries.get_mut(idx) {
            e.failed = true;
            e.probed = true;
            self.version += 1;
        }
    }

    pub fn set_current(&mut self, idx: Option<usize>) {
        if self.current != idx {
            self.current = idx;
            self.version += 1;
        }
    }

    fn order_pos(&self, idx: Option<usize>) -> Option<usize> {
        let idx = idx?;
        self.order.iter().position(|&i| i == idx)
    }

    /// The entry that plays after `idx` on its own (end of track).
    /// Repeat One plays the same track again; Repeat All wraps.
    pub fn upcoming(&self, idx: usize) -> Option<usize> {
        if self.repeat == Repeat::One {
            return Some(idx);
        }
        self.step(idx, 1)
    }

    /// The entry `delta` steps away in play order, for next and back.
    /// Ignores Repeat One, since pressing next should move on. Wraps only
    /// with Repeat All.
    pub fn step(&self, idx: usize, delta: isize) -> Option<usize> {
        let n = self.order.len() as isize;
        if n == 0 {
            return None;
        }
        let pos = self.order_pos(Some(idx))? as isize + delta;
        if (0..n).contains(&pos) {
            Some(self.order[pos as usize])
        } else if self.repeat == Repeat::All {
            Some(self.order[pos.rem_euclid(n) as usize])
        } else {
            None
        }
    }

    /// First entry in play order.
    pub fn first(&self) -> Option<usize> {
        self.order.first().copied()
    }

    /// Turn shuffle on or off. The current track stays put and becomes the
    /// first in the new order, so what's playing never jumps.
    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
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
        self.version += 1;
    }

    /// Entries in play order, for drawing.
    pub fn play_order(&self) -> &[usize] {
        &self.order
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

    fn q(n: usize) -> Queue {
        let mut q = Queue::new();
        q.append((0..n).map(|i| PathBuf::from(format!("{i}.flac"))).collect());
        q
    }

    #[test]
    fn upcoming_and_repeat() {
        let mut q = q(3);
        assert_eq!(q.upcoming(0), Some(1));
        assert_eq!(q.upcoming(2), None);
        q.repeat = Repeat::All;
        assert_eq!(q.upcoming(2), Some(0));
        q.repeat = Repeat::One;
        assert_eq!(q.upcoming(1), Some(1));
        assert_eq!(q.step(1, 1), Some(2), "next ignores repeat one");
    }

    #[test]
    fn shuffle_keeps_current_first_and_all_entries() {
        let mut q = q(50);
        q.set_current(Some(17));
        q.set_shuffle(true);
        assert_eq!(q.play_order()[0], 17);
        let mut sorted = q.play_order().to_vec();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        q.set_shuffle(false);
        assert_eq!(q.play_order(), (0..50).collect::<Vec<_>>().as_slice());
    }
}
