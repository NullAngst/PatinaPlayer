//! Artist pictures. A local artist's folder can hold one (`artist.jpg`,
//! or a `folder.jpg` in the folder above the albums), and Subsonic and
//! Ampache servers keep their own. Whatever's found is kept in the image
//! cache; an artist with no picture anywhere is remembered for a week, so
//! a list of thousands of artists doesn't ask the servers again on every
//! visit.

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

/// A picture file in an artist's own folder: the folder all their albums
/// sit in, if they share one and it isn't a library folder itself (whose
/// `folder.jpg` would be the whole library's).
pub fn local_file(lib: &Library, name: &str) -> Option<PathBuf> {
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
    let bytes = local_file(lib, name)
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
}
