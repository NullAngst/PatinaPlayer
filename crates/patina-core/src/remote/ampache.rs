//! Ampache's own JSON API (version 6). Ampache speaks Subsonic as well,
//! but its own API is the complete one: smart lists, ratings, catalogs.
//!
//! Login is a handshake that trades the password (or an API key) for a
//! session token. Sessions run out, so a call that's told the session is
//! gone logs in again and tries once more. The password is kept in memory
//! for that; it's never written anywhere by this module.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::http::{self, id, list, num, opt_id};
use super::{
    Client, RemoteAlbum, RemoteEpisode, RemoteError, RemotePlaylist, RemotePodcast, RemoteSong,
    RemoteStation, Result, SavedQueue, ServerInfo, ServerKind, StreamFormat,
};

const VERSION: &str = "6.0.0";
const PAGE: usize = 500;
/// Log in again this long before the session would end.
const RENEW: Duration = Duration::from_secs(300);

struct Session {
    token: String,
    /// When to log in again, by our clock.
    renew_at: Instant,
}

pub struct Ampache {
    base: String,
    user: String,
    secret: String,
    http: reqwest::blocking::Client,
    session: Mutex<Session>,
    info: ServerInfo,
    stream: StreamFormat,
    bitrate: u32,
    /// Catalog id to its folder on the server, for relative song paths.
    catalogs: Mutex<Option<Vec<(String, String)>>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Handshake {
    auth: Option<String>,
    api: Option<String>,
    session_expire: Option<String>,
    server: Option<String>,
    version: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    #[serde(deserialize_with = "opt_id")]
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Album {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    artist: Option<Named>,
    #[serde(deserialize_with = "num")]
    time: Option<f64>,
    #[serde(deserialize_with = "num")]
    year: Option<f64>,
    #[serde(deserialize_with = "num")]
    songcount: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Song {
    #[serde(deserialize_with = "id")]
    id: String,
    title: Option<String>,
    name: Option<String>,
    artist: Option<Named>,
    album: Option<Named>,
    albumartist: Option<Named>,
    #[serde(deserialize_with = "num")]
    disk: Option<f64>,
    #[serde(deserialize_with = "num")]
    track: Option<f64>,
    filename: Option<String>,
    #[serde(deserialize_with = "list")]
    genre: Vec<Named>,
    #[serde(deserialize_with = "num")]
    time: Option<f64>,
    #[serde(deserialize_with = "num")]
    year: Option<f64>,
    #[serde(deserialize_with = "num")]
    bitrate: Option<f64>,
    #[serde(deserialize_with = "num")]
    rate: Option<f64>,
    #[serde(deserialize_with = "num")]
    channels: Option<f64>,
    mime: Option<String>,
    #[serde(deserialize_with = "num")]
    size: Option<f64>,
    #[serde(deserialize_with = "num")]
    playcount: Option<f64>,
    #[serde(deserialize_with = "opt_id")]
    catalog: Option<String>,
    #[serde(deserialize_with = "num")]
    replaygain_track_gain: Option<f64>,
    #[serde(deserialize_with = "num")]
    replaygain_track_peak: Option<f64>,
    #[serde(deserialize_with = "num")]
    replaygain_album_gain: Option<f64>,
    #[serde(deserialize_with = "num")]
    replaygain_album_peak: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Playlist {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    owner: Option<String>,
    #[serde(deserialize_with = "num")]
    items: Option<f64>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LiveStream {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Episode {
    #[serde(deserialize_with = "id")]
    id: String,
    title: Option<String>,
    name: Option<String>,
    description: Option<String>,
    pubdate: Option<String>,
    state: Option<String>,
    filename: Option<String>,
    #[serde(deserialize_with = "num")]
    time: Option<f64>,
    #[serde(deserialize_with = "num")]
    size: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Podcast {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    description: Option<String>,
    feed_url: Option<String>,
    #[serde(deserialize_with = "list")]
    podcast_episode: Vec<Episode>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Catalog {
    #[serde(deserialize_with = "id")]
    id: String,
    path: Option<String>,
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// An Ampache error, if `v` is one.
fn error_of(v: &Value) -> Option<(i64, String)> {
    let e = v.get("error")?;
    let code = match e.get("errorCode").or_else(|| e.get("code"))? {
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Number(n) => n.as_i64().unwrap_or(0),
        _ => 0,
    };
    let msg = e
        .get("errorMessage")
        .or_else(|| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    Some((code, msg))
}

/// Seconds until an ISO date, from the server's clock to ours.
fn until(expire: Option<&str>) -> Duration {
    let secs = expire
        .and_then(http::date_secs)
        .map(|t| t - chrono::Utc::now().timestamp())
        .unwrap_or(3600);
    Duration::from_secs(secs.clamp(60, 7 * 24 * 3600) as u64)
}

impl Ampache {
    pub fn connect(
        base: &str,
        user: &str,
        secret: &str,
        stream: StreamFormat,
        bitrate: u32,
    ) -> Result<Self> {
        let http = http::client()?;
        let base = base.trim_end_matches('/').to_string();
        let (token, renew_at, hs) = handshake(&http, &base, user, secret)?;
        let product = hs
            .server
            .clone()
            .or(hs.api.clone())
            .or(hs.version.clone())
            .map(|v| format!("Ampache {v}"))
            .unwrap_or_else(|| "Ampache".into());
        Ok(Self {
            info: ServerInfo {
                kind: ServerKind::Ampache,
                product,
                open_subsonic: false,
                api_version: hs.api.clone().unwrap_or_default(),
                play_queue: false,
            },
            base,
            user: user.into(),
            secret: secret.into(),
            http,
            session: Mutex::new(Session { token, renew_at }),
            stream,
            bitrate,
            catalogs: Mutex::new(None),
        })
    }

    /// Does `base` look like an Ampache server at all? Ping answers
    /// without a login.
    pub fn is_there(base: &str) -> bool {
        let Ok(c) = http::client() else { return false };
        let url = format!("{}/server/json.server.php", base.trim_end_matches('/'));
        http::get_bytes(&c, &url, &[("action", "ping".into())])
            .ok()
            .and_then(|(b, _)| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| {
                v.get("server").is_some() || v.get("version").is_some() || v.get("error").is_some()
            })
    }

    fn endpoint(&self) -> String {
        format!("{}/server/json.server.php", self.base)
    }

    fn token(&self) -> Result<String> {
        let mut s = self.session.lock().unwrap();
        if Instant::now() >= s.renew_at {
            let (token, renew_at, _) = handshake(&self.http, &self.base, &self.user, &self.secret)?;
            s.token = token;
            s.renew_at = renew_at;
        }
        Ok(s.token.clone())
    }

    fn relogin(&self) -> Result<()> {
        let (token, renew_at, _) = handshake(&self.http, &self.base, &self.user, &self.secret)?;
        let mut s = self.session.lock().unwrap();
        s.token = token;
        s.renew_at = renew_at;
        Ok(())
    }

    fn get_once(&self, action: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut q: Vec<(&str, String)> = vec![
            ("action", action.to_string()),
            ("auth", self.token()?),
            ("version", VERSION.into()),
        ];
        q.extend(params.iter().cloned());
        let (body, _) = http::get_bytes(&self.http, &self.endpoint(), &q)?;
        serde_json::from_slice(&body).map_err(|e| {
            RemoteError::Parse(format!(
                "{action}: {e}: {}",
                String::from_utf8_lossy(&body[..body.len().min(80)])
            ))
        })
    }

    /// One call, logging in again once if the session ran out.
    fn get(&self, action: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut v = self.get_once(action, params)?;
        if let Some((4701, _)) = error_of(&v) {
            self.relogin()?;
            v = self.get_once(action, params)?;
        }
        match error_of(&v) {
            None => Ok(v),
            // "Not found" on a list just means an empty one.
            Some((4704, _)) => Ok(Value::Null),
            Some((4701 | 4703 | 4742, _)) => Err(RemoteError::Auth),
            Some((code, message)) => Err(RemoteError::Server { code, message }),
        }
    }

    /// The items under `key`: `{"song": [...]}`. Missing means none.
    fn items<T: DeserializeOwned>(&self, v: Value, key: &str) -> Result<Vec<T>> {
        let inner = match v {
            Value::Object(mut m) => m.remove(key).unwrap_or(Value::Null),
            // A bare list, as some versions answer.
            other @ Value::Array(_) => other,
            _ => Value::Null,
        };
        let one = |v: Value| {
            serde_json::from_value::<T>(v).map_err(|e| RemoteError::Parse(format!("{key}: {e}")))
        };
        match inner {
            Value::Array(a) => a.into_iter().map(one).collect(),
            Value::Null => Ok(Vec::new()),
            obj => Ok(vec![one(obj)?]),
        }
    }

    fn paged<T: DeserializeOwned>(
        &self,
        action: &str,
        key: &str,
        params: &[(&str, String)],
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let mut q = params.to_vec();
            q.push(("offset", offset.to_string()));
            q.push(("limit", PAGE.to_string()));
            let page: Vec<T> = self.items(self.get(action, &q)?, key)?;
            let n = page.len();
            out.extend(page);
            if n < PAGE {
                break;
            }
            offset += n;
        }
        Ok(out)
    }

    fn catalog_dirs(&self) -> Vec<(String, String)> {
        let mut c = self.catalogs.lock().unwrap();
        if c.is_none() {
            let list: Vec<Catalog> = self
                .get("catalogs", &[])
                .and_then(|v| self.items(v, "catalog"))
                .unwrap_or_default();
            *c = Some(
                list.into_iter()
                    .filter_map(|c| Some((c.id, c.path.filter(|p| !p.is_empty())?)))
                    .collect(),
            );
        }
        c.clone().unwrap_or_default()
    }

    fn song(&self, s: Song, catalogs: &[(String, String)]) -> RemoteSong {
        let suffix = s
            .filename
            .as_deref()
            .and_then(|f| f.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase())
            .or_else(|| {
                s.mime
                    .as_deref()
                    .and_then(|m| m.rsplit('/').next())
                    .map(|m| m.trim_start_matches("x-").to_string())
            })
            .unwrap_or_default();
        // The file's place inside its catalog, which is what the folder
        // view shows.
        let path = s.filename.as_deref().and_then(|f| {
            let (_, dir) = catalogs
                .iter()
                .find(|(id, _)| Some(id) == s.catalog.as_ref())?;
            f.strip_prefix(dir.as_str())
                .map(|r| r.trim_start_matches('/').to_string())
        });
        let album = s.album.unwrap_or_default();
        RemoteSong {
            title: s.title.or(s.name).unwrap_or_else(|| s.id.clone()),
            artist: s.artist.and_then(|a| a.name),
            album_artist: s.albumartist.and_then(|a| a.name),
            album: album.name,
            album_id: album.id.clone().unwrap_or_default(),
            cover: album.id.map(|id| format!("album:{id}")),
            track_no: s.track.filter(|t| *t > 0.0).map(|t| t as u32),
            disc_no: s.disk.filter(|d| *d > 0.0).map(|d| d as u32),
            year: s.year.filter(|y| *y > 0.0).map(|y| y as i32),
            genres: s.genre.into_iter().filter_map(|g| g.name).collect(),
            duration: s.time.filter(|t| *t > 0.0).map(Duration::from_secs_f64),
            suffix,
            sample_rate: s.rate.filter(|r| *r > 0.0).map(|r| r as u32),
            bits: None,
            channels: s.channels.filter(|c| *c > 0.0).map(|c| c as u16),
            // Ampache reports bits per second.
            kbps: s.bitrate.filter(|b| *b > 0.0).map(|b| {
                if b > 10_000.0 {
                    (b / 1000.0) as u32
                } else {
                    b as u32
                }
            }),
            size: s.size.map(|n| n as i64),
            path,
            rg_track_gain: s.replaygain_track_gain,
            rg_track_peak: s.replaygain_track_peak,
            rg_album_gain: s.replaygain_album_gain,
            rg_album_peak: s.replaygain_album_peak,
            play_count: s.playcount.unwrap_or(0.0) as i64,
            id: s.id,
        }
    }

    fn signed(&self, params: &[(&str, String)]) -> Result<String> {
        let mut u =
            url::Url::parse(&self.endpoint()).map_err(|e| RemoteError::Parse(e.to_string()))?;
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("auth", &self.token()?);
            q.append_pair("version", VERSION);
            for (k, v) in params {
                q.append_pair(k, v);
            }
        }
        Ok(u.into())
    }

    /// The single object some actions return, wrapped or not.
    fn object(v: Value, key: &str) -> Value {
        match v {
            Value::Object(mut m) => match m.remove(key) {
                Some(Value::Array(mut a)) if !a.is_empty() => a.remove(0),
                Some(o @ Value::Object(_)) => o,
                _ => Value::Object(m),
            },
            other => other,
        }
    }
}

fn handshake(
    http: &reqwest::blocking::Client,
    base: &str,
    user: &str,
    secret: &str,
) -> Result<(String, Instant, Handshake)> {
    let url = format!("{base}/server/json.server.php");
    let mut q: Vec<(&str, String)> =
        vec![("action", "handshake".into()), ("version", VERSION.into())];
    if user.is_empty() {
        // An API key on its own.
        q.push(("auth", secret.into()));
    } else {
        let ts = chrono::Utc::now().timestamp().to_string();
        q.push(("auth", sha256_hex(&format!("{ts}{}", sha256_hex(secret)))));
        q.push(("timestamp", ts));
        q.push(("user", user.into()));
    }
    let (body, _) = http::get_bytes(http, &url, &q)?;
    let v: Value = serde_json::from_slice(&body).map_err(|_| {
        RemoteError::Parse(format!(
            "not an Ampache answer: {}",
            String::from_utf8_lossy(&body[..body.len().min(80)])
        ))
    })?;
    if let Some((code, message)) = error_of(&v) {
        return Err(match code {
            4701 | 4703 | 4742 => RemoteError::Auth,
            _ => RemoteError::Server { code, message },
        });
    }
    let hs: Handshake =
        serde_json::from_value(v).map_err(|e| RemoteError::Parse(format!("handshake: {e}")))?;
    let token = hs
        .auth
        .clone()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| RemoteError::Parse("handshake without a session".into()))?;
    let life = until(hs.session_expire.as_deref());
    Ok((token, Instant::now() + life.saturating_sub(RENEW), hs))
}

impl Client for Ampache {
    fn info(&self) -> &ServerInfo {
        &self.info
    }

    fn albums(&self) -> Result<Vec<RemoteAlbum>> {
        let list: Vec<Album> = self.paged("albums", "album", &[])?;
        Ok(list
            .into_iter()
            .map(|a| RemoteAlbum {
                name: a.name.unwrap_or_default(),
                artist: a.artist.and_then(|x| x.name).unwrap_or_default(),
                song_count: a.songcount.unwrap_or(0.0) as u32,
                duration_s: a.time.unwrap_or(0.0) as u64,
                year: a.year.filter(|y| *y > 0.0).map(|y| y as i32),
                changed: None,
                id: a.id,
            })
            .collect())
    }

    fn album_songs(&self, album_id: &str) -> Result<Vec<RemoteSong>> {
        let cats = self.catalog_dirs();
        let list: Vec<Song> = self.paged("album_songs", "song", &[("filter", album_id.into())])?;
        Ok(list.into_iter().map(|s| self.song(s, &cats)).collect())
    }

    fn stream_url_as(
        &self,
        song_id: &str,
        format: StreamFormat,
        kbps: u32,
    ) -> Result<(String, bool)> {
        let mut q = vec![
            ("action", "stream".to_string()),
            ("type", "song".into()),
            ("id", song_id.into()),
        ];
        match format {
            StreamFormat::Raw => q.push(("format", "raw".into())),
            f => {
                q.push(("format", f.as_str().into()));
                q.push(("bitrate", (kbps.max(32) * 1000).to_string()));
            }
        }
        Ok((self.signed(&q)?, format == StreamFormat::Raw))
    }

    fn stream_setting(&self) -> (StreamFormat, u32) {
        (self.stream, self.bitrate)
    }

    fn ping(&self) -> Result<()> {
        // Ping with a session also keeps the session alive.
        self.get("ping", &[]).map(|_| ())
    }

    fn similar_songs(&self, song_id: &str, count: usize) -> Result<Vec<String>> {
        let v = self.get(
            "get_similar",
            &[
                ("type", "song".into()),
                ("filter", song_id.into()),
                ("limit", count.to_string()),
            ],
        )?;
        let songs: Vec<super::Named> = self.items(v, "song")?;
        Ok(songs.into_iter().filter_map(|n| n.id).collect())
    }

    fn similar_artists(&self, artist: &str, count: usize) -> Result<Vec<String>> {
        let v = self.get(
            "artists",
            &[("filter", artist.into()), ("exact", "1".into())],
        )?;
        let hits: Vec<super::Named> = self.items(v, "artist")?;
        let Some(id) = hits
            .into_iter()
            .find(|a| a.name.eq_ignore_ascii_case(artist))
            .and_then(|a| a.id)
        else {
            return Ok(Vec::new());
        };
        let v = self.get(
            "get_similar",
            &[
                ("type", "artist".into()),
                ("filter", id),
                ("limit", count.to_string()),
            ],
        )?;
        let similar: Vec<super::Named> = self.items(v, "artist")?;
        Ok(similar.into_iter().map(|a| a.name).collect())
    }

    fn episode_url(&self, stream_id: &str) -> Result<String> {
        self.signed(&[
            ("action", "stream".into()),
            ("type", "podcast_episode".into()),
            ("id", stream_id.into()),
        ])
    }

    fn cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        let (kind, id) = cover_id.split_once(':').unwrap_or(("album", cover_id));
        let q = vec![
            ("action", "get_art".to_string()),
            ("auth", self.token()?),
            ("version", VERSION.into()),
            ("type", kind.into()),
            ("id", id.into()),
            ("size", format!("{size}x{size}")),
        ];
        let (body, ctype) = http::get_bytes(&self.http, &self.endpoint(), &q)?;
        if ctype.contains("json") {
            return Err(RemoteError::Parse("no image".into()));
        }
        Ok(body)
    }

    fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        let list: Vec<Playlist> = self.paged("playlists", "playlist", &[])?;
        Ok(list
            .into_iter()
            .map(|p| RemotePlaylist {
                // Smart lists are rules, not songs; someone else's list is
                // theirs to change.
                readonly: p.id.starts_with("smart_")
                    || p.kind.as_deref() == Some("search")
                    || p.owner
                        .as_deref()
                        .is_some_and(|o| !self.user.is_empty() && o != self.user),
                name: p.name.unwrap_or_default(),
                song_count: p.items.unwrap_or(0.0) as u32,
                changed: None,
                id: p.id,
            })
            .collect())
    }

    fn playlist_songs(&self, id: &str) -> Result<Vec<RemoteSong>> {
        let cats = self.catalog_dirs();
        let list: Vec<Song> = self.paged("playlist_songs", "song", &[("filter", id.into())])?;
        Ok(list.into_iter().map(|s| self.song(s, &cats)).collect())
    }

    fn create_playlist(&self, name: &str, songs: &[String]) -> Result<String> {
        let v = self.get(
            "playlist_create",
            &[("name", name.into()), ("type", "private".into())],
        )?;
        let p: Playlist = serde_json::from_value(Self::object(v, "playlist"))
            .map_err(|e| RemoteError::Parse(format!("playlist_create: {e}")))?;
        for s in songs {
            self.get(
                "playlist_add_song",
                &[("filter", p.id.clone()), ("song", s.clone())],
            )?;
        }
        Ok(p.id)
    }

    fn replace_playlist(&self, id: &str, songs: &[String]) -> Result<()> {
        self.get(
            "playlist_remove_song",
            &[("filter", id.into()), ("clear", "1".into())],
        )?;
        for s in songs {
            self.get(
                "playlist_add_song",
                &[("filter", id.into()), ("song", s.clone())],
            )?;
        }
        Ok(())
    }

    fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.get(
            "playlist_edit",
            &[("filter", id.into()), ("name", name.into())],
        )
        .map(|_| ())
    }

    fn delete_playlist(&self, id: &str) -> Result<()> {
        self.get("playlist_delete", &[("filter", id.into())])
            .map(|_| ())
    }

    fn radio(&self) -> Result<Vec<RemoteStation>> {
        let list: Vec<LiveStream> = self.items(self.get("live_streams", &[])?, "live_stream")?;
        Ok(list
            .into_iter()
            .filter_map(|s| {
                Some(RemoteStation {
                    url: s.url.filter(|u| !u.is_empty())?,
                    name: s.name.unwrap_or_else(|| s.id.clone()),
                    id: s.id,
                })
            })
            .collect())
    }

    fn podcasts(&self) -> Result<Vec<RemotePodcast>> {
        let list: Vec<Podcast> = self.items(
            self.get("podcasts", &[("include", "episodes".into())])?,
            "podcast",
        )?;
        Ok(list
            .into_iter()
            .map(|p| RemotePodcast {
                title: p.name.unwrap_or_else(|| p.id.clone()),
                id: p.id,
                author: None,
                description: p.description,
                feed_url: p.feed_url,
                episodes: p
                    .podcast_episode
                    .into_iter()
                    .map(|e| {
                        let on_server = e.state.as_deref() == Some("completed")
                            || e.filename.as_deref().is_some_and(|f| !f.is_empty());
                        RemoteEpisode {
                            stream_id: on_server.then(|| e.id.clone()),
                            title: e.title.or(e.name).unwrap_or_else(|| e.id.clone()),
                            published: e.pubdate.as_deref().and_then(http::date_secs),
                            duration: e.time.filter(|t| *t > 0.0).map(Duration::from_secs_f64),
                            description: e.description,
                            size: e.size.map(|s| s as i64),
                            id: e.id,
                        }
                    })
                    .collect(),
            })
            .collect())
    }

    fn save_queue(&self, _: &[String], _: Option<&str>, _: Duration) -> Result<()> {
        Err(RemoteError::Unsupported("a saved play queue"))
    }

    fn load_queue(&self) -> Result<Option<SavedQueue>> {
        Err(RemoteError::Unsupported("a saved play queue"))
    }

    fn bookmark(&self, id: &str, pos: Duration) -> Result<()> {
        self.get(
            "bookmark_create",
            &[
                ("filter", id.into()),
                ("type", "podcast_episode".into()),
                ("position", pos.as_secs().to_string()),
                ("client", super::CLIENT.into()),
            ],
        )
        .map(|_| ())
    }

    fn report_play(&self, id: &str, at: i64, submission: bool) -> Result<()> {
        // Ampache has no "now playing" call; only finished plays count.
        if !submission {
            return Ok(());
        }
        self.get(
            "record_play",
            &[
                ("id", id.into()),
                ("client", super::CLIENT.into()),
                ("date", at.to_string()),
            ],
        )
        .map(|_| ())
    }

    fn star(&self, id: &str, on: bool) -> Result<()> {
        self.get(
            "flag",
            &[
                ("type", "song".into()),
                ("id", id.into()),
                ("flag", if on { "1" } else { "0" }.into()),
            ],
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_parse_both_ways() {
        let v: Value = serde_json::from_str(
            r#"{"error":{"errorCode":"4701","errorAction":"handshake","errorType":"account",
            "errorMessage":"Received Invalid Handshake - Incorrect username or password"}}"#,
        )
        .unwrap();
        assert_eq!(error_of(&v).unwrap().0, 4701);
        let v: Value =
            serde_json::from_str(r#"{"error":{"code":"4704","message":"Not Found"}}"#).unwrap();
        assert_eq!(error_of(&v).unwrap().0, 4704);
        assert!(error_of(&serde_json::json!({"song": []})).is_none());
    }

    #[test]
    fn the_handshake_passphrase() {
        // Ampache's documented formula: sha256(time + sha256(password)).
        let p = sha256_hex(&format!("1700000000{}", sha256_hex("secret")));
        assert_eq!(p.len(), 64);
        assert_ne!(p, sha256_hex("secret"));
    }
}
