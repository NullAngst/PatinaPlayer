//! Podcasts: fetching feeds and downloading episodes, on a thread of their
//! own so a slow server never holds up the UI or playback.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::library::Library;
use crate::library::podcasts::{EpisodeData, FeedData};

#[derive(Debug, thiserror::Error)]
pub enum PodcastError {
    #[error("{0}")]
    Http(String),
    #[error("not a podcast feed: {0}")]
    Feed(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Db(#[from] crate::library::LibError),
}

fn client(timeout: Option<Duration>) -> Result<reqwest::blocking::Client, PodcastError> {
    let mut b = reqwest::blocking::Client::builder()
        .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10));
    b = match timeout {
        Some(t) => b.timeout(t),
        None => b.timeout(None),
    };
    b.build().map_err(|e| PodcastError::Http(e.to_string()))
}

/// Fetch and parse a feed.
pub fn fetch_feed(url: &str) -> Result<FeedData, PodcastError> {
    let resp = client(Some(Duration::from_secs(30)))?
        .get(url)
        .send()
        .map_err(|e| PodcastError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(PodcastError::Http(format!("HTTP {}", resp.status())));
    }
    let bytes = resp
        .bytes()
        .map_err(|e| PodcastError::Http(e.to_string()))?;
    parse_feed(&bytes)
}

/// Parse RSS bytes. Items without an audio enclosure are skipped, since
/// there's nothing to play.
pub fn parse_feed(bytes: &[u8]) -> Result<FeedData, PodcastError> {
    let ch = rss::Channel::read_from(bytes).map_err(|e| PodcastError::Feed(e.to_string()))?;
    let it = ch.itunes_ext();
    let mut episodes = Vec::new();
    for item in ch.items() {
        let Some(enc) = item.enclosure() else {
            continue;
        };
        let url = enc.url().trim();
        if url.is_empty() {
            continue;
        }
        let mime = Some(enc.mime_type().to_string()).filter(|m| !m.is_empty());
        if mime.as_deref().is_some_and(|m| m.starts_with("video/")) {
            continue;
        }
        let title = item
            .title()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Untitled episode".into());
        // Feeds without guids are common; the enclosure URL is the next
        // most stable thing.
        let guid = item
            .guid()
            .map(|g| g.value().trim().to_string())
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| url.to_string());
        episodes.push(EpisodeData {
            guid,
            title,
            published: item.pub_date().and_then(parse_date),
            duration: item
                .itunes_ext()
                .and_then(|i| i.duration())
                .and_then(parse_itunes_duration),
            audio_url: url.to_string(),
            mime,
            size: enc.length().trim().parse().ok().filter(|&n: &i64| n > 0),
            description: item
                .itunes_ext()
                .and_then(|i| i.summary())
                .or(item.description())
                .map(strip_html),
        });
    }
    if episodes.is_empty() && ch.items().is_empty() && ch.title().is_empty() {
        return Err(PodcastError::Feed("no channel title and no items".into()));
    }
    Ok(FeedData {
        title: Some(ch.title().trim().to_string()).filter(|t| !t.is_empty()),
        author: it
            .and_then(|i| i.author())
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
        description: Some(strip_html(ch.description())).filter(|d| !d.is_empty()),
        image_url: it
            .and_then(|i| i.image())
            .map(str::to_string)
            .or_else(|| ch.image().map(|i| i.url().to_string())),
        link: Some(ch.link().to_string()).filter(|l| !l.is_empty()),
        episodes,
    })
}

/// RFC 2822 dates, plus the near misses feeds actually send.
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(d) = chrono::DateTime::parse_from_rfc2822(s) {
        return Some(d.timestamp());
    }
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp());
    }
    // "Tue, 3 Oct 2026 10:00:00 EDT" style zones chrono doesn't take: drop
    // the zone and read it as UTC. Off by a few hours at worst.
    let no_zone = s.rsplit_once(' ').map(|(a, _)| a)?;
    chrono::NaiveDateTime::parse_from_str(no_zone, "%a, %d %b %Y %H:%M:%S")
        .ok()
        .map(|d| d.and_utc().timestamp())
}

/// `HH:MM:SS`, `MM:SS` or plain seconds.
pub fn parse_itunes_duration(s: &str) -> Option<Duration> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let nums: Option<Vec<f64>> = parts.iter().map(|p| p.trim().parse::<f64>().ok()).collect();
    let nums = nums?;
    let secs = match nums.as_slice() {
        [s] => *s,
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    };
    (secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Feed descriptions are HTML more often than not. Plain text for a
/// terminal: tags out, common entities decoded, whitespace collapsed.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Safe file name piece: letters, digits, spaces and a few marks.
fn slug(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.,()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let t = t.trim().trim_matches('.').to_string();
    let t: String = t.chars().take(80).collect();
    if t.is_empty() { "episode".into() } else { t }
}

fn extension(url: &str, mime: Option<&str>) -> &'static str {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    for ext in [
        "mp3", "m4a", "aac", "ogg", "opus", "flac", "wav", "mp4", "m4b",
    ] {
        if path.ends_with(&format!(".{ext}")) {
            return match ext {
                "mp3" => "mp3",
                "m4a" => "m4a",
                "aac" => "aac",
                "ogg" => "ogg",
                "opus" => "opus",
                "flac" => "flac",
                "wav" => "wav",
                "mp4" => "mp4",
                _ => "m4b",
            };
        }
    }
    match mime.unwrap_or("") {
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" => "flac",
        _ => "mp3",
    }
}

/// `~/.local/share/patinaplayer/offline/podcasts/<show>/<date> <title>.<ext>`
pub fn download_path(
    root: &Path,
    show: &str,
    ep: &crate::library::podcasts::EpisodeRow,
) -> PathBuf {
    let date = ep
        .published
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|d| d.format("%Y-%m-%d ").to_string())
        .unwrap_or_default();
    root.join(slug(show)).join(format!(
        "{date}{}.{}",
        slug(&ep.title),
        extension(&ep.audio_url, ep.mime.as_deref())
    ))
}

/// Download `url` to `dest`, resuming a previous partial download. The
/// file lands under its real name only once it's complete, so a half
/// download never looks like a finished one.
pub fn download(
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, Option<u64>),
) -> Result<(), PodcastError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = client(None)?.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().map_err(|e| PodcastError::Http(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
        // Already have all of it.
        std::fs::rename(&part, dest)?;
        return Ok(());
    }
    if !status.is_success() {
        return Err(PodcastError::Http(format!("HTTP {status}")));
    }
    // 206 means the server honoured the range; 200 means start over.
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)?;
    let mut done = if resumed { have } else { 0 };
    let total = resp.content_length().map(|l| l + done);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(PodcastError::Http("cancelled".into()));
        }
        let n = resp
            .read(&mut buf)
            .map_err(|e| PodcastError::Http(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    drop(file);
    if let Some(t) = total
        && done < t
    {
        return Err(PodcastError::Http(format!(
            "connection closed at {done} of {t} bytes"
        )));
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub enum PodEvent {
    /// Feeds were fetched (or failed); views should re-query.
    Updated,
    Download {
        episode: i64,
        done: u64,
        total: Option<u64>,
    },
    Downloaded(i64),
    Error(String),
}

enum Cmd {
    SetFeeds(Vec<String>),
    Refresh,
    Download(i64),
    CancelDownload(i64),
    Quit,
}

#[derive(Debug, Clone, Default)]
pub struct PodStatus {
    pub refreshing: bool,
    /// Episode id, bytes done, total if known.
    pub downloading: Option<(i64, u64, Option<u64>)>,
    pub queued: Vec<i64>,
}

/// The download in progress and the flag that stops it.
type CancelSlot = Arc<Mutex<Option<(i64, Arc<AtomicBool>)>>>;

pub struct PodcastService {
    tx: Sender<Cmd>,
    events: Receiver<PodEvent>,
    status: Arc<Mutex<PodStatus>>,
    cancel: CancelSlot,
}

impl PodcastService {
    pub fn start(
        db: &Path,
        offline_root: PathBuf,
        feeds: Vec<String>,
    ) -> Result<Self, PodcastError> {
        let lib = Library::open(db)?;
        lib.sync_podcasts(&feeds)?;
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let status = Arc::new(Mutex::new(PodStatus::default()));
        let cancel = Arc::new(Mutex::new(None));
        let w = Worker {
            lib,
            feeds,
            offline_root,
            events: ev_tx,
            status: status.clone(),
            cancel: cancel.clone(),
        };
        let cmds = rx;
        std::thread::Builder::new()
            .name("patina-podcasts".into())
            .spawn(move || w.run(cmds))
            .expect("spawn podcast thread");
        let _ = tx.send(Cmd::Refresh);
        Ok(Self {
            tx,
            events: ev_rx,
            status,
            cancel,
        })
    }

    pub fn events(&self) -> &Receiver<PodEvent> {
        &self.events
    }

    pub fn status(&self) -> PodStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn set_feeds(&self, feeds: Vec<String>) {
        let _ = self.tx.send(Cmd::SetFeeds(feeds));
    }

    pub fn refresh(&self) {
        let _ = self.tx.send(Cmd::Refresh);
    }

    pub fn download(&self, episode: i64) {
        self.status.lock().unwrap().queued.push(episode);
        let _ = self.tx.send(Cmd::Download(episode));
    }

    pub fn cancel_download(&self, episode: i64) {
        if let Some((id, flag)) = &*self.cancel.lock().unwrap()
            && *id == episode
        {
            flag.store(true, Ordering::Relaxed);
        }
        self.status.lock().unwrap().queued.retain(|&e| e != episode);
        let _ = self.tx.send(Cmd::CancelDownload(episode));
    }
}

impl Drop for PodcastService {
    fn drop(&mut self) {
        if let Some((_, flag)) = &*self.cancel.lock().unwrap() {
            flag.store(true, Ordering::Relaxed);
        }
        let _ = self.tx.send(Cmd::Quit);
    }
}

struct Worker {
    lib: Library,
    feeds: Vec<String>,
    offline_root: PathBuf,
    events: Sender<PodEvent>,
    status: Arc<Mutex<PodStatus>>,
    cancel: CancelSlot,
}

impl Worker {
    fn run(mut self, cmds: Receiver<Cmd>) {
        let mut cancelled: HashSet<i64> = HashSet::new();
        for cmd in cmds {
            match cmd {
                Cmd::Quit => return,
                Cmd::SetFeeds(f) => {
                    self.feeds = f;
                    if let Err(e) = self.lib.sync_podcasts(&self.feeds) {
                        let _ = self.events.send(PodEvent::Error(e.to_string()));
                    }
                    self.refresh();
                }
                Cmd::Refresh => self.refresh(),
                Cmd::CancelDownload(id) => {
                    cancelled.insert(id);
                }
                Cmd::Download(id) => {
                    if cancelled.remove(&id) {
                        continue;
                    }
                    self.download(id);
                }
            }
        }
    }

    fn refresh(&mut self) {
        self.status.lock().unwrap().refreshing = true;
        for url in self.feeds.clone() {
            match fetch_feed(&url) {
                Ok(data) => {
                    if let Err(e) = self.lib.store_feed(&url, &data) {
                        let _ = self.events.send(PodEvent::Error(e.to_string()));
                    }
                }
                Err(e) => {
                    let msg = format!("{url}: {e}");
                    let _ = self.lib.set_feed_error(&url, &e.to_string());
                    let _ = self.events.send(PodEvent::Error(msg));
                }
            }
            let _ = self.events.send(PodEvent::Updated);
        }
        self.status.lock().unwrap().refreshing = false;
        let _ = self.events.send(PodEvent::Updated);
    }

    fn download(&mut self, id: i64) {
        let ep = match self.lib.episode(id) {
            Ok(Some(e)) => e,
            _ => return,
        };
        {
            let mut st = self.status.lock().unwrap();
            st.queued.retain(|&e| e != id);
            st.downloading = Some((id, 0, ep.size.map(|s| s as u64)));
        }
        let flag = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap() = Some((id, flag.clone()));
        let dest = download_path(&self.offline_root, &ep.podcast_title, &ep);
        let status = self.status.clone();
        let events = self.events.clone();
        let last_sent = std::cell::Cell::new(0u64);
        let progress = move |done: u64, total: Option<u64>| {
            status.lock().unwrap().downloading = Some((id, done, total));
            // An event every half megabyte is plenty for a progress readout.
            if done >= last_sent.get() + 512 * 1024 {
                last_sent.set(done);
                let _ = events.send(PodEvent::Download {
                    episode: id,
                    done,
                    total,
                });
            }
        };
        let res = download(&ep.audio_url, &dest, &flag, &progress);
        *self.cancel.lock().unwrap() = None;
        self.status.lock().unwrap().downloading = None;
        match res {
            Ok(()) => {
                if let Err(e) = self.lib.set_episode_download(id, Some(&dest)) {
                    let _ = self.events.send(PodEvent::Error(e.to_string()));
                }
                let _ = self.events.send(PodEvent::Downloaded(id));
            }
            Err(e) if flag.load(Ordering::Relaxed) => {
                log::info!("download of {} cancelled: {e}", ep.title);
            }
            Err(e) => {
                let _ = self.events.send(PodEvent::Error(format!(
                    "download of \"{}\" failed: {e}",
                    ep.title
                )));
            }
        }
        let _ = self.events.send(PodEvent::Updated);
    }
}

/// Delete an episode's local copy and forget it.
pub fn delete_download(lib: &Library, id: i64) -> Result<(), PodcastError> {
    if let Some(ep) = lib.episode(id)?
        && let Some(p) = &ep.download
    {
        std::fs::remove_file(p)?;
    }
    lib.set_episode_download(id, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
<channel>
  <title>Rust Decay</title>
  <link>https://example.com</link>
  <description>&lt;p&gt;About &amp;amp; things&lt;/p&gt;</description>
  <itunes:author>Zack</itunes:author>
  <item>
    <title>Episode 2</title>
    <guid>ep-2</guid>
    <pubDate>Sat, 03 Oct 2026 10:00:00 +0000</pubDate>
    <itunes:duration>1:02:03</itunes:duration>
    <enclosure url="https://example.com/2.mp3" length="1234" type="audio/mpeg"/>
  </item>
  <item>
    <title>Episode 1</title>
    <pubDate>Sat, 26 Sep 2026 10:00:00 EDT</pubDate>
    <itunes:duration>754</itunes:duration>
    <enclosure url="https://example.com/1.m4a?x=1" length="0" type="audio/mp4"/>
  </item>
  <item><title>Text post, no audio</title></item>
</channel>
</rss>"#;

    #[test]
    fn parses_feed() {
        let f = parse_feed(FEED.as_bytes()).unwrap();
        assert_eq!(f.title.as_deref(), Some("Rust Decay"));
        assert_eq!(f.author.as_deref(), Some("Zack"));
        assert_eq!(f.episodes.len(), 2, "the item without audio is skipped");
        let e2 = &f.episodes[0];
        assert_eq!(e2.guid, "ep-2");
        assert_eq!(e2.duration, Some(Duration::from_secs(3723)));
        assert_eq!(e2.size, Some(1234));
        let e1 = &f.episodes[1];
        assert_eq!(
            e1.guid, "https://example.com/1.m4a?x=1",
            "falls back to the URL"
        );
        assert_eq!(e1.duration, Some(Duration::from_secs(754)));
        assert!(e1.published.is_some(), "EDT zone still parses");
        assert_eq!(e1.size, None);
    }

    #[test]
    fn durations_and_html() {
        assert_eq!(
            parse_itunes_duration("45:00"),
            Some(Duration::from_secs(2700))
        );
        assert_eq!(parse_itunes_duration("nope"), None);
        assert_eq!(strip_html("<p>Hi&nbsp;<b>there</b></p>"), "Hi there");
        assert_eq!(extension("https://x/a.M4A?y", None), "m4a");
        assert_eq!(extension("https://x/stream", Some("audio/ogg")), "ogg");
        assert_eq!(slug("a/b: c?"), "a_b_ c_");
    }
}
