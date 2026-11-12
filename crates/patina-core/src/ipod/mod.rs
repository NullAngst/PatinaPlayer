//! iPod sync: songs, albums and playlists onto a classic iPod or an older
//! nano, the way Rhythmbox and gtkpod do it, through libgpod.
//!
//! An iPod shows up in the sidebar while one is mounted (anything with an
//! `iPod_Control/iTunes` folder at the top of a mounted drive). Sending
//! songs copies them over, converting what the iPod can't play with the
//! `ffmpeg` command, and writes the iPod's database once at the end, so a
//! long send that's stopped halfway loses nothing already written. Songs
//! already on it (same artist, album, title and track number) aren't sent
//! twice.
//!
//! What works is what libgpod supports: iPod classic, the older nanos
//! (up to the 5th generation), minis, shuffles up to the 2nd generation.
//! The 6th and 7th generation nanos and anything touch-screened don't.

pub mod gpod;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// Where songs the iPod can't play end up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// Lossless stays lossless, as Apple Lossless at 16 bits; lossy
    /// formats it can't play become AAC 256 kbps.
    #[default]
    Alac,
    /// Everything that isn't MP3 or AAC becomes AAC 256 kbps.
    Aac,
    /// Everything that isn't MP3 becomes MP3 320 kbps.
    Mp3,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Alac, Target::Aac, Target::Mp3];

    pub fn as_str(self) -> &'static str {
        match self {
            Target::Alac => "alac",
            Target::Aac => "aac",
            Target::Mp3 => "mp3",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Alac => "Apple Lossless (lossy ones to AAC)",
            Target::Aac => "AAC 256 kbps",
            Target::Mp3 => "MP3 320 kbps",
        }
    }
}

impl std::str::FromStr for Target {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Target::ALL
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("unknown \"{s}\" (alac, aac, mp3)"))
    }
}

/// How one song goes onto the iPod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// As it is.
    Copy(&'static str),
    Alac {
        rate: Option<u32>,
    },
    Aac,
    Mp3,
}

/// The iPod's name for a file type, for one it plays as it is.
fn playable(codec: &str) -> Option<&'static str> {
    match codec.to_ascii_lowercase().as_str() {
        "mp3" => Some("MPEG audio file"),
        "aac" => Some("AAC audio file"),
        "alac" => Some("Apple Lossless audio file"),
        "wav" | "pcm" => Some("WAV audio file"),
        "aiff" => Some("AIFF audio file"),
        _ => None,
    }
}

/// A rate an iPod plays: 44.1 kHz and its multiples come down to 44.1,
/// the rest to 48.
fn ipod_rate(rate: u32) -> Option<u32> {
    if rate <= 48_000 {
        None
    } else if rate.is_multiple_of(44_100) {
        Some(44_100)
    } else {
        Some(48_000)
    }
}

/// What a song needs to play on an iPod. Hi-res lossless comes down to
/// 16 bits and 44.1 or 48 kHz, since the iPod's DAC plays nothing higher
/// and some models refuse it.
pub fn plan(codec: &str, lossless: bool, rate: u32, bits: Option<u32>, target: Target) -> Plan {
    let hires = lossless && (rate > 48_000 || bits.is_some_and(|b| b > 16));
    let codec_l = codec.to_ascii_lowercase();
    match target {
        Target::Mp3 => {
            if codec_l == "mp3" {
                Plan::Copy("MPEG audio file")
            } else {
                Plan::Mp3
            }
        }
        Target::Aac => match codec_l.as_str() {
            "mp3" => Plan::Copy("MPEG audio file"),
            "aac" => Plan::Copy("AAC audio file"),
            _ => Plan::Aac,
        },
        Target::Alac => {
            if let Some(ft) = playable(codec)
                && !hires
            {
                return Plan::Copy(ft);
            }
            if lossless {
                Plan::Alac {
                    rate: ipod_rate(rate),
                }
            } else {
                Plan::Aac
            }
        }
    }
}

/// A song to send, with the file it's in here.
#[derive(Debug, Clone, PartialEq)]
pub struct SendSong {
    pub file: PathBuf,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: i32,
    pub track_no: i32,
    pub disc_no: i32,
    pub duration_ms: i32,
    pub codec: String,
    pub lossless: bool,
    pub sample_rate: u32,
    pub bits: Option<u32>,
    pub kbps: u32,
    /// The library path, for finding cover art.
    pub art_from: PathBuf,
}

/// What's on the iPod, for its view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contents {
    pub name: String,
    pub songs: usize,
    pub playlists: Vec<(String, usize)>,
    /// Artist, album, songs, total length.
    pub albums: Vec<(String, String, usize, Duration)>,
    pub free: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IpodEvent {
    Contents(Contents),
    /// Sending: songs done, songs in all, the one now.
    Progress {
        done: usize,
        total: usize,
        now: String,
    },
    Done(String),
    Error(String),
    Ejected,
}

/// What to take off.
#[derive(Debug, Clone, PartialEq)]
pub enum Removal {
    Playlist(String),
    Album { artist: String, album: String },
}

enum Cmd {
    Scan,
    Send {
        songs: Vec<SendSong>,
        playlist: Option<String>,
        target: Target,
    },
    Remove(Removal),
    Eject,
}

/// A mounted iPod, and the thread that works on it.
pub struct Ipod {
    pub mount: PathBuf,
    /// The block device it's mounted from, for ejecting.
    pub device: String,
    tx: Option<Sender<Cmd>>,
    events: Receiver<IpodEvent>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Ipod {
    pub fn open(mount: PathBuf, device: String) -> Self {
        let (tx, rx) = unbounded();
        let (etx, events) = unbounded();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = Worker {
            mount: mount.clone(),
            device: device.clone(),
            rx,
            events: etx,
            stop: stop.clone(),
            gpod: None,
        };
        let join = std::thread::Builder::new()
            .name("patina-ipod".into())
            .spawn(move || w.run())
            .expect("spawn iPod thread");
        let me = Self {
            mount,
            device,
            tx: Some(tx),
            events,
            stop,
            join: Some(join),
        };
        me.scan();
        me
    }

    fn send(&self, c: Cmd) {
        if let Some(t) = &self.tx {
            let _ = t.send(c);
        }
    }

    pub fn scan(&self) {
        self.send(Cmd::Scan);
    }

    pub fn send_songs(&self, songs: Vec<SendSong>, playlist: Option<String>, target: Target) {
        self.stop.store(false, std::sync::atomic::Ordering::Relaxed);
        self.send(Cmd::Send {
            songs,
            playlist,
            target,
        });
    }

    /// Stop a send after the song it's on.
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn remove(&self, r: Removal) {
        self.send(Cmd::Remove(r));
    }

    pub fn eject(&self) {
        self.send(Cmd::Eject);
    }

    pub fn events(&self) -> &Receiver<IpodEvent> {
        &self.events
    }
}

impl Drop for Ipod {
    fn drop(&mut self) {
        self.stop();
        self.tx = None;
        // Don't wait on a send still copying to a drive that's gone; the
        // thread stops at the next song.
        if let Some(j) = self.join.take()
            && j.is_finished()
        {
            let _ = j.join();
        }
    }
}

/// What the browser keeps about iPods.
#[derive(Default)]
pub struct IpodState {
    pub ipod: Option<Ipod>,
    pub contents: Option<Contents>,
    /// Sending: done, of, the song now.
    pub progress: Option<(usize, usize, String)>,
    /// What went wrong last, shown in the view.
    pub problem: Option<String>,
    pub checked: Option<std::time::Instant>,
}

/// Mounted iPods: (mount point, device), from /proc/self/mounts.
pub fn find() -> Vec<(PathBuf, String)> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };
    find_in(&text, |p| p.join("iPod_Control").join("iTunes").is_dir())
}

/// [`find`] over a mounts table, with the iPod test passed in.
pub fn find_in(mounts: &str, is_ipod: impl Fn(&Path) -> bool) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let mut f = line.split_whitespace();
        let (Some(dev), Some(mp)) = (f.next(), f.next()) else {
            continue;
        };
        // Real drives only: no /proc, tmpfs, overlays.
        if !dev.starts_with('/') {
            continue;
        }
        let mp = PathBuf::from(unescape_mount(mp));
        if is_ipod(&mp) && !out.iter().any(|(m, _)| *m == mp) {
            out.push((mp, dev.to_string()));
        }
    }
    out
}

/// /proc/mounts writes spaces and the like as octal: "\040".
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A program on PATH, the way `command -v` finds it.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Space on the drive: (free, total) bytes, from `df`.
fn space(mount: &Path) -> (u64, u64) {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(mount)
        .env("LC_ALL", "C")
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return (0, 0) };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().nth(1) else {
        return (0, 0);
    };
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 4 {
        return (0, 0);
    }
    let k = |s: &str| s.parse::<u64>().unwrap_or(0) * 1024;
    (k(f[3]), k(f[1]))
}

/// Same song, for not sending it twice.
fn same_key(artist: &str, album: &str, title: &str, track: i32) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{track}",
        artist.trim().to_lowercase(),
        album.trim().to_lowercase(),
        title.trim().to_lowercase()
    )
}

struct Worker {
    mount: PathBuf,
    device: String,
    rx: Receiver<Cmd>,
    events: Sender<IpodEvent>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    gpod: Option<gpod::Gpod>,
}

impl Worker {
    fn emit(&self, e: IpodEvent) {
        let _ = self.events.send(e);
    }

    fn run(mut self) {
        while let Ok(c) = self.rx.recv() {
            if self.gpod.is_none() {
                match gpod::Gpod::load() {
                    Ok(g) => self.gpod = Some(g),
                    Err(e) => {
                        self.emit(IpodEvent::Error(e));
                        continue;
                    }
                }
            }
            match c {
                Cmd::Scan => self.scan(),
                Cmd::Send {
                    songs,
                    playlist,
                    target,
                } => {
                    match self.send_songs(&songs, playlist.as_deref(), target) {
                        Ok(msg) => self.emit(IpodEvent::Done(msg)),
                        Err(e) => self.emit(IpodEvent::Error(e)),
                    }
                    self.scan();
                }
                Cmd::Remove(r) => {
                    match self.remove(&r) {
                        Ok(msg) => self.emit(IpodEvent::Done(msg)),
                        Err(e) => self.emit(IpodEvent::Error(e)),
                    }
                    self.scan();
                }
                Cmd::Eject => match eject(&self.device) {
                    Ok(()) => {
                        self.emit(IpodEvent::Ejected);
                        return;
                    }
                    Err(e) => self.emit(IpodEvent::Error(e)),
                },
            }
        }
    }

    fn scan(&self) {
        let g = self.gpod.as_ref().unwrap();
        match contents(g, &self.mount) {
            Ok(c) => self.emit(IpodEvent::Contents(c)),
            Err(e) => self.emit(IpodEvent::Error(e)),
        }
    }

    fn send_songs(
        &self,
        songs: &[SendSong],
        playlist: Option<&str>,
        target: Target,
    ) -> Result<String, String> {
        let g = self.gpod.as_ref().unwrap();
        let db = g.parse(&self.mount).map_err(|e| e.to_string())?;
        let res = send_into(
            g,
            db,
            &self.mount,
            songs,
            playlist,
            target,
            &self.stop,
            |d, t, now| {
                self.emit(IpodEvent::Progress {
                    done: d,
                    total: t,
                    now: now.to_string(),
                })
            },
        );
        let write = g.write(db);
        g.free(db);
        let msg = res?;
        write.map_err(|e| e.to_string())?;
        sync_disk();
        Ok(msg)
    }

    fn remove(&self, r: &Removal) -> Result<String, String> {
        let g = self.gpod.as_ref().unwrap();
        let db = g.parse(&self.mount).map_err(|e| e.to_string())?;
        let res = remove_from(g, db, r);
        let write = g.write(db);
        g.free(db);
        let msg = res?;
        write.map_err(|e| e.to_string())?;
        sync_disk();
        Ok(msg)
    }
}

fn sync_disk() {
    let _ = std::process::Command::new("sync").status();
}

/// Read what's on the iPod.
pub fn contents(g: &gpod::Gpod, mount: &Path) -> Result<Contents, String> {
    use gpod::track as t;
    let db = g.parse(mount).map_err(|e| e.to_string())?;
    let tracks = g.tracks(db);
    let mut albums: std::collections::BTreeMap<(String, String), (String, String, usize, u64)> =
        Default::default();
    for &tr in &tracks {
        let artist = {
            let a = g.get_str(tr, t::ALBUMARTIST);
            if a.is_empty() {
                g.get_str(tr, t::ARTIST)
            } else {
                a
            }
        };
        let album = g.get_str(tr, t::ALBUM);
        let e = albums
            .entry((artist.to_lowercase(), album.to_lowercase()))
            .or_insert_with(|| (artist.clone(), album.clone(), 0, 0));
        e.2 += 1;
        e.3 += g.get_i32(tr, t::TRACKLEN).max(0) as u64;
    }
    let mut name = String::new();
    let mut playlists = Vec::new();
    for pl in g.playlists(db) {
        if g.is_special(pl) {
            if name.is_empty() && pl == g.mpl(db) {
                name = g.playlist_name(pl);
            }
            continue;
        }
        playlists.push((g.playlist_name(pl), g.members(pl).len()));
    }
    let songs = tracks.len();
    g.free(db);
    playlists.sort_by_key(|p| p.0.to_lowercase());
    let (free, total) = space(mount);
    if name.is_empty() {
        name = mount
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "iPod".into());
    }
    Ok(Contents {
        name,
        songs,
        playlists,
        albums: albums
            .into_values()
            .map(|(a, al, n, ms)| (a, al, n, Duration::from_millis(ms)))
            .collect(),
        free,
        total,
    })
}

/// Copy songs into an open database, and put them in a playlist if one's
/// named. Doesn't write the database; the caller does, once.
#[allow(clippy::too_many_arguments)]
pub fn send_into(
    g: &gpod::Gpod,
    db: gpod::Db,
    mount: &Path,
    songs: &[SendSong],
    playlist: Option<&str>,
    target: Target,
    stop: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize, &str),
) -> Result<String, String> {
    use gpod::track as t;
    let _ = mount;
    let mut have: std::collections::HashMap<String, gpod::Track> = g
        .tracks(db)
        .into_iter()
        .map(|tr| {
            (
                same_key(
                    &g.get_str(tr, t::ARTIST),
                    &g.get_str(tr, t::ALBUM),
                    &g.get_str(tr, t::TITLE),
                    g.get_i32(tr, t::TRACK_NR),
                ),
                tr,
            )
        })
        .collect();
    let pl = playlist.map(|name| {
        g.playlists(db)
            .into_iter()
            .find(|&p| !g.is_special(p) && g.playlist_name(p) == name)
            .unwrap_or_else(|| g.new_playlist(db, name))
    });
    let mpl = g.mpl(db);
    let ffmpeg = which("ffmpeg");
    let tmp = crate::config::cache_dir().join("ipod");
    let _ = std::fs::create_dir_all(&tmp);
    let (mut sent, mut already, mut failed, mut needs_ffmpeg) = (0usize, 0usize, 0usize, 0usize);
    let mut first_error: Option<String> = None;
    let mut in_playlist: std::collections::HashSet<gpod::Track> = pl
        .map(|p| g.members(p).into_iter().collect())
        .unwrap_or_default();
    for (i, s) in songs.iter().enumerate() {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        progress(i, songs.len(), &s.title);
        let key = same_key(&s.artist, &s.album, &s.title, s.track_no);
        if let Some(&tr) = have.get(&key) {
            already += 1;
            if let Some(p) = pl
                && in_playlist.insert(tr)
            {
                g.add_to_playlist(p, tr);
            }
            continue;
        }
        let how = plan(&s.codec, s.lossless, s.sample_rate, s.bits, target);
        let (file, filetype, kbps, rate, temp) = match how {
            Plan::Copy(ft) => (s.file.clone(), ft, s.kbps, s.sample_rate, false),
            other => {
                let Some(ff) = &ffmpeg else {
                    needs_ffmpeg += 1;
                    continue;
                };
                match convert(ff, &s.file, &tmp, other) {
                    Ok((f, ft, k, r)) => (f, ft, k, r.unwrap_or(s.sample_rate), true),
                    Err(e) => {
                        failed += 1;
                        first_error.get_or_insert(format!("{}: {e}", s.title));
                        continue;
                    }
                }
            }
        };
        let tr = g.new_track();
        g.set_str(tr, t::TITLE, &s.title);
        g.set_str(tr, t::ARTIST, &s.artist);
        g.set_str(tr, t::ALBUM, &s.album);
        if !s.album_artist.is_empty() {
            g.set_str(tr, t::ALBUMARTIST, &s.album_artist);
        }
        if !s.genre.is_empty() {
            g.set_str(tr, t::GENRE, &s.genre);
        }
        g.set_str(tr, t::FILETYPE, filetype);
        g.set_i32(tr, t::YEAR, s.year);
        g.set_i32(tr, t::TRACK_NR, s.track_no);
        g.set_i32(tr, t::CD_NR, s.disc_no);
        g.set_i32(tr, t::TRACKLEN, s.duration_ms);
        g.set_i32(tr, t::BITRATE, kbps as i32);
        g.set_u16(tr, t::SAMPLERATE, rate.min(u16::MAX as u32) as u16);
        g.set_u32(
            tr,
            t::SIZE,
            std::fs::metadata(&file)
                .map(|m| m.len())
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
        );
        g.set_u32(tr, t::MEDIATYPE, gpod::MEDIATYPE_AUDIO);
        g.set_i64(tr, t::TIME_ADDED, crate::library::now_secs());
        if let Some(art) = crate::art::original_bytes(&s.art_from) {
            g.set_thumbnail(tr, &art);
        }
        g.add_track(db, tr);
        g.add_to_playlist(mpl, tr);
        let copied = g.copy_to_ipod(tr, &file);
        if temp {
            let _ = std::fs::remove_file(&file);
        }
        if let Err(e) = copied {
            // Out of the database again, so it doesn't list a song that
            // isn't there.
            g.remove_track(db, tr);
            failed += 1;
            first_error.get_or_insert(format!("{}: {e}", s.title));
            if e.0.contains("No space") {
                break;
            }
            continue;
        }
        have.insert(key, tr);
        if let Some(p) = pl {
            in_playlist.insert(tr);
            g.add_to_playlist(p, tr);
        }
        sent += 1;
    }
    progress(songs.len(), songs.len(), "");
    let mut msg = format!("Sent {} to the iPod", songs_n(sent));
    if let Some(p) = playlist {
        msg.push_str(&format!(", in \"{p}\""));
    }
    if already > 0 {
        msg.push_str(&format!(", {already} already on it"));
    }
    if needs_ffmpeg > 0 {
        msg.push_str(&format!(
            ". {needs_ffmpeg} need converting and ffmpeg isn't installed; install it to send those"
        ));
    }
    if failed > 0 {
        msg.push_str(&format!(". {failed} failed"));
        if let Some(e) = first_error {
            msg.push_str(&format!(", the first: {e}"));
        }
    }
    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        msg.push_str(". Stopped before the end");
    }
    Ok(msg)
}

fn songs_n(n: usize) -> String {
    format!("{n} song{}", if n == 1 { "" } else { "s" })
}

/// Convert a song for the iPod with ffmpeg. Returns the file, its iPod
/// file type, its bitrate and its new sample rate.
fn convert(
    ffmpeg: &Path,
    src: &Path,
    dir: &Path,
    how: Plan,
) -> Result<(PathBuf, &'static str, u32, Option<u32>), String> {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-map", "0:a:0", "-map_metadata", "-1", "-vn"]);
    let (ext, ft, kbps) = match how {
        Plan::Alac { rate } => {
            cmd.args(["-c:a", "alac", "-sample_fmt", "s16p"]);
            if let Some(r) = rate {
                cmd.args(["-ar", &r.to_string()]);
            }
            ("m4a", "Apple Lossless audio file", 0)
        }
        Plan::Aac => {
            cmd.args(["-c:a", "aac", "-b:a", "256k"]);
            ("m4a", "AAC audio file", 256)
        }
        Plan::Mp3 => {
            cmd.args(["-c:a", "libmp3lame", "-b:a", "320k"]);
            ("mp3", "MPEG audio file", 320)
        }
        Plan::Copy(_) => unreachable!(),
    };
    // AAC and MP3 top out at 48 kHz on an iPod too.
    if !matches!(how, Plan::Alac { .. }) {
        cmd.args(["-ar", "44100"]);
    }
    let out = dir.join(format!("send-{n}.{ext}"));
    cmd.arg(&out).stdout(Stdio::null()).stderr(Stdio::piped());
    let res = cmd
        .output()
        .map_err(|e| format!("couldn't run ffmpeg: {e}"))?;
    if !res.status.success() {
        let _ = std::fs::remove_file(&out);
        let why = String::from_utf8_lossy(&res.stderr);
        let why = why
            .lines()
            .last()
            .unwrap_or("ffmpeg failed")
            .trim()
            .to_string();
        return Err(why);
    }
    let rate = match how {
        Plan::Alac { rate } => rate,
        _ => Some(44_100),
    };
    Ok((out, ft, kbps, rate))
}

/// Take an album or a playlist off.
pub fn remove_from(g: &gpod::Gpod, db: gpod::Db, r: &Removal) -> Result<String, String> {
    use gpod::track as t;
    match r {
        Removal::Playlist(name) => {
            let Some(pl) = g
                .playlists(db)
                .into_iter()
                .find(|&p| !g.is_special(p) && g.playlist_name(p) == *name)
            else {
                return Err(format!("There's no playlist \"{name}\" on the iPod"));
            };
            g.remove_playlist(pl);
            Ok(format!(
                "Took the playlist \"{name}\" off the iPod. Its songs are still on it."
            ))
        }
        Removal::Album { artist, album } => {
            let (a, al) = (artist.to_lowercase(), album.to_lowercase());
            let gone: Vec<gpod::Track> = g
                .tracks(db)
                .into_iter()
                .filter(|&tr| {
                    let ar = {
                        let x = g.get_str(tr, t::ALBUMARTIST);
                        if x.is_empty() {
                            g.get_str(tr, t::ARTIST)
                        } else {
                            x
                        }
                    };
                    ar.to_lowercase() == a && g.get_str(tr, t::ALBUM).to_lowercase() == al
                })
                .collect();
            let n = gone.len();
            for tr in gone {
                if let Some(f) = g.file_on_ipod(tr) {
                    let _ = std::fs::remove_file(f);
                }
                g.remove_track(db, tr);
            }
            Ok(format!("Took {album} off the iPod ({})", songs_n(n)))
        }
    }
}

/// Unmount and power off, with udisks, the way the file manager does it.
fn eject(device: &str) -> Result<(), String> {
    let Some(udisks) = which("udisksctl") else {
        return Err(
            "Ejecting needs udisksctl (udisks2). Eject the iPod from your file manager instead."
                .into(),
        );
    };
    let run = |args: &[&str]| {
        std::process::Command::new(&udisks)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
    };
    sync_disk();
    let out = run(&["unmount", "-b", device]).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't unmount the iPod: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // Powering off lets it say it's safe to unplug. Not every iPod takes
    // it, and the unmount already made it safe, so a failure here is fine.
    let disk = device.trim_end_matches(|c: char| c.is_ascii_digit());
    let disk = disk
        .strip_suffix('p')
        .filter(|d| d.ends_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(disk);
    let _ = run(&["power-off", "-b", disk]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        assert_eq!(
            plan("FLAC", true, 44_100, Some(16), Target::Alac),
            Plan::Alac { rate: None }
        );
        assert_eq!(
            plan("FLAC", true, 96_000, Some(24), Target::Alac),
            Plan::Alac { rate: Some(48_000) }
        );
        assert_eq!(
            plan("FLAC", true, 88_200, Some(24), Target::Alac),
            Plan::Alac { rate: Some(44_100) }
        );
        assert_eq!(
            plan("ALAC", true, 44_100, Some(16), Target::Alac),
            Plan::Copy("Apple Lossless audio file")
        );
        assert_eq!(
            plan("ALAC", true, 96_000, Some(24), Target::Alac),
            Plan::Alac { rate: Some(48_000) }
        );
        assert_eq!(plan("Opus", false, 48_000, None, Target::Alac), Plan::Aac);
        assert_eq!(
            plan("MP3", false, 44_100, None, Target::Alac),
            Plan::Copy("MPEG audio file")
        );
        assert_eq!(plan("FLAC", true, 44_100, Some(16), Target::Aac), Plan::Aac);
        assert_eq!(plan("AAC", false, 44_100, None, Target::Mp3), Plan::Mp3);
    }

    #[test]
    fn finds_ipods_in_mounts() {
        let m = "proc /proc proc rw 0 0\n/dev/sdb2 /run/media/me/ZACK\\040IPOD vfat rw 0 0\n/dev/sda1 / ext4 rw 0 0\n";
        let found = find_in(m, |p| p.to_string_lossy().contains("IPOD"));
        assert_eq!(
            found,
            vec![(
                PathBuf::from("/run/media/me/ZACK IPOD"),
                "/dev/sdb2".to_string()
            )]
        );
    }
}
