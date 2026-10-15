//! Mixes from the browser's side: starting them from a row, marking
//! genres for a genre mix, topping up endless mixes as the queue runs
//! down, and saved mixes in the sidebar and in Playlists.

use std::collections::HashSet;

use super::settings::{Control, Key};
use super::{Browser, Item, Section};
use crate::Command;
use crate::cache::SongKey;
use crate::config;
use crate::library::GenreRow;
use crate::library::mixes::SavedMixRow;
use crate::mix::{self, Avail, MixEvent, MixSeed, Similar};
use crate::remote::ServerState;

/// The mix playing.
#[derive(Debug, Clone)]
pub struct MixRun {
    pub id: u64,
    pub seed: MixSeed,
    pub similar: Similar,
    /// The first songs are in the queue.
    pub started: bool,
    /// Waiting on more songs.
    asked: bool,
    /// It came from (or was saved as) this saved mix.
    pub saved: Option<i64>,
}

impl Browser {
    /// What can play right now, for the mix thread.
    fn avail_now(&self) -> Avail {
        let offline: HashSet<String> = self
            .servers
            .iter()
            .filter(|s| {
                matches!(
                    s.state,
                    ServerState::Offline(_) | ServerState::NoPassword | ServerState::BadLogin
                )
            })
            .map(|s| s.name.clone())
            .collect();
        let kept: HashSet<SongKey> = self
            .cache
            .as_ref()
            .map(|c| c.stored().into_keys().collect())
            .unwrap_or_default();
        Avail { offline, kept }
    }

    /// Whether a row can start a mix: an artist, an album (its artist), a
    /// song, a genre, or a saved mix.
    pub fn can_mix(&self, item: &Item) -> bool {
        self.mixes.is_some()
            && matches!(
                item,
                Item::Artist(_) | Item::Album(_) | Item::Track(_) | Item::Genre(_) | Item::Mix(_)
            )
    }

    /// Start a mix from a row. On a genre, it's the genres marked for a
    /// mix, this one included.
    pub fn mix_item(&mut self, item: &Item) {
        let seed = match item {
            Item::Artist(a) => MixSeed::Artist {
                name: a.name.clone(),
            },
            Item::Album(a) => MixSeed::Artist {
                name: a.artist.clone(),
            },
            Item::Track(t) => MixSeed::of_track(t),
            Item::Genre(g) => {
                if !self.genre_marked(&g.norm) {
                    self.genre_marks.push(g.clone());
                }
                return self.play_genre_mix();
            }
            Item::Mix(m) => return self.play_saved_mix(m.id),
            _ => return self.note("A mix starts from an artist, an album, a song or a genre"),
        };
        self.start_mix(seed, None);
    }

    pub fn mix_selected(&mut self) {
        match self.selected_item() {
            Some(item) => self.mix_item(&item),
            None => self.note("Pick an artist, an album, a song or a genre to mix from"),
        }
    }

    fn start_mix(&mut self, seed: MixSeed, saved: Option<i64>) {
        let Some(svc) = &self.mixes else {
            return self.error("Mixes need the library");
        };
        self.mix_runs += 1;
        let id = self.mix_runs;
        let avail = self.avail_now();
        if let Some(sid) = saved {
            let row = self
                .lib
                .as_ref()
                .and_then(|l| l.saved_mix(sid).ok().flatten());
            match row {
                Some(r) => {
                    svc.play_saved(id, &r, avail);
                }
                None => return self.error("That saved mix is gone"),
            }
        } else {
            svc.play(id, seed.clone(), avail);
        }
        self.note(format!("Making a {}…", seed.title()));
        self.mix = Some(MixRun {
            id,
            seed,
            similar: Similar::default(),
            started: false,
            asked: false,
            saved,
        });
    }

    // ----- Genre marks -----

    pub fn genre_marked(&self, norm: &str) -> bool {
        self.genre_marks.iter().any(|g| g.norm == norm)
    }

    /// Mark a genre for a mix, or unmark it.
    pub fn toggle_genre_mark(&mut self, g: &GenreRow) {
        if self.genre_marked(&g.norm) {
            self.genre_marks.retain(|x| x.norm != g.norm);
        } else {
            self.genre_marks.push(g.clone());
        }
        match self.genre_marks.len() {
            0 => self.note("No genres marked"),
            1 => self.note(format!("{} marked for a mix", g.name)),
            n => self.note(format!(
                "{n} genres marked: albums carrying all of {}",
                self.genre_marks
                    .iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
        self.touch();
    }

    pub fn clear_genre_marks(&mut self) {
        self.genre_marks.clear();
        self.touch();
    }

    /// Play a mix of the marked genres, or of the selected genre when none
    /// are marked.
    pub fn play_genre_mix(&mut self) {
        let mut genres = self.genre_marks.clone();
        if genres.is_empty()
            && let Some(Item::Genre(g)) = self.selected_item()
        {
            genres.push(g);
        }
        if genres.is_empty() {
            return self.note("Mark a genre or two first (M in Genres)");
        }
        let seed = MixSeed::Genre {
            norms: genres.iter().map(|g| g.norm.clone()).collect(),
            names: genres.iter().map(|g| g.name.clone()).collect(),
            by_track: self.config.mixes.by_track,
        };
        // The marks stay until the mix starts, so an empty one can be
        // changed rather than marked again from scratch.
        self.start_mix(seed, None);
    }

    // ----- Running -----

    /// What the mix thread found.
    pub(super) fn poll_mixes(&mut self) {
        let Some(svc) = &self.mixes else { return };
        let events: Vec<MixEvent> = svc.events().try_iter().collect();
        for ev in events {
            let current = self.mix.as_ref().map(|m| m.id);
            match ev {
                MixEvent::Started {
                    run,
                    seed,
                    similar,
                    tracks,
                } if Some(run) == current => {
                    let n = tracks.len();
                    let infos = tracks.iter().map(|t| t.to_info()).collect();
                    self.player.send(Command::PlayAll {
                        tracks: infos,
                        shuffle: false,
                    });
                    let from = similar.source.label();
                    let what = if seed.endless() {
                        "endless".to_string()
                    } else {
                        super::count(n)
                    };
                    self.note(if from.is_empty() {
                        format!("Playing the {} ({what})", seed.title())
                    } else {
                        format!("Playing the {} ({what}, similar {from})", seed.title())
                    });
                    if let Some(m) = self.mix.as_mut() {
                        m.started = true;
                        m.similar = similar;
                    }
                    self.player.send(Command::SetMixing(true));
                    if matches!(seed, MixSeed::Genre { .. }) {
                        self.genre_marks.clear();
                    }
                    self.touch();
                }
                MixEvent::More { run, tracks } if Some(run) == current => {
                    if let Some(m) = self.mix.as_mut() {
                        m.asked = false;
                    }
                    if tracks.is_empty() {
                        self.note("The mix has run out of songs it can play");
                        if let Some(m) = self.mix.as_mut() {
                            // Don't keep asking.
                            m.asked = true;
                        }
                    } else {
                        self.player.send(Command::AppendTracks(
                            tracks.iter().map(|t| t.to_info()).collect(),
                        ));
                    }
                }
                MixEvent::Empty { run, why } if Some(run) == current => {
                    self.mix = None;
                    self.player.send(Command::SetMixing(false));
                    self.note(why);
                    self.touch();
                }
                _ => {}
            }
        }
    }

    /// The queue moved: top up an endless mix running low.
    pub(super) fn mix_tick(&mut self) {
        let Some(m) = &self.mix else { return };
        if !m.started || m.asked || !m.seed.endless() {
            return;
        }
        let left = self.player.queue().ahead(mix::REFILL_AT + 1).len();
        if left >= mix::REFILL_AT {
            return;
        }
        let (id, avail) = (m.id, self.avail_now());
        if let Some(m) = self.mix.as_mut() {
            m.asked = true;
        }
        if let Some(svc) = &self.mixes {
            svc.more(id, avail);
        }
    }

    /// Something else took over the queue: the mix is over.
    pub(super) fn end_mix(&mut self) {
        if self.mix.take().is_some() {
            self.player.send(Command::SetMixing(false));
            self.touch();
        }
    }

    /// "Nirvana mix", while one is playing.
    pub fn mix_title(&self) -> Option<String> {
        self.mix.as_ref().map(|m| m.seed.title())
    }

    // ----- Saved -----

    pub fn saved_mixes(&self) -> Vec<SavedMixRow> {
        self.lib
            .as_ref()
            .and_then(|l| l.saved_mixes().ok())
            .unwrap_or_default()
    }

    /// Keep the mix playing, so it's one click away in the sidebar.
    pub fn save_mix(&mut self) {
        let Some(m) = self.mix.clone() else {
            return self.note("No mix is playing");
        };
        if let Some(id) = m.saved {
            return self.note(format!("Already saved as \"{}\"", self.mix_name(id)));
        }
        let Some(lib) = &self.lib else { return };
        let seed = serde_json::to_string(&m.seed).unwrap_or_default();
        let similar = serde_json::to_string(&m.similar).unwrap_or_default();
        let name = m.seed.title();
        match lib.save_mix(m.seed.kind(), &name, &seed, &similar) {
            Ok(id) => {
                if let Some(run) = self.mix.as_mut() {
                    run.saved = Some(id);
                }
                self.note(format!("Saved \"{name}\" to the sidebar"));
            }
            Err(e) => self.error(format!("couldn't save the mix: {e}")),
        }
        self.mixes_changed();
    }

    fn mix_name(&self, id: i64) -> String {
        self.lib
            .as_ref()
            .and_then(|l| l.saved_mix(id).ok().flatten())
            .map(|m| m.name)
            .unwrap_or_default()
    }

    pub fn play_saved_mix(&mut self, id: i64) {
        let Some(row) = self
            .lib
            .as_ref()
            .and_then(|l| l.saved_mix(id).ok().flatten())
        else {
            return self.error("That saved mix is gone");
        };
        let Ok(seed) = serde_json::from_str::<MixSeed>(&row.seed) else {
            return self.error("That saved mix can't be read");
        };
        self.start_mix(seed, Some(id));
    }

    pub fn rename_mix(&mut self, id: i64, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(lib) = &self.lib
            && let Err(e) = lib.rename_mix(id, name)
        {
            return self.error(e.to_string());
        }
        self.mixes_changed();
    }

    pub fn delete_mix(&mut self, id: i64) {
        if let Some(lib) = &self.lib
            && let Err(e) = lib.delete_mix(id)
        {
            return self.error(e.to_string());
        }
        if let Some(m) = self.mix.as_mut()
            && m.saved == Some(id)
        {
            m.saved = None;
        }
        if let Some(c) = &self.cache {
            // Its offline pin went with it.
            c.resync();
        }
        self.note("Mix deleted");
        self.mixes_changed();
    }

    fn mixes_changed(&mut self) {
        self.refresh_section(Section::Playlists);
        self.rebuild_sidebar();
    }

    // ----- Settings -----

    pub(super) fn change_mix_setting(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        match (key, new) {
            (Key::GenreMatch, Control::Choice { options, index }) => {
                let v = &options[*index].0;
                w(config::set_value(&path, "mixes", "genre_match", v.as_str()))?;
                self.config.mixes.by_track = v == "track";
            }
            (Key::LastFmKey, Control::Text { value, .. }) => {
                w(config::set_value(
                    &path,
                    "lastfm",
                    "api_key",
                    value.as_str(),
                ))?;
                self.config.lastfm.api_key = value.clone();
                if let Some(svc) = &self.mixes {
                    svc.set_lastfm_key(value);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
