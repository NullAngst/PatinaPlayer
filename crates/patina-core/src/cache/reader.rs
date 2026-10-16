//! Reading a song while it downloads, so playback starts as soon as the
//! first bytes are in instead of after the whole file.
//!
//! Reads behind the download come from the `.part` file. A read just past
//! it waits for the download to catch up, which it does at network speed.
//! A read far past it (a seek to the end of the song, or a format that
//! keeps its index at the end of the file) goes to the server with a
//! range request of its own, so it doesn't wait for everything between.
//! Once the download overtakes that spot, reads come from the file again.
//!
//! When playback catches up with the download, it doesn't go on playing
//! each bit the moment it lands, which on a slow server is a stutter of
//! tiny pieces. It waits for a cushion first: a few seconds of the song,
//! or, when the download is slower than the song plays, enough that the
//! rest can play through without catching up again (up to a limit, so a
//! very slow server still plays now and then instead of never).
//! A song streamed straight from the server, with no cache, is FFmpeg's
//! to buffer and doesn't come through here.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::fetch::{Progress, State};

/// How far past the download a read may be and still wait for it rather
/// than ask the server for that spot directly.
const WAIT_AHEAD: u64 = 512 * 1024;

/// How long a read waits for a download that has stopped moving before it
/// gives up on it.
const STALL: Duration = Duration::from_secs(45);

/// After catching up with the download: at least this much of the song
/// waits on disk before playing on.
const CUSHION_MIN: f64 = 4.0;

/// And never more than this much, however slow the download.
const CUSHION_MAX: f64 = 30.0;

/// The cushion in bytes when the song's rate isn't known (a converted
/// stream the server gave no size for). About 8 seconds at 128 kbps.
const CUSHION_BYTES: u64 = 128 * 1024;

/// Head room on the download's speed: it has to beat the song by this
/// much to count as keeping up, since it doesn't hold steady.
const MARGIN: f64 = 1.15;

/// A song file that may still be downloading.
pub struct PartReader {
    file: File,
    pos: u64,
    progress: Arc<Progress>,
    /// Where to ask for a range, if the server takes ranges.
    url: Option<String>,
    http: Option<reqwest::blocking::Client>,
    ahead: Option<Ahead>,
    abort: Option<Arc<AtomicBool>>,
    /// The song's length, for working out how many bytes a second of it
    /// takes.
    duration: Option<Duration>,
    /// Caught up with the download: waiting for a cushion.
    refilling: bool,
    /// Bytes downloaded when the reader opened, and when, for the speed.
    opened: (u64, Instant),
}

/// A range request reading ahead of the download.
struct Ahead {
    at: u64,
    resp: reqwest::blocking::Response,
}

impl PartReader {
    /// Open the `.part` of a download (or `dest`, if it finished and took
    /// its real name in the meantime). The downloader creates the `.part`
    /// before anyone is handed a reader. `url` is where the same bytes can
    /// be asked for by range; None waits for the download every time.
    pub fn open(
        dest: &Path,
        part: &Path,
        progress: Arc<Progress>,
        url: Option<String>,
        abort: Option<Arc<AtomicBool>>,
    ) -> io::Result<Self> {
        let file = match File::open(part) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => File::open(dest)?,
            Err(e) => return Err(e),
        };
        let opened = (progress.done(), Instant::now());
        Ok(Self {
            file,
            pos: 0,
            progress,
            url,
            http: None,
            ahead: None,
            abort,
            duration: None,
            refilling: false,
            opened,
        })
    }

    /// The song's length. With it (and the file's size), a stall waits for
    /// seconds of the song instead of a fixed number of bytes, and knows
    /// whether the download keeps up.
    pub fn with_duration(mut self, d: Option<Duration>) -> Self {
        self.duration = d.filter(|d| *d >= Duration::from_secs(1));
        self
    }

    /// Bytes a second of the song takes, when its size and length are known.
    fn byte_rate(&self) -> Option<f64> {
        let total = self.progress.size_hint()?;
        let d = self.duration?;
        Some(total as f64 / d.as_secs_f64()).filter(|r| *r > 0.0)
    }

    /// Bytes a second the download has managed since this reader opened.
    /// None until it has run for a second.
    fn download_rate(&self, done: u64) -> Option<f64> {
        let secs = self.opened.1.elapsed().as_secs_f64();
        (secs >= 1.0).then(|| done.saturating_sub(self.opened.0) as f64 / secs)
    }

    /// Whether enough has come in since playback caught up to play on.
    fn cushion_ready(&self, done: u64, state: State) -> bool {
        // Nothing more is coming for now (finished, failed, paused): play
        // what's there.
        if !matches!(state, State::Running | State::Waiting) {
            return true;
        }
        let total = self.progress.total();
        if total.is_some_and(|t| done >= t) {
            return true;
        }
        let have = done.saturating_sub(self.pos);
        let Some(rate) = self.byte_rate() else {
            return have >= CUSHION_BYTES;
        };
        let secs = have as f64 / rate;
        if secs >= CUSHION_MAX {
            return true;
        }
        if secs < CUSHION_MIN {
            return false;
        }
        match self.download_rate(done) {
            // The cushion came in under a second: plenty fast.
            None => true,
            Some(dl) if dl >= rate * MARGIN => true,
            Some(dl) if dl > 0.0 => match total {
                // Slower than the song: wait until what's left downloads
                // in less time than what's left takes to play, so it
                // plays through from here.
                Some(t) => {
                    let to_fetch = t.saturating_sub(done) as f64 / dl * MARGIN;
                    let to_play = t.saturating_sub(self.pos) as f64 / rate;
                    to_fetch <= to_play
                }
                None => false,
            },
            Some(_) => false,
        }
    }

    fn aborted(&self) -> bool {
        self.abort
            .as_ref()
            .is_some_and(|a| a.load(Ordering::Relaxed))
    }

    /// Read at `self.pos` from a range request, opening one if the current
    /// one (if any) is somewhere else. None if the server won't do ranges.
    fn read_ahead(&mut self, buf: &mut [u8]) -> Option<io::Result<usize>> {
        let url = self.url.clone()?;
        if self.ahead.as_ref().is_none_or(|a| a.at != self.pos) {
            self.ahead = None;
            let http = self.http.get_or_insert_with(|| {
                crate::net::client_builder()
                    .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(Duration::from_secs(15))
                    .build()
                    .expect("HTTP client")
            });
            let resp = http
                .get(&url)
                .header(reqwest::header::RANGE, format!("bytes={}-", self.pos))
                .send();
            match resp {
                Ok(r) if r.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
                    self.ahead = Some(Ahead {
                        at: self.pos,
                        resp: r,
                    });
                }
                Ok(_) | Err(_) => {
                    // No ranges here: wait for the download from now on.
                    log::info!("the server doesn't take ranges; waiting for the download");
                    self.url = None;
                    return None;
                }
            }
        }
        let a = self.ahead.as_mut()?;
        Some(match a.resp.read(buf) {
            Ok(n) => {
                a.at += n as u64;
                self.pos += n as u64;
                Ok(n)
            }
            Err(e) => {
                self.ahead = None;
                Err(e)
            }
        })
    }
}

impl Read for PartReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut last = self.progress.done();
        let mut moved = Instant::now();
        loop {
            let done = self.progress.done();
            let state = self.progress.state();
            if self.pos < done && (!self.refilling || self.cushion_ready(done, state)) {
                if self.refilling {
                    self.refilling = false;
                    log::info!(
                        "{} KB ahead of playback, playing on",
                        (done - self.pos) / 1024
                    );
                }
                // Behind the download: from the file.
                self.ahead = None;
                let n = (done - self.pos).min(buf.len() as u64) as usize;
                let got = self.file.read_at(&mut buf[..n], self.pos)?;
                self.pos += got as u64;
                return Ok(got);
            }
            if self.pos < done {
                // Waiting for the cushion. The download is moving, or the
                // stall check below says so.
                if self.aborted() {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                if done != last {
                    last = done;
                    moved = Instant::now();
                } else if moved.elapsed() > STALL {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the download stopped moving",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            match state {
                State::Done => return Ok(0),
                State::Failed => {
                    return Err(io::Error::other(
                        self.progress
                            .error()
                            .unwrap_or_else(|| "download failed".into()),
                    ));
                }
                // Stopped for a moment to let something else go first, or
                // between tries: it comes back, or the stall check gives up.
                State::Stopped | State::Waiting | State::Running => {}
            }
            if self.aborted() {
                // StreamIo checks the same flag and gives up on this.
                return Err(io::ErrorKind::Interrupted.into());
            }
            let total = self.progress.total();
            if total.is_some_and(|t| self.pos >= t) {
                return Ok(0);
            }
            if self.pos > done + WAIT_AHEAD
                && total.is_some()
                && let Some(r) = self.read_ahead(buf)
            {
                return r;
            }
            // Caught up with the download (playing it, or just started):
            // what comes next waits for a cushion.
            if !self.refilling && self.pos > 0 {
                log::info!("playback caught up with the download, buffering");
            }
            self.refilling = true;
            if done != last {
                last = done;
                moved = Instant::now();
            } else if moved.elapsed() > STALL {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the download stopped moving",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Seek for PartReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let size = || match self.progress.state() {
            State::Done => Some(self.progress.done()),
            _ => self.progress.total(),
        };
        let new = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => self
                .pos
                .checked_add_signed(d)
                .ok_or(io::ErrorKind::InvalidInput)?,
            SeekFrom::End(d) => {
                // Unknown size (a converted stream still coming): FFmpeg
                // takes that as "not seekable to the end", which is right.
                let s = size().ok_or(io::ErrorKind::Unsupported)?;
                s.checked_add_signed(d).ok_or(io::ErrorKind::InvalidInput)?
            }
        };
        self.pos = new;
        Ok(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_follow_the_download() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.flac");
        let part = dir.path().join("a.flac.part");
        std::fs::write(&part, b"").unwrap();
        let p = Arc::new(Progress::new());
        p.preset(0, Some(10));
        p.set_state(State::Running);
        let mut r = PartReader::open(&dest, &part, p.clone(), None, None).unwrap();
        let (p2, part2) = (p.clone(), part.clone());
        let writer = std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&part2)
                .unwrap();
            for i in 0..10u8 {
                std::thread::sleep(Duration::from_millis(15));
                f.write_all(&[i]).unwrap();
                p2.preset(u64::from(i) + 1, None);
            }
            p2.set_state(State::Done);
        });
        let mut all = Vec::new();
        r.read_to_end(&mut all).unwrap();
        writer.join().unwrap();
        assert_eq!(all, (0..10u8).collect::<Vec<_>>());
        // The size is known, so the end can be found.
        assert_eq!(r.seek(SeekFrom::End(-2)).unwrap(), 8);
        let mut b = [0u8; 4];
        assert_eq!(r.read(&mut b).unwrap(), 2);
        assert_eq!(&b[..2], &[8, 9]);
    }

    #[test]
    fn a_stall_waits_for_a_cushion() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("b.flac");
        let part = dir.path().join("b.flac.part");
        std::fs::write(&part, b"").unwrap();
        // 100 bytes a second of song, 100 seconds long.
        let p = Arc::new(Progress::new());
        p.preset(0, Some(10_000));
        p.set_state(State::Running);
        let mut r = PartReader::open(&dest, &part, p.clone(), None, None)
            .unwrap()
            .with_duration(Some(Duration::from_secs(100)));
        let (p2, part2) = (p.clone(), part.clone());
        // Trickles in 50 bytes at a time: far slower than the song.
        let writer = std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&part2)
                .unwrap();
            for i in 0..40u64 {
                std::thread::sleep(Duration::from_millis(5));
                f.write_all(&[7u8; 50]).unwrap();
                p2.preset((i + 1) * 50, None);
            }
            p2.set_state(State::Stopped);
        });
        let mut b = [0u8; 4096];
        let n = r.read(&mut b).unwrap();
        // Not a 50 byte scrap: at least the minimum cushion, 4 seconds.
        assert!(n >= 400, "read {n} bytes");
        writer.join().unwrap();
    }

    #[test]
    fn abort_cuts_a_wait_short() {
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(Progress::new());
        p.set_state(State::Running);
        let abort = Arc::new(AtomicBool::new(false));
        let part = dir.path().join("x.part");
        std::fs::write(&part, b"").unwrap();
        let mut r =
            PartReader::open(&dir.path().join("x"), &part, p, None, Some(abort.clone())).unwrap();
        let a2 = abort.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            a2.store(true, Ordering::Relaxed);
        });
        let e = r.read(&mut [0u8; 8]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
        // No size yet: the end isn't known.
        assert!(r.seek(SeekFrom::End(0)).is_err());
    }
}
