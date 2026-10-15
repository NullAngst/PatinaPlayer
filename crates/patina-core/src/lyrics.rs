//! Lyrics, looked up only when you ask. In order, the first that has
//! them wins:
//!
//! 1. The song's own tags (USLT in ID3, LYRICS in Vorbis comments, ©lyr in
//!    MP4). For a server song, from its cached copy if there is one.
//! 2. An .lrc file next to the song, with the same name.
//! 3. The song's server (OpenSubsonic's lyrics by song, else plain
//!    Subsonic's by artist and title).
//! 4. LRCLIB, an open lyrics database, only if you turned it on.
//!
//! Text in LRC form (`[01:23.45] a line`), wherever it came from, is
//! synced: the lines carry times and follow the song.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::file::TaggedFileExt;
use lofty::tag::ItemKey;

use crate::track::TrackInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Tags,
    LrcFile,
    Server,
    Lrclib,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Tags => "from the file's tags",
            Origin::LrcFile => "from the .lrc file next to it",
            Origin::Server => "from its server",
            Origin::Lrclib => "from LRCLIB",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    /// Lines in order; a time on each when synced.
    pub lines: Vec<(Option<Duration>, String)>,
    pub synced: bool,
    pub origin: Origin,
}

impl Lyrics {
    /// The line playing at `pos`, for synced lyrics.
    pub fn line_at(&self, pos: Duration) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let mut cur = None;
        for (i, (t, _)) in self.lines.iter().enumerate() {
            match t {
                Some(t) if *t <= pos => cur = Some(i),
                Some(_) => break,
                None => {}
            }
        }
        cur
    }

    fn from_text(text: &str, origin: Origin) -> Option<Lyrics> {
        let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if text.is_empty() {
            return None;
        }
        if let Some(lines) = parse_lrc(text) {
            return Some(Lyrics {
                lines,
                synced: true,
                origin,
            });
        }
        Some(Lyrics {
            lines: text
                .lines()
                .map(|l| (None, l.trim_end().to_string()))
                .collect(),
            synced: false,
            origin,
        })
    }
}

/// LRC text to timed lines, sorted. None if it isn't LRC. Takes several
/// times on one line (`[00:12.00][01:30.50] chorus`), and skips the tag
/// lines (`[ar: ...]`). An `[offset: ms]` tag shifts every line.
pub fn parse_lrc(text: &str) -> Option<Vec<(Option<Duration>, String)>> {
    let mut out: Vec<(Duration, String)> = Vec::new();
    let mut offset_ms: i64 = 0;
    let mut timed = 0;
    for raw in text.lines() {
        let mut line = raw.trim();
        let mut times = Vec::new();
        while let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[..end];
            line = rest[end + 1..].trim_start();
            if let Some(t) = parse_time(tag) {
                times.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset_ms = v.trim().parse().unwrap_or(0);
            }
        }
        if times.is_empty() {
            continue;
        }
        timed += 1;
        for t in times {
            out.push((t, line.to_string()));
        }
    }
    if timed == 0 {
        return None;
    }
    out.sort_by_key(|(t, _)| *t);
    let shift = |t: Duration| {
        let ms = t.as_millis() as i64 + offset_ms;
        Duration::from_millis(ms.max(0) as u64)
    };
    Some(out.into_iter().map(|(t, l)| (Some(shift(t)), l)).collect())
}

/// "01:23.45", "1:23", "01:23:450" to a time.
fn parse_time(s: &str) -> Option<Duration> {
    let (m, rest) = s.split_once(':')?;
    let m: u64 = m.trim().parse().ok()?;
    let rest = rest.trim();
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let sec: u64 = sec.parse().ok()?;
    if sec >= 60 {
        return None;
    }
    let ms = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take(3).collect();
        let v: u64 = digits.parse().ok()?;
        match digits.len() {
            1 => v * 100,
            2 => v * 10,
            _ => v,
        }
    };
    Some(Duration::from_millis((m * 60 + sec) * 1000 + ms))
}

/// The file to read tags and look beside: a local song's own, or a
/// server song's cached copy.
fn local_file(info: &TrackInfo) -> Option<PathBuf> {
    if let Some(r) = &info.remote {
        let key = crate::cache::SongKey::of(r)?;
        return crate::cache::stored_file(&key);
    }
    (!info.path.as_os_str().is_empty() && info.url.is_none()).then(|| info.path.clone())
}

fn from_tags(path: &Path) -> Option<Lyrics> {
    let f = lofty::read_from_path(path).ok()?;
    for t in f.tags() {
        for key in [ItemKey::Lyrics, ItemKey::UnsyncLyrics] {
            if let Some(text) = t.get_string(key)
                && let Some(l) = Lyrics::from_text(text, Origin::Tags)
            {
                return Some(l);
            }
        }
    }
    None
}

fn from_lrc_file(path: &Path) -> Option<Lyrics> {
    let lrc = path.with_extension("lrc");
    let bytes = std::fs::read(&lrc).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Lyrics::from_text(&text, Origin::LrcFile)
}

fn from_server(info: &TrackInfo) -> Result<Option<Lyrics>, String> {
    let Some(r) = info.remote.as_ref().filter(|r| !r.episode) else {
        return Ok(None);
    };
    let client = crate::remote::registry()
        .client(&r.server)
        .map_err(|e| e.to_string())?;
    let artist = info.artist.clone().unwrap_or_default();
    let title = info.title.clone().unwrap_or_default();
    match client.lyrics(&r.id, &artist, &title) {
        Ok(Some(l)) => {
            // Some servers send LRC text as one unsynced block.
            if !l.synced {
                let text: Vec<String> = l.lines.iter().map(|x| x.1.clone()).collect();
                return Ok(Lyrics::from_text(&text.join("\n"), Origin::Server));
            }
            Ok(Some(Lyrics {
                lines: l.lines,
                synced: true,
                origin: Origin::Server,
            }))
        }
        Ok(None) | Err(crate::remote::RemoteError::Unsupported(_)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Where LRCLIB answers. Tests point this somewhere else.
fn lrclib_base() -> String {
    std::env::var("PATINA_LRCLIB_URL").unwrap_or_else(|_| "https://lrclib.net".into())
}

fn from_lrclib(info: &TrackInfo) -> Result<Option<Lyrics>, String> {
    let (Some(artist), Some(title)) = (info.artist.as_deref(), info.title.as_deref()) else {
        return Ok(None);
    };
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "PatinaPlayer/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/patinaplayer)"
        ))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let mut q: Vec<(&str, String)> = vec![
        ("artist_name", artist.to_string()),
        ("track_name", title.to_string()),
    ];
    if let Some(a) = &info.album {
        q.push(("album_name", a.clone()));
    }
    if let Some(d) = info.duration {
        q.push(("duration", d.as_secs().to_string()));
    }
    let resp = http
        .get(format!("{}/api/get", lrclib_base()))
        .query(&q)
        .send()
        .map_err(|e| format!("LRCLIB: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("LRCLIB answered {}", resp.status()));
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct Got {
        synced_lyrics: Option<String>,
        plain_lyrics: Option<String>,
        instrumental: bool,
    }
    let body = resp.bytes().map_err(|e| format!("LRCLIB: {e}"))?;
    let g: Got = serde_json::from_slice(&body).map_err(|e| format!("LRCLIB: {e}"))?;
    if g.instrumental {
        return Ok(Some(Lyrics {
            lines: vec![(None, "(instrumental)".into())],
            synced: false,
            origin: Origin::Lrclib,
        }));
    }
    Ok(g.synced_lyrics
        .as_deref()
        .and_then(|t| Lyrics::from_text(t, Origin::Lrclib))
        .or_else(|| {
            g.plain_lyrics
                .as_deref()
                .and_then(|t| Lyrics::from_text(t, Origin::Lrclib))
        }))
}

/// Look a song's lyrics up, everywhere allowed, in order. `online` lets
/// it ask LRCLIB. Ok(None) means nobody had them. Network trouble at one
/// step doesn't stop the next; it's reported only if nothing turned up.
pub fn find(info: &TrackInfo, online: bool) -> Result<Option<Lyrics>, String> {
    if let Some(p) = local_file(info) {
        if let Some(l) = from_tags(&p) {
            return Ok(Some(l));
        }
        if info.remote.is_none()
            && let Some(l) = from_lrc_file(&p)
        {
            return Ok(Some(l));
        }
    }
    let mut trouble = None;
    match from_server(info) {
        Ok(Some(l)) => return Ok(Some(l)),
        Ok(None) => {}
        Err(e) => trouble = Some(e),
    }
    if online && !info.is_live() {
        match from_lrclib(info) {
            Ok(Some(l)) => return Ok(Some(l)),
            Ok(None) => {}
            Err(e) => trouble = Some(e),
        }
    }
    match trouble {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_parses_with_repeats_tags_and_offset() {
        let l = parse_lrc(
            "[ar: Someone]\n[offset: +500]\n[00:01.00]one\n[00:12.5][00:03.25]chorus\n\n[1:02.123]last",
        )
        .unwrap();
        let t: Vec<u64> = l.iter().map(|x| x.0.unwrap().as_millis() as u64).collect();
        assert_eq!(t, vec![1500, 3750, 13000, 62623]);
        assert_eq!(l[1].1, "chorus");
        assert!(parse_lrc("just\nwords").is_none());
    }

    #[test]
    fn the_line_playing() {
        let l = Lyrics::from_text("[00:01.00]a\n[00:05.00]b\n[00:09.00]c", Origin::Tags).unwrap();
        assert!(l.synced);
        assert_eq!(l.line_at(Duration::from_millis(500)), None);
        assert_eq!(l.line_at(Duration::from_secs(6)), Some(1));
        assert_eq!(l.line_at(Duration::from_secs(60)), Some(2));
        let plain = Lyrics::from_text("a\nb", Origin::Tags).unwrap();
        assert!(!plain.synced);
        assert_eq!(plain.line_at(Duration::from_secs(1)), None);
    }

    #[test]
    fn tags_then_lrc_file() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.flac");
        let st = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=f=440:d=1")
            .args(["-metadata", "LYRICS=sung words\nsecond line"])
            .arg(&song)
            .status()
            .expect("ffmpeg must be installed");
        assert!(st.success());
        let info = TrackInfo {
            path: song.clone(),
            ..Default::default()
        };
        let l = find(&info, false).unwrap().unwrap();
        assert_eq!(l.origin, Origin::Tags);
        assert_eq!(l.lines[1].1, "second line");
        // No tags: the .lrc beside it.
        let bare = dir.path().join("bare.flac");
        std::fs::copy(crate::lyrics::tests::plain_flac(dir.path()), &bare).unwrap();
        std::fs::write(
            bare.with_extension("lrc"),
            "[00:00.50]hello\n[00:00.90]there",
        )
        .unwrap();
        let l = find(
            &TrackInfo {
                path: bare,
                ..Default::default()
            },
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(l.origin, Origin::LrcFile);
        assert!(l.synced);
    }

    pub(super) fn plain_flac(dir: &Path) -> PathBuf {
        let p = dir.join("plain-src.flac");
        let st = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=f=440:d=1")
            .arg(&p)
            .status()
            .expect("ffmpeg must be installed");
        assert!(st.success());
        p
    }
}
