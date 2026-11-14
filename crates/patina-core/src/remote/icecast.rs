//! Icecast servers: their mount points, as radio stations.
//!
//! Icecast 2.4 and newer answer `/status-json.xsl` with every live mount:
//! its name, description, genre, bitrate and what's playing. Nothing needs
//! a login. Each mount becomes a station under the server's name, in Radio
//! and on the Icecast page, and the list is read again with every sync, so
//! a mount that goes live shows up and one that stops goes away.
//!
//! A mount's `listenurl` is built from the hostname Icecast was set up
//! with, which is often "localhost". So the stream address is the server
//! address you typed with the mount's path, which works from anywhere that
//! address does.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use super::http::{self, list, num};
use super::{
    Client, RemoteAlbum, RemoteError, RemotePlaylist, RemotePodcast, RemoteSong, RemoteStation,
    Result, SavedQueue, ServerInfo, ServerKind, StreamFormat,
};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Source {
    listenurl: Option<String>,
    server_name: Option<String>,
    server_description: Option<String>,
    genre: Option<String>,
    #[serde(deserialize_with = "num")]
    bitrate: Option<f64>,
    #[serde(deserialize_with = "num")]
    listeners: Option<f64>,
    title: Option<String>,
    artist: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Stats {
    server_id: Option<String>,
    #[serde(deserialize_with = "list")]
    source: Vec<Source>,
}

/// One live mount, with what Icecast says about it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mount {
    pub path: String,
    pub url: String,
    pub name: String,
    pub description: Option<String>,
    pub genre: Option<String>,
    pub kbps: Option<u32>,
    pub listeners: Option<u32>,
    /// What's playing, as the source client reports it.
    pub now: Option<String>,
}

pub struct Icecast {
    base: String,
    http: reqwest::blocking::Client,
    info: ServerInfo,
}

/// Unspecified fields come as these placeholders from a default setup.
fn real(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| {
        !s.is_empty() && !s.starts_with("Unspecified") && s != "(null)" && s != "no name"
    })
}

/// "https://radio.example.com:8000" from whatever was typed, with any
/// path or status page taken off.
pub fn normalize(url: &str) -> String {
    let mut u = url.trim().to_string();
    if !u.contains("://") {
        u = format!("http://{u}");
    }
    match url::Url::parse(&u) {
        Ok(p) => {
            let port = p.port().map(|n| format!(":{n}")).unwrap_or_default();
            format!("{}://{}{port}", p.scheme(), p.host_str().unwrap_or(""))
        }
        Err(_) => u.trim_end_matches('/').to_string(),
    }
}

impl Icecast {
    pub fn connect(url: &str) -> Result<Self> {
        let http = http::client()?;
        let base = normalize(url);
        let stats = fetch(&http, &base)?;
        let product = real(stats.server_id).unwrap_or_else(|| "Icecast".into());
        Ok(Self {
            base,
            http,
            info: ServerInfo {
                kind: ServerKind::Icecast,
                product,
                open_subsonic: false,
                api_version: String::new(),
                play_queue: false,
            },
        })
    }

    /// Every live mount now.
    pub fn mounts(&self) -> Result<Vec<Mount>> {
        Ok(mounts_of(&self.base, fetch(&self.http, &self.base)?))
    }
}

fn fetch(http: &reqwest::blocking::Client, base: &str) -> Result<Stats> {
    let (body, _) = http::get_bytes(http, &format!("{base}/status-json.xsl"), &[])?;
    // Older Icecast puts a bare "-" for an empty title, which isn't JSON.
    let text = String::from_utf8_lossy(&body).replace("\"title\":-}", "\"title\":null}");
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        RemoteError::Parse(
            "not an Icecast status page (Icecast 2.4 or newer has /status-json.xsl)".into(),
        )
    })?;
    let stats = v
        .get("icestats")
        .ok_or_else(|| RemoteError::Parse("not an Icecast status page".into()))?;
    serde_json::from_value(stats.clone()).map_err(|e| RemoteError::Parse(format!("icestats: {e}")))
}

fn mounts_of(base: &str, stats: Stats) -> Vec<Mount> {
    stats
        .source
        .into_iter()
        .filter_map(|s| {
            let listen = s.listenurl.clone()?;
            let path = url::Url::parse(&listen)
                .map(|u| u.path().to_string())
                .unwrap_or_else(|_| format!("/{}", listen.rsplit('/').next().unwrap_or_default()));
            if path.is_empty() || path == "/" {
                return None;
            }
            let now = match (real(s.artist), real(s.title)) {
                (Some(a), Some(t)) => Some(format!("{a} - {t}")),
                (None, Some(t)) => Some(t),
                (Some(a), None) => Some(a),
                (None, None) => None,
            };
            Some(Mount {
                url: format!("{base}{path}"),
                name: real(s.server_name)
                    .unwrap_or_else(|| path.trim_start_matches('/').to_string()),
                description: real(s.server_description),
                genre: real(s.genre),
                kbps: s.bitrate.filter(|b| *b > 0.0).map(|b| {
                    // Some setups report bits per second.
                    if b > 4000.0 {
                        (b / 1000.0) as u32
                    } else {
                        b as u32
                    }
                }),
                listeners: s.listeners.map(|l| l as u32),
                now,
                path,
            })
        })
        .collect()
}

impl Client for Icecast {
    fn info(&self) -> &ServerInfo {
        &self.info
    }

    fn ping(&self) -> Result<()> {
        fetch(&self.http, &self.base).map(|_| ())
    }

    fn albums(&self) -> Result<Vec<RemoteAlbum>> {
        Ok(Vec::new())
    }

    fn album_songs(&self, _album_id: &str) -> Result<Vec<RemoteSong>> {
        Ok(Vec::new())
    }

    fn stream_url_as(&self, _: &str, _: StreamFormat, _: u32) -> Result<(String, bool)> {
        Err(RemoteError::Unsupported("songs"))
    }

    fn stream_setting(&self) -> (StreamFormat, u32) {
        (StreamFormat::Raw, 0)
    }

    fn episode_url(&self, _: &str) -> Result<String> {
        Err(RemoteError::Unsupported("podcasts"))
    }

    fn cover(&self, _: &str, _: u32) -> Result<Vec<u8>> {
        Err(RemoteError::Unsupported("covers"))
    }

    fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        Ok(Vec::new())
    }

    fn playlist_songs(&self, _: &str) -> Result<Vec<RemoteSong>> {
        Ok(Vec::new())
    }

    fn create_playlist(&self, _: &str, _: &[String]) -> Result<String> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn replace_playlist(&self, _: &str, _: &[String]) -> Result<()> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn rename_playlist(&self, _: &str, _: &str) -> Result<()> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn delete_playlist(&self, _: &str) -> Result<()> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn radio(&self) -> Result<Vec<RemoteStation>> {
        Ok(self
            .mounts()?
            .into_iter()
            .map(|m| RemoteStation {
                id: m.path.clone(),
                name: match (&m.genre, m.kbps) {
                    (Some(g), Some(k)) => format!("{} ({g}, {k} kbps)", m.name),
                    (Some(g), None) => format!("{} ({g})", m.name),
                    (None, Some(k)) => format!("{} ({k} kbps)", m.name),
                    (None, None) => m.name.clone(),
                },
                url: m.url,
            })
            .collect())
    }

    fn podcasts(&self) -> Result<Vec<RemotePodcast>> {
        Ok(Vec::new())
    }

    fn save_queue(&self, _: &[String], _: Option<&str>, _: Duration) -> Result<()> {
        Err(RemoteError::Unsupported("a saved play queue"))
    }

    fn load_queue(&self) -> Result<Option<SavedQueue>> {
        Err(RemoteError::Unsupported("a saved play queue"))
    }

    fn bookmark(&self, _: &str, _: Duration) -> Result<()> {
        Err(RemoteError::Unsupported("podcasts"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_mount_or_many_and_localhost_urls() {
        let one: Stats = serde_json::from_str(
            r#"{"server_id":"Icecast 2.4.4","source":{"listenurl":"http://localhost:8000/live.ogg",
            "server_name":"Unspecified name","genre":"various","bitrate":128,"title":"Song","artist":"Band"}}"#,
        )
        .unwrap();
        let m = mounts_of("http://radio.lan:8000", one);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].url, "http://radio.lan:8000/live.ogg");
        assert_eq!(m[0].name, "live.ogg");
        assert_eq!(m[0].now.as_deref(), Some("Band - Song"));
        let many: Stats = serde_json::from_str(
            r#"{"source":[{"listenurl":"http://localhost:8000/a","server_name":"A","bitrate":"320"},
            {"listenurl":"http://localhost:8000/b","server_name":"B","audio_bitrate":128000}]}"#,
        )
        .unwrap();
        let m = mounts_of("http://x:8000", many);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].name.as_str(), m[0].kbps), ("A", Some(320)));
    }

    #[test]
    fn addresses_lose_their_paths() {
        assert_eq!(
            normalize("radio.example.com:8000/status.xsl"),
            "http://radio.example.com:8000"
        );
        assert_eq!(
            normalize("https://r.example.com/live"),
            "https://r.example.com"
        );
    }
}
