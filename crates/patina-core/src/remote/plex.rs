//! Plex Media Server, through the API Plex documents at developer.plex.tv/pms.
//!
//! Signing in goes through plex.tv (see [`super::plex_tv`]); everything
//! else is the server's own API, asked for in JSON. Each request carries
//! the X-Plex headers the docs list, with X-Plex-Pms-Api-Version saying
//! which version of the API this was written against.
//!
//! What maps to what:
//!
//! - Music libraries (sections of type "artist") hold the albums (type 9)
//!   and their tracks (the album's children). An album's tracks are read
//!   again with `/library/metadata/{ids}` for their streams, which carry
//!   the sample rate, bit depth and, with Plex Pass, the loudness.
//! - Songs stream as the original file, from the part's key, so bit-perfect
//!   and gapless work as with local files. A conversion goes through the
//!   universal transcoder, which the docs offer as HLS; the cache downloads
//!   its segments into one file (see [`crate::cache::fetch`]).
//! - Ratings are Plex's 0 to 10, so a star is 2.
//! - Plays count the way they do for Plex's own players: by timeline
//!   reports while a song plays. Plex counts a play once one gets past the
//!   end of the song, and shows the player on its dashboard meanwhile.
//! - With "Plex Pass features" on: similar songs come from the sonic
//!   analysis (`/nearest`), lyrics from the song's lyric stream, and
//!   loudness from the audio stream's gain. A server without Plex Pass
//!   just has none of these, and Patina falls back to what it does
//!   without them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

use super::http::{self, list, num, opt_id};
use super::plex_tv::{self, Login};
use super::{
    Client, RemoteAlbum, RemoteError, RemotePlaylist, RemotePodcast, RemoteSong, RemoteStation,
    Result, SavedQueue, ServerInfo, ServerKind, ServerLyrics, StreamFormat, Timeline,
};

/// The API version this follows, sent as X-Plex-Pms-Api-Version.
pub const API_VERSION: &str = "1.1.1";
/// Items per page when listing.
const PAGE: usize = 500;
/// Ids per `/library/metadata/{ids}` request.
const IDS_PER_CALL: usize = 50;
/// Ids per playlist add, to keep the URL a sane length.
const PLAYLIST_CHUNK: usize = 200;
/// The library provider every rating and play goes to.
const LIBRARY: &str = "com.plexapp.plugins.library";

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Tag {
    tag: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct Stream {
    #[serde(deserialize_with = "num")]
    stream_type: Option<f64>,
    #[serde(deserialize_with = "num")]
    sampling_rate: Option<f64>,
    #[serde(deserialize_with = "num")]
    bit_depth: Option<f64>,
    #[serde(deserialize_with = "num")]
    channels: Option<f64>,
    #[serde(deserialize_with = "num")]
    gain: Option<f64>,
    #[serde(deserialize_with = "num")]
    album_gain: Option<f64>,
    #[serde(deserialize_with = "num")]
    peak: Option<f64>,
    #[serde(deserialize_with = "num")]
    album_peak: Option<f64>,
    key: Option<String>,
    format: Option<String>,
    codec: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct Part {
    key: Option<String>,
    file: Option<String>,
    container: Option<String>,
    #[serde(deserialize_with = "num")]
    size: Option<f64>,
    #[serde(rename = "Stream", deserialize_with = "list")]
    stream: Vec<Stream>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct Media {
    #[serde(deserialize_with = "num")]
    bitrate: Option<f64>,
    #[serde(deserialize_with = "num")]
    audio_channels: Option<f64>,
    container: Option<String>,
    #[serde(rename = "Part", deserialize_with = "list")]
    part: Vec<Part>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Item {
    #[serde(deserialize_with = "opt_id")]
    rating_key: Option<String>,
    key: Option<String>,
    title: Option<String>,
    original_title: Option<String>,
    parent_title: Option<String>,
    grandparent_title: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    parent_rating_key: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    grandparent_rating_key: Option<String>,
    #[serde(rename = "librarySectionID", deserialize_with = "opt_id")]
    library_section_id: Option<String>,
    #[serde(deserialize_with = "num")]
    index: Option<f64>,
    #[serde(deserialize_with = "num")]
    parent_index: Option<f64>,
    #[serde(deserialize_with = "num")]
    year: Option<f64>,
    #[serde(deserialize_with = "num")]
    parent_year: Option<f64>,
    #[serde(deserialize_with = "num")]
    leaf_count: Option<f64>,
    #[serde(deserialize_with = "num")]
    duration: Option<f64>,
    #[serde(deserialize_with = "num")]
    user_rating: Option<f64>,
    #[serde(deserialize_with = "num")]
    view_count: Option<f64>,
    #[serde(deserialize_with = "num")]
    updated_at: Option<f64>,
    thumb: Option<String>,
    parent_thumb: Option<String>,
    smart: Option<Value>,
    #[serde(rename = "Media", deserialize_with = "list")]
    media: Vec<Media>,
    #[serde(rename = "Genre", deserialize_with = "list")]
    genre: Vec<Tag>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Location {
    path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Directory {
    #[serde(deserialize_with = "opt_id")]
    key: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "Location", deserialize_with = "list")]
    location: Vec<Location>,
}

/// A true that may come as true, 1 or "1".
fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// The MediaContainer of an answer.
fn container(v: &Value) -> &Value {
    v.get("MediaContainer").unwrap_or(v)
}

fn items(v: &Value) -> Vec<Item> {
    container(v)
        .get("Metadata")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| serde_json::from_value(x.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// A Plex rating (0 to 10) as stars.
fn stars(r: Option<f64>) -> Option<u8> {
    super::rating_of(r.map(|r| r / 2.0))
}

/// Whether an address is on this network: a private IP, a .local name, or
/// Plex's plex.direct name for a private IP.
fn is_local(base: &str) -> bool {
    let Some(host) = url::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    else {
        return false;
    };
    let ip_text = if host.ends_with(".plex.direct") {
        host.split('.').next().unwrap_or("").replace('-', ".")
    } else {
        host.clone()
    };
    if let Ok(ip) = ip_text.parse::<std::net::Ipv4Addr>() {
        return ip.is_private() || ip.is_loopback() || ip.is_link_local();
    }
    host == "localhost" || host.ends_with(".local") || host.ends_with(".lan")
}

/// A song's place in its library folder, for the folder view.
fn relative(file: &str, roots: &[String]) -> Option<String> {
    roots.iter().find_map(|r| {
        file.strip_prefix(r.trim_end_matches('/'))
            .filter(|rest| rest.starts_with('/'))
            .map(|rest| rest.trim_start_matches('/').to_string())
    })
}

/// What the sync remembers about an album from the album list.
#[derive(Debug, Clone, Default)]
struct AlbumFacts {
    year: Option<i32>,
    genres: Vec<String>,
    thumb: Option<String>,
}

pub struct Plex {
    base: RwLock<String>,
    http: reqwest::blocking::Client,
    login: Mutex<Login>,
    /// The token the server takes.
    token: RwLock<String>,
    machine: String,
    info: ServerInfo,
    stream: StreamFormat,
    bitrate: u32,
    /// Plex Pass features asked for.
    extras: bool,
    /// The server's owner has Plex Pass.
    pass: bool,
    /// Music section id to its folders.
    sections: Vec<(String, Vec<String>)>,
    /// Artist name, lowercased, to its id and picture.
    artists: Mutex<HashMap<String, (String, Option<String>)>>,
    albums: Mutex<HashMap<String, AlbumFacts>>,
    /// Song id to its part's key, so a stream doesn't ask every time.
    parts: Mutex<HashMap<String, String>>,
    /// A login that changed and wants saving.
    fresh: Mutex<Option<String>>,
    /// This server only takes the timeline by GET.
    timeline_by_get: AtomicBool,
    /// The song the timeline reports on, and its playback session id.
    session: Mutex<(String, String)>,
}

/// A connection that answers, with its identity.
fn identity(http: &reqwest::blocking::Client, base: &str) -> Option<(String, String)> {
    let resp = http
        .get(format!("{base}/identity"))
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(5))
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&resp.bytes().ok()?).ok()?;
    let c = container(&v);
    Some((
        c.get("machineIdentifier")?.as_str()?.to_string(),
        c.get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ))
}

impl Plex {
    /// Connect to the server `machine` (empty: whatever answers at `url`),
    /// trying `url` first and then the addresses plex.tv knows for it.
    pub fn connect(
        url: &str,
        machine: &str,
        secret: &str,
        stream: StreamFormat,
        bitrate: u32,
        extras: bool,
    ) -> Result<Self> {
        let http = http::client()?;
        let mut login = Login::parse(secret);
        let mut changed = false;
        match plex_tv::refresh(&mut login, false) {
            Ok(c) => changed |= c,
            Err(RemoteError::Auth) => return Err(RemoteError::Auth),
            // plex.tv out of reach: the token may still have days left.
            Err(e) => log::info!("Plex sign-in refresh waits: {e}"),
        }
        let mut candidates = vec![url.trim_end_matches('/').to_string()];
        let mut server_token = login.server_token.clone();
        if !login.seed.is_empty() && !machine.is_empty() {
            match plex_tv::servers(&login) {
                Ok(list) => {
                    if let Some(r) = list.iter().find(|r| r.id == machine) {
                        if !r.token.is_empty() {
                            server_token = r.token.clone();
                        }
                        candidates.extend(r.ordered());
                    }
                }
                Err(RemoteError::Auth) => return Err(RemoteError::Auth),
                Err(e) => log::info!("plex.tv's server list: {e}"),
            }
        }
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|c| !c.is_empty() && seen.insert(c.clone()));
        let mut found = None;
        for c in &candidates {
            if let Some((id, version)) = identity(&http, c)
                && (machine.is_empty() || id == machine)
            {
                found = Some((c.clone(), id, version));
                break;
            }
        }
        let Some((base, id, version)) = found else {
            return Err(RemoteError::Http(format!(
                "no answer from the Plex server at {}",
                candidates.join(", ")
            )));
        };
        // The server's own token first, then the account's. Some servers
        // turn down one and take the other.
        let mut tokens = vec![server_token.clone(), login.token.clone()];
        tokens.dedup();
        tokens.retain(|t| !t.is_empty());
        let mut root = None;
        for t in &tokens {
            match get_json(&http, &base, &login.client, t, "/", &[], None) {
                Ok(v) => {
                    root = Some((t.clone(), v));
                    break;
                }
                Err(RemoteError::Auth) => continue,
                Err(e) => return Err(e),
            }
        }
        let Some((token, root)) = root else {
            return Err(RemoteError::Auth);
        };
        if token != login.server_token {
            login.server_token = token.clone();
            changed = true;
        }
        let c = container(&root);
        let pass = truthy(c.get("myPlexSubscription"));
        let version = c
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(version);
        let sections = music_sections(&http, &base, &login.client, &token)?;
        let mut product = format!(
            "Plex Media Server {}",
            version.split('-').next().unwrap_or("")
        );
        if pass {
            product.push_str(" (Plex Pass)");
        }
        let fresh = changed.then(|| login.to_secret());
        Ok(Self {
            info: ServerInfo {
                kind: ServerKind::Plex,
                product: product.trim().to_string(),
                open_subsonic: false,
                api_version: API_VERSION.into(),
                play_queue: false,
            },
            base: RwLock::new(base),
            http,
            login: Mutex::new(login),
            token: RwLock::new(token),
            machine: id,
            stream,
            bitrate,
            extras,
            pass,
            sections,
            artists: Mutex::new(HashMap::new()),
            albums: Mutex::new(HashMap::new()),
            parts: Mutex::new(HashMap::new()),
            fresh: Mutex::new(fresh),
            timeline_by_get: AtomicBool::new(false),
            session: Mutex::new((String::new(), String::new())),
        })
    }

    /// The server's machine identifier.
    pub fn machine(&self) -> &str {
        &self.machine
    }

    fn base(&self) -> String {
        self.base.read().unwrap().clone()
    }

    fn client_id(&self) -> String {
        self.login.lock().unwrap().client.clone()
    }

    /// The token went bad mid-session (a Plex JWT runs out after 7 days):
    /// refresh it, ask plex.tv for the server's token again, and keep
    /// whichever the server takes.
    fn reauth(&self) -> Result<()> {
        let mut login = self.login.lock().unwrap().clone();
        if !login.jwt {
            return Err(RemoteError::Auth);
        }
        plex_tv::refresh(&mut login, true)?;
        let mut tokens = vec![];
        if let Ok(list) = plex_tv::servers(&login)
            && let Some(r) = list.iter().find(|r| r.id == self.machine)
        {
            tokens.push(r.token.clone());
        }
        tokens.push(login.token.clone());
        let base = self.base();
        for t in tokens.into_iter().filter(|t| !t.is_empty()) {
            if get_json(&self.http, &base, &login.client, &t, "/", &[], None).is_ok() {
                login.server_token = t.clone();
                *self.token.write().unwrap() = t;
                *self.fresh.lock().unwrap() = Some(login.to_secret());
                *self.login.lock().unwrap() = login;
                return Ok(());
            }
        }
        Err(RemoteError::Auth)
    }

    fn call(
        &self,
        method: Method,
        path: &str,
        q: &[(&str, String)],
        page: Option<(usize, usize)>,
    ) -> Result<Value> {
        let once = || {
            let token = self.token.read().unwrap().clone();
            send(
                &self.http,
                method.clone(),
                &self.base(),
                &self.client_id(),
                &token,
                path,
                q,
                page,
            )
        };
        match once() {
            Err(RemoteError::Auth) => {
                self.reauth()?;
                once()
            }
            other => other,
        }
    }

    fn get(&self, path: &str, q: &[(&str, String)]) -> Result<Value> {
        self.call(Method::GET, path, q, None)
    }

    /// Every item under `path`, a page at a time.
    fn paged(&self, path: &str, q: &[(&str, String)]) -> Result<Vec<Item>> {
        let mut out = Vec::new();
        let mut start = 0;
        loop {
            let v = self.call(Method::GET, path, q, Some((start, PAGE)))?;
            let c = container(&v);
            let page = items(&v);
            let n = page.len();
            out.extend(page);
            let total = c
                .get("totalSize")
                .and_then(Value::as_u64)
                .map(|t| t as usize);
            start += n;
            let more = match total {
                Some(t) => start < t,
                None => n == PAGE,
            };
            if n == 0 || !more {
                break;
            }
        }
        Ok(out)
    }

    /// A URL with the token and client in its query, for FFmpeg and the
    /// cache, which can't send headers. The docs allow any X-Plex header
    /// as a query argument.
    fn url(&self, path: &str, extra: &[(&str, String)]) -> Result<String> {
        let mut u = url::Url::parse(&format!("{}{path}", self.base()))
            .map_err(|e| RemoteError::Parse(e.to_string()))?;
        {
            let mut q = u.query_pairs_mut();
            for (k, v) in extra {
                q.append_pair(k, v);
            }
            q.append_pair("X-Plex-Token", &self.token.read().unwrap());
            q.append_pair("X-Plex-Client-Identifier", &self.client_id());
            q.append_pair("X-Plex-Product", super::CLIENT);
        }
        Ok(u.into())
    }

    fn roots(&self, section: Option<&str>) -> Vec<String> {
        self.sections
            .iter()
            .filter(|(id, _)| section.is_none_or(|s| s == id))
            .flat_map(|(_, l)| l.clone())
            .collect()
    }

    fn song(&self, t: &Item) -> Option<RemoteSong> {
        let id = t.rating_key.clone()?;
        let media = t.media.first().cloned().unwrap_or_default();
        let part = media.part.first().cloned().unwrap_or_default();
        let audio = part
            .stream
            .iter()
            .find(|s| s.stream_type == Some(2.0))
            .cloned()
            .unwrap_or_default();
        if let Some(k) = &part.key {
            self.parts.lock().unwrap().insert(id.clone(), k.clone());
        }
        let suffix = part
            .file
            .as_deref()
            .and_then(|f| f.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase())
            .filter(|e| e.len() <= 5)
            .or_else(|| {
                part.container
                    .clone()
                    .or(media.container.clone())
                    .or(audio.codec.clone())
            })
            .map(|c| if c == "mp4" { "m4a".into() } else { c })
            .unwrap_or_default();
        let album_id = t.parent_rating_key.clone().unwrap_or_default();
        let facts = self
            .albums
            .lock()
            .unwrap()
            .get(&album_id)
            .cloned()
            .unwrap_or_default();
        let album_artist = t.grandparent_title.clone();
        // A track's own artist, when it differs from the album's, is its
        // original title in Plex's terms.
        let artist = t
            .original_title
            .clone()
            .filter(|a| !a.is_empty())
            .or(album_artist.clone());
        let loud = self.extras;
        Some(RemoteSong {
            title: t.title.clone().unwrap_or_else(|| id.clone()),
            artist,
            album_artist,
            album: t.parent_title.clone(),
            album_id,
            track_no: t.index.filter(|n| *n > 0.0).map(|n| n as u32),
            disc_no: t.parent_index.filter(|n| *n > 0.0).map(|n| n as u32),
            year: t
                .parent_year
                .or(t.year)
                .filter(|y| *y > 0.0)
                .map(|y| y as i32)
                .or(facts.year),
            genres: if t.genre.is_empty() {
                facts.genres
            } else {
                t.genre.iter().filter_map(|g| g.tag.clone()).collect()
            },
            duration: t
                .duration
                .filter(|d| *d > 0.0)
                .map(|d| Duration::from_millis(d as u64)),
            suffix,
            sample_rate: audio.sampling_rate.filter(|r| *r > 0.0).map(|r| r as u32),
            bits: audio.bit_depth.filter(|b| *b > 0.0).map(|b| b as u32),
            channels: media
                .audio_channels
                .or(audio.channels)
                .filter(|c| *c > 0.0)
                .map(|c| c as u16),
            kbps: media.bitrate.filter(|b| *b > 0.0).map(|b| b as u32),
            size: part.size.map(|s| s as i64),
            path: part.file.as_deref().and_then(|f| {
                relative(f, &self.roots(t.library_section_id.as_deref()))
                    .or_else(|| relative(f, &self.roots(None)))
            }),
            cover: t.parent_thumb.clone().or(t.thumb.clone()).or(facts.thumb),
            rg_track_gain: audio.gain.filter(|_| loud),
            rg_track_peak: audio.peak.filter(|_| loud && audio.gain.is_some()),
            rg_album_gain: audio.album_gain.filter(|_| loud),
            rg_album_peak: audio
                .album_peak
                .filter(|_| loud && audio.album_gain.is_some()),
            play_count: t.view_count.unwrap_or(0.0) as i64,
            rating: stars(t.user_rating),
            id,
        })
    }

    /// Full items, streams and all, for these ids.
    fn details(&self, ids: &[String]) -> Result<HashMap<String, Item>> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(IDS_PER_CALL) {
            let v = self.get(&format!("/library/metadata/{}", chunk.join(",")), &[])?;
            for it in items(&v) {
                if let Some(k) = it.rating_key.clone() {
                    out.insert(k, it);
                }
            }
        }
        Ok(out)
    }

    fn songs_of(&self, list: Vec<Item>, detail: bool) -> Result<Vec<RemoteSong>> {
        let detailed = if detail {
            let ids: Vec<String> = list.iter().filter_map(|t| t.rating_key.clone()).collect();
            self.details(&ids)?
        } else {
            HashMap::new()
        };
        Ok(list
            .iter()
            .filter_map(|t| {
                let full = t.rating_key.as_ref().and_then(|k| detailed.get(k));
                self.song(full.unwrap_or(t))
            })
            .collect())
    }

    /// The part to stream for a song.
    fn part_key(&self, id: &str) -> Result<String> {
        if let Some(k) = self.parts.lock().unwrap().get(id) {
            return Ok(k.clone());
        }
        let v = self.get(&format!("/library/metadata/{id}"), &[])?;
        let key = items(&v)
            .into_iter()
            .next()
            .and_then(|t| t.media.into_iter().next())
            .and_then(|m| m.part.into_iter().next())
            .and_then(|p| p.key)
            .ok_or_else(|| RemoteError::Parse(format!("Plex has no file for song {id}")))?;
        self.parts.lock().unwrap().insert(id.into(), key.clone());
        Ok(key)
    }

    /// "server://<machine>/com.plexapp.plugins.library/library/metadata/1,2"
    fn uri_for(&self, ids: &[String]) -> String {
        format!(
            "server://{}/{LIBRARY}/library/metadata/{}",
            self.machine,
            ids.join(",")
        )
    }

    fn add_to_playlist(&self, id: &str, songs: &[String]) -> Result<()> {
        for chunk in songs.chunks(PLAYLIST_CHUNK) {
            self.call(
                Method::PUT,
                &format!("/playlists/{id}/items"),
                &[("uri", self.uri_for(chunk))],
                None,
            )?;
        }
        Ok(())
    }

    fn artist_key(&self, name: &str) -> Option<(String, Option<String>)> {
        self.artists
            .lock()
            .unwrap()
            .get(&name.to_lowercase())
            .cloned()
    }

    fn extras_on(&self) -> bool {
        self.extras && self.pass
    }
}

fn music_sections(
    http: &reqwest::blocking::Client,
    base: &str,
    client: &str,
    token: &str,
) -> Result<Vec<(String, Vec<String>)>> {
    // `/library/sections/all` is the documented listing; older servers
    // answer the plain one.
    let v = match get_json(
        http,
        base,
        client,
        token,
        "/library/sections/all",
        &[],
        None,
    ) {
        Ok(v) => v,
        Err(RemoteError::Server { code: 404, .. }) => {
            get_json(http, base, client, token, "/library/sections", &[], None)?
        }
        Err(e) => return Err(e),
    };
    let dirs: Vec<Directory> = container(&v)
        .get("Directory")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| serde_json::from_value(x.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    Ok(dirs
        .into_iter()
        .filter(|d| d.kind.as_deref() == Some("artist"))
        .filter_map(|d| {
            Some((
                d.key?,
                d.location.into_iter().filter_map(|l| l.path).collect(),
            ))
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn send(
    http: &reqwest::blocking::Client,
    method: Method,
    base: &str,
    client: &str,
    token: &str,
    path: &str,
    q: &[(&str, String)],
    page: Option<(usize, usize)>,
) -> Result<Value> {
    let mut rb = http.request(method, format!("{base}{path}")).query(q);
    for (k, v) in plex_tv::headers(client) {
        rb = rb.header(k, v);
    }
    rb = rb
        .header("X-Plex-Token", token)
        .header("X-Plex-Pms-Api-Version", API_VERSION);
    if let Some((start, size)) = page {
        // The docs ask for both headers to get a page.
        rb = rb
            .header("X-Plex-Container-Start", start.to_string())
            .header("X-Plex-Container-Size", size.to_string());
    }
    let resp = rb.send().map_err(http::http_err)?;
    let status = resp.status();
    let body = resp.bytes().map_err(http::http_err)?;
    match status.as_u16() {
        401 | 403 => return Err(RemoteError::Auth),
        404 | 405 => {
            return Err(RemoteError::Server {
                code: status.as_u16() as i64,
                message: format!("{path}: not here"),
            });
        }
        s if !(200..300).contains(&s) => {
            return Err(RemoteError::Http(format!("HTTP {status}")));
        }
        _ => {}
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body).map_err(|e| {
        RemoteError::Parse(format!(
            "{path}: {e}: {}",
            String::from_utf8_lossy(&body[..body.len().min(80)])
        ))
    })
}

fn get_json(
    http: &reqwest::blocking::Client,
    base: &str,
    client: &str,
    token: &str,
    path: &str,
    q: &[(&str, String)],
    page: Option<(usize, usize)>,
) -> Result<Value> {
    send(http, Method::GET, base, client, token, path, q, page)
}

/// A fresh id for a playback or transcode session.
fn session_id() -> String {
    plex_tv::new_client_id()
}

impl Client for Plex {
    fn info(&self) -> &ServerInfo {
        &self.info
    }

    fn ping(&self) -> Result<()> {
        // A Plex JWT is refreshed a day before it runs out, while the
        // app's running, so a week-long session never drops.
        let due = {
            let l = self.login.lock().unwrap();
            l.due(chrono::Utc::now().timestamp())
        };
        if due {
            let mut l = self.login.lock().unwrap().clone();
            match plex_tv::refresh(&mut l, false) {
                Ok(true) => {
                    *self.fresh.lock().unwrap() = Some(l.to_secret());
                    *self.login.lock().unwrap() = l;
                }
                Ok(false) => {}
                Err(e) => log::info!("Plex sign-in refresh waits: {e}"),
            }
        }
        let resp = self
            .http
            .get(format!("{}/identity", self.base()))
            .header("Accept", "application/json")
            .send()
            .map_err(http::http_err)?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(RemoteError::Http(format!("HTTP {}", resp.status())))
        }
    }

    fn fresh_secret(&self) -> Option<String> {
        self.fresh.lock().unwrap().take()
    }

    fn albums(&self) -> Result<Vec<RemoteAlbum>> {
        let mut out = Vec::new();
        let mut facts = HashMap::new();
        let mut artists = HashMap::new();
        for (sid, _) in &self.sections {
            for a in self.paged(
                &format!("/library/sections/{sid}/all"),
                &[("type", "8".into())],
            )? {
                if let (Some(k), Some(name)) = (a.rating_key, a.title) {
                    artists.insert(name.to_lowercase(), (k, a.thumb));
                }
            }
            for a in self.paged(
                &format!("/library/sections/{sid}/all"),
                &[("type", "9".into())],
            )? {
                let Some(id) = a.rating_key.clone() else {
                    continue;
                };
                let year = a.year.filter(|y| *y > 0.0).map(|y| y as i32);
                facts.insert(
                    id.clone(),
                    AlbumFacts {
                        year,
                        genres: a.genre.iter().filter_map(|g| g.tag.clone()).collect(),
                        thumb: a.thumb.clone(),
                    },
                );
                out.push(RemoteAlbum {
                    name: a.title.unwrap_or_default(),
                    artist: a.parent_title.unwrap_or_default(),
                    song_count: a.leaf_count.unwrap_or(0.0) as u32,
                    duration_s: a.duration.map(|d| (d / 1000.0) as u64).unwrap_or(0),
                    year,
                    // Plex bumps this when anything about the album changes.
                    changed: a.updated_at.map(|u| (u as i64).to_string()),
                    rating: stars(a.user_rating),
                    id,
                });
            }
        }
        *self.albums.lock().unwrap() = facts;
        *self.artists.lock().unwrap() = artists;
        Ok(out)
    }

    fn album_songs(&self, album_id: &str) -> Result<Vec<RemoteSong>> {
        let list = self.paged(&format!("/library/metadata/{album_id}/children"), &[])?;
        self.songs_of(list, true)
    }

    fn stream_url_as(
        &self,
        song_id: &str,
        format: StreamFormat,
        kbps: u32,
    ) -> Result<(String, bool)> {
        if format == StreamFormat::Raw {
            let key = self.part_key(song_id)?;
            return Ok((self.url(&key, &[])?, true));
        }
        // The universal transcoder, as the docs describe it: HLS, with the
        // music bitrate to aim for. Plex makes AAC by default; MP3 is
        // asked for through the client profile.
        let session = session_id();
        let mut q = vec![
            ("path", format!("/library/metadata/{song_id}")),
            ("mediaIndex", "0".into()),
            ("partIndex", "0".into()),
            ("protocol", "hls".into()),
            ("directPlay", "0".into()),
            ("directStream", "0".into()),
            ("directStreamAudio", "0".into()),
            ("musicBitrate", kbps.clamp(32, 512).to_string()),
            ("transcodeSessionId", session.clone()),
            ("X-Plex-Session-Identifier", session),
            (
                "location",
                if is_local(&self.base()) { "lan" } else { "wan" }.into(),
            ),
        ];
        if format == StreamFormat::Mp3 {
            q.push((
                "X-Plex-Client-Profile-Extra",
                "add-transcode-target(type=musicProfile&context=streaming&protocol=hls&container=mpegts&audioCodec=mp3)"
                    .into(),
            ));
        }
        Ok((
            self.url("/music/:/transcode/universal/start.m3u8", &q)?,
            false,
        ))
    }

    fn stream_setting(&self) -> (StreamFormat, u32) {
        (self.stream, self.bitrate)
    }

    fn episode_url(&self, _stream_id: &str) -> Result<String> {
        Err(RemoteError::Unsupported("podcasts"))
    }

    fn cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        // Ids are the thumb paths Plex gives; older ids were album ids.
        let thumb = if cover_id.starts_with('/') {
            cover_id.to_string()
        } else {
            format!("/library/metadata/{cover_id}/thumb")
        };
        let size = size.clamp(32, 2000).to_string();
        let mut rb = self
            .http
            .get(format!("{}/photo/:/transcode", self.base()))
            .query(&[
                ("url", thumb.as_str()),
                ("width", size.as_str()),
                ("height", size.as_str()),
                ("minSize", "1"),
                ("upscale", "1"),
            ]);
        for (k, v) in plex_tv::headers(&self.client_id()) {
            rb = rb.header(k, v);
        }
        let resp = rb
            .header("X-Plex-Token", self.token.read().unwrap().as_str())
            .header("X-Plex-Pms-Api-Version", API_VERSION)
            .header("Accept", "image/*")
            .send()
            .map_err(http::http_err)?;
        if !resp.status().is_success() {
            return Err(RemoteError::Http(format!("HTTP {}", resp.status())));
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp.bytes().map_err(http::http_err)?.to_vec();
        if ctype.contains("json") || ctype.contains("xml") || body.is_empty() {
            return Err(RemoteError::Parse("no image".into()));
        }
        Ok(body)
    }

    fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        let list = self.paged("/playlists", &[("playlistType", "audio".into())])?;
        let now = chrono::Utc::now().timestamp();
        Ok(list
            .into_iter()
            .filter_map(|p| {
                let smart = truthy(p.smart.as_ref());
                Some(RemotePlaylist {
                    name: p.title.unwrap_or_default(),
                    song_count: p.leaf_count.unwrap_or(0.0) as u32,
                    // A smart playlist's songs are Plex's rules at work.
                    readonly: smart,
                    // Its songs change without it changing, so it's read
                    // again every half hour or so.
                    changed: if smart {
                        Some(format!("smart:{}", now / 1800))
                    } else {
                        p.updated_at.map(|u| (u as i64).to_string())
                    },
                    id: p.rating_key?,
                })
            })
            .collect())
    }

    fn playlist_songs(&self, id: &str) -> Result<Vec<RemoteSong>> {
        let list = self.paged(&format!("/playlists/{id}/items"), &[])?;
        Ok(list.iter().filter_map(|t| self.song(t)).collect())
    }

    fn create_playlist(&self, name: &str, songs: &[String]) -> Result<String> {
        let (first, rest) = songs.split_at(songs.len().min(PLAYLIST_CHUNK));
        let mut q = vec![
            ("type", "audio".to_string()),
            ("title", name.to_string()),
            ("smart", "0".into()),
        ];
        if !first.is_empty() {
            q.push(("uri", self.uri_for(first)));
        }
        let v = self.call(Method::POST, "/playlists", &q, None)?;
        let id = items(&v)
            .into_iter()
            .find_map(|p| p.rating_key)
            .ok_or_else(|| RemoteError::Parse("Plex made no playlist".into()))?;
        self.add_to_playlist(&id, rest)?;
        Ok(id)
    }

    fn replace_playlist(&self, id: &str, songs: &[String]) -> Result<()> {
        self.call(Method::DELETE, &format!("/playlists/{id}/items"), &[], None)?;
        self.add_to_playlist(id, songs)
    }

    fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.call(
            Method::PUT,
            &format!("/playlists/{id}"),
            &[("title", name.to_string())],
            None,
        )
        .map(|_| ())
    }

    fn delete_playlist(&self, id: &str) -> Result<()> {
        self.call(Method::DELETE, &format!("/playlists/{id}"), &[], None)
            .map(|_| ())
    }

    fn radio(&self) -> Result<Vec<RemoteStation>> {
        Ok(Vec::new())
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

    fn bookmark(&self, _id: &str, _pos: Duration) -> Result<()> {
        Err(RemoteError::Unsupported("podcasts"))
    }

    fn similar_songs(&self, song_id: &str, count: usize) -> Result<Vec<String>> {
        if !self.extras_on() {
            return Err(RemoteError::Unsupported("sonic analysis without Plex Pass"));
        }
        let v = self.get(
            &format!("/library/metadata/{song_id}/nearest"),
            &[("limit", count.to_string())],
        )?;
        Ok(items(&v)
            .into_iter()
            .filter_map(|t| t.rating_key)
            .filter(|k| k != song_id)
            .collect())
    }

    fn similar_artists(&self, artist: &str, count: usize) -> Result<Vec<String>> {
        let Some((key, _)) = self.artist_key(artist) else {
            return Ok(Vec::new());
        };
        let v = self.get(
            &format!("/library/metadata/{key}/similar"),
            &[("count", count.to_string())],
        )?;
        Ok(items(&v).into_iter().filter_map(|a| a.title).collect())
    }

    fn artist_image(&self, name: &str, size: u32) -> Result<Option<Vec<u8>>> {
        match self.artist_key(name) {
            Some((_, Some(thumb))) => Ok(self.cover(&thumb, size).ok()),
            _ => Ok(None),
        }
    }

    fn report_play(&self, _id: &str, _at: i64, _submission: bool) -> Result<()> {
        // Plays count through the timeline, the way Plex's own players do,
        // so this adds nothing (and a second count would be wrong).
        Ok(())
    }

    fn timeline(
        &self,
        id: &str,
        state: Timeline,
        pos: Duration,
        duration: Option<Duration>,
        continuing: bool,
    ) -> Result<()> {
        let session = {
            let mut s = self.session.lock().unwrap();
            if s.0 != id {
                *s = (id.to_string(), session_id());
            }
            s.1.clone()
        };
        let mut q = vec![
            ("ratingKey", id.to_string()),
            ("key", format!("/library/metadata/{id}")),
            ("state", state.as_str().to_string()),
            ("time", pos.as_millis().to_string()),
            ("X-Plex-Session-Identifier", session),
        ];
        if let Some(d) = duration {
            q.push(("duration", d.as_millis().to_string()));
        }
        if state == Timeline::Stopped {
            q.push(("continuing", u8::from(continuing).to_string()));
        }
        // The docs give POST. A server that only knows the GET older
        // players use gets that from then on.
        if !self.timeline_by_get.load(Ordering::Relaxed) {
            match self.call(Method::POST, "/:/timeline", &q, None) {
                Err(RemoteError::Server {
                    code: 404 | 405, ..
                }) => {
                    self.timeline_by_get.store(true, Ordering::Relaxed);
                }
                other => return other.map(|_| ()),
            }
        }
        self.call(Method::GET, "/:/timeline", &q, None).map(|_| ())
    }

    fn set_rating(&self, _what: super::Rated, id: &str, stars: u8) -> Result<()> {
        self.call(
            Method::PUT,
            "/:/rate",
            &[
                ("key", id.to_string()),
                ("identifier", LIBRARY.to_string()),
                ("rating", (u32::from(stars.min(5)) * 2).to_string()),
            ],
            None,
        )
        .map(|_| ())
    }

    fn lyrics(&self, song_id: &str, _artist: &str, _title: &str) -> Result<Option<ServerLyrics>> {
        if !self.extras_on() {
            return Err(RemoteError::Unsupported("lyrics without Plex Pass"));
        }
        let v = self.get(&format!("/library/metadata/{song_id}"), &[])?;
        let Some(track) = items(&v).into_iter().next() else {
            return Ok(None);
        };
        // Stream type 4 is lyrics. LRC is timed; plain text isn't.
        let streams: Vec<Stream> = track
            .media
            .iter()
            .flat_map(|m| m.part.iter())
            .flat_map(|p| p.stream.iter().cloned())
            .filter(|s| s.stream_type == Some(4.0) && s.key.is_some())
            .collect();
        let Some(s) = streams
            .iter()
            .find(|s| s.format.as_deref() == Some("lrc"))
            .or(streams.first())
        else {
            return Ok(None);
        };
        let url = self.url(s.key.as_deref().unwrap_or_default(), &[])?;
        let resp = self.http.get(url).send().map_err(http::http_err)?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let text = resp.text().map_err(http::http_err)?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        // Handed over as text: the lyrics reader picks out LRC times.
        Ok(Some(ServerLyrics {
            synced: false,
            lines: text.lines().map(|l| (None, l.to_string())).collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses() {
        assert!(is_local("http://192.168.1.5:32400"));
        assert!(is_local("https://192-168-1-5.abcdef.plex.direct:32400"));
        assert!(is_local("http://nas.local:32400"));
        assert!(!is_local("https://84-1-2-3.abcdef.plex.direct:32400"));
        assert!(!is_local("https://plex.example.com"));
    }

    #[test]
    fn paths_inside_the_library_folder() {
        let roots = vec!["/data/music".to_string(), "/mnt/more/".to_string()];
        assert_eq!(
            relative("/data/music/Nirvana/Bleach/01.flac", &roots).as_deref(),
            Some("Nirvana/Bleach/01.flac")
        );
        assert_eq!(
            relative("/mnt/more/a.mp3", &roots).as_deref(),
            Some("a.mp3")
        );
        assert_eq!(relative("/data/musicals/x.flac", &roots), None);
    }

    #[test]
    fn ratings_are_out_of_ten() {
        assert_eq!(stars(Some(10.0)), Some(5));
        assert_eq!(stars(Some(7.0)), Some(4));
        assert_eq!(stars(Some(2.0)), Some(1));
        assert_eq!(stars(Some(0.0)), None);
        assert_eq!(stars(None), None);
    }

    #[test]
    fn truthy_values() {
        assert!(truthy(Some(&Value::Bool(true))));
        assert!(truthy(Some(&serde_json::json!(1))));
        assert!(truthy(Some(&serde_json::json!("1"))));
        assert!(!truthy(Some(&serde_json::json!("0"))));
        assert!(!truthy(None));
    }
}
