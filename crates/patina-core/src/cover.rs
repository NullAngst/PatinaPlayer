//! Choosing an album's cover by hand: an image file you pick, or one found
//! online. A local album's cover is saved next to its music as
//! `album_art.jpg` (or `.png`), where every other player finds it too. A
//! server album's folder, or a local one the app can't write to (most
//! folders on Android, where the app can only read music), keeps it in
//! the app's own folder instead, under that album's folder path.
//!
//! [`crate::art`] looks for these before anything else, so a cover you
//! chose wins over the one embedded in the files.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The stem of a cover file this app writes into an album's folder.
pub const STEM: &str = "album_art";
/// The extensions it writes, and looks for.
const EXTS: [&str; 3] = ["jpg", "jpeg", "png"];

/// Where a chosen cover went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// Next to the music, as this file.
    Folder(PathBuf),
    /// In the app's own folder: a server album, or a folder it can't
    /// write to.
    App(PathBuf),
}

impl Saved {
    pub fn describe(&self) -> String {
        match self {
            Saved::Folder(p) => format!(
                "Saved {} in {}",
                p.file_name().unwrap_or_default().to_string_lossy(),
                p.parent()
                    .and_then(|d| d.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
            Saved::App(_) => {
                "Saved the cover in PatinaPlayer's own folder (it can't write into that album's folder)"
                    .into()
            }
        }
    }
}

/// The app's own covers.
fn store_dir() -> PathBuf {
    crate::config::data_dir().join("covers")
}

/// The app's own cover for a folder: a file named after the folder's
/// path, so it outlives the image cache being cleared.
fn store_stem(dir: &Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    store_dir().join(format!("{:016x}", h.finish()))
}

fn with_ext(stem: &Path, ext: &str) -> PathBuf {
    let mut s = stem.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// The cover chosen for a folder, if one was: `album_art.*` in it, or
/// the app's own copy. Disc folders (CD1) look at their album's folder.
pub fn chosen(dir: &Path) -> Option<PathBuf> {
    let album = crate::library::tags::album_dir(dir);
    for d in [dir, album] {
        if !crate::remote::is_remote(d) {
            for ext in EXTS {
                let p = d.join(format!("{STEM}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        let stem = store_stem(d);
        for ext in EXTS {
            let p = with_ext(&stem, ext);
            if p.is_file() {
                return Some(p);
            }
        }
        if album == dir {
            break;
        }
    }
    None
}

/// A file this module saved: an `album_art.*`, or one of the app's own.
pub fn is_chosen_file(p: &Path) -> bool {
    p.file_stem().is_some_and(|s| s == STEM) || p.starts_with(store_dir())
}

/// What kind of image some bytes are, as the extension to save them
/// with. Anything that isn't JPEG or PNG is converted to PNG first.
fn prepare(bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let fmt = image::guess_format(bytes).map_err(|_| "That isn't an image".to_string())?;
    // Check it decodes before anything is written over.
    let img =
        image::load_from_memory(bytes).map_err(|e| format!("Couldn't read that image: {e}"))?;
    if img.width() < 16 || img.height() < 16 {
        return Err("That image is too small to be a cover".into());
    }
    match fmt {
        image::ImageFormat::Jpeg => Ok((bytes.to_vec(), "jpg")),
        image::ImageFormat::Png => Ok((bytes.to_vec(), "png")),
        _ => {
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            Ok((out.into_inner(), "png"))
        }
    }
}

/// Take any other `album_art.*` or stored copy out of the way, so the
/// new one is the only one found.
fn clear_others(stem_of: impl Fn(&str) -> PathBuf, keep: &str) {
    for ext in EXTS {
        if ext != keep {
            let _ = std::fs::remove_file(stem_of(ext));
        }
    }
}

/// Save `bytes` as the cover of the album in `dir`.
pub fn save(dir: &Path, bytes: &[u8]) -> Result<Saved, String> {
    let saved = save_now(dir, bytes)?;
    // The car's copies are made again, with the new one.
    let _ = std::fs::remove_dir_all(crate::browse::auto::car_art_dir());
    Ok(saved)
}

fn save_now(dir: &Path, bytes: &[u8]) -> Result<Saved, String> {
    let (data, ext) = prepare(bytes)?;
    let dir = crate::library::tags::album_dir(dir);
    if !crate::remote::is_remote(dir) && dir.is_dir() {
        let path = dir.join(format!("{STEM}.{ext}"));
        let tmp = dir.join(format!(".{STEM}.{ext}.part"));
        let wrote = std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &path));
        match wrote {
            Ok(()) => {
                clear_others(|e| dir.join(format!("{STEM}.{e}")), ext);
                // A copy kept from before, when the folder couldn't be
                // written to, would hide this one.
                let stem = store_stem(dir);
                clear_others(|e| with_ext(&stem, e), "");
                return Ok(Saved::Folder(path));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                log::info!("can't write a cover into {}: {e}", dir.display());
            }
        }
    }
    let stem = store_stem(dir);
    std::fs::create_dir_all(store_dir()).map_err(|e| format!("Couldn't save the cover: {e}"))?;
    let path = with_ext(&stem, ext);
    std::fs::write(&path, &data).map_err(|e| format!("Couldn't save the cover: {e}"))?;
    clear_others(|e| with_ext(&stem, e), ext);
    Ok(Saved::App(path))
}

/// Save an image file as the cover of the album in `dir`.
pub fn save_file(dir: &Path, file: &Path) -> Result<Saved, String> {
    let meta = std::fs::metadata(file).map_err(|e| format!("Couldn't read that file: {e}"))?;
    if meta.len() > 40 << 20 {
        return Err("That file is too big to be a cover".into());
    }
    let bytes = std::fs::read(file).map_err(|e| format!("Couldn't read that file: {e}"))?;
    save(dir, &bytes)
}

// ----- Looking online -----

/// A cover found online.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where it's from: "iTunes", "Deezer", "MusicBrainz".
    pub source: &'static str,
    /// The album and artist as that source names them.
    pub title: String,
    /// A small copy, for picking from.
    pub thumb: String,
    /// The biggest copy it offers.
    pub full: String,
}

fn http() -> Option<reqwest::blocking::Client> {
    crate::net::client_builder()
        .user_agent(concat!(
            "PatinaPlayer/",
            env!("CARGO_PKG_VERSION"),
            " ( https://github.com/NullAngst/PatinaPlayer )"
        ))
        .timeout(Duration::from_secs(12))
        .build()
        .ok()
}

fn get_json(
    c: &reqwest::blocking::Client,
    url: &str,
    query: &[(&str, &str)],
) -> Option<serde_json::Value> {
    let r = c.get(url).query(query).send().ok()?;
    if !r.status().is_success() {
        log::info!("cover search: {url} answered {}", r.status());
        return None;
    }
    serde_json::from_slice(&r.bytes().ok()?).ok()
}

/// What iTunes' search answered.
pub fn parse_itunes(v: &serde_json::Value) -> Vec<Found> {
    let Some(rs) = v.get("results").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    rs.iter()
        .filter_map(|r| {
            let art = r.get("artworkUrl100")?.as_str()?;
            let title = format!(
                "{} · {}",
                r.get("collectionName")?.as_str()?,
                r.get("artistName").and_then(|a| a.as_str()).unwrap_or("")
            );
            // The same picture at any size: the size is in its name.
            Some(Found {
                source: "iTunes",
                title,
                thumb: art.replace("100x100bb", "300x300bb"),
                full: art.replace("100x100bb", "1200x1200bb"),
            })
        })
        .collect()
}

/// What Deezer's album search answered.
pub fn parse_deezer(v: &serde_json::Value) -> Vec<Found> {
    let Some(rs) = v.get("data").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    rs.iter()
        .filter_map(|r| {
            let thumb = r
                .get("cover_medium")
                .or_else(|| r.get("cover_big"))?
                .as_str()?;
            let full = r
                .get("cover_xl")
                .or_else(|| r.get("cover_big"))
                .and_then(|c| c.as_str())
                .unwrap_or(thumb);
            let artist = r
                .get("artist")
                .and_then(|a| a.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            Some(Found {
                source: "Deezer",
                title: format!("{} · {artist}", r.get("title")?.as_str()?),
                thumb: thumb.to_string(),
                full: full.to_string(),
            })
        })
        .collect()
}

/// What MusicBrainz's release group search answered, as Cover Art
/// Archive pictures. Not every release group has one; those fail to load
/// and are left out of what's shown.
pub fn parse_musicbrainz(v: &serde_json::Value) -> Vec<Found> {
    let Some(rs) = v.get("release-groups").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    rs.iter()
        .filter_map(|r| {
            let id = r.get("id")?.as_str()?;
            let artist: String = r
                .get("artist-credit")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| {
                            let name = c.get("name")?.as_str()?;
                            let join = c.get("joinphrase").and_then(|j| j.as_str()).unwrap_or("");
                            Some(format!("{name}{join}"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Found {
                source: "MusicBrainz",
                title: format!("{} · {artist}", r.get("title")?.as_str()?),
                thumb: format!("https://coverartarchive.org/release-group/{id}/front-250"),
                full: format!("https://coverartarchive.org/release-group/{id}/front-1200"),
            })
        })
        .collect()
}

/// Lucene's special characters, escaped for a MusicBrainz query.
fn lucene(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Look for covers of an album. Asks iTunes, Deezer and MusicBrainz, and
/// takes a few from each, best matches first.
pub fn search(artist: &str, album: &str) -> Vec<Found> {
    let Some(c) = http() else { return Vec::new() };
    let artist = artist.trim();
    let album = album.trim();
    let term = format!("{artist} {album}");
    let term = term.trim();
    if term.is_empty() {
        return Vec::new();
    }
    let (it, dz, mb) = std::thread::scope(|s| {
        let it = s.spawn(|| {
            get_json(
                &c,
                "https://itunes.apple.com/search",
                &[("term", term), ("entity", "album"), ("limit", "8")],
            )
            .map(|v| parse_itunes(&v))
            .unwrap_or_default()
        });
        let dz = s.spawn(|| {
            let q = match (artist.is_empty(), album.is_empty()) {
                (false, false) => format!("artist:\"{artist}\" album:\"{album}\""),
                _ => term.to_string(),
            };
            get_json(
                &c,
                "https://api.deezer.com/search/album",
                &[("q", q.as_str()), ("limit", "8")],
            )
            .map(|v| parse_deezer(&v))
            .unwrap_or_default()
        });
        let mb = s.spawn(|| {
            let mut q = format!(
                "releasegroup:\"{}\"",
                lucene(if album.is_empty() { term } else { album })
            );
            if !artist.is_empty() {
                q.push_str(&format!(" AND artist:\"{}\"", lucene(artist)));
            }
            get_json(
                &c,
                "https://musicbrainz.org/ws/2/release-group/",
                &[("query", q.as_str()), ("fmt", "json"), ("limit", "6")],
            )
            .map(|v| parse_musicbrainz(&v))
            .unwrap_or_default()
        });
        (
            it.join().unwrap_or_default(),
            dz.join().unwrap_or_default(),
            mb.join().unwrap_or_default(),
        )
    });
    // Take turns between the sources, so the first few aren't all one.
    let mut out = Vec::new();
    let (mut it, mut dz, mut mb) = (it.into_iter(), dz.into_iter(), mb.into_iter());
    loop {
        let mut any = false;
        for next in [it.next(), dz.next(), mb.next()].into_iter().flatten() {
            any = true;
            if !out.iter().any(|f: &Found| f.full == next.full) {
                out.push(next);
            }
        }
        if !any {
            break;
        }
    }
    out.truncate(18);
    out
}

/// Download one picture.
pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let c = http().ok_or("No network client")?;
    let r = c.get(url).send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("the server answered {}", r.status()));
    }
    let b = r.bytes().map_err(|e| e.to_string())?;
    if b.len() > 40 << 20 {
        return Err("that picture is too big".into());
    }
    Ok(b.to_vec())
}

/// A picture as a square thumbnail, for picking from.
pub fn thumbnail(bytes: &[u8], size: u32) -> Option<image::RgbaImage> {
    let img = image::load_from_memory(bytes).ok()?;
    Some(crate::art::square(img, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([90, 140, 120]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    #[test]
    fn parses_each_source() {
        let it = serde_json::json!({"resultCount": 1, "results": [{
            "collectionName": "Bleach", "artistName": "Nirvana",
            "artworkUrl100": "https://x/100x100bb.jpg"}]});
        let f = parse_itunes(&it);
        assert_eq!(f[0].full, "https://x/1200x1200bb.jpg");
        assert_eq!(f[0].title, "Bleach · Nirvana");
        let dz = serde_json::json!({"data": [{"title": "Bleach", "cover_medium": "m", "cover_xl": "xl",
            "artist": {"name": "Nirvana"}}]});
        let f = parse_deezer(&dz);
        assert_eq!((f[0].thumb.as_str(), f[0].full.as_str()), ("m", "xl"));
        let mb = serde_json::json!({"release-groups": [{"id": "abc", "title": "Bleach",
            "artist-credit": [{"name": "Nirvana", "joinphrase": ""}]}]});
        let f = parse_musicbrainz(&mb);
        assert!(f[0].full.ends_with("/release-group/abc/front-1200"));
        assert!(parse_deezer(&serde_json::json!({"error": {}})).is_empty());
    }

    #[test]
    fn lucene_escapes() {
        assert_eq!(lucene("AC/DC: Live!"), "AC\\/DC\\: Live\\!");
    }

    #[test]
    fn rejects_what_isnt_a_cover() {
        let d = tempfile::tempdir().unwrap();
        assert!(save(d.path(), b"not a picture").is_err());
        assert!(save(d.path(), &jpeg(4, 4)).is_err());
    }

    #[test]
    fn saves_next_to_the_music_and_is_found() {
        let d = tempfile::tempdir().unwrap();
        let album = d.path().join("Album");
        std::fs::create_dir_all(album.join("CD2")).unwrap();
        let saved = save(&album.join("CD2"), &jpeg(64, 64)).unwrap();
        assert_eq!(saved, Saved::Folder(album.join("album_art.jpg")));
        assert_eq!(
            chosen(&album.join("CD2")),
            Some(album.join("album_art.jpg"))
        );
        // A PNG after it replaces it, rather than sitting beside it.
        let img = image::RgbImage::from_pixel(32, 32, image::Rgb([1, 2, 3]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        save(&album, &png.into_inner()).unwrap();
        assert!(!album.join("album_art.jpg").exists());
        assert_eq!(chosen(&album), Some(album.join("album_art.png")));
    }
}
