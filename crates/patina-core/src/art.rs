//! Cover art for local files: the picture embedded in the file if there
//! is one, otherwise a cover image in its folder. Scaled copies are cached
//! on disk at the sizes the frontends draw, so a big library doesn't
//! decode a 3000-pixel JPEG every time a row scrolls into view.

use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::{DynamicImage, ImageFormat, RgbaImage};
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;

/// Names checked, in order, when a file has no embedded art.
const FOLDER_NAMES: &[&str] = &[
    "cover",
    "folder",
    "front",
    "album",
    "albumart",
    "albumartsmall",
    "artwork",
    "thumb",
];
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// Where a track's art comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Embedded(PathBuf),
    File(PathBuf),
}

impl Origin {
    fn path(&self) -> &Path {
        match self {
            Origin::Embedded(p) | Origin::File(p) => p,
        }
    }
}

/// An image file next to the music: a known cover name first, then any
/// image if the folder has exactly one. Disc folders (CD1) look one level
/// up as well.
pub fn folder_image(dir: &Path) -> Option<PathBuf> {
    let try_dir = |d: &Path| -> Option<PathBuf> {
        let mut images: Vec<PathBuf> = std::fs::read_dir(d)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| IMAGE_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
            })
            .collect();
        images.sort();
        for name in FOLDER_NAMES {
            if let Some(p) = images.iter().find(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            }) {
                return Some(p.clone());
            }
        }
        (images.len() == 1).then(|| images.remove(0))
    };
    try_dir(dir).or_else(|| {
        let up = crate::library::tags::album_dir(dir);
        (up != dir).then(|| try_dir(up)).flatten()
    })
}

fn embedded_bytes(track: &Path) -> Option<Vec<u8>> {
    let f = lofty::read_from_path(track).ok()?;
    let mut best: Option<&lofty::picture::Picture> = None;
    for tag in f.tags() {
        for p in tag.pictures() {
            // Front cover beats anything else; otherwise take the first.
            if p.pic_type() == PictureType::CoverFront {
                return Some(p.data().to_vec());
            }
            if best.is_none() {
                best = Some(p);
            }
        }
    }
    best.map(|p| p.data().to_vec())
}

/// Find art for a track without decoding it.
pub fn find(track: &Path) -> Option<Origin> {
    // Reading pictures means reading tags; cheap next to decoding audio.
    if let Ok(f) = lofty::read_from_path(track)
        && f.tags().iter().any(|t| !t.pictures().is_empty())
    {
        return Some(Origin::Embedded(track.to_path_buf()));
    }
    folder_image(track.parent()?).map(Origin::File)
}

fn cache_dir() -> PathBuf {
    crate::config::cache_dir().join("art")
}

/// Cache file for an origin at a size. Keyed on the source path and its
/// modification time, so editing the art makes a new entry.
fn cache_path(origin: &Origin, size: u32) -> PathBuf {
    let mtime = std::fs::metadata(origin.path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Tracks with embedded art share one entry per folder when the folder
    // is one album, but tracks can carry different art, so key on the file.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    origin.path().hash(&mut h);
    matches!(origin, Origin::Embedded(_)).hash(&mut h);
    mtime.hash(&mut h);
    cache_dir().join(format!("{:016x}-{size}.png", h.finish()))
}

fn decode(origin: &Origin) -> Option<DynamicImage> {
    let bytes = match origin {
        Origin::Embedded(p) => embedded_bytes(p)?,
        Origin::File(p) => std::fs::read(p).ok()?,
    };
    image::load_from_memory(&bytes).ok()
}

/// Scale to fit inside `size`×`size`, square-cropping from the center so a
/// slightly off-square scan doesn't letterbox.
fn square(img: DynamicImage, size: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let img = if w != h {
        img.crop_imm((w - side) / 2, (h - side) / 2, side, side)
    } else {
        img
    };
    img.resize_exact(size, size, image::imageops::FilterType::CatmullRom)
        .to_rgba8()
}

/// Art for a track at `size` pixels square, from the cache when possible.
pub fn load(track: &Path, size: u32) -> Option<RgbaImage> {
    let origin = find(track)?;
    load_origin(&origin, size)
}

pub fn load_origin(origin: &Origin, size: u32) -> Option<RgbaImage> {
    let cached = cache_path(origin, size);
    if let Ok(img) = image::open(&cached) {
        return Some(img.to_rgba8());
    }
    let img = square(decode(origin)?, size);
    if let Some(dir) = cached.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut buf = Cursor::new(Vec::new());
    if img.write_to(&mut buf, ImageFormat::Png).is_ok() {
        let _ = std::fs::write(&cached, buf.into_inner());
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_and_disc_dirs() {
        let d = tempfile::tempdir().unwrap();
        let album = d.path().join("Album");
        std::fs::create_dir_all(album.join("CD1")).unwrap();
        let img = image::RgbImage::from_pixel(40, 30, image::Rgb([200, 100, 50]));
        img.save(album.join("Cover.JPG")).unwrap();
        img.save(album.join("back.png")).unwrap();
        assert_eq!(folder_image(&album).unwrap(), album.join("Cover.JPG"));
        assert_eq!(
            folder_image(&album.join("CD1")).unwrap(),
            album.join("Cover.JPG")
        );
        let s = square(DynamicImage::ImageRgb8(img), 16);
        assert_eq!((s.width(), s.height()), (16, 16));
    }
}
