//! The Info page: everything known about a song or an album. The library's
//! side (tags as indexed, plays, rating, ReplayGain, where it lives), and
//! for a song whose file is on this device, or kept in the cache, every
//! tag in the file as it is now.

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;

use super::{Browser, Item, fmt_duration, fmt_length};
use crate::library::{Library, TrackRow};
use crate::track::TrackInfo;

/// What the Info page is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoTarget {
    /// A song, by its path (a file, or a server song's own path).
    Song(PathBuf),
    /// An album, by its key.
    Album(String),
}

/// The Info page's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoView {
    /// "SONG" or "ALBUM".
    pub kicker: String,
    pub title: String,
    pub rows: Vec<InfoRow>,
}

/// One line on the Info page: a label and its value, or a heading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoRow {
    pub label: String,
    pub value: String,
    /// A heading over the rows after it. Its value is empty.
    pub head: bool,
    /// A path or an id: break it anywhere, in a fixed-width font.
    pub code: bool,
}

/// What the index keeps of a song besides [`TrackRow`].
#[derive(Debug, Clone, Default)]
struct Extras {
    genres: Vec<String>,
    last_played: Option<i64>,
    rg: [Option<f64>; 4],
    bpm: Option<i64>,
    composer: Option<String>,
    grouping: Option<String>,
    category: Option<String>,
    comment: Option<String>,
}

type Raw = (
    Option<i64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn extras(lib: &Library, t: &TrackRow) -> Extras {
    let conn = &lib.conn;
    let raw: Option<Raw> = conn
        .query_row(
            "SELECT last_played, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, \
             bpm, composer, grouping, category, comment FROM tracks WHERE id = ?1",
            [t.id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten();
    let genres = conn
        .prepare_cached("SELECT genre FROM track_genres WHERE track_id = ?1 ORDER BY rowid")
        .and_then(|mut st| {
            st.query_map([t.id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();
    let mut e = Extras {
        genres,
        ..Default::default()
    };
    if let Some((lp, tg, tp, ag, ap, bpm, composer, grouping, category, comment)) = raw {
        e.last_played = lp;
        e.rg = [tg, tp, ag, ap];
        e.bpm = bpm;
        e.composer = composer;
        e.grouping = grouping;
        e.category = category;
        e.comment = comment;
    }
    e
}

impl Browser {
    /// What Info would show for the selected row: a song or an album.
    pub fn selected_info_target(&self) -> Option<InfoTarget> {
        match self.selected_item()? {
            Item::Track(t) => Some(InfoTarget::Song(t.path)),
            Item::Album(a) => Some(InfoTarget::Album(a.key)),
            _ => None,
        }
    }

    /// The song at a place in the queue. None for radio.
    pub fn queue_info_target(&self, pos: usize) -> Option<InfoTarget> {
        let q = self.player.queue();
        let e = q.at(pos)?;
        (!e.info.is_live()).then(|| InfoTarget::Song(e.info.path.clone()))
    }

    /// The song playing, for Info from Now Playing. None for radio.
    pub fn playing_info_target(&self) -> Option<InfoTarget> {
        let t = self.player.snapshot().track?;
        (!t.is_live()).then_some(InfoTarget::Song(t.path))
    }

    /// The Info page for a song or an album. None when there's nothing to
    /// say: an album no longer in the library, a song neither in it nor
    /// playing.
    pub fn info(&self, target: &InfoTarget) -> Option<InfoView> {
        match target {
            InfoTarget::Song(p) => self.song_info(p),
            InfoTarget::Album(k) => self.album_info(k),
        }
    }

    fn song_info(&self, path: &Path) -> Option<InfoView> {
        let lib = self.lib.as_ref();
        let row = lib.and_then(|l| l.track_by_path(path).ok().flatten());
        let mut out = Vec::new();
        let title;
        match (&row, lib) {
            (Some(t), Some(l)) => {
                title = t.title.clone();
                song_rows(&mut out, l, t);
            }
            _ => {
                // Not in the library (a file played from outside it, an
                // episode): what the player read from it.
                let t = self.player.snapshot().track.filter(|t| t.path == path)?;
                title = t.display_title();
                player_rows(&mut out, &t);
            }
        }
        // The file itself: on this device, or a server song kept in the cache.
        let file = if crate::remote::is_remote(path) {
            row.as_ref().and_then(|t| {
                let r = t.remote()?;
                lib?.cached_file(&r.server, &r.id)
            })
        } else {
            Some(path.to_path_buf())
        };
        if let Some(f) = file.filter(|f| f.is_file()) {
            file_tag_rows(&mut out, &f);
        }
        Some(InfoView {
            kicker: "SONG".into(),
            title,
            rows: out,
        })
    }

    fn album_info(&self, key: &str) -> Option<InfoView> {
        let lib = self.lib.as_ref()?;
        let a = lib.album(key).ok().flatten()?;
        let tracks = lib.album_tracks(key).unwrap_or_default();
        let mut out = Vec::new();
        head(&mut out, "Album");
        line(&mut out, "Album", &a.name);
        line(&mut out, "Album artist", &a.artist);
        // Every artist on it, for a compilation.
        let mut artists: Vec<&str> = Vec::new();
        for t in &tracks {
            if let Some(x) = t.artist.as_deref()
                && !artists.iter().any(|y| y.eq_ignore_ascii_case(x))
            {
                artists.push(x);
            }
        }
        if artists.len() > 1
            || artists
                .first()
                .is_some_and(|x| !x.eq_ignore_ascii_case(&a.artist))
        {
            line(&mut out, "Artists", &artists.join(", "));
        }
        let years = distinct(tracks.iter().filter_map(|t| t.year.map(|y| y.to_string())));
        match years.as_slice() {
            [] => opt(&mut out, "Year", a.year.map(|y| y.to_string())),
            ys => line(
                &mut out,
                if ys.len() > 1 { "Years" } else { "Year" },
                &ys.join(", "),
            ),
        }
        let mut genres: Vec<String> = Vec::new();
        for t in &tracks {
            for g in extras(lib, t).genres {
                if !genres.iter().any(|x| x.eq_ignore_ascii_case(&g)) {
                    genres.push(g);
                }
            }
        }
        if genres.is_empty() {
            opt(&mut out, "Genre", a.genre.clone());
        } else {
            line(
                &mut out,
                if genres.len() > 1 { "Genres" } else { "Genre" },
                &genres.join(", "),
            );
        }
        line(&mut out, "Songs", &a.track_count.to_string());
        let discs = distinct(
            tracks
                .iter()
                .filter_map(|t| t.disc_no.map(|d| d.to_string())),
        );
        if discs.len() > 1 {
            line(&mut out, "Discs", &discs.len().to_string());
        }
        line(&mut out, "Length", &fmt_length(a.duration));

        head(&mut out, "Listening");
        line(&mut out, "Rating", &stars(a.rating));
        line(&mut out, "Plays", &a.play_count.to_string());
        line(&mut out, "Added", &when(a.added_at));

        head(&mut out, "Files");
        let mut formats: Vec<(String, usize)> = Vec::new();
        for t in &tracks {
            let b = t.to_info().badge();
            match formats.iter_mut().find(|(f, _)| *f == b) {
                Some((_, n)) => *n += 1,
                None => formats.push((b, 1)),
            }
        }
        let formats = if formats.len() == 1 {
            formats[0].0.clone()
        } else {
            formats
                .iter()
                .map(|(f, n)| format!("{f} ({n})"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        line(&mut out, "Format", &formats);
        let size: u64 = tracks.iter().map(|t| t.size).sum();
        if size > 0 {
            line(&mut out, "Size", &bytes(size));
        }
        match a.server() {
            Some(s) => line(&mut out, "Server", s),
            None => code(&mut out, "Folder", &a.dir.display().to_string()),
        }
        Some(InfoView {
            kicker: "ALBUM".into(),
            title: a.name,
            rows: out,
        })
    }
}

fn song_rows(out: &mut Vec<InfoRow>, lib: &Library, t: &TrackRow) {
    let e = extras(lib, t);
    head(out, "Song");
    line(out, "Title", &t.title);
    opt(out, "Artist", t.artist.clone());
    opt(out, "Album artist", t.album_artist.clone());
    opt(out, "Album", t.album.clone());
    opt(out, "Track", t.track_no.map(|n| n.to_string()));
    opt(out, "Disc", t.disc_no.map(|n| n.to_string()));
    opt(out, "Year", t.year.map(|y| y.to_string()));
    if e.genres.is_empty() {
        opt(out, "Genre", t.genre.clone());
    } else {
        let label = if e.genres.len() > 1 {
            "Genres"
        } else {
            "Genre"
        };
        line(out, label, &e.genres.join(", "));
    }
    opt(out, "Composer", e.composer);
    opt(out, "Grouping", e.grouping);
    opt(out, "BPM", e.bpm.filter(|b| *b > 0).map(|b| b.to_string()));
    opt(out, "Category", e.category);
    opt(out, "Comment", e.comment);
    opt(out, "Length", t.duration.map(fmt_duration));

    head(out, "Listening");
    line(out, "Rating", &stars(t.rating));
    let loved = t
        .artist
        .as_deref()
        .and_then(|a| lib.loved(a, &t.title).ok())
        .unwrap_or(false);
    line(out, "Hearted", if loved { "Yes" } else { "No" });
    line(out, "Plays", &t.play_count.to_string());
    opt(
        out,
        "Last played",
        e.last_played.filter(|t| *t > 0).map(when),
    );
    line(out, "Added", &when(t.added_at));

    head(out, "File");
    let info = t.to_info();
    opt(out, "Format", Some(info.badge()));
    if t.sample_rate > 0 {
        line(out, "Sample rate", &format!("{} Hz", t.sample_rate));
    }
    opt(out, "Bit depth", t.bits.map(|b| format!("{b} bit")));
    if t.channels > 0 {
        line(out, "Channels", &channels(t.channels));
    }
    opt(out, "Bitrate", t.kbps.map(|k| format!("{k} kbps")));
    line(out, "Lossless", if t.lossless { "Yes" } else { "No" });
    if t.size > 0 {
        line(out, "Size", &bytes(t.size));
    }
    match t.remote() {
        Some(r) => {
            line(out, "Server", &r.server);
            code(out, "Server id", &r.id);
        }
        None => code(out, "Path", &t.path.display().to_string()),
    }

    let [tg, tp, ag, ap] = e.rg;
    if e.rg.iter().any(Option::is_some) {
        head(out, "ReplayGain");
        opt(out, "Track gain", tg.map(|g| format!("{g:+.2} dB")));
        opt(out, "Track peak", tp.map(|p| format!("{p:.6}")));
        opt(out, "Album gain", ag.map(|g| format!("{g:+.2} dB")));
        opt(out, "Album peak", ap.map(|p| format!("{p:.6}")));
    }
}

/// A song the library doesn't have, from what the player knows of it.
fn player_rows(out: &mut Vec<InfoRow>, t: &TrackInfo) {
    head(out, "Song");
    line(out, "Title", &t.display_title());
    opt(out, "Artist", t.artist.clone());
    opt(out, "Album", t.album.clone());
    opt(out, "Length", t.duration.map(fmt_duration));
    head(out, "File");
    opt(out, "Format", Some(t.badge()));
    if t.sample_rate > 0 {
        line(out, "Sample rate", &format!("{} Hz", t.sample_rate));
    }
    opt(out, "Bit depth", t.bits.map(|b| format!("{b} bit")));
    if t.channels > 0 {
        line(out, "Channels", &channels(t.channels));
    }
    opt(out, "Bitrate", t.kbps.map(|k| format!("{k} kbps")));
    code(out, "Path", &t.path.display().to_string());
}

/// Every tag in the file as it is now, and the pictures in it.
fn file_tag_rows(out: &mut Vec<InfoRow>, path: &Path) {
    use lofty::file::TaggedFileExt;
    let Ok(f) = lofty::read_from_path(path) else {
        return;
    };
    for tag in f.tags() {
        if tag.items().len() == 0 && tag.pictures().is_empty() {
            continue;
        }
        head(
            out,
            &format!(
                "Tags in the file ({})",
                words(&format!("{:?}", tag.tag_type()))
            ),
        );
        for item in tag.items() {
            let value = match item.value() {
                lofty::tag::ItemValue::Text(s) | lofty::tag::ItemValue::Locator(s) => s.clone(),
                lofty::tag::ItemValue::Binary(b) => format!("{} bytes of data", b.len()),
            };
            if value.trim().is_empty() {
                continue;
            }
            let label = words(&format!("{:?}", item.key()));
            // Ids and addresses read better unbroken by words.
            let long_token = !value.contains(' ') && value.len() > 24;
            out.push(InfoRow {
                label,
                value,
                head: false,
                code: long_token,
            });
        }
        for p in tag.pictures() {
            let kind = words(&format!("{:?}", p.pic_type()));
            let mime = p
                .mime_type()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let size = bytes(p.data().len() as u64);
            let value = if mime.is_empty() {
                size
            } else {
                format!("{mime}, {size}")
            };
            line(out, &format!("Picture: {kind}"), &value);
        }
    }
}

/// "TrackTitle" as "Track title", "MusicBrainzRecordingId" as
/// "Music brainz recording id": lofty's names read as words.
fn words(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        if i > 0 && c.is_uppercase() && (prev_lower || next_lower) {
            out.push(' ');
        }
        if i > 0 && c.is_uppercase() && next_lower {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn distinct(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for x in it {
        if !v.contains(&x) {
            v.push(x);
        }
    }
    v.sort();
    v
}

fn head(out: &mut Vec<InfoRow>, label: &str) {
    out.push(InfoRow {
        label: label.to_string(),
        head: true,
        ..Default::default()
    });
}

fn line(out: &mut Vec<InfoRow>, label: &str, value: &str) {
    out.push(InfoRow {
        label: label.to_string(),
        value: value.to_string(),
        ..Default::default()
    });
}

fn code(out: &mut Vec<InfoRow>, label: &str, value: &str) {
    out.push(InfoRow {
        label: label.to_string(),
        value: value.to_string(),
        code: true,
        ..Default::default()
    });
}

/// A row only when there's something to put in it.
fn opt(out: &mut Vec<InfoRow>, label: &str, value: Option<String>) {
    if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
        line(out, label, &v);
    }
}

fn stars(n: u8) -> String {
    match n {
        0 => "None".into(),
        n => format!("{} ({n} of 5)", "★".repeat(n.min(5) as usize)),
    }
}

fn channels(n: u16) -> String {
    match n {
        1 => "1 (mono)".into(),
        2 => "2 (stereo)".into(),
        n => n.to_string(),
    }
}

fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.2} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0} KB", n / 1e3)
    } else {
        format!("{n} bytes")
    }
}

/// A time as the date and the time of day where you are.
fn when(ts: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.format("%Y-%m-%d %H:%M").to_string()
        }
        chrono::LocalResult::None => ts.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lofty_names_read_as_words() {
        assert_eq!(words("TrackTitle"), "Track title");
        assert_eq!(words("AlbumArtist"), "Album artist");
        assert_eq!(words("MusicBrainzRecordingId"), "Music brainz recording id");
        assert_eq!(words("Id3v2"), "Id3v2");
        assert_eq!(words("VorbisComments"), "Vorbis comments");
        assert_eq!(words("ISRC"), "ISRC");
        assert_eq!(words("CoverFront"), "Cover front");
    }

    #[test]
    fn sizes_and_stars() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(34_200_000), "34.2 MB");
        assert_eq!(stars(0), "None");
        assert_eq!(stars(3), "★★★ (3 of 5)");
    }
}
