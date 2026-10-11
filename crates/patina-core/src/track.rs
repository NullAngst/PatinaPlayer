//! Track metadata as the player needs it: what to show and what the
//! format badge says. Library-grade tag reading (lofty, ReplayGain fields)
//! arrives with the library index; for now FFmpeg's metadata is enough.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ffmpeg_next as ffmpeg;

use crate::audio::decoder::{DecodeError, init_ffmpeg};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_no: Option<String>,
    pub duration: Option<Duration>,
    /// Short codec name for the badge: FLAC, MP3, Opus, ALAC...
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Bits per sample for lossless sources. None for lossy codecs.
    pub bits: Option<u32>,
    /// Bitrate in kbps for lossy sources.
    pub kbps: Option<u32>,
    pub lossless: bool,
}

impl TrackInfo {
    /// Read metadata without decoding. Cheap enough to run over a queue.
    pub fn probe(path: &Path) -> Result<Self, DecodeError> {
        init_ffmpeg();
        let input = ffmpeg::format::input(path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or(DecodeError::NoAudio)?;
        Ok(Self::from_input(path, &input, stream.index()))
    }

    pub(crate) fn from_input(
        path: &Path,
        input: &ffmpeg::format::context::Input,
        stream_index: usize,
    ) -> Self {
        let stream = input
            .stream(stream_index)
            .expect("stream index from best()");
        let params = stream.parameters();
        let id = params.id();

        // Container tags first, stream tags as a fallback (Ogg puts them
        // on the stream).
        let tag = |key: &str| -> Option<String> {
            let find = |d: ffmpeg::DictionaryRef| {
                d.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            find(input.metadata()).or_else(|| find(stream.metadata()))
        };

        // SAFETY: plain field reads on a valid AVCodecParameters.
        let (rate, channels, raw_bits, coded_bits) = unsafe {
            let p = &*params.as_ptr();
            (
                p.sample_rate.max(0) as u32,
                p.ch_layout.nb_channels.max(0) as u16,
                p.bits_per_raw_sample,
                p.bits_per_coded_sample,
            )
        };

        let lossless = is_lossless(id);
        let bits = if lossless {
            let b = if raw_bits > 0 { raw_bits } else { coded_bits };
            (b > 0).then_some(b as u32)
        } else {
            None
        };

        let duration = {
            let tb = stream.time_base();
            let d = stream.duration();
            if d > 0 && tb.denominator() > 0 {
                Some(Duration::from_secs_f64(
                    d as f64 * tb.numerator() as f64 / tb.denominator() as f64,
                ))
            } else if input.duration() > 0 {
                Some(Duration::from_secs_f64(
                    input.duration() as f64 / ffmpeg::ffi::AV_TIME_BASE as f64,
                ))
            } else {
                None
            }
        };

        let kbps = if lossless {
            None
        } else {
            let br = if params.bit_rate() > 0 {
                params.bit_rate()
            } else {
                input.bit_rate()
            };
            (br > 0).then(|| ((br as f64) / 1000.0).round() as u32)
        };

        Self {
            path: path.to_path_buf(),
            title: tag("title"),
            artist: tag("artist").or_else(|| tag("album_artist")),
            album: tag("album"),
            track_no: tag("track"),
            duration,
            codec: codec_label(id),
            sample_rate: rate,
            channels,
            bits,
            kbps,
            lossless,
        }
    }

    /// A placeholder for a file that has not been probed yet.
    pub fn unprobed(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            ..Default::default()
        }
    }

    /// Title, or the file name when the file has no title tag.
    pub fn display_title(&self) -> String {
        self.title.clone().unwrap_or_else(|| {
            self.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    }

    /// Format badge text: "FLAC 44.1/16" or "Opus 192".
    pub fn badge(&self) -> String {
        if self.codec.is_empty() {
            return String::new();
        }
        let khz = format_khz(self.sample_rate);
        match (self.lossless, self.bits, self.kbps) {
            (true, Some(b), _) => format!("{} {}/{}", self.codec, khz, b),
            (true, None, _) => format!("{} {}", self.codec, khz),
            (false, _, Some(k)) => format!("{} {}", self.codec, k),
            (false, _, None) => self.codec.clone(),
        }
    }
}

fn format_khz(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{}", rate / 1000)
    } else {
        format!("{:.1}", rate as f64 / 1000.0)
    }
}

fn is_lossless(id: ffmpeg::codec::Id) -> bool {
    use ffmpeg::codec::Id;
    let name = id.name();
    name.starts_with("pcm_")
        || name.starts_with("dsd_")
        || matches!(
            id,
            Id::FLAC | Id::ALAC | Id::APE | Id::WAVPACK | Id::TTA | Id::TAK | Id::MLP | Id::TRUEHD
        )
}

fn codec_label(id: ffmpeg::codec::Id) -> String {
    use ffmpeg::codec::Id;
    let name = id.name();
    let s = match id {
        Id::FLAC => "FLAC",
        Id::ALAC => "ALAC",
        Id::APE => "APE",
        Id::WAVPACK => "WavPack",
        Id::MP3 => "MP3",
        Id::AAC => "AAC",
        Id::OPUS => "Opus",
        Id::VORBIS => "Vorbis",
        Id::TTA => "TTA",
        Id::TAK => "TAK",
        _ if name.starts_with("pcm_") => "PCM",
        _ if name.starts_with("dsd_") => "DSD",
        _ => return name.to_uppercase(),
    };
    s.to_string()
}

/// Extensions the folder scanner treats as audio.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "m4b", "aac", "alac", "ogg", "oga", "opus", "wav", "wave", "aif", "aiff",
    "aifc", "ape", "wv", "tta", "tak", "dsf", "dff", "wma", "mka", "mpc", "spx",
];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// Expand files and folders into a sorted list of audio files. Folders are
/// walked recursively and sorted by path, which keeps albums in disc and
/// track order when files are named the usual way.
pub fn expand_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut found = Vec::new();
            walk(p, &mut found);
            found.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
            out.extend(found);
        } else if p.is_file() {
            out.push(p.clone());
        }
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out);
        } else if is_audio_file(&path) {
            out.push(path);
        }
    }
}

/// Compare strings so "2 Song" sorts before "10 Song".
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    a.next();
                }
                let mut nb = String::new();
                while let Some(c) = b.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    b.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort() {
        let mut v = vec!["10 b.flac", "2 a.flac", "1 c.flac", "02 x.flac"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["1 c.flac", "2 a.flac", "02 x.flac", "10 b.flac"]);
    }

    #[test]
    fn badge_text() {
        let mut t = TrackInfo {
            codec: "FLAC".into(),
            sample_rate: 44100,
            bits: Some(16),
            lossless: true,
            ..Default::default()
        };
        assert_eq!(t.badge(), "FLAC 44.1/16");
        t.sample_rate = 96000;
        t.bits = Some(24);
        assert_eq!(t.badge(), "FLAC 96/24");
        let o = TrackInfo {
            codec: "Opus".into(),
            sample_rate: 48000,
            kbps: Some(192),
            ..Default::default()
        };
        assert_eq!(o.badge(), "Opus 192");
    }
}
