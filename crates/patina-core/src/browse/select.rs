//! Picking several rows at once, and the actions that grew out of the
//! Playlists view: smart playlists, playlist files in and out, and the
//! listening history. Also what the Now Playing page shows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Browser, Item, Section, Source, count};
use crate::library::TrackRow;
use crate::library::playlist_io;
use crate::library::smart;
use crate::{Command, Snapshot, TrackInfo};

/// What a smart playlist's query would do, for the box it's typed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartCheck {
    pub ok: bool,
    /// "Matches 132 songs", or why the query doesn't work.
    pub message: String,
}

/// What the Now Playing page shows besides the cover and the transport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// "FLAC 96/24", "MP3 320", "Live radio".
    pub format: String,
    /// "Bit-perfect to USB DAC as S24_3LE", "Through PipeWire, resampled 96 → 48 kHz".
    pub path: String,
    /// The output: "PipeWire", "ALSA hw:CARD=DAC".
    pub output: String,
    /// Each stage of the sound chain, in order, with what it's doing.
    pub chain: Vec<(String, String)>,
}

impl Browser {
    // ----- Picking rows -----

    /// The rows an action works on: the marked ones in list order, or the
    /// selected one when nothing is marked.
    pub fn selection_items(&self) -> Vec<Item> {
        let Some(l) = self.level() else {
            return Vec::new();
        };
        let marked = l.marked_items();
        if !marked.is_empty() {
            return marked.into_iter().cloned().collect();
        }
        l.selected_item().cloned().into_iter().collect()
    }

    pub fn marked_count(&self) -> usize {
        self.level().map(|l| l.marked_items().len()).unwrap_or(0)
    }

    pub fn is_marked(&self, index: usize) -> bool {
        self.level().is_some_and(|l| l.is_marked(index))
    }

    // Marking changes no rows, so it doesn't count as a change to the
    // view: the GUI updates the marked rows itself rather than rebuilding
    // every row on each click, which stalls a long list.

    /// Pick a row, or let it go. It becomes where a range starts.
    pub fn toggle_mark(&mut self, index: usize) {
        let Some(l) = self.level_sel() else { return };
        let Some(item) = l.items.get(index) else {
            return;
        };
        if !markable(item) {
            return;
        }
        let k = item.key();
        if !l.marked.remove(&k) {
            l.marked.insert(k);
        }
        l.anchor = Some(index);
        l.selected = index;
    }

    /// Pick every row from where the last pick was to `index`, the way
    /// Shift-click does. With nothing picked yet, it starts from the
    /// selected row.
    pub fn mark_range(&mut self, index: usize) {
        let Some(l) = self.level_sel() else { return };
        if index >= l.items.len() {
            return;
        }
        let from = l.anchor.unwrap_or(l.selected).min(l.items.len() - 1);
        let (a, b) = if from <= index {
            (from, index)
        } else {
            (index, from)
        };
        let keys: Vec<String> = l.items[a..=b]
            .iter()
            .filter(|i| markable(i))
            .map(Item::key)
            .collect();
        l.marked.extend(keys);
        l.selected = index;
    }

    /// Pick every row in the list.
    pub fn mark_all(&mut self) {
        let Some(l) = self.level_sel() else { return };
        let keys: Vec<String> = l
            .items
            .iter()
            .filter(|i| markable(i))
            .map(Item::key)
            .collect();
        l.marked.extend(keys);
    }

    pub fn clear_marks(&mut self) -> bool {
        let Some(l) = self.level_sel() else {
            return false;
        };
        let had = !l.marked.is_empty();
        l.marked.clear();
        l.anchor = None;
        had
    }

    /// Play the marked rows, in list order.
    pub(super) fn play_marked(&mut self) -> super::Activated {
        let items = self.selection_items();
        let tracks: Vec<TrackInfo> = self.playable(
            items
                .iter()
                .flat_map(|i| super::infos_of(self.lib.as_ref(), i, &self.prefs))
                .collect(),
        );
        if tracks.is_empty() {
            self.note("Nothing marked can play right now");
            return super::Activated::Nothing;
        }
        let n = tracks.len();
        self.end_mix();
        self.player.send(Command::SetQueueTracks {
            tracks,
            start: 0,
            pos: Duration::ZERO,
        });
        self.note(format!("Playing {}", count(n)));
        super::Activated::Played
    }

    /// Move the selected row of the open playlist to `to`, the way a drag
    /// drops it. Marked rows don't move with it: a drag is one row.
    pub fn move_playlist_item(&mut self, from: usize, to: usize) {
        let Some(id) = self.in_playlist() else { return };
        if id == crate::library::HEARTED {
            return self.note("Hearted songs is in the order you hearted them");
        }
        if self.playlist_row(id).is_some_and(|p| p.readonly) {
            return self.note(
                "That playlist is someone else's on the server; it can't be changed from here",
            );
        }
        let Some(level) = self.level() else { return };
        let mut uris: Vec<Vec<u8>> = level
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Track(t) => Some(t.uri()),
                Item::Missing { uri } => Some(uri.clone()),
                _ => None,
            })
            .collect();
        if from >= uris.len() || from == to {
            return;
        }
        let to = to.min(uris.len() - 1);
        let u = uris.remove(from);
        uris.insert(to, u);
        self.store_playlist(id, uris, to, None);
    }

    // ----- The queue -----

    pub fn is_queue_marked(&self, pos: usize) -> bool {
        if self.queue_marks.is_empty() {
            return false;
        }
        let q = self.player.queue();
        q.at(pos).is_some_and(|e| self.queue_marks.contains(&e.id))
    }

    pub fn toggle_queue_mark(&mut self, pos: usize) {
        let id = self.player.queue().at(pos).map(|e| e.id);
        let Some(id) = id else { return };
        if let Some(i) = self.queue_marks.iter().position(|&x| x == id) {
            self.queue_marks.remove(i);
        } else {
            self.queue_marks.push(id);
        }
        self.queue_anchor = Some(pos);
        self.touch();
    }

    pub fn mark_queue_range(&mut self, pos: usize) {
        let from = self.queue_anchor.unwrap_or(pos);
        let (a, b) = if from <= pos {
            (from, pos)
        } else {
            (pos, from)
        };
        let ids: Vec<u64> = {
            let q = self.player.queue();
            (a..=b).filter_map(|i| q.at(i).map(|e| e.id)).collect()
        };
        for id in ids {
            if !self.queue_marks.contains(&id) {
                self.queue_marks.push(id);
            }
        }
        self.touch();
    }

    pub fn clear_queue_marks(&mut self) -> bool {
        let had = !self.queue_marks.is_empty();
        self.queue_marks.clear();
        self.queue_anchor = None;
        if had {
            self.touch();
        }
        had
    }

    /// The marked queue entries in play order, or the one at `pos` when
    /// none are marked.
    fn queue_targets(&self, pos: usize) -> Vec<(u64, PathBuf)> {
        let q = self.player.queue();
        if self.queue_marks.is_empty() {
            return q
                .at(pos)
                .map(|e| vec![(e.id, e.info.path.clone())])
                .unwrap_or_default();
        }
        q.play_order()
            .iter()
            .filter(|id| self.queue_marks.contains(id))
            .filter_map(|&id| q.get(id).map(|e| (id, e.info.path.clone())))
            .collect()
    }

    /// Take the marked entries out of the queue, or the one at `pos`.
    pub fn remove_queue_selection(&mut self, pos: usize) {
        let ids: Vec<u64> = self
            .queue_targets(pos)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let n = ids.len();
        for id in ids {
            self.player.send(Command::RemoveEntry(id));
        }
        if self.clear_queue_marks() {
            self.note(format!("Took {} out of the queue", count(n)));
        }
    }

    /// The marked queue songs (or the one at `pos`), for a playlist.
    pub fn queue_selection_paths(&self, pos: usize) -> Vec<PathBuf> {
        self.queue_targets(pos)
            .into_iter()
            .map(|(_, p)| p)
            .filter(|p| !p.as_os_str().is_empty())
            .collect()
    }

    /// Move a queue entry from one place in play order to another, the way
    /// a drag drops it.
    pub fn move_queue_to(&self, from: usize, to: usize) {
        let (id, to) = {
            let q = self.player.queue();
            let Some(e) = q.at(from) else { return };
            (e.id, to.min(q.len().saturating_sub(1)))
        };
        if from != to {
            self.player.send(Command::MoveEntry { id, to });
        }
    }

    /// Queue entries that left the queue drop out of the marks.
    pub(super) fn prune_queue_marks(&mut self) {
        if self.queue_marks.is_empty() {
            return;
        }
        let q = self.player.queue();
        self.queue_marks.retain(|&id| q.contains(id));
    }

    // ----- Recently played -----

    pub fn clear_history(&mut self) {
        let Some(lib) = &self.lib else { return };
        match lib.clear_history() {
            Ok(n) => self.note(format!(
                "Cleared {n} play{} from the history. Play counts stay.",
                if n == 1 { "" } else { "s" }
            )),
            Err(e) => self.error(e.to_string()),
        }
        self.refresh_section(Section::History);
    }

    // ----- Smart playlists -----

    /// What both boxes would match, as they're typed: the songs, and how
    /// many the leave-out box takes away, so a leave-out rule that misses
    /// (a misspelled artist) shows as 0 left out.
    pub fn check_smart(&self, query: &str, exclude: &str) -> SmartCheck {
        match smart::parse_with(query, exclude) {
            Err(e) => SmartCheck {
                ok: false,
                message: e.0,
            },
            Ok(q) => {
                let lib = self.lib.as_ref();
                let n = lib.and_then(|l| l.query_count(&q).ok()).unwrap_or(0);
                let mut message = format!("Matches {}", count(n as usize));
                if q.exclude.is_some() {
                    // Counted without the limit, or a limit would hide them.
                    let unlimited = smart::Query {
                        limit: None,
                        ..q.clone()
                    };
                    let without = smart::Query {
                        exclude: None,
                        ..unlimited.clone()
                    };
                    let all = lib.and_then(|l| l.query_count(&without).ok()).unwrap_or(0);
                    let kept = lib
                        .and_then(|l| l.query_count(&unlimited).ok())
                        .unwrap_or(0);
                    message.push_str(&format!(", {} left out", (all - kept).max(0)));
                }
                // Nothing at all, from rules that say "and" where "or" was
                // likely meant: say so, rather than just "no songs".
                if n == 0
                    && let Some(hint) = q.both_hint()
                {
                    message.push_str(". ");
                    message.push_str(&hint);
                }
                SmartCheck { ok: true, message }
            }
        }
    }

    /// Save a new smart playlist. False if the query didn't work, with a
    /// note saying why, so the frontend keeps the box open.
    pub fn create_smart(&mut self, name: &str, query: &str, exclude: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            self.note("A smart playlist needs a name");
            return false;
        }
        let Some(lib) = &self.lib else {
            self.error("Smart playlists need the library");
            return false;
        };
        match lib.create_smart(name, query, exclude) {
            Ok(_) => {
                let c = self.check_smart(query, exclude);
                self.note(format!("Made the smart playlist \"{name}\". {}", c.message));
                self.refresh_section(Section::Playlists);
                true
            }
            Err(e) => {
                self.error(e.to_string());
                false
            }
        }
    }

    /// Change a smart playlist's name and both sets of rules.
    pub fn update_smart(&mut self, id: i64, name: &str, query: &str, exclude: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            self.note("A smart playlist needs a name");
            return false;
        }
        let Some(lib) = &self.lib else { return false };
        match lib.update_smart(id, name, query, exclude) {
            Ok(()) => {
                // An open copy of it has the old name in its source.
                if let Some(stack) = self.stacks.get_mut(&Section::Playlists) {
                    for l in stack.iter_mut() {
                        if let Source::Smart { id: x, name: n } = &mut l.source
                            && *x == id
                        {
                            *n = name.to_string();
                        }
                    }
                }
                self.refresh_section(Section::Playlists);
                true
            }
            Err(e) => {
                self.error(e.to_string());
                false
            }
        }
    }

    pub fn delete_smart(&mut self, id: i64) {
        let Some(lib) = &self.lib else { return };
        if let Err(e) = lib.delete_smart(id) {
            return self.error(e.to_string());
        }
        if let Some(stack) = self.stacks.get_mut(&Section::Playlists) {
            stack.retain(|l| !matches!(l.source, Source::Smart { id: x, .. } if x == id));
        }
        self.refresh_section(Section::Playlists);
        self.note("Deleted the smart playlist. The songs are still in the library.");
    }

    /// The smart playlist open or selected, for Edit.
    pub fn smart_target(&self) -> Option<smart::SmartRow> {
        let lib = self.lib.as_ref()?;
        if let Some(Source::Smart { id, .. }) = self.level().map(|l| &l.source) {
            return lib.smart_playlist(*id).ok().flatten();
        }
        match self.selected_item()? {
            Item::Smart(s) => Some(s),
            _ => None,
        }
    }

    // ----- Playlist files -----

    /// What Export would write: the selected playlist in Playlists, or the
    /// open list of songs. Its name, for the file name.
    pub fn export_subject(&self) -> Option<(String, Vec<TrackRow>)> {
        let lib = self.lib.as_ref()?;
        let level = self.level()?;
        if matches!(level.source, Source::Section(Section::Playlists)) {
            let item = level.selected_item()?;
            let name = match item {
                Item::Playlist(p) => p.name.clone(),
                Item::Smart(p) => p.name.clone(),
                Item::Mix(m) => m.name.clone(),
                _ => return None,
            };
            return Some((name, super::tracks_of(lib, item, &self.prefs)));
        }
        if !self.can_play_view() {
            return None;
        }
        let tracks = self.view_tracks();
        (!tracks.is_empty()).then(|| (level.title.clone(), tracks))
    }

    /// Where an export goes unless asked otherwise: a Playlists folder in
    /// your first music folder, or in your home folder.
    pub fn default_export_path(&self, name: &str) -> PathBuf {
        let base = self
            .config
            .library
            .folders
            .first()
            .cloned()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Playlists")
            .join(format!("{}.m3u8", playlist_io::file_name_for(name)))
    }

    /// Write what [`Self::export_subject`] names to `path`, as M3U8.
    pub fn export_to(&mut self, path: &Path) {
        let Some((name, tracks)) = self.export_subject() else {
            return self.note("Pick a playlist, or open a list of songs, to export");
        };
        self.write_export(&name, &tracks, path);
    }

    /// Write the queue's songs to `path`, as M3U8.
    pub fn export_queue_to(&mut self, path: &Path) {
        let paths = self.queue_playlist_paths();
        let tracks: Vec<TrackRow> = self
            .lib
            .as_ref()
            .map(|l| {
                paths
                    .iter()
                    .filter_map(|p| l.track_by_path(p).ok().flatten())
                    .collect()
            })
            .unwrap_or_default();
        self.write_export("Queue", &tracks, path);
    }

    fn write_export(&mut self, name: &str, tracks: &[TrackRow], path: &Path) {
        if tracks.is_empty() {
            return self.note(format!("\"{name}\" has no songs to export"));
        }
        let path = expand(path);
        let server = tracks.iter().filter(|t| t.remote_id.is_some()).count();
        match playlist_io::write_m3u8(&path, tracks) {
            Ok(n) => {
                let mut msg = format!("Exported {} to {}", count(n), path.display());
                if server > 0 {
                    msg.push_str(&format!(
                        ". {server} {} on a server, which only PatinaPlayer can read back",
                        if server == 1 { "is" } else { "are" }
                    ));
                }
                self.note(msg);
            }
            Err(e) => self.error(format!("Couldn't write {}: {e}", path.display())),
        }
    }

    /// Read an M3U, M3U8 or PLS file into a new playlist.
    pub fn import_playlist_file(&mut self, path: &Path) {
        let path = expand(path);
        if !path.is_file() {
            return self.error(format!("There's no file at {}", path.display()));
        }
        if !playlist_io::is_playlist_file(&path) {
            return self.error("That isn't an .m3u, .m3u8 or .pls file");
        }
        let name = playlist_io::name_of(&path);
        let Some(lib) = self.lib.as_mut() else {
            return self.error("Playlists need the library");
        };
        match lib.import_playlist(&path, &name) {
            Ok(r) => {
                let mut msg = format!(
                    "Imported \"{}\": {} of {} matched",
                    r.name,
                    r.matched,
                    count(r.entries)
                );
                if r.missing > 0 {
                    msg.push_str(&format!(
                        ", {} kept as missing until the files turn up",
                        r.missing
                    ));
                }
                if r.skipped > 0 {
                    msg.push_str(&format!(", {} web or server links left out", r.skipped));
                }
                self.note(msg);
                self.refresh_section(Section::Playlists);
            }
            Err(e) => self.error(format!("Couldn't import {}: {e}", path.display())),
        }
    }

    // ----- Now Playing -----

    /// The format, the path to the speakers and the sound chain for the
    /// Now Playing page.
    pub fn now_playing(&self, s: &Snapshot) -> NowPlaying {
        let Some(t) = &s.track else {
            return NowPlaying::default();
        };
        let format = if t.is_live() {
            match t.badge() {
                b if b.is_empty() => "Live radio".to_string(),
                b => format!("Live radio · {b}"),
            }
        } else {
            t.badge()
        };
        let path = match (&s.exclusive, &s.bit_perfect_refused) {
            (Some(e), _) if e.exact => format!("Bit-perfect to {} as {}", e.device, e.format),
            (Some(e), _) => format!("Exclusive and unprocessed to {}, as {}", e.device, e.format),
            (None, refused) => {
                let mut p = match s.resample {
                    Some((a, b)) => format!("Resampled {} → {} kHz", khz(a), khz(b)),
                    None if t.sample_rate > 0 => format!("At {} kHz", khz(t.sample_rate)),
                    None => String::new(),
                };
                if let Some(r) = refused {
                    if !p.is_empty() {
                        p.push_str(". ");
                    }
                    p.push_str(&format!("Not bit-perfect: {r}"));
                }
                p
            }
        };
        let bp = s.exclusive.is_some();
        let skipped = || "skipped (bit-perfect)".to_string();
        let mut chain = Vec::new();
        chain.push((
            "Leveling".to_string(),
            match s.level {
                Some(l) => l.text(),
                None => "off".into(),
            },
        ));
        chain.push((
            "Amp stack".to_string(),
            if bp {
                skipped()
            } else if self.amp.active.on && !self.amp.active.changes_sound() {
                "on, nothing in the rack touching it".into()
            } else if self.amp.active.on {
                let n = self.amp.active.units.len();
                match self
                    .amp
                    .linked
                    .as_deref()
                    .or(Some(self.amp.active.preset.as_str()))
                {
                    Some(p) if !p.is_empty() => {
                        format!("on, {p}, {n} unit{}", if n == 1 { "" } else { "s" })
                    }
                    _ => format!("on, {n} unit{}", if n == 1 { "" } else { "s" }),
                }
            } else {
                "off".into()
            },
        ));
        chain.push((
            "EQ".to_string(),
            if bp {
                skipped()
            } else if self.eq.active.on && self.eq.active.is_flat() {
                "on, flat: not touching it".into()
            } else if self.eq.active.on {
                let name = self
                    .eq
                    .linked
                    .clone()
                    .unwrap_or_else(|| self.eq.active.preset.clone());
                if name.is_empty() {
                    "on".into()
                } else {
                    format!("on, {name}")
                }
            } else {
                "off".into()
            },
        ));
        chain.push((
            "Channel tools".to_string(),
            if bp {
                skipped()
            } else if self.config.channel.on {
                "on".into()
            } else {
                "off".into()
            },
        ));
        chain.push((
            "Limiter".to_string(),
            if bp {
                skipped()
            } else if !self.config.limiter.on {
                "off".into()
            } else if s.limiting_db >= 0.1 {
                format!(
                    "{:.1} dBTP, working: -{:.1} dB",
                    self.config.limiter.ceiling_db,
                    s.limiting_db.min(99.9)
                )
            } else {
                format!(
                    "{:.1} dBTP, not touching it",
                    self.config.limiter.ceiling_db
                )
            },
        ));
        NowPlaying {
            title: t.display_title(),
            artist: t.artist.clone().unwrap_or_default(),
            album: t.album.clone().unwrap_or_default(),
            format,
            path,
            output: s.output.clone().unwrap_or_default(),
            chain,
        }
    }
}

fn khz(r: u32) -> String {
    if r.is_multiple_of(1000) {
        format!("{}", r / 1000)
    } else {
        format!("{:.1}", r as f64 / 1000.0)
    }
}

fn expand(p: &Path) -> PathBuf {
    match p.to_str() {
        Some(s) => crate::config::expand_tilde(s.trim()),
        None => p.to_path_buf(),
    }
}

/// Rows that can be picked: songs and the things that stand for songs.
fn markable(i: &Item) -> bool {
    matches!(
        i,
        Item::Track(_)
            | Item::Missing { .. }
            | Item::Album(_)
            | Item::Artist(_)
            | Item::Dir { .. }
            | Item::Root { .. }
            | Item::Genre(_)
            | Item::Playlist(_)
            | Item::Smart(_)
            | Item::Mix(_)
            | Item::Episode(_)
            | Item::Station { .. }
    )
}
