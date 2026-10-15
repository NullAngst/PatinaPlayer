//! Mixes, Plexamp style.
//!
//! - Genre mix: every song on albums tagged with all the genres you pick
//!   (or, by setting, songs tagged with all of them), shuffled up front.
//! - Artist mix: the artist and similar artists, endless.
//! - Track mix: songs similar to one song, endless.
//!
//! Who's similar comes from the servers first (Subsonic's getArtistInfo2
//! and getSimilarSongs, Ampache's get_similar), then Last.fm when a key is
//! set, then your own tags: artists and songs that share genres. Only
//! what's in your libraries plays, from every source, each song once
//! however many places it lives, preferring a local copy.
//!
//! The work happens on its own thread, since servers and Last.fm can be
//! slow; the browser hands results to the player.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::{Deserialize, Serialize};

use crate::cache::SongKey;
use crate::lastfm::LastFm;
use crate::library::mixes::SavedMixRow;
use crate::library::{LibError, Library, TrackRow};
use crate::remote::registry;

/// How many songs an endless mix adds at a time.
pub const BATCH: usize = 25;
/// An endless mix tops up when fewer than this many are left to play.
pub const REFILL_AT: usize = 10;
/// Songs picked this recently aren't picked again.
const RECENT: usize = 100;
/// The most similar artists a mix draws from.
const MAX_ARTISTS: usize = 25;

/// What a mix starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MixSeed {
    /// Normalized genre names, and how they're shown.
    Genre {
        norms: Vec<String>,
        names: Vec<String>,
        /// Match each song's own tags instead of its album's.
        by_track: bool,
    },
    Artist {
        name: String,
    },
    /// A song, by artist and title so it's found again after a re-sync
    /// or on another source.
    Track {
        artist: String,
        title: String,
        /// The server it was picked on, and its id there, for asking that
        /// server what's similar.
        #[serde(default)]
        server: Option<String>,
        #[serde(default)]
        id: Option<String>,
    },
}

impl MixSeed {
    pub fn kind(&self) -> &'static str {
        match self {
            MixSeed::Genre { .. } => "genre",
            MixSeed::Artist { .. } => "artist",
            MixSeed::Track { .. } => "track",
        }
    }

    /// "Blues Rock + Post-Grunge", "Nirvana", "About a Girl".
    pub fn name(&self) -> String {
        match self {
            MixSeed::Genre { names, .. } => names.join(" + "),
            MixSeed::Artist { name } => name.clone(),
            MixSeed::Track { title, .. } => title.clone(),
        }
    }

    /// "Blues Rock mix", "Nirvana mix", "About a Girl mix".
    pub fn title(&self) -> String {
        format!("{} mix", self.name())
    }

    pub fn endless(&self) -> bool {
        !matches!(self, MixSeed::Genre { .. })
    }

    pub fn of_track(t: &TrackRow) -> Self {
        let r = t.remote();
        MixSeed::Track {
            artist: t
                .artist
                .clone()
                .or_else(|| t.album_artist.clone())
                .unwrap_or_default(),
            title: t.title.clone(),
            server: r.as_ref().map(|r| r.server.clone()),
            id: r.map(|r| r.id),
        }
    }
}

/// Where the similar artists or songs came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    None,
    Server,
    LastFm,
    /// Your own tags: shared genres.
    Tags,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::None => "",
            Source::Server => "from your server",
            Source::LastFm => "from Last.fm",
            Source::Tags => "from your genre tags",
        }
    }
}

/// What was found similar for a mix. Saved with it, so a saved mix plays
/// the same way without a server or Last.fm.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Similar {
    #[serde(default)]
    pub artists: Vec<String>,
    /// (artist, title)
    #[serde(default)]
    pub tracks: Vec<(String, String)>,
    #[serde(default)]
    pub source: Source,
}

/// What can play right now: servers that can't be reached are left out,
/// except for their songs that are kept on disk.
#[derive(Debug, Clone, Default)]
pub struct Avail {
    pub offline: HashSet<String>,
    pub kept: HashSet<SongKey>,
}

impl Avail {
    fn plays(&self, t: &TrackRow) -> bool {
        match t.remote() {
            Some(r) if self.offline.contains(&r.server) => {
                self.kept.contains(&SongKey::new(r.server, r.id))
            }
            _ => true,
        }
    }

    /// Lower is better: a local file, then a reachable server, then a
    /// copy kept from one that isn't.
    fn rank(&self, t: &TrackRow) -> u8 {
        match t.remote() {
            None => 0,
            Some(r) if !self.offline.contains(&r.server) => 1,
            Some(_) => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MixEvent {
    /// A mix is ready: play these, in this order.
    Started {
        run: u64,
        seed: MixSeed,
        similar: Similar,
        tracks: Vec<TrackRow>,
    },
    /// More for an endless mix.
    More { run: u64, tracks: Vec<TrackRow> },
    /// Nothing in your libraries fits.
    Empty { run: u64, why: String },
}

enum Req {
    Start {
        run: u64,
        seed: MixSeed,
        /// What was found before, for a saved mix.
        similar: Option<Similar>,
        avail: Avail,
    },
    More {
        run: u64,
        avail: Avail,
    },
    LastFmKey(String),
    Quit,
}

pub struct MixService {
    tx: Sender<Req>,
    events: Receiver<MixEvent>,
}

impl MixService {
    pub fn start(db: &Path, lastfm_key: &str) -> Result<Self, LibError> {
        Self::start_with(db, LastFm::new(lastfm_key))
    }

    /// With a given Last.fm (a stand-in, in tests), or none.
    pub fn start_with(db: &Path, lastfm: Option<LastFm>) -> Result<Self, LibError> {
        let lib = Library::open(db)?;
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let mut w = Mixer {
            lib,
            lastfm,
            events: ev_tx,
            run: None,
            recent: VecDeque::new(),
            rng: Rng::new(),
        };
        std::thread::Builder::new()
            .name("patina-mixes".into())
            .spawn(move || {
                for r in rx {
                    match r {
                        Req::Quit => break,
                        Req::LastFmKey(k) => w.lastfm = LastFm::new(&k),
                        Req::Start {
                            run,
                            seed,
                            similar,
                            avail,
                        } => w.start(run, seed, similar, &avail),
                        Req::More { run, avail } => w.more(run, &avail),
                    }
                }
            })
            .expect("spawn mix thread");
        Ok(Self { tx, events: ev_rx })
    }

    pub fn events(&self) -> &Receiver<MixEvent> {
        &self.events
    }

    /// Make a mix. `run` numbers it, so answers for an older one can be
    /// told apart.
    pub fn play(&self, run: u64, seed: MixSeed, avail: Avail) {
        let _ = self.tx.send(Req::Start {
            run,
            seed,
            similar: None,
            avail,
        });
    }

    /// Play a saved mix, with what was found similar when it was saved.
    pub fn play_saved(&self, run: u64, row: &SavedMixRow, avail: Avail) -> bool {
        let Ok(seed) = serde_json::from_str::<MixSeed>(&row.seed) else {
            return false;
        };
        let similar = serde_json::from_str::<Similar>(&row.similar).ok();
        let _ = self.tx.send(Req::Start {
            run,
            seed,
            similar,
            avail,
        });
        true
    }

    /// The next batch for an endless mix.
    pub fn more(&self, run: u64, avail: Avail) {
        let _ = self.tx.send(Req::More { run, avail });
    }

    pub fn set_lastfm_key(&self, key: &str) {
        let _ = self.tx.send(Req::LastFmKey(key.to_string()));
    }
}

impl Drop for MixService {
    fn drop(&mut self) {
        let _ = self.tx.send(Req::Quit);
    }
}

/// One song per artist and title, wherever it lives.
fn song_key(t: &TrackRow) -> String {
    let who = t
        .artist
        .as_deref()
        .or(t.album_artist.as_deref())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    format!("{who}\u{1f}{}", t.title.trim().to_lowercase())
}

/// Keep one copy of each song: the best place to play it from, and only
/// if it can play right now.
fn dedupe(tracks: Vec<TrackRow>, avail: &Avail) -> Vec<TrackRow> {
    let mut best: HashMap<String, TrackRow> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for t in tracks {
        if !avail.plays(&t) {
            continue;
        }
        let k = song_key(&t);
        match best.get(&k) {
            Some(b) if avail.rank(b) <= avail.rank(&t) => {}
            Some(_) => {
                best.insert(k, t);
            }
            None => {
                order.push(k.clone());
                best.insert(k, t);
            }
        }
    }
    order.into_iter().filter_map(|k| best.remove(&k)).collect()
}

/// The mix being played.
struct Run {
    id: u64,
    seed: MixSeed,
    /// The seed artist's own songs (artist mix) or the seed song's
    /// neighbours (track mix).
    near: Vec<TrackRow>,
    /// Everything else it draws from.
    far: Vec<TrackRow>,
    /// Songs whose similar songs were already looked up (track mix).
    walked: HashSet<String>,
    last_artist: String,
}

struct Mixer {
    lib: Library,
    lastfm: Option<LastFm>,
    events: Sender<MixEvent>,
    run: Option<Run>,
    /// Keys of the last songs picked, across mixes.
    recent: VecDeque<String>,
    rng: Rng,
}

impl Mixer {
    fn emit(&self, e: MixEvent) {
        let _ = self.events.send(e);
    }

    fn start(&mut self, run: u64, seed: MixSeed, similar: Option<Similar>, avail: &Avail) {
        self.run = None;
        let result = match &seed {
            MixSeed::Genre {
                norms, by_track, ..
            } => self.genre(run, &seed, norms, *by_track, avail),
            MixSeed::Artist { name } => self.artist(run, &seed, name, similar, avail),
            MixSeed::Track {
                artist,
                title,
                server,
                id,
            } => self.track(
                run,
                &seed,
                (artist, title),
                server.as_deref().zip(id.as_deref()),
                similar,
                avail,
            ),
        };
        if let Err(why) = result {
            self.emit(MixEvent::Empty { run, why });
        }
    }

    fn genre(
        &mut self,
        run: u64,
        seed: &MixSeed,
        norms: &[String],
        by_track: bool,
        avail: &Avail,
    ) -> Result<(), String> {
        let rows = self
            .lib
            .genre_mix_tracks(norms, by_track)
            .map_err(|e| e.to_string())?;
        let mut tracks = dedupe(rows, avail);
        if tracks.is_empty() {
            return Err(if norms.len() > 1 {
                "No albums carry all of those genres".into()
            } else {
                "Nothing in that genre can play right now".into()
            });
        }
        self.rng.shuffle(&mut tracks);
        self.emit(MixEvent::Started {
            run,
            seed: seed.clone(),
            similar: Similar::default(),
            tracks,
        });
        Ok(())
    }

    fn artist(
        &mut self,
        run: u64,
        seed: &MixSeed,
        name: &str,
        saved: Option<Similar>,
        avail: &Avail,
    ) -> Result<(), String> {
        let similar = match saved {
            Some(s) if !s.artists.is_empty() => s,
            _ => self.similar_artists(name),
        };
        let near = dedupe(
            self.lib.tracks_by_artist(name).map_err(|e| e.to_string())?,
            avail,
        );
        let mut far = Vec::new();
        for a in &similar.artists {
            far.extend(self.lib.tracks_by_artist(a).unwrap_or_default());
        }
        let near_keys: HashSet<String> = near.iter().map(song_key).collect();
        let far: Vec<TrackRow> = dedupe(far, avail)
            .into_iter()
            .filter(|t| !near_keys.contains(&song_key(t)))
            .collect();
        if near.is_empty() && far.is_empty() {
            return Err(format!(
                "Nothing by {name} or anyone like them can play right now"
            ));
        }
        self.begin(run, seed.clone(), similar, near, far)
    }

    fn track(
        &mut self,
        run: u64,
        seed: &MixSeed,
        (artist, title): (&str, &str),
        on_server: Option<(&str, &str)>,
        saved: Option<Similar>,
        avail: &Avail,
    ) -> Result<(), String> {
        let similar = match saved {
            Some(s) if !s.tracks.is_empty() || !s.artists.is_empty() => s,
            _ => self.similar_tracks(artist, title, on_server),
        };
        let mut near = Vec::new();
        for (a, t) in &similar.tracks {
            near.extend(self.lib.find_tracks(a, t).unwrap_or_default());
        }
        // Around the edges: the artist's own songs and similar artists'
        // songs that share a genre with the seed.
        let seed_rows = self.lib.find_tracks(artist, title).unwrap_or_default();
        let genres: HashSet<String> = seed_rows
            .iter()
            .flat_map(|t| self.lib.track_genres(t.id).unwrap_or_default())
            .collect();
        let mut far = self.lib.tracks_by_artist(artist).unwrap_or_default();
        for a in &similar.artists {
            far.extend(self.lib.tracks_by_artist(a).unwrap_or_default());
        }
        if !genres.is_empty() {
            far.retain(|t| {
                self.lib
                    .track_genres(t.id)
                    .unwrap_or_default()
                    .iter()
                    .any(|g| genres.contains(g))
            });
        }
        let seed_key = format!(
            "{}\u{1f}{}",
            artist.trim().to_lowercase(),
            title.trim().to_lowercase()
        );
        let near: Vec<TrackRow> = dedupe(near, avail)
            .into_iter()
            .filter(|t| song_key(t) != seed_key)
            .collect();
        let near_keys: HashSet<String> = near.iter().map(song_key).collect();
        let far: Vec<TrackRow> = dedupe(far, avail)
            .into_iter()
            .filter(|t| song_key(t) != seed_key && !near_keys.contains(&song_key(t)))
            .collect();
        if near.is_empty() && far.is_empty() {
            return Err(format!("Nothing like {title} can play right now"));
        }
        // The song itself leads.
        let first = dedupe(seed_rows, avail).into_iter().next();
        self.begin_with(run, seed.clone(), similar, near, far, first)
    }

    fn begin(
        &mut self,
        run: u64,
        seed: MixSeed,
        similar: Similar,
        near: Vec<TrackRow>,
        far: Vec<TrackRow>,
    ) -> Result<(), String> {
        self.begin_with(run, seed, similar, near, far, None)
    }

    fn begin_with(
        &mut self,
        run: u64,
        seed: MixSeed,
        similar: Similar,
        near: Vec<TrackRow>,
        far: Vec<TrackRow>,
        first: Option<TrackRow>,
    ) -> Result<(), String> {
        let mut walked = HashSet::new();
        if let Some(f) = &first {
            walked.insert(song_key(f));
        }
        self.run = Some(Run {
            id: run,
            seed: seed.clone(),
            near,
            far,
            walked,
            last_artist: String::new(),
        });
        let mut tracks: Vec<TrackRow> = first.into_iter().collect();
        for t in &tracks {
            self.remember(song_key(t));
        }
        tracks.extend(self.pick(BATCH - tracks.len()));
        if tracks.is_empty() {
            return Err("Nothing to play".into());
        }
        self.emit(MixEvent::Started {
            run,
            seed,
            similar,
            tracks,
        });
        Ok(())
    }

    fn more(&mut self, run: u64, avail: &Avail) {
        if self.run.as_ref().is_none_or(|r| r.id != run) {
            return;
        }
        // A server may have gone away since: drop what can't play now.
        if let Some(r) = self.run.as_mut() {
            r.near.retain(|t| avail.plays(t));
            r.far.retain(|t| avail.plays(t));
        }
        self.walk(avail);
        let tracks = self.pick(BATCH);
        self.emit(MixEvent::More { run, tracks });
    }

    /// A track mix running low on songs it hasn't played: look up what's
    /// similar to the songs it picked, and add those.
    fn walk(&mut self, avail: &Avail) {
        let Some(r) = self.run.as_ref() else { return };
        if !matches!(r.seed, MixSeed::Track { .. }) || self.fresh(&r.near) >= BATCH {
            return;
        }
        let picks: Vec<TrackRow> = r
            .near
            .iter()
            .filter(|t| !r.walked.contains(&song_key(t)))
            .take(3)
            .cloned()
            .collect();
        let mut found = Vec::new();
        for t in &picks {
            let artist = t.artist.clone().unwrap_or_default();
            let on = t.remote();
            let s = self.similar_tracks(
                &artist,
                &t.title,
                on.as_ref().map(|r| (r.server.as_str(), r.id.as_str())),
            );
            for (a, title) in &s.tracks {
                found.extend(self.lib.find_tracks(a, title).unwrap_or_default());
            }
        }
        let Some(r) = self.run.as_mut() else { return };
        for t in &picks {
            r.walked.insert(song_key(t));
        }
        let have: HashSet<String> = r.near.iter().chain(&r.far).map(song_key).collect();
        r.near.extend(
            dedupe(found, avail)
                .into_iter()
                .filter(|t| !have.contains(&song_key(t))),
        );
    }

    fn fresh(&self, pool: &[TrackRow]) -> usize {
        pool.iter()
            .filter(|t| !self.recent.contains(&song_key(t)))
            .count()
    }

    fn remember(&mut self, k: String) {
        self.recent.push_back(k);
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
    }

    /// Up to `n` songs: about a third from `near`, the rest from `far`,
    /// none picked lately, and not the same artist twice running when
    /// there's a choice.
    fn pick(&mut self, n: usize) -> Vec<TrackRow> {
        let mut out = Vec::new();
        for _ in 0..n {
            let Some(r) = self.run.as_ref() else { break };
            let total = r.near.len() + r.far.len();
            if total == 0 {
                break;
            }
            // A small pool can't avoid the last hundred: avoid the last
            // three quarters of it, so it goes round before repeating
            // without settling into the same order.
            let window = RECENT.min(total * 3 / 4);
            let recent: HashSet<&String> = self.recent.iter().rev().take(window).collect();
            let ok = |t: &TrackRow| !recent.contains(&song_key(t));
            let near: Vec<&TrackRow> = r.near.iter().filter(|t| ok(t)).collect();
            let far: Vec<&TrackRow> = r.far.iter().filter(|t| ok(t)).collect();
            let near_share = match r.seed {
                // Mostly the songs found similar; the rest round it out.
                MixSeed::Track { .. } => 3,
                _ => 1,
            };
            let from_near = !near.is_empty() && (far.is_empty() || self.rng.below(4) < near_share);
            let pool = if from_near { near } else { far };
            if pool.is_empty() {
                break;
            }
            let other: Vec<&TrackRow> = pool
                .iter()
                .copied()
                .filter(|t| {
                    !t.artist
                        .as_deref()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(&r.last_artist)
                })
                .collect();
            let pool = if other.is_empty() { pool } else { other };
            let t = pool[self.rng.below(pool.len())].clone();
            let artist = t.artist.clone().unwrap_or_default();
            self.remember(song_key(&t));
            if let Some(r) = self.run.as_mut() {
                r.last_artist = artist;
            }
            out.push(t);
        }
        out
    }

    // ----- Who's similar -----

    /// Artists like `name` that are in the index: from the servers, then
    /// Last.fm, then shared genres.
    fn similar_artists(&self, name: &str) -> Similar {
        let keep = |names: Vec<String>| -> Vec<String> {
            let mut seen = HashSet::new();
            names
                .into_iter()
                .filter_map(|n| self.lib.artist_name(&n).ok().flatten())
                .filter(|n| !n.eq_ignore_ascii_case(name) && seen.insert(n.to_lowercase()))
                .take(MAX_ARTISTS)
                .collect()
        };
        let mut from_servers = Vec::new();
        for server in registry().connected() {
            if let Ok(c) = registry().client(&server) {
                match c.similar_artists(name, 50) {
                    Ok(v) => from_servers.extend(v),
                    Err(e) => log::debug!("{server} has no similar artists: {e}"),
                }
            }
        }
        let a = keep(from_servers);
        if !a.is_empty() {
            return Similar {
                artists: a,
                source: Source::Server,
                ..Default::default()
            };
        }
        if let Some(fm) = &self.lastfm {
            match fm.similar_artists(name, 100) {
                Ok(v) => {
                    let a = keep(v);
                    if !a.is_empty() {
                        return Similar {
                            artists: a,
                            source: Source::LastFm,
                            ..Default::default()
                        };
                    }
                }
                Err(e) => log::info!("Last.fm: {e}"),
            }
        }
        let a = keep(
            self.lib
                .artists_sharing_genres(name, MAX_ARTISTS)
                .unwrap_or_default(),
        );
        Similar {
            source: if a.is_empty() {
                Source::None
            } else {
                Source::Tags
            },
            artists: a,
            ..Default::default()
        }
    }

    /// Songs like this one that are in the index, as (artist, title),
    /// plus similar artists for around the edges.
    fn similar_tracks(&self, artist: &str, title: &str, on: Option<(&str, &str)>) -> Similar {
        let mut tracks: Vec<(String, String)> = Vec::new();
        let mut source = Source::None;
        let pair = |t: &TrackRow| {
            (
                t.artist
                    .clone()
                    .or_else(|| t.album_artist.clone())
                    .unwrap_or_default(),
                t.title.clone(),
            )
        };
        if let Some((server, id)) = on
            && let Ok(c) = registry().client(server)
        {
            match c.similar_songs(id, 50) {
                Ok(ids) => {
                    let rows = self.lib.tracks_by_remote(server, &ids).unwrap_or_default();
                    tracks.extend(rows.into_iter().flatten().map(|t| pair(&t)));
                    if !tracks.is_empty() {
                        source = Source::Server;
                    }
                }
                Err(e) => log::debug!("{server} has no similar songs: {e}"),
            }
        }
        if tracks.len() < 5
            && let Some(fm) = &self.lastfm
        {
            match fm.similar_tracks(artist, title, 100) {
                Ok(v) => {
                    let before = tracks.len();
                    for (a, t) in v {
                        if let Some(row) = self
                            .lib
                            .find_tracks(&a, &t)
                            .ok()
                            .and_then(|r| r.into_iter().next())
                        {
                            tracks.push(pair(&row));
                        }
                    }
                    if tracks.len() > before && source == Source::None {
                        source = Source::LastFm;
                    }
                }
                Err(e) => log::info!("Last.fm: {e}"),
            }
        }
        let artists = self.similar_artists(artist);
        if source == Source::None {
            source = artists.source;
        }
        let mut seen = HashSet::new();
        tracks.retain(|p| seen.insert((p.0.to_lowercase(), p.1.to_lowercase())));
        Similar {
            artists: artists.artists,
            tracks,
            source,
        }
    }
}

/// The songs a saved mix stands for, for keeping it offline: a genre mix's
/// whole set, an artist mix's artists, a track mix's songs and artists.
pub fn resolve_saved(lib: &Library, row: &SavedMixRow) -> Vec<TrackRow> {
    let Ok(seed) = serde_json::from_str::<MixSeed>(&row.seed) else {
        return Vec::new();
    };
    let similar: Similar = serde_json::from_str(&row.similar).unwrap_or_default();
    let all = Avail::default();
    let rows = match seed {
        MixSeed::Genre {
            norms, by_track, ..
        } => lib.genre_mix_tracks(&norms, by_track).unwrap_or_default(),
        MixSeed::Artist { name } => {
            let mut v = lib.tracks_by_artist(&name).unwrap_or_default();
            for a in &similar.artists {
                v.extend(lib.tracks_by_artist(a).unwrap_or_default());
            }
            v
        }
        MixSeed::Track { artist, title, .. } => {
            let mut v = lib.find_tracks(&artist, &title).unwrap_or_default();
            for (a, t) in &similar.tracks {
                v.extend(lib.find_tracks(a, t).unwrap_or_default());
            }
            v
        }
    };
    dedupe(rows, &all)
}

/// xorshift64*, like the queue's shuffle. Plenty for picking songs.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_round_trip_as_json() {
        let s = MixSeed::Track {
            artist: "Nirvana".into(),
            title: "Blew".into(),
            server: Some("Home".into()),
            id: Some("11".into()),
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<MixSeed>(&j).unwrap(), s);
        assert_eq!(s.title(), "Blew mix");
        let g = MixSeed::Genre {
            norms: vec!["blues-rock".into(), "post-grunge".into()],
            names: vec!["Blues Rock".into(), "Post-Grunge".into()],
            by_track: false,
        };
        assert_eq!(g.name(), "Blues Rock + Post-Grunge");
        assert!(!g.endless());
    }

    #[test]
    fn shuffle_keeps_everything() {
        let mut r = Rng::new();
        let mut v: Vec<u32> = (0..100).collect();
        r.shuffle(&mut v);
        let mut s = v.clone();
        s.sort();
        assert_eq!(s, (0..100).collect::<Vec<_>>());
        assert_ne!(v, s, "a hundred in order would be a bad shuffle");
    }
}
