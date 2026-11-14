//! Resumable downloads to a `.part` file, with progress any thread can
//! watch and a flag that stops them from another thread.
//!
//! The file only takes its real name once it's complete, so a half
//! download never passes for a finished one. A stopped or broken download
//! leaves its `.part` behind, and the next try asks the server for the
//! rest with an HTTP range.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

/// How long to wait between reads of an HLS playlist that's still growing.
const HLS_POLL: Duration = Duration::from_secs(2);
/// Give up on an HLS playlist that stops growing without ending.
const HLS_STALL: u32 = 60;

/// A run of spaces at least this long at the very end of a download is
/// filler, not the song. Ampache sends the size its catalog has for a file
/// and, when the file is shorter than that (retagged since the last scan),
/// makes up the difference with spaces. Ampache 7 does the same for an MP3
/// it converts, up to a size it estimated. No audio format ends in 256
/// spaces: the most an ID3v1 tag could hold is 125.
const FILLER_MIN: u64 = 256;

/// Where a download in progress lives: `song.flac` becomes `song.flac.part`.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// Not started, or waiting its turn.
    Waiting = 0,
    Running = 1,
    /// Complete and under its real name.
    Done = 2,
    Failed = 3,
    /// Paused or cancelled. The `.part` may still be there.
    Stopped = 4,
}

/// How far a download has got. Shared between the thread that downloads
/// and anyone reading the file as it grows.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    /// 0 while the size isn't known.
    total: AtomicU64,
    /// The size the server gave for a converted stream: only its guess
    /// (so not `total`, which readers seek by), but near enough to tell
    /// how many bytes a second of the song takes. 0 when there's none.
    estimate: AtomicU64,
    state: AtomicU8,
    error: Mutex<Option<String>>,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes in the file so far.
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Acquire)
    }

    pub fn total(&self) -> Option<u64> {
        match self.total.load(Ordering::Acquire) {
            0 => None,
            t => Some(t),
        }
    }

    /// The real size if it's known, else the server's guess at it.
    pub fn size_hint(&self) -> Option<u64> {
        self.total()
            .or(match self.estimate.load(Ordering::Acquire) {
                0 => None,
                t => Some(t),
            })
    }

    pub fn state(&self) -> State {
        match self.state.load(Ordering::Acquire) {
            1 => State::Running,
            2 => State::Done,
            3 => State::Failed,
            4 => State::Stopped,
            _ => State::Waiting,
        }
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Release);
    }

    fn fail(&self, e: &str) {
        *self.error.lock().unwrap() = Some(e.to_string());
        self.set_state(State::Failed);
    }

    /// Set what's known before the download starts: a size from the index,
    /// bytes already in a `.part`.
    pub fn preset(&self, done: u64, total: Option<u64>) {
        self.done.store(done, Ordering::Release);
        if let Some(t) = total {
            self.total.store(t, Ordering::Release);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// How a fetch ended, short of an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// `stop` was raised. The `.part` stays for a later resume.
    Stopped,
}

pub fn client() -> reqwest::blocking::Client {
    crate::net::client_builder()
        .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        // Per read in the blocking client, so this catches a stalled
        // server without limiting how long a big file may take.
        .timeout(Duration::from_secs(30))
        .build()
        .expect("HTTP client")
}

/// Download `url` to `dest`, picking up a `.part` left by an earlier try
/// when `resume` is set. Converted streams can't resume (the server makes
/// a different file each time), so they start over.
///
/// A connection that breaks partway is retried from where it stopped, a
/// few times, before giving up.
pub fn fetch(
    http: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    resume: bool,
    progress: &Progress,
    stop: &AtomicBool,
) -> Result<Outcome, FetchError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    progress.set_state(State::Running);
    let mut tries = 0;
    loop {
        let attempt = if is_hls(url) {
            fetch_hls(http, url, dest, &part, progress, stop)
        } else {
            fetch_once(http, url, dest, &part, resume, progress, stop)
        };
        match attempt {
            Ok(o) => {
                progress.set_state(match o {
                    Outcome::Done => State::Done,
                    Outcome::Stopped => State::Stopped,
                });
                return Ok(o);
            }
            Err(e) => {
                tries += 1;
                let moved = progress.done() > 0;
                if !resume || !moved || tries >= 4 || stop.load(Ordering::Relaxed) {
                    progress.fail(&e.to_string());
                    return Err(e);
                }
                log::info!("download broke ({e}), picking it up again");
                std::thread::sleep(Duration::from_millis(500 * tries));
            }
        }
    }
}

fn fetch_once(
    http: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    part: &Path,
    resume: bool,
    progress: &Progress,
    stop: &AtomicBool,
) -> Result<Outcome, FetchError> {
    let have = if resume {
        std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    progress.done.store(have, Ordering::Release);
    let mut req = http.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().map_err(|e| FetchError::Http(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
        // The .part already holds all of it.
        std::fs::rename(part, dest)?;
        return Ok(Outcome::Done);
    }
    if !status.is_success() {
        return Err(FetchError::Http(format!("HTTP {status}")));
    }
    // A Subsonic error comes back as 200 with a JSON or XML body.
    if let Some(ct) = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|c| c.to_str().ok())
        && (ct.contains("json") || ct.contains("xml"))
    {
        let body = resp.text().unwrap_or_default();
        return Err(FetchError::Http(server_message(&body)));
    }
    // 206 means the server took the range; 200 means start over.
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)?;
    let mut done = if resumed { have } else { 0 };
    progress.done.store(done, Ordering::Release);
    let total = resp.content_length().map(|l| l + done);
    // A converted stream's length is the server's guess (Ampache works it
    // out from the bitrate and the song's length), so readers aren't told
    // it: a seek to the end of a guess lands in the wrong place.
    if let Some(t) = total {
        if resume {
            progress.total.store(t, Ordering::Release);
        } else {
            progress.estimate.store(t, Ordering::Release);
        }
    }
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if stop.load(Ordering::Relaxed) {
            file.flush()?;
            return Ok(Outcome::Stopped);
        }
        let n = resp
            .read(&mut buf)
            .map_err(|e| FetchError::Http(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        // Readers trust `done`, so it moves only once the bytes are written.
        progress.done.store(done, Ordering::Release);
    }
    file.flush()?;
    // Readers stop at the new end; one already past it sees the end of
    // the song there.
    let filler = trailing_filler(&mut file, done)?;
    if filler > 0 {
        log::info!("dropping {filler} bytes of filler the server added at the end");
        file.set_len(done - filler)?;
        progress.done.store(done - filler, Ordering::Release);
    }
    drop(file);
    if let Some(t) = total
        && done < t
    {
        return Err(FetchError::Http(format!(
            "connection closed at {done} of {t} bytes"
        )));
    }
    if done == 0 {
        return Err(FetchError::Http("the server sent an empty file".into()));
    }
    let done = done - filler;
    progress.total.store(done, Ordering::Release);
    std::fs::rename(part, dest)?;
    Ok(Outcome::Done)
}

/// An HLS playlist rather than a file: Plex's universal transcoder.
pub fn is_hls(url: &str) -> bool {
    url::Url::parse(url)
        .map(|u| u.path().ends_with(".m3u8"))
        .unwrap_or(false)
}

/// A URL from a playlist, made absolute, with the playlist's X-Plex-
/// arguments carried over (the token among them) when it has none.
fn hls_resolve(playlist: &url::Url, line: &str) -> Option<url::Url> {
    let mut u = playlist.join(line.trim()).ok()?;
    if u.host_str() == playlist.host_str() && !u.query_pairs().any(|(k, _)| k == "X-Plex-Token") {
        let carried: Vec<(String, String)> = playlist
            .query_pairs()
            .filter(|(k, _)| k.starts_with("X-Plex-"))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut q = u.query_pairs_mut();
        for (k, v) in carried {
            q.append_pair(&k, &v);
        }
    }
    Some(u)
}

/// What a playlist lists: variants (a master playlist) or segments, and
/// whether it's complete.
fn hls_parse(base: &url::Url, text: &str) -> (Vec<url::Url>, Vec<url::Url>, bool) {
    let mut variants = Vec::new();
    let mut segments = Vec::new();
    let mut next_is_variant = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-STREAM-INF") {
            next_is_variant = true;
        } else if line.is_empty() || line.starts_with('#') {
            continue;
        } else if let Some(u) = hls_resolve(base, line) {
            if next_is_variant || line.contains(".m3u8") {
                variants.push(u);
            } else {
                segments.push(u);
            }
            next_is_variant = false;
        }
    }
    (variants, segments, text.contains("#EXT-X-ENDLIST"))
}

/// Download an HLS stream's segments, in order, into one file. MPEG-TS
/// segments joined end to end are a file FFmpeg plays. Can't resume: the
/// server transcodes afresh each time.
fn fetch_hls(
    http: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    part: &Path,
    progress: &Progress,
    stop: &AtomicBool,
) -> Result<Outcome, FetchError> {
    let get_text = |u: &url::Url| -> Result<String, FetchError> {
        let resp = http
            .get(u.as_str())
            .send()
            .map_err(|e| FetchError::Http(e.without_url().to_string()))?;
        if !resp.status().is_success() {
            return Err(FetchError::Http(format!("HTTP {}", resp.status())));
        }
        resp.text()
            .map_err(|e| FetchError::Http(e.without_url().to_string()))
    };
    let mut playlist = url::Url::parse(url).map_err(|e| FetchError::Http(e.to_string()))?;
    // A master playlist points at the real one.
    for _ in 0..3 {
        let text = get_text(&playlist)?;
        let (variants, _, _) = hls_parse(&playlist, &text);
        match variants.into_iter().next() {
            Some(v) => playlist = v,
            None => break,
        }
    }
    let mut file = std::fs::File::create(part)?;
    let mut done = 0u64;
    progress.done.store(0, Ordering::Release);
    let mut fetched = 0usize;
    let mut idle = 0;
    loop {
        let text = get_text(&playlist)?;
        let (_, segments, ended) = hls_parse(&playlist, &text);
        let new = segments.len().saturating_sub(fetched);
        for seg in segments.iter().skip(fetched) {
            if stop.load(Ordering::Relaxed) {
                file.flush()?;
                return Ok(Outcome::Stopped);
            }
            let mut resp = http
                .get(seg.as_str())
                .send()
                .map_err(|e| FetchError::Http(e.without_url().to_string()))?;
            if !resp.status().is_success() {
                return Err(FetchError::Http(format!("segment: HTTP {}", resp.status())));
            }
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = resp
                    .read(&mut buf)
                    .map_err(|e| FetchError::Http(e.to_string()))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])?;
                done += n as u64;
                progress.done.store(done, Ordering::Release);
            }
            fetched += 1;
        }
        if ended {
            break;
        }
        // A live playlist grows as the server transcodes.
        if new == 0 {
            idle += 1;
            if idle > HLS_STALL {
                return Err(FetchError::Http("the conversion stopped".into()));
            }
        } else {
            idle = 0;
        }
        if stop.load(Ordering::Relaxed) {
            file.flush()?;
            return Ok(Outcome::Stopped);
        }
        std::thread::sleep(HLS_POLL);
    }
    file.flush()?;
    drop(file);
    if done == 0 {
        return Err(FetchError::Http("the server sent an empty stream".into()));
    }
    progress.total.store(done, Ordering::Release);
    std::fs::rename(part, dest)?;
    Ok(Outcome::Done)
}

/// How many bytes of space filler end a file `len` bytes long: 0, or at
/// least [`FILLER_MIN`].
fn trailing_filler(file: &mut std::fs::File, len: u64) -> std::io::Result<u64> {
    let mut run = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(buf.len() as u64);
        let n = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf[..n])?;
        let spaces = buf[..n].iter().rev().take_while(|&&b| b == b' ').count();
        run += spaces as u64;
        if spaces < n {
            break;
        }
        end = start;
    }
    Ok(if run >= FILLER_MIN && run < len {
        run
    } else {
        0
    })
}

/// "Song not found" out of a Subsonic error body, or the body itself.
fn server_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(m) = v
            .pointer("/subsonic-response/error/message")
            .or_else(|| v.pointer("/error/errorMessage"))
            .and_then(|m| m.as_str())
    {
        return format!("the server said: {m}");
    }
    let short: String = body.chars().take(120).collect();
    format!("the server sent an error instead of the song: {short}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_playlists_and_their_tokens() {
        let base = url::Url::parse(
            "http://pms:32400/music/:/transcode/universal/start.m3u8?path=x&X-Plex-Token=T&X-Plex-Client-Identifier=C",
        )
        .unwrap();
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nsession/abc/base/index.m3u8\n";
        let (v, s, end) = hls_parse(&base, master);
        assert_eq!(v.len(), 1);
        assert!(s.is_empty() && !end);
        assert!(v[0].as_str().starts_with(
            "http://pms:32400/music/:/transcode/universal/session/abc/base/index.m3u8?"
        ));
        assert!(
            v[0].query_pairs()
                .any(|(k, val)| k == "X-Plex-Token" && val == "T")
        );
        let list = "#EXTM3U\n#EXTINF:10,\n00000.ts\n#EXTINF:10,\n00001.ts\n#EXT-X-ENDLIST\n";
        let (v, s, end) = hls_parse(&v[0], list);
        assert!(v.is_empty() && end);
        assert_eq!(s.len(), 2);
        assert!(s[1].path().ends_with("/session/abc/base/00001.ts"));
        assert!(is_hls(base.as_str()));
        assert!(!is_hls("http://x/library/parts/1/2/file.flac"));
    }

    #[test]
    fn part_name() {
        assert_eq!(
            part_path(Path::new("/x/01 Blew.flac")),
            PathBuf::from("/x/01 Blew.flac.part")
        );
    }

    #[test]
    fn filler_at_the_end_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let check = |body: &[u8]| {
            std::fs::write(&p, body).unwrap();
            let mut f = std::fs::File::open(&p).unwrap();
            trailing_filler(&mut f, body.len() as u64).unwrap()
        };
        let mut song = vec![0xffu8; 200_000];
        assert_eq!(check(&song), 0);
        // An ID3v1 tag of blank fields ends in spaces too: not filler.
        song.extend(std::iter::repeat_n(b' ', 125));
        assert_eq!(check(&song), 0);
        // More than a chunk of it, to find across reads.
        song.extend(std::iter::repeat_n(b' ', 100_000));
        assert_eq!(check(&song), 100_125);
        // A file of nothing but spaces isn't trimmed to nothing.
        assert_eq!(check(&[b' '; 4000]), 0);
    }

    #[test]
    fn subsonic_errors_read_well() {
        let b = r#"{"subsonic-response":{"status":"failed","error":{"code":70,"message":"Song not found"}}}"#;
        assert_eq!(server_message(b), "the server said: Song not found");
    }
}
