//! Resumable downloads to a `.part` file, with progress any thread can
//! watch and a flag that stops them from another thread.
//!
//! The file only takes its real name once it's complete, so a half
//! download never passes for a finished one. A stopped or broken download
//! leaves its `.part` behind, and the next try asks the server for the
//! rest with an HTTP range.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

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
        match fetch_once(http, url, dest, &part, resume, progress, stop) {
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
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)?;
    let mut done = if resumed { have } else { 0 };
    progress.done.store(done, Ordering::Release);
    let total = resp.content_length().map(|l| l + done);
    if let Some(t) = total {
        progress.total.store(t, Ordering::Release);
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
    progress.total.store(done, Ordering::Release);
    std::fs::rename(part, dest)?;
    Ok(Outcome::Done)
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
    fn part_name() {
        assert_eq!(
            part_path(Path::new("/x/01 Blew.flac")),
            PathBuf::from("/x/01 Blew.flac.part")
        );
    }

    #[test]
    fn subsonic_errors_read_well() {
        let b = r#"{"subsonic-response":{"status":"failed","error":{"code":70,"message":"Song not found"}}}"#;
        assert_eq!(server_message(b), "the server said: Song not found");
    }
}
