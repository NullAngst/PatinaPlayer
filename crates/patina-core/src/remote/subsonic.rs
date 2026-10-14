//! The Subsonic API (1.16.1) with the OpenSubsonic extensions, which is
//! what Navidrome, Gonic, Airsonic, LMS and most other servers speak.
//!
//! Login is a salted MD5 token, or an OpenSubsonic API key where the
//! server offers that. The plain password is never put in a URL.
//!
//! Servers disagree on small things, and this client takes all of them:
//! ids as strings or numbers, a list sent as a lone object, a field left
//! out instead of empty, `.view` or not. Gonic's ids carry a kind prefix
//! ("al-12", "tr-345"); they're opaque strings here, so that's fine.

use std::time::Duration;

use md5::{Digest, Md5};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::http::{self, id, list, num, opt_id};
use super::{
    Client, RemoteAlbum, RemoteEpisode, RemoteError, RemotePlaylist, RemotePodcast, RemoteSong,
    RemoteStation, Result, SavedQueue, ServerInfo, ServerKind, StreamFormat,
};

const API: &str = "1.16.1";
/// Most servers cap list pages at 500.
const PAGE: usize = 500;
/// Song ids per request when a server can't take a POST.
const CHUNK: usize = 150;

enum Auth {
    Token { password: String },
    ApiKey(String),
}

pub struct Subsonic {
    base: String,
    user: String,
    auth: Auth,
    http: reqwest::blocking::Client,
    info: ServerInfo,
    stream: StreamFormat,
    bitrate: u32,
    /// OpenSubsonic formPost: long id lists go in a POST body.
    form_post: bool,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    r: Resp,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resp {
    status: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(rename = "type", default)]
    server_type: Option<String>,
    #[serde(default)]
    server_version: Option<String>,
    #[serde(default)]
    open_subsonic: bool,
    #[serde(default)]
    error: Option<ErrBody>,
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct ErrBody {
    #[serde(default, deserialize_with = "num")]
    code: Option<f64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct NameObj {
    name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Rg {
    #[serde(deserialize_with = "num")]
    track_gain: Option<f64>,
    #[serde(deserialize_with = "num")]
    album_gain: Option<f64>,
    #[serde(deserialize_with = "num")]
    track_peak: Option<f64>,
    #[serde(deserialize_with = "num")]
    album_peak: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Child {
    #[serde(deserialize_with = "id")]
    id: String,
    title: Option<String>,
    album: Option<String>,
    artist: Option<String>,
    #[serde(deserialize_with = "num")]
    track: Option<f64>,
    #[serde(deserialize_with = "num")]
    year: Option<f64>,
    genre: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    cover_art: Option<String>,
    #[serde(deserialize_with = "num")]
    size: Option<f64>,
    suffix: Option<String>,
    #[serde(deserialize_with = "num")]
    duration: Option<f64>,
    #[serde(deserialize_with = "num")]
    bit_rate: Option<f64>,
    path: Option<String>,
    #[serde(deserialize_with = "num")]
    disc_number: Option<f64>,
    #[serde(deserialize_with = "opt_id")]
    album_id: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    parent: Option<String>,
    #[serde(deserialize_with = "num")]
    play_count: Option<f64>,
    is_dir: bool,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(deserialize_with = "num")]
    bit_depth: Option<f64>,
    #[serde(deserialize_with = "num")]
    sampling_rate: Option<f64>,
    #[serde(deserialize_with = "num")]
    channel_count: Option<f64>,
    #[serde(deserialize_with = "list")]
    genres: Vec<NameObj>,
    replay_gain: Option<Rg>,
    display_album_artist: Option<String>,
    #[serde(deserialize_with = "list")]
    album_artists: Vec<NameObj>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct AlbumId3 {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    artist: Option<String>,
    display_artist: Option<String>,
    #[serde(deserialize_with = "num")]
    song_count: Option<f64>,
    #[serde(deserialize_with = "num")]
    duration: Option<f64>,
    #[serde(deserialize_with = "num")]
    year: Option<f64>,
    created: Option<String>,
    #[serde(deserialize_with = "list")]
    song: Vec<Child>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumList {
    #[serde(deserialize_with = "list")]
    album: Vec<AlbumId3>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Playlist {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    owner: Option<String>,
    #[serde(deserialize_with = "num")]
    song_count: Option<f64>,
    changed: Option<String>,
    readonly: Option<bool>,
    #[serde(deserialize_with = "list")]
    entry: Vec<Child>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Playlists {
    #[serde(deserialize_with = "list")]
    playlist: Vec<Playlist>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Station {
    #[serde(deserialize_with = "id")]
    id: String,
    name: Option<String>,
    stream_url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Stations {
    #[serde(deserialize_with = "list")]
    internet_radio_station: Vec<Station>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Episode {
    #[serde(deserialize_with = "id")]
    id: String,
    #[serde(deserialize_with = "opt_id")]
    stream_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    publish_date: Option<String>,
    #[serde(deserialize_with = "num")]
    duration: Option<f64>,
    #[serde(deserialize_with = "num")]
    size: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Channel {
    #[serde(deserialize_with = "id")]
    id: String,
    url: Option<String>,
    title: Option<String>,
    description: Option<String>,
    #[serde(deserialize_with = "list")]
    episode: Vec<Episode>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Podcasts {
    #[serde(deserialize_with = "list")]
    channel: Vec<Channel>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PlayQueue {
    #[serde(deserialize_with = "list")]
    entry: Vec<Child>,
    #[serde(deserialize_with = "opt_id")]
    current: Option<String>,
    #[serde(deserialize_with = "num")]
    position: Option<f64>,
    changed: Option<String>,
    changed_by: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Extension {
    name: String,
}

fn n32(v: Option<f64>) -> Option<u32> {
    v.filter(|n| *n > 0.0).map(|n| n as u32)
}

fn product_name(t: Option<&str>, v: Option<&str>) -> String {
    let name = match t {
        Some("navidrome") => "Navidrome".to_string(),
        Some("gonic") => "gonic".to_string(),
        Some("") | None => "Subsonic".to_string(),
        Some(other) => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    };
    match v {
        Some(v) if !v.is_empty() => format!("{name} {v}"),
        _ => name,
    }
}

impl Child {
    fn into_song(self, album: Option<&AlbumId3>) -> RemoteSong {
        let suffix = self.suffix.clone().unwrap_or_default();
        let mut genres: Vec<String> = self.genres.into_iter().filter_map(|g| g.name).collect();
        if genres.is_empty()
            && let Some(g) = self.genre.filter(|g| !g.is_empty())
        {
            genres.push(g);
        }
        let album_artist = self
            .display_album_artist
            .filter(|s| !s.is_empty())
            .or_else(|| self.album_artists.into_iter().find_map(|a| a.name))
            .or_else(|| {
                album.and_then(|a| {
                    a.display_artist
                        .clone()
                        .filter(|s| !s.is_empty())
                        .or_else(|| a.artist.clone())
                })
            });
        let rg = self.replay_gain.unwrap_or_default();
        RemoteSong {
            title: self.title.unwrap_or_else(|| self.id.clone()),
            id: self.id,
            artist: self.artist,
            album_artist,
            album: self.album.or_else(|| album.and_then(|a| a.name.clone())),
            album_id: self
                .album_id
                .or_else(|| album.map(|a| a.id.clone()))
                .or(self.parent)
                .unwrap_or_default(),
            track_no: n32(self.track),
            disc_no: n32(self.disc_number),
            year: self.year.filter(|y| *y > 0.0).map(|y| y as i32),
            genres,
            duration: self
                .duration
                .filter(|d| *d > 0.0)
                .map(Duration::from_secs_f64),
            suffix,
            sample_rate: n32(self.sampling_rate),
            bits: n32(self.bit_depth),
            channels: n32(self.channel_count).map(|c| c as u16),
            kbps: n32(self.bit_rate),
            size: self.size.map(|s| s as i64),
            path: self.path.filter(|p| !p.is_empty()),
            cover: self.cover_art,
            rg_track_gain: rg.track_gain,
            rg_track_peak: rg.track_peak,
            rg_album_gain: rg.album_gain,
            rg_album_peak: rg.album_peak,
            play_count: self.play_count.unwrap_or(0.0) as i64,
        }
    }
}

impl Subsonic {
    pub fn connect(
        base: &str,
        user: &str,
        secret: &str,
        stream: StreamFormat,
        bitrate: u32,
    ) -> Result<Self> {
        let mut s = Self {
            base: base.trim_end_matches('/').to_string(),
            user: user.to_string(),
            auth: Auth::Token {
                password: secret.to_string(),
            },
            http: http::client()?,
            info: ServerInfo {
                kind: ServerKind::Subsonic,
                ..Default::default()
            },
            stream,
            bitrate,
            form_post: false,
        };
        // OpenSubsonic servers answer this without a login.
        let exts: Vec<String> = match s.raw("getOpenSubsonicExtensions", &[], false) {
            Ok(r) if r.status == "ok" => r
                .rest
                .get("openSubsonicExtensions")
                .cloned()
                .and_then(|v| serde_json::from_value::<Vec<Extension>>(v).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.name)
                .collect(),
            _ => Vec::new(),
        };
        let api_key = exts.iter().any(|e| e == "apiKeyAuthentication");
        s.form_post = exts.iter().any(|e| e == "formPost");
        if user.is_empty() && api_key {
            s.auth = Auth::ApiKey(secret.to_string());
        }
        let r = match s.raw("ping", &[], true) {
            Ok(r) => r,
            Err(RemoteError::Server { code: 41 | 42, .. }) if api_key => {
                s.auth = Auth::ApiKey(secret.to_string());
                s.raw("ping", &[], true)?
            }
            Err(e) => return Err(e),
        };
        s.check(&r)?;
        s.info.open_subsonic = r.open_subsonic || !exts.is_empty();
        s.info.api_version = r.version.clone().unwrap_or_default();
        s.info.product = product_name(r.server_type.as_deref(), r.server_version.as_deref());
        s.info.play_queue = true;
        Ok(s)
    }

    fn auth_params(&self) -> Vec<(&'static str, String)> {
        let mut q = vec![
            ("v", API.to_string()),
            ("c", super::CLIENT.to_string()),
            ("f", "json".to_string()),
        ];
        match &self.auth {
            Auth::Token { password } => {
                let salt = http::salt();
                let mut h = Md5::new();
                h.update(password.as_bytes());
                h.update(salt.as_bytes());
                q.push(("u", self.user.clone()));
                q.push(("t", hex::encode(h.finalize())));
                q.push(("s", salt));
            }
            Auth::ApiKey(k) => q.push(("apiKey", k.clone())),
        }
        q
    }

    fn endpoint(&self, method: &str) -> String {
        format!("{}/rest/{method}", self.base)
    }

    fn raw(&self, method: &str, params: &[(&str, String)], auth: bool) -> Result<Resp> {
        let mut q: Vec<(&str, String)> = if auth {
            self.auth_params()
        } else {
            vec![
                ("v", API.to_string()),
                ("c", super::CLIENT.to_string()),
                ("f", "json".to_string()),
            ]
        };
        q.extend(params.iter().cloned());
        let (body, _) = http::get_bytes(&self.http, &self.endpoint(method), &q)?;
        parse(&body)
    }

    fn check(&self, r: &Resp) -> Result<()> {
        if r.status == "ok" {
            return Ok(());
        }
        let (code, message) = r
            .error
            .as_ref()
            .map(|e| {
                (
                    e.code.unwrap_or(0.0) as i64,
                    e.message.clone().unwrap_or_default(),
                )
            })
            .unwrap_or((0, "unknown error".into()));
        Err(match code {
            40 | 44 => RemoteError::Auth,
            _ => RemoteError::Server { code, message },
        })
    }

    /// Call `method` and take `key` out of the answer. A missing key comes
    /// back as T's default, since servers leave out empty results.
    fn call<T: DeserializeOwned + Default>(
        &self,
        method: &str,
        params: &[(&str, String)],
        key: &str,
    ) -> Result<T> {
        let r = self.raw(method, params, true)?;
        self.check(&r)?;
        match r.rest.get(key) {
            None | Some(Value::Null) => Ok(T::default()),
            // Navidrome sends `"playQueue": {}` when there's none.
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| RemoteError::Parse(format!("{method}: {e}"))),
        }
    }

    /// For calls with long lists of song ids: a POST when the server takes
    /// one, a GET otherwise.
    fn call_ids(&self, method: &str, params: &[(&str, String)], key: &str) -> Result<Value> {
        let mut q = self.auth_params();
        q.extend(params.iter().cloned());
        let body = if self.form_post {
            self.http
                .post(self.endpoint(method))
                .form(&q)
                .send()
                .and_then(|r| r.bytes())
                .map_err(http::http_err)?
                .to_vec()
        } else {
            http::get_bytes(&self.http, &self.endpoint(method), &q)?.0
        };
        let r = parse(&body)?;
        self.check(&r)?;
        Ok(r.rest.get(key).cloned().unwrap_or(Value::Null))
    }

    fn chunks<'a>(&self, ids: &'a [String]) -> Vec<&'a [String]> {
        if self.form_post || ids.len() <= CHUNK {
            vec![ids]
        } else {
            ids.chunks(CHUNK).collect()
        }
    }

    /// A stream URL with a login in it, for FFmpeg.
    fn signed(&self, method: &str, extra: &[(&str, String)]) -> Result<String> {
        let mut u = url::Url::parse(&self.endpoint(method))
            .map_err(|e| RemoteError::Parse(e.to_string()))?;
        {
            let mut q = u.query_pairs_mut();
            for (k, v) in self.auth_params().iter().chain(extra.iter()) {
                q.append_pair(k, v);
            }
        }
        Ok(u.into())
    }
}

fn parse(body: &[u8]) -> Result<Resp> {
    serde_json::from_slice::<Envelope>(body)
        .map(|e| e.r)
        .map_err(|e| {
            let head = String::from_utf8_lossy(&body[..body.len().min(80)]).into_owned();
            RemoteError::Parse(format!("not a Subsonic answer ({e}): {head}"))
        })
}

impl Client for Subsonic {
    fn info(&self) -> &ServerInfo {
        &self.info
    }

    fn albums(&self) -> Result<Vec<RemoteAlbum>> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let page: AlbumList = self.call(
                "getAlbumList2",
                &[
                    ("type", "alphabeticalByName".into()),
                    ("size", PAGE.to_string()),
                    ("offset", offset.to_string()),
                ],
                "albumList2",
            )?;
            let n = page.album.len();
            out.extend(page.album.into_iter().map(|a| RemoteAlbum {
                name: a.name.unwrap_or_default(),
                artist: a.display_artist.or(a.artist).unwrap_or_default(),
                song_count: n32(a.song_count).unwrap_or(0),
                duration_s: a.duration.unwrap_or(0.0) as u64,
                year: a.year.filter(|y| *y > 0.0).map(|y| y as i32),
                changed: a.created,
                id: a.id,
            }));
            if n < PAGE {
                break;
            }
            offset += n;
        }
        Ok(out)
    }

    fn album_songs(&self, album_id: &str) -> Result<Vec<RemoteSong>> {
        let mut a: AlbumId3 = self.call("getAlbum", &[("id", album_id.into())], "album")?;
        let songs = std::mem::take(&mut a.song);
        Ok(songs
            .into_iter()
            .filter(|c| !c.is_dir && c.kind.as_deref().is_none_or(|k| k == "music"))
            .map(|c| c.into_song(Some(&a)))
            .collect())
    }

    fn stream_url(&self, song_id: &str) -> Result<(String, bool)> {
        let mut q = vec![("id", song_id.to_string())];
        match self.stream {
            StreamFormat::Raw => q.push(("format", "raw".into())),
            f => {
                q.push(("format", f.as_str().into()));
                q.push(("maxBitRate", self.bitrate.max(32).to_string()));
            }
        }
        Ok((self.signed("stream", &q)?, self.stream == StreamFormat::Raw))
    }

    fn episode_url(&self, stream_id: &str) -> Result<String> {
        self.signed("stream", &[("id", stream_id.to_string())])
    }

    fn cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        let mut q = self.auth_params();
        q.push(("id", cover_id.to_string()));
        q.push(("size", size.to_string()));
        let (body, ctype) = http::get_bytes(&self.http, &self.endpoint("getCoverArt"), &q)?;
        if ctype.contains("json") || ctype.contains("xml") {
            let r = parse(&body)?;
            self.check(&r)?;
            return Err(RemoteError::Parse("no image".into()));
        }
        Ok(body)
    }

    fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        let p: Playlists = self.call("getPlaylists", &[], "playlists")?;
        Ok(p.playlist
            .into_iter()
            .map(|p| RemotePlaylist {
                readonly: p.readonly.unwrap_or_else(|| {
                    p.owner
                        .as_deref()
                        .is_some_and(|o| !self.user.is_empty() && o != self.user)
                }),
                name: p.name.unwrap_or_default(),
                song_count: n32(p.song_count).unwrap_or(0),
                changed: p.changed,
                id: p.id,
            })
            .collect())
    }

    fn playlist_songs(&self, id: &str) -> Result<Vec<RemoteSong>> {
        let p: Playlist = self.call("getPlaylist", &[("id", id.into())], "playlist")?;
        Ok(p.entry.into_iter().map(|c| c.into_song(None)).collect())
    }

    fn create_playlist(&self, name: &str, songs: &[String]) -> Result<String> {
        let parts = self.chunks(songs);
        let first = parts.first().copied().unwrap_or(&[]);
        let mut q: Vec<(&str, String)> = vec![("name", name.to_string())];
        q.extend(first.iter().map(|s| ("songId", s.clone())));
        let v = self.call_ids("createPlaylist", &q, "playlist")?;
        let id = match serde_json::from_value::<Playlist>(v) {
            Ok(p) if !p.id.is_empty() => p.id,
            // Before 1.14 createPlaylist answered with nothing. Find it.
            _ => self
                .playlists()?
                .into_iter()
                .rfind(|p| p.name == name)
                .map(|p| p.id)
                .ok_or_else(|| RemoteError::Parse("the new playlist didn't show up".into()))?,
        };
        for part in parts.iter().skip(1) {
            let mut q: Vec<(&str, String)> = vec![("playlistId", id.clone())];
            q.extend(part.iter().map(|s| ("songIdToAdd", s.clone())));
            self.call_ids("updatePlaylist", &q, "playlist")?;
        }
        Ok(id)
    }

    fn replace_playlist(&self, id: &str, songs: &[String]) -> Result<()> {
        let parts = self.chunks(songs);
        // createPlaylist with a playlistId replaces its songs.
        let mut q: Vec<(&str, String)> = vec![("playlistId", id.to_string())];
        q.extend(
            parts
                .first()
                .copied()
                .unwrap_or(&[])
                .iter()
                .map(|s| ("songId", s.clone())),
        );
        self.call_ids("createPlaylist", &q, "playlist")?;
        for part in parts.iter().skip(1) {
            let mut q: Vec<(&str, String)> = vec![("playlistId", id.to_string())];
            q.extend(part.iter().map(|s| ("songIdToAdd", s.clone())));
            self.call_ids("updatePlaylist", &q, "playlist")?;
        }
        Ok(())
    }

    fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.call::<Value>(
            "updatePlaylist",
            &[("playlistId", id.into()), ("name", name.into())],
            "playlist",
        )
        .map(|_| ())
    }

    fn delete_playlist(&self, id: &str) -> Result<()> {
        self.call::<Value>("deletePlaylist", &[("id", id.into())], "playlist")
            .map(|_| ())
    }

    fn radio(&self) -> Result<Vec<RemoteStation>> {
        let s: Stations = self.call("getInternetRadioStations", &[], "internetRadioStations")?;
        Ok(s.internet_radio_station
            .into_iter()
            .filter_map(|s| {
                Some(RemoteStation {
                    url: s.stream_url.filter(|u| !u.is_empty())?,
                    name: s.name.unwrap_or_else(|| s.id.clone()),
                    id: s.id,
                })
            })
            .collect())
    }

    fn podcasts(&self) -> Result<Vec<RemotePodcast>> {
        let p: Podcasts = self.call(
            "getPodcasts",
            &[("includeEpisodes", "true".into())],
            "podcasts",
        )?;
        Ok(p.channel
            .into_iter()
            .map(|c| RemotePodcast {
                title: c.title.unwrap_or_else(|| c.id.clone()),
                id: c.id,
                author: None,
                description: c.description,
                feed_url: c.url,
                episodes: c
                    .episode
                    .into_iter()
                    .map(|e| RemoteEpisode {
                        title: e.title.unwrap_or_else(|| e.id.clone()),
                        id: e.id,
                        stream_id: e.stream_id,
                        published: e.publish_date.as_deref().and_then(http::date_secs),
                        duration: e.duration.filter(|d| *d > 0.0).map(Duration::from_secs_f64),
                        description: e.description,
                        size: e.size.map(|s| s as i64),
                    })
                    .collect(),
            })
            .collect())
    }

    fn save_queue(&self, ids: &[String], current: Option<&str>, pos: Duration) -> Result<()> {
        let mut q: Vec<(&str, String)> = ids.iter().map(|i| ("id", i.clone())).collect();
        if let Some(c) = current {
            q.push(("current", c.to_string()));
        }
        q.push(("position", pos.as_millis().to_string()));
        self.call_ids("savePlayQueue", &q, "playQueue").map(|_| ())
    }

    fn load_queue(&self) -> Result<Option<SavedQueue>> {
        let q: PlayQueue = self.call("getPlayQueue", &[], "playQueue")?;
        if q.entry.is_empty() {
            return Ok(None);
        }
        Ok(Some(SavedQueue {
            ids: q.entry.into_iter().map(|c| c.id).collect(),
            current: q.current,
            position: Duration::from_millis(q.position.unwrap_or(0.0).max(0.0) as u64),
            changed_by: q.changed_by,
            changed: q.changed,
        }))
    }

    fn bookmark(&self, id: &str, pos: Duration) -> Result<()> {
        self.call::<Value>(
            "createBookmark",
            &[("id", id.into()), ("position", pos.as_millis().to_string())],
            "bookmark",
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navidrome_and_gonic_songs_parse() {
        // Navidrome, OpenSubsonic fields and all.
        let nd: Child = serde_json::from_str(
            r#"{"id":"2f6e","parent":"9a1","isDir":false,"title":"Blew","album":"Bleach",
            "artist":"Nirvana","track":1,"year":1989,"genre":"Grunge","coverArt":"al-9a1_0",
            "size":5853923,"contentType":"audio/flac","suffix":"flac","duration":175,
            "bitRate":915,"path":"Nirvana/Bleach/01 - Blew.flac","discNumber":1,
            "albumId":"9a1","type":"music","playCount":3,"bitDepth":16,"samplingRate":44100,
            "channelCount":2,"genres":[{"name":"Grunge"},{"name":"Rock"}],
            "replayGain":{"trackGain":-6.5,"albumGain":-7.1,"trackPeak":0.98,"albumPeak":1.0},
            "displayAlbumArtist":"Nirvana","albumArtists":[{"id":"x","name":"Nirvana"}]}"#,
        )
        .unwrap();
        let s = nd.into_song(None);
        assert_eq!(s.genres, vec!["Grunge", "Rock"]);
        assert_eq!(
            (s.sample_rate, s.bits, s.channels),
            (Some(44100), Some(16), Some(2))
        );
        assert_eq!(s.rg_track_gain, Some(-6.5));
        assert_eq!(s.album_artist.as_deref(), Some("Nirvana"));
        assert_eq!(s.path.as_deref(), Some("Nirvana/Bleach/01 - Blew.flac"));

        // Gonic: prefixed ids, a lone genre, no OpenSubsonic extras.
        let g: Child = serde_json::from_str(
            r#"{"id":"tr-12","album":"Bleach","albumId":"al-3","artist":"Nirvana",
            "coverArt":"al-3","duration":175,"genre":"Grunge","isDir":false,
            "parent":"al-3","path":"Nirvana/Bleach/01 Blew.flac","size":5853923,
            "suffix":"flac","title":"Blew","track":1,"discNumber":1,"type":"music","year":1989}"#,
        )
        .unwrap();
        let album = AlbumId3 {
            id: "al-3".into(),
            artist: Some("Nirvana".into()),
            ..Default::default()
        };
        let s = g.into_song(Some(&album));
        assert_eq!(s.id, "tr-12");
        assert_eq!(s.album_id, "al-3");
        assert_eq!(s.genres, vec!["Grunge"]);
        assert_eq!(s.album_artist.as_deref(), Some("Nirvana"));
    }

    #[test]
    fn errors_and_product_names() {
        let r = parse(
            br#"{"subsonic-response":{"status":"failed","version":"1.16.1",
            "error":{"code":40,"message":"Wrong username or password"}}}"#,
        )
        .unwrap();
        assert_eq!(r.status, "failed");
        assert_eq!(
            product_name(Some("navidrome"), Some("0.58.0")),
            "Navidrome 0.58.0"
        );
        assert_eq!(product_name(Some("lms"), None), "Lms");
        assert!(parse(b"<html>nope</html>").is_err());
    }
}
