//! Artist pictures. A local artist's folder can hold one (`artist.jpg`,
//! or a `folder.jpg` in the folder above the albums), and Subsonic and
//! Ampache servers keep their own. Whatever's found is kept in the image
//! cache; an artist with no picture anywhere is remembered for a week, so
//! a list of thousands of artists doesn't ask the servers again on every
//! visit.
//!
//! One can also be picked by hand, from an image file or from Deezer's
//! artist search ([`search`]). It's saved as `artist.jpg` in the artist's
//! folder where there is one the app can write to, where other players
//! find it too, or in the app's own folder otherwise. Either way it wins
//! over everything else.

use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::{ImageFormat, RgbaImage};

use crate::library::{ArtistMode, Library};

/// How long "no picture" is believed before asking again.
const NONE_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

/// File names an artist picture goes by, in the order they're tried.
const NAMES: [&str; 4] = ["artist", "band", "photo", "folder"];
const EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

fn dir() -> PathBuf {
    crate::config::cache_dir().join("artists")
}

fn stem(name: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.trim().to_lowercase().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn cache_path(name: &str, size: u32) -> PathBuf {
    dir().join(format!("{}-{size}.png", stem(name)))
}

fn none_path(name: &str) -> PathBuf {
    dir().join(format!("{}.none", stem(name)))
}

/// Pictures picked by hand that couldn't go in the artist's folder. Not
/// in the cache, so clearing it doesn't lose them.
fn store_dir() -> PathBuf {
    crate::config::data_dir().join("artists")
}

/// The picture picked by hand and kept in the app's folder, if there is one.
fn stored(name: &str) -> Option<PathBuf> {
    ["jpg", "png"]
        .iter()
        .map(|e| store_dir().join(format!("{}.{e}", stem(name))))
        .find(|p| p.is_file())
}

/// The artist's own folder: the one all their albums sit in, if they
/// share one and it isn't a library folder itself (whose `folder.jpg`
/// would be the whole library's).
pub fn artist_dir(lib: &Library, name: &str) -> Option<PathBuf> {
    let albums = lib.artist_albums(name, ArtistMode::AlbumArtist).ok()?;
    let roots: Vec<PathBuf> = lib.sources().ok()?.into_iter().map(|s| s.root).collect();
    let mut parent: Option<PathBuf> = None;
    for a in &albums {
        let album = a
            .cover_track
            .as_deref()
            .and_then(Path::parent)
            .map(crate::library::tags::album_dir)
            .unwrap_or(&a.dir);
        if crate::remote::is_remote(album) {
            continue;
        }
        let p = album.parent()?.to_path_buf();
        match &parent {
            None => parent = Some(p),
            Some(q) if *q == p => {}
            Some(_) => return None,
        }
    }
    let parent = parent?;
    if roots.contains(&parent) {
        return None;
    }
    Some(parent)
}

/// A picture file in the artist's own folder ([`artist_dir`]).
pub fn local_file(lib: &Library, name: &str) -> Option<PathBuf> {
    let parent = artist_dir(lib, name)?;
    for n in NAMES {
        for e in EXTS {
            for f in [format!("{n}.{e}"), format!("{}.{e}", capitalize(n))] {
                let p = parent.join(f);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The servers the artist has songs on.
fn servers(lib: &Library, name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in lib
        .artist_albums(name, ArtistMode::AlbumArtist)
        .unwrap_or_default()
    {
        if let Some(s) = a.cover_track.as_deref().and_then(crate::remote::server_of)
            && !out.iter().any(|o| o == s)
        {
            out.push(s.to_string());
        }
    }
    out
}

/// An artist's picture, square, `size` pixels. Slow the first time (it may
/// ask a server); cached after.
pub fn load(lib: &Library, name: &str, size: u32) -> Option<RgbaImage> {
    let cached = cache_path(name, size);
    if let Ok(img) = image::open(&cached) {
        return Some(img.to_rgba8());
    }
    let none = none_path(name);
    if let Ok(m) = std::fs::metadata(&none)
        && m.modified()
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < NONE_FOR)
    {
        return None;
    }
    let bytes = stored(name)
        .or_else(|| local_file(lib, name))
        .and_then(|p| std::fs::read(p).ok())
        .or_else(|| {
            servers(lib, name).into_iter().find_map(|s| {
                crate::remote::registry()
                    .client(&s)
                    .ok()?
                    .artist_image(name, size.max(300))
                    .ok()
                    .flatten()
            })
        });
    let _ = std::fs::create_dir_all(dir());
    let Some(img) = bytes.and_then(|b| image::load_from_memory(&b).ok()) else {
        let _ = std::fs::write(&none, b"");
        return None;
    };
    let img = crate::art::square(img, size);
    let mut buf = Cursor::new(Vec::new());
    if img.write_to(&mut buf, ImageFormat::Png).is_ok() {
        let _ = std::fs::write(&cached, buf.into_inner());
    }
    Some(img)
}

/// Save `bytes` as the artist's picture: `artist.jpg` (or `.png`) in
/// `folder`, their own folder from [`artist_dir`], when there is one and
/// it can be written to; the app's own folder otherwise. What was cached
/// for them is dropped, so the new one shows.
pub fn save(
    name: &str,
    folder: Option<&Path>,
    bytes: &[u8],
) -> Result<crate::cover::Saved, String> {
    let (data, ext) = crate::cover::prepare(bytes)?;
    let saved = match folder.filter(|d| !crate::remote::is_remote(d) && d.is_dir()) {
        Some(d) => {
            let path = d.join(format!("artist.{ext}"));
            let tmp = d.join(format!(".artist.{ext}.part"));
            match std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &path)) {
                Ok(()) => {
                    // The other kind would be found first, or alongside.
                    for other in ["jpg", "jpeg", "png"] {
                        if other != ext {
                            let _ = std::fs::remove_file(d.join(format!("artist.{other}")));
                        }
                    }
                    // A copy kept from before would win over it.
                    if let Some(old) = stored(name) {
                        let _ = std::fs::remove_file(old);
                    }
                    Some(crate::cover::Saved::Folder(path))
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    log::info!("can't write an artist picture into {}: {e}", d.display());
                    None
                }
            }
        }
        None => None,
    };
    let saved = match saved {
        Some(s) => s,
        None => {
            std::fs::create_dir_all(store_dir())
                .map_err(|e| format!("Couldn't save the picture: {e}"))?;
            if let Some(old) = stored(name) {
                let _ = std::fs::remove_file(old);
            }
            let path = store_dir().join(format!("{}.{ext}", stem(name)));
            std::fs::write(&path, &data).map_err(|e| format!("Couldn't save the picture: {e}"))?;
            crate::cover::Saved::ArtistApp(path)
        }
    };
    forget(name);
    // The car's copies are made again, with the new one.
    let _ = std::fs::remove_dir_all(crate::browse::auto::car_art_dir());
    Ok(saved)
}

/// Save an image file as the artist's picture: see [`save`].
pub fn save_file(
    name: &str,
    folder: Option<&Path>,
    file: &Path,
) -> Result<crate::cover::Saved, String> {
    let meta = std::fs::metadata(file).map_err(|e| format!("Couldn't read that file: {e}"))?;
    if meta.len() > 40 << 20 {
        return Err("That file is too big to be a picture".into());
    }
    let bytes = std::fs::read(file).map_err(|e| format!("Couldn't read that file: {e}"))?;
    save(name, folder, &bytes)
}

/// Drop what's cached for an artist, at every size, and the note that
/// they had no picture.
fn forget(name: &str) {
    let prefix = format!("{}-", stem(name));
    if let Ok(rd) = std::fs::read_dir(dir()) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let _ = std::fs::remove_file(none_path(name));
}

/// Look for pictures of an artist online: Deezer's artist search, the one
/// open source of them that needs no key. iTunes and MusicBrainz have no
/// artist pictures.
pub fn search(name: &str) -> Vec<crate::cover::Found> {
    let name = name.trim();
    if name.is_empty() {
        return Vec::new();
    }
    let Some(c) = crate::cover::http() else {
        return Vec::new();
    };
    crate::cover::get_json(
        &c,
        "https://api.deezer.com/search/artist",
        &[("q", name), ("limit", "12")],
    )
    .map(|v| parse_deezer(&v))
    .unwrap_or_default()
}

/// What Deezer's artist search answered. Artists without a picture have
/// Deezer's grey placeholder, whose address has no picture id in it.
pub fn parse_deezer(v: &serde_json::Value) -> Vec<crate::cover::Found> {
    let Some(rs) = v.get("data").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    rs.iter()
        .filter_map(|r| {
            let thumb = r.get("picture_medium")?.as_str()?;
            if thumb.contains("/artist//") {
                return None;
            }
            let full = r
                .get("picture_xl")
                .or_else(|| r.get("picture_big"))
                .and_then(|p| p.as_str())
                .unwrap_or(thumb);
            let fans = r.get("nb_fan").and_then(|n| n.as_i64()).unwrap_or(0);
            Some(crate::cover::Found {
                source: "Deezer",
                title: format!("{} · {fans} fans", r.get("name")?.as_str()?),
                thumb: thumb.to_string(),
                full: full.to_string(),
                year: None,
            })
        })
        .collect()
}

/// [`load`], as a PNG file, for things that want a path (the car).
pub fn cached_file(lib: &Library, name: &str, size: u32) -> Option<PathBuf> {
    let p = cache_path(name, size);
    if !p.is_file() {
        load(lib, name, size)?;
    }
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_fm_star_is_no_picture() {
        assert!(!crate::remote::real_image_url(
            "https://lastfm.freetls.fastly.net/i/u/300x300/2a96cbd8b46e442fc41c2b86b821562f.png"
        ));
        assert!(crate::remote::real_image_url("https://x/y.jpg"));
        assert!(!crate::remote::real_image_url("ftp://x/y.jpg"));
    }

    #[test]
    fn stems_ignore_case_and_spaces() {
        assert_eq!(stem("Nirvana "), stem("nirvana"));
    }

    #[test]
    fn parses_deezer_artists_without_the_placeholder() {
        let v = serde_json::json!({"data": [
            {"name": "Nirvana", "nb_fan": 5, "picture_medium": "https://x/images/artist/abc/250x250.jpg",
             "picture_xl": "https://x/images/artist/abc/1000x1000.jpg"},
            {"name": "Nobody", "picture_medium": "https://x/images/artist//250x250.jpg"}
        ]});
        let f = parse_deezer(&v);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].title, "Nirvana · 5 fans");
        assert!(f[0].full.ends_with("1000x1000.jpg"));
    }

    #[test]
    fn a_picked_picture_goes_in_the_artists_folder_or_the_apps() {
        let d = tempfile::tempdir().unwrap();
        // A name no real library has: saving drops what's cached for it.
        let name = "PatinaPlayer test artist 4c1e";
        let img = image::RgbImage::from_pixel(32, 32, image::Rgb([9, 9, 9]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, ImageFormat::Png).unwrap();
        let png = png.into_inner();
        let artist = d.path().join("Nirvana");
        std::fs::create_dir_all(&artist).unwrap();
        let saved = save(name, Some(&artist), &png).unwrap();
        assert_eq!(
            saved,
            crate::cover::Saved::Folder(artist.join("artist.png"))
        );
        assert!(artist.join("artist.png").is_file());
        assert!(save(name, Some(&artist), b"not a picture").is_err());
    }
}
