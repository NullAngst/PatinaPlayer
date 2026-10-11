//! Tag reading for the index. lofty does the work for every format it
//! knows; anything it can't read (or an MP4, where lofty can't tell ALAC
//! from AAC) falls back to FFmpeg's probe.

use std::path::Path;
use std::time::Duration;

use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey, Tag};

use crate::track::TrackInfo;

/// Everything the index stores about one file, from its tags and headers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<i32>,
    /// Every genre value, split on ';' and on multiple tag values.
    pub genres: Vec<String>,
    pub duration: Option<Duration>,
    pub codec: String,
    pub sample_rate: u32,
    pub bits: Option<u32>,
    pub channels: u16,
    pub kbps: Option<u32>,
    pub lossless: bool,
    pub rg_track_gain: Option<f64>,
    pub rg_track_peak: Option<f64>,
    pub rg_album_gain: Option<f64>,
    pub rg_album_peak: Option<f64>,
}

pub fn read(path: &Path) -> Option<TrackMeta> {
    match read_lofty(path) {
        Some(mut m) => {
            if m.codec.is_empty() {
                // MP4 container: ask FFmpeg which codec is inside.
                if let Ok(info) = TrackInfo::probe(path) {
                    m.codec = info.codec;
                    m.lossless = info.lossless;
                    m.bits = if info.lossless {
                        info.bits.or(m.bits)
                    } else {
                        None
                    };
                    if !info.lossless {
                        m.kbps = info.kbps.or(m.kbps);
                    }
                }
            }
            Some(m)
        }
        None => TrackInfo::probe(path).ok().map(|i| from_info(&i)),
    }
}

fn read_lofty(path: &Path) -> Option<TrackMeta> {
    let file = lofty::read_from_path(path).ok()?;
    let props = file.properties();
    let (codec, lossless) = match file.file_type() {
        FileType::Flac => ("FLAC", true),
        FileType::Ape => ("APE", true),
        FileType::WavPack => ("WavPack", true),
        FileType::Wav => ("PCM", true),
        FileType::Aiff => ("PCM", true),
        FileType::Mpeg => ("MP3", false),
        FileType::Aac => ("AAC", false),
        FileType::Opus => ("Opus", false),
        FileType::Vorbis => ("Vorbis", false),
        FileType::Speex => ("Speex", false),
        FileType::Mpc => ("Musepack", false),
        // MP4 holds AAC or ALAC; leave it for the FFmpeg check.
        _ => ("", false),
    };

    let mut m = TrackMeta {
        duration: Some(props.duration()).filter(|d| !d.is_zero()),
        codec: codec.to_string(),
        sample_rate: props.sample_rate().unwrap_or(0),
        bits: if lossless {
            props.bit_depth().map(u32::from)
        } else {
            None
        },
        channels: props.channels().map(u16::from).unwrap_or(0),
        kbps: if lossless {
            None
        } else {
            props.audio_bitrate().or(props.overall_bitrate())
        },
        lossless,
        ..Default::default()
    };

    // Primary tag first, then any other tag in the file for fields the
    // primary one leaves empty (ID3v1 next to ID3v2, APE next to ID3).
    let mut tags: Vec<&Tag> = Vec::new();
    if let Some(t) = file.primary_tag() {
        tags.push(t);
    }
    for t in file.tags() {
        if !tags.iter().any(|x| std::ptr::eq(*x, t)) {
            tags.push(t);
        }
    }
    let text = |f: &dyn Fn(&Tag) -> Option<String>| tags.iter().find_map(|t| f(t)).and_then(clean);

    m.title = text(&|t| t.title().map(|s| s.into_owned()));
    m.artist = text(&|t| t.artist().map(|s| s.into_owned()));
    m.album = text(&|t| t.album().map(|s| s.into_owned()));
    m.album_artist = text(&|t| t.get_string(ItemKey::AlbumArtist).map(str::to_string));
    m.track_no = tags.iter().find_map(|t| t.track()).filter(|&n| n > 0);
    m.disc_no = tags.iter().find_map(|t| t.disk()).filter(|&n| n > 0);
    m.year = tags
        .iter()
        .find_map(|t| t.date().map(|d| d.year as i32))
        .or_else(|| {
            tags.iter().find_map(|t| {
                t.get_string(ItemKey::Year)
                    .or_else(|| t.get_string(ItemKey::RecordingDate))
                    .and_then(parse_year)
            })
        })
        .filter(|&y| y > 0);

    for t in &tags {
        for g in t.get_strings(ItemKey::Genre) {
            for part in split_genres(g) {
                if !m
                    .genres
                    .iter()
                    .any(|x| normalize_genre(x) == normalize_genre(&part))
                {
                    m.genres.push(part);
                }
            }
        }
        if !m.genres.is_empty() {
            break;
        }
    }

    let gain = |key: ItemKey| {
        tags.iter()
            .find_map(|t| t.get_string(key).and_then(parse_gain))
    };
    m.rg_track_gain = gain(ItemKey::ReplayGainTrackGain);
    m.rg_track_peak = gain(ItemKey::ReplayGainTrackPeak);
    m.rg_album_gain = gain(ItemKey::ReplayGainAlbumGain);
    m.rg_album_peak = gain(ItemKey::ReplayGainAlbumPeak);
    Some(m)
}

fn from_info(i: &TrackInfo) -> TrackMeta {
    TrackMeta {
        title: i.title.clone(),
        artist: i.artist.clone(),
        album: i.album.clone(),
        track_no: i
            .track_no
            .as_deref()
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.trim().parse().ok())
            .filter(|&n| n > 0),
        duration: i.duration,
        codec: i.codec.clone(),
        sample_rate: i.sample_rate,
        bits: i.bits,
        channels: i.channels,
        kbps: i.kbps,
        lossless: i.lossless,
        ..Default::default()
    }
}

fn clean(s: String) -> Option<String> {
    let t = s.trim().trim_matches('\0').trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn parse_year(s: &str) -> Option<i32> {
    let digits: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

/// "-6.54 dB" or "0.987654" to a number.
fn parse_gain(s: &str) -> Option<f64> {
    let t = s.trim();
    let t = t
        .strip_suffix("dB")
        .or_else(|| t.strip_suffix("db"))
        .or_else(|| t.strip_suffix("DB"))
        .unwrap_or(t);
    t.trim().parse().ok()
}

/// Split one genre field into genres. Semicolons and NULs separate values;
/// slashes don't, since "R&B/Soul" style names are common.
pub fn split_genres(s: &str) -> Vec<String> {
    s.split([';', '\0'])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string)
        .collect()
}

/// Matching form of a genre: "Blues Rock", "blues-rock" and "Blues_Rock"
/// all become "blues-rock".
pub fn normalize_genre(g: &str) -> String {
    let mut out = String::with_capacity(g.len());
    let mut dash = false;
    for c in g.trim().chars() {
        if c.is_whitespace() || c == '-' || c == '_' {
            dash = !out.is_empty();
        } else {
            if dash {
                out.push('-');
                dash = false;
            }
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Case and whitespace folding for grouping keys.
pub fn fold(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Folder an album lives in, skipping a trailing disc folder ("CD1",
/// "Disc 2") so multi-disc albums group as one.
pub fn album_dir(dir: &Path) -> &Path {
    let is_disc = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            let l = n.to_ascii_lowercase();
            let rest = l
                .strip_prefix("cd")
                .or_else(|| l.strip_prefix("disc"))
                .or_else(|| l.strip_prefix("disk"));
            match rest {
                Some(r) => {
                    let r = r.trim_start_matches([' ', '-', '_', '.']);
                    !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())
                }
                None => false,
            }
        })
        .unwrap_or(false);
    if is_disc {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    }
}

/// The key that decides which tracks are one album.
///
/// With an album artist tag: album artist plus album name, wherever the
/// files sit. Without one: album name plus folder, which keeps two
/// different "Greatest Hits" apart and keeps a compilation together even
/// though every track has a different artist.
pub fn album_key(meta: &TrackMeta, dir: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let folder = String::from_utf8_lossy(album_dir(dir).as_os_str().as_bytes()).into_owned();
    match (&meta.album_artist, &meta.album) {
        (Some(aa), Some(al)) => format!("aa\u{1f}{}\u{1f}{}", fold(aa), fold(al)),
        (None, Some(al)) => format!("dir\u{1f}{folder}\u{1f}{}", fold(al)),
        (_, None) => format!("dir\u{1f}{folder}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genre_normalizing() {
        for g in [
            "Blues Rock",
            "blues-rock",
            "Blues-Rock",
            " blues_rock ",
            "Blues  -  Rock",
        ] {
            assert_eq!(normalize_genre(g), "blues-rock", "{g}");
        }
        assert_eq!(
            split_genres("Blues Rock; Post-Grunge;"),
            vec!["Blues Rock", "Post-Grunge"]
        );
        assert_eq!(split_genres("R&B/Soul"), vec!["R&B/Soul"]);
    }

    #[test]
    fn disc_folders_group() {
        assert_eq!(album_dir(Path::new("/m/Album/CD1")), Path::new("/m/Album"));
        assert_eq!(
            album_dir(Path::new("/m/Album/Disc 2")),
            Path::new("/m/Album")
        );
        assert_eq!(
            album_dir(Path::new("/m/Album/disk-03")),
            Path::new("/m/Album")
        );
        assert_eq!(album_dir(Path::new("/m/CDs")), Path::new("/m/CDs"));
        assert_eq!(
            album_dir(Path::new("/m/Discography")),
            Path::new("/m/Discography")
        );
    }

    #[test]
    fn keys() {
        let mut m = TrackMeta {
            album: Some("Bleach".into()),
            ..Default::default()
        };
        let a = album_key(&m, Path::new("/m/Nirvana/Bleach/CD1"));
        let b = album_key(&m, Path::new("/m/Nirvana/Bleach/CD2"));
        assert_eq!(a, b);
        m.album_artist = Some("Nirvana".into());
        let c = album_key(&m, Path::new("/elsewhere"));
        m.album = Some("bleach ".into());
        assert_eq!(c, album_key(&m, Path::new("/x")));
    }

    #[test]
    fn gains() {
        assert_eq!(parse_gain("-6.54 dB"), Some(-6.54));
        assert_eq!(parse_gain("0.98"), Some(0.98));
        assert_eq!(parse_year("1989-06-15"), Some(1989));
        assert_eq!(parse_year("89"), None);
    }
}
