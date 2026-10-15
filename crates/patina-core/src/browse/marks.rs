//! Ratings, favorites and lyrics, as the views use them. The library
//! keeps the marks ([`crate::library::marks`]); this decides what a row,
//! a view or the playing song stands for, tells servers, and runs lyrics
//! lookups off the UI thread.

use std::path::PathBuf;

use crossbeam_channel::Receiver;

use super::{Browser, Item, Section, SideEntry, Source};
use crate::library::marks::{FavKind, Favorite};
use crate::lyrics::Lyrics;
use crate::remote::ServerState;
use crate::remote::service::Job;

/// Something you can star.
#[derive(Debug, Clone, PartialEq)]
pub struct FavTarget {
    pub kind: FavKind,
    pub key: Vec<u8>,
    pub label: String,
    /// The server it lives on, which should hear.
    pub server: Option<String>,
}

/// The lyrics panel.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsView {
    /// The song they're for: its title line, and its path for telling
    /// whether it's still the one playing.
    pub title: String,
    pub path: PathBuf,
    pub status: LyricsStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LyricsStatus {
    Looking,
    Found(Lyrics),
    /// Nobody had them. Says where it looked.
    Nothing(String),
    Failed(String),
}

pub(super) struct LyricsJob {
    rx: Receiver<Result<Option<Lyrics>, String>>,
    path: PathBuf,
}

/// What a row would be kept under in Favorites, without asking which
/// server it's on: for showing whether it's in there.
pub fn fav_key(item: &Item) -> Option<(FavKind, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt;
    match item {
        Item::Dir { path, .. } | Item::Root { path, .. } => {
            Some((FavKind::Folder, path.as_os_str().as_bytes().to_vec()))
        }
        Item::Album(a) => Some((FavKind::Album, a.key.clone().into_bytes())),
        Item::Artist(a) => Some((FavKind::Artist, a.name.clone().into_bytes())),
        _ => None,
    }
}

/// Rated, and so starred in menus: stars as text, "★★★☆☆".
pub fn stars_text(n: u8) -> String {
    let n = n.min(5) as usize;
    format!("{}{}", "★".repeat(n), "☆".repeat(5 - n))
}

impl Browser {
    // ----- Favorites -----

    fn artist_server(&self, name: &str) -> Option<String> {
        let lib = self.lib.as_ref()?;
        let root: Option<Vec<u8>> = lib
            .conn
            .query_row(
                "SELECT s.root FROM tracks t JOIN sources s ON s.id = t.source_id \
                 WHERE (t.artist = ?1 COLLATE NOCASE OR t.album_artist = ?1 COLLATE NOCASE) \
                 AND t.remote_id IS NOT NULL LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .ok();
        let root = crate::library::bytes_path(root?);
        crate::remote::server_of(&root).map(str::to_string)
    }

    fn album_target(&self, key: &str) -> Option<FavTarget> {
        let a = self.lib.as_ref()?.album(key).ok().flatten()?;
        Some(FavTarget {
            kind: FavKind::Album,
            key: a.key.clone().into_bytes(),
            label: a.name.clone(),
            server: a.server().map(str::to_string),
        })
    }

    fn folder_target(&self, path: &std::path::Path, name: Option<&str>) -> FavTarget {
        use std::os::unix::ffi::OsStrExt;
        FavTarget {
            kind: FavKind::Folder,
            key: path.as_os_str().as_bytes().to_vec(),
            label: name.map(str::to_string).unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            }),
            server: None,
        }
    }

    /// What a row stands for, if it can be starred: a folder, an album or
    /// an artist.
    pub fn fav_target(&self, item: &Item) -> Option<FavTarget> {
        match item {
            Item::Dir { path, name } => Some(self.folder_target(path, Some(name))),
            Item::Root { path, .. } => Some(self.folder_target(path, None)),
            Item::Album(a) => Some(FavTarget {
                kind: FavKind::Album,
                key: a.key.clone().into_bytes(),
                label: a.name.clone(),
                server: a.server().map(str::to_string),
            }),
            Item::Artist(a) => Some(FavTarget {
                kind: FavKind::Artist,
                key: a.name.clone().into_bytes(),
                label: a.name.clone(),
                server: self.artist_server(&a.name),
            }),
            _ => None,
        }
    }

    /// What the open view stands for: a folder, an album or an artist.
    pub fn level_fav_target(&self) -> Option<FavTarget> {
        match &self.level()?.source {
            Source::Folder(p) => Some(self.folder_target(p, None)),
            Source::Album { key } => self.album_target(key),
            Source::ArtistAlbums(name) => Some(FavTarget {
                kind: FavKind::Artist,
                key: name.clone().into_bytes(),
                label: name.clone(),
                server: self.artist_server(name),
            }),
            _ => None,
        }
    }

    pub fn is_favorite(&self, t: &FavTarget) -> bool {
        self.lib
            .as_ref()
            .and_then(|l| l.favorite_id(t.kind, &t.key).ok().flatten())
            .is_some()
    }

    /// Star it, or unstar it.
    pub fn toggle_favorite(&mut self, t: &FavTarget) {
        let Some(lib) = self.lib.as_ref() else {
            return self.note("Favorites need the library");
        };
        let res = match lib.favorite_id(t.kind, &t.key) {
            Ok(Some(id)) => lib.remove_favorite(id).map(|_| false),
            Ok(None) => lib
                .add_favorite(t.kind, &t.key, &t.label, t.server.as_deref())
                .map(|_| true),
            Err(e) => Err(e),
        };
        match res {
            Ok(on) => {
                self.note(if on {
                    format!("\"{}\" is in Favorites", t.label)
                } else {
                    format!("\"{}\" left Favorites", t.label)
                });
                if let Some(s) = &t.server {
                    self.push_marks(s);
                }
                self.rebuild_sidebar();
                self.touch();
            }
            Err(e) => self.error(format!("couldn't change Favorites: {e}")),
        }
    }

    pub fn toggle_favorite_selected(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        match self.fav_target(&item) {
            Some(t) => self.toggle_favorite(&t),
            None => self.note("Only folders, albums and artists go in Favorites"),
        }
    }

    pub fn toggle_favorite_level(&mut self) {
        if let Some(t) = self.level_fav_target() {
            self.toggle_favorite(&t);
        }
    }

    pub fn favorites(&self) -> Vec<Favorite> {
        self.lib
            .as_ref()
            .and_then(|l| l.favorites().ok())
            .unwrap_or_default()
    }

    pub fn favorite_move(&mut self, id: i64, by: i64) {
        if let Some(lib) = self.lib.as_mut()
            && let Err(e) = lib.move_favorite(id, by)
        {
            return self.error(format!("couldn't move it: {e}"));
        }
        self.rebuild_sidebar();
        // Keep the moved one selected.
        if let Some(i) = self
            .sidebar
            .iter()
            .position(|e| matches!(e, SideEntry::Favorite { id: x, .. } if *x == id))
        {
            self.sidebar_sel = i;
        }
        self.touch();
    }

    pub fn unfavorite(&mut self, id: i64) {
        let server = self
            .favorites()
            .into_iter()
            .find(|f| f.id == id)
            .and_then(|f| f.server);
        if let Some(lib) = self.lib.as_ref()
            && let Err(e) = lib.remove_favorite(id)
        {
            return self.error(format!("couldn't remove it: {e}"));
        }
        if let Some(s) = server {
            self.push_marks(&s);
        }
        self.rebuild_sidebar();
        self.touch();
    }

    /// Open a favorite: its folder, album or artist, under Folders,
    /// Albums or Artists.
    pub(super) fn open_favorite(&mut self, f: &Favorite) {
        let (section, src) = match f.kind {
            FavKind::Folder => (Section::Folders, Source::Folder(f.path())),
            FavKind::Album => (Section::Albums, Source::Album { key: f.key_str() }),
            FavKind::Artist => (Section::Artists, Source::ArtistAlbums(f.key_str())),
        };
        self.switch(section);
        if let Some(stack) = self.stacks.get_mut(&section) {
            stack.truncate(1);
        }
        self.push(src);
    }

    // ----- Ratings -----

    /// Tell a server about marks waiting for it, if it's there now. If
    /// it isn't, the next sync sends them.
    fn push_marks(&self, server: &str) {
        let ready = self
            .servers
            .iter()
            .any(|s| s.name == server && matches!(s.state, ServerState::Ready));
        if ready && let Some(r) = &self.remote {
            r.job(server, Job::PushMarks);
        }
    }

    fn rate_song_path(&mut self, path: &std::path::Path, stars: u8) {
        let server = crate::remote::server_of(path).map(str::to_string);
        let Some(lib) = self.lib.as_ref() else {
            return self.note("Ratings need the library");
        };
        if let Err(e) = lib.set_song_rating(path, stars, server.is_some()) {
            return self.error(format!("couldn't rate it: {e}"));
        }
        if let Some(s) = server {
            self.push_marks(&s);
        }
        self.playing_rating = None;
        self.after_rating();
    }

    fn rate_album_key(&mut self, key: &str, stars: u8) {
        let server = self
            .lib
            .as_ref()
            .and_then(|l| l.album(key).ok().flatten())
            .and_then(|a| a.server().map(str::to_string));
        let Some(lib) = self.lib.as_ref() else {
            return self.note("Ratings need the library");
        };
        if let Err(e) = lib.set_album_rating(key, stars, server.is_some()) {
            return self.error(format!("couldn't rate it: {e}"));
        }
        if let Some(s) = server {
            self.push_marks(&s);
        }
        self.after_rating();
    }

    fn after_rating(&mut self) {
        self.refresh_current();
        for s in [Section::Albums, Section::Songs] {
            if s != self.section {
                self.refresh_section(s);
            }
        }
        self.touch();
    }

    /// The rating a row shows: songs and albums only.
    pub fn item_rating(item: &Item) -> Option<u8> {
        match item {
            Item::Track(t) => Some(t.rating),
            Item::Album(a) => Some(a.rating),
            _ => None,
        }
    }

    /// Rate a song or an album row, 0 to 5.
    pub fn rate_item(&mut self, item: &Item, stars: u8) {
        match item {
            Item::Track(t) => self.rate_song_path(&t.path.clone(), stars),
            Item::Album(a) => self.rate_album_key(&a.key.clone(), stars),
            _ => self.note("Only songs and albums take a rating"),
        }
    }

    pub fn rate_selected(&mut self, stars: u8) {
        if let Some(item) = self.selected_item() {
            self.rate_item(&item, stars);
        }
    }

    /// The open album's rating, for its header.
    pub fn level_rating(&self) -> Option<u8> {
        match &self.level()?.source {
            Source::Album { key } => self.lib.as_ref()?.album_rating(key).ok(),
            _ => None,
        }
    }

    pub fn rate_level(&mut self, stars: u8) {
        if let Some(Source::Album { key }) = self.level().map(|l| l.source.clone()) {
            self.rate_album_key(&key, stars);
        }
    }

    /// The playing song's rating. None for radio, podcasts and songs
    /// outside the library.
    pub fn playing_rating(&mut self) -> Option<u8> {
        let t = self.player.snapshot().track?;
        if t.kind != crate::track::MediaKind::File || t.path.as_os_str().is_empty() {
            return None;
        }
        if let Some((p, r)) = &self.playing_rating
            && *p == t.path
        {
            return *r;
        }
        let lib = self.lib.as_ref()?;
        let known = lib.track_by_path(&t.path).ok().flatten().is_some();
        let r = known.then(|| lib.song_rating(&t.path).unwrap_or(0));
        self.playing_rating = Some((t.path.clone(), r));
        r
    }

    pub fn rate_playing(&mut self, stars: u8) {
        let Some(t) = self.player.snapshot().track else {
            return;
        };
        if self.playing_rating().is_none() {
            return self.note("Only songs in the library take a rating");
        }
        self.rate_song_path(&t.path, stars);
    }

    // ----- Lyrics -----

    /// Look up the playing song's lyrics and show them. Nothing is looked
    /// up until this is called, and only for that song.
    pub fn lyrics_show(&mut self) {
        let Some(t) = self.player.snapshot().track else {
            self.lyrics = None;
            return self.note("Play something first");
        };
        if t.is_live() {
            return self.note("Radio has no lyrics to look up");
        }
        let title = match &t.artist {
            Some(a) => format!("{} · {a}", t.display_title()),
            None => t.display_title(),
        };
        let online = self.config.lyrics.online;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let info = t.clone();
        std::thread::Builder::new()
            .name("patina-lyrics".into())
            .spawn(move || {
                let _ = tx.send(crate::lyrics::find(&info, online));
            })
            .ok();
        self.lyrics = Some(LyricsView {
            title,
            path: t.path.clone(),
            status: LyricsStatus::Looking,
        });
        self.lyrics_job = Some(LyricsJob {
            rx,
            path: t.path.clone(),
        });
        self.touch();
    }

    pub fn lyrics_hide(&mut self) {
        self.lyrics = None;
        self.lyrics_job = None;
        self.touch();
    }

    pub fn lyrics(&self) -> Option<&LyricsView> {
        self.lyrics.as_ref()
    }

    /// The song changed since the lyrics were looked up.
    pub fn lyrics_stale(&self) -> bool {
        let Some(v) = &self.lyrics else { return false };
        self.player
            .snapshot()
            .track
            .is_some_and(|t| t.path != v.path)
    }

    pub(super) fn poll_lyrics(&mut self) {
        let Some(job) = &self.lyrics_job else { return };
        let Ok(res) = job.rx.try_recv() else { return };
        let path = job.path.clone();
        self.lyrics_job = None;
        let Some(v) = self.lyrics.as_mut().filter(|v| v.path == path) else {
            return;
        };
        let online = self.config.lyrics.online;
        v.status = match res {
            Ok(Some(l)) => LyricsStatus::Found(l),
            Ok(None) => LyricsStatus::Nothing(if online {
                "No lyrics in the file, beside it, on its server or on LRCLIB.".into()
            } else {
                "No lyrics in the file, beside it or on its server. LRCLIB is off in Settings."
                    .into()
            }),
            Err(e) => LyricsStatus::Failed(e),
        };
        self.touch();
    }
}
