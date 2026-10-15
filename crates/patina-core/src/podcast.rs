//! Podcasts: fetching feeds on a thread of their own, so a slow server
//! never holds up the UI or playback. Episode downloads go through the
//! cache's download queue, with the songs.

use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone)]
pub enum PodEvent {
    /// Feeds were fetched (or failed); views should re-query.
    Updated,
    Error(String),
}

enum Cmd {
    SetFeeds(Vec<String>),
    Refresh,
    Quit,
}

#[derive(Debug, Clone, Default)]
pub struct PodStatus {
    pub refreshing: bool,
}

pub struct PodcastService {
    tx: Sender<Cmd>,
    events: Receiver<PodEvent>,
    status: Arc<Mutex<PodStatus>>,
}

impl PodcastService {
    pub fn start(db: &Path, feeds: Vec<String>) -> Result<Self, PodcastError> {
        let lib = Library::open(db)?;
        lib.sync_podcasts(&feeds)?;
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let status = Arc::new(Mutex::new(PodStatus::default()));
        let w = Worker {
            lib,
            feeds,
            events: ev_tx,
            status: status.clone(),
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
}

impl Drop for PodcastService {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
}

struct Worker {
    lib: Library,
    feeds: Vec<String>,
    events: Sender<PodEvent>,
    status: Arc<Mutex<PodStatus>>,
}

impl Worker {
    fn run(mut self, cmds: Receiver<Cmd>) {
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
