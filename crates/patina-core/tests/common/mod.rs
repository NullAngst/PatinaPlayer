//! Stand-in music servers for the server tests: a small HTTP server and
//! Subsonic and Ampache APIs on top of it, holding a library in memory.
//!
//! The Subsonic one answers the way Navidrome or Gonic does, depending on
//! the flavor: Navidrome's OpenSubsonic fields, made-up paths and hex ids;
//! Gonic's "al-"/"tr-" ids, real paths, missing fields and lists sent as a
//! lone object. The Ampache one follows the JSON API 6 documentation,
//! sessions that run out included. The DLNA one answers SSDP searches and
//! ContentDirectory calls three ways: a folder view like MiniDLNA, search
//! only like Jellyfin, or neither, with every song under several views.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};

use md5::{Digest, Md5};
use serde_json::{Value, json};
use sha2::Sha256;

// ----- HTTP -----

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
    }
    pub fn all(&self, k: &str) -> Vec<String> {
        self.query
            .iter()
            .filter(|(a, _)| a == k)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

pub struct Response {
    pub status: u16,
    pub ctype: String,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(v: Value) -> Self {
        Self {
            status: 200,
            ctype: "application/json".into(),
            body: v.to_string().into_bytes(),
        }
    }
    pub fn bytes(ctype: &str, b: Vec<u8>) -> Self {
        Self {
            status: 200,
            ctype: ctype.into(),
            body: b,
        }
    }
    pub fn not_found() -> Self {
        Self {
            status: 404,
            ctype: "text/plain".into(),
            body: b"not found".to_vec(),
        }
    }
}

type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// Serve `handler` on a free port. Returns the base URL.
pub fn serve(handler: Handler) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let h = handler.clone();
            std::thread::spawn(move || {
                let _ = handle(s, &h);
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn handle(s: TcpStream, h: &Handler) -> std::io::Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l)?;
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let mut body = Vec::new();
    if let Some(n) = headers
        .get("content-length")
        .and_then(|n| n.parse::<usize>().ok())
    {
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    }
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let mut query: Vec<(String, String)> = url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect();
    if method == "POST"
        && headers
            .get("content-type")
            .is_some_and(|c| c.contains("form-urlencoded"))
    {
        query.extend(url::form_urlencoded::parse(&body).into_owned());
    }
    let req = Request {
        method,
        path: path.to_string(),
        query,
        headers,
        body,
    };
    let resp = h(&req);
    let mut out = s;
    // Byte ranges, for FFmpeg seeking in a stream.
    if resp.status == 200
        && !resp.ctype.contains("json")
        && let Some(range) = req.headers.get("range")
        && let Some(spec) = range.strip_prefix("bytes=")
    {
        let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
        let len = resp.body.len();
        let start: usize = a.parse().unwrap_or(0).min(len);
        let end: usize = b.parse::<usize>().map(|e| e + 1).unwrap_or(len).min(len);
        let part = &resp.body[start..end.max(start)];
        write!(
            out,
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Content-Range: bytes {start}-{}/{len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            resp.ctype,
            part.len(),
            end.max(start + 1) - 1
        )?;
        out.write_all(part)?;
        return Ok(());
    }
    write!(
        out,
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\
         Connection: close\r\n\r\n",
        resp.status,
        resp.ctype,
        resp.body.len()
    )?;
    out.write_all(&resp.body)?;
    Ok(())
}

// ----- A library in memory -----

#[derive(Clone)]
pub struct MSong {
    pub id: String,
    pub title: String,
    pub track: u32,
    pub artist: String,
    pub path: String,
    pub plays: i64,
    /// What the server says the file's size is.
    pub size: u64,
}

#[derive(Clone)]
pub struct MAlbum {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub year: i32,
    pub genre: String,
    pub songs: Vec<MSong>,
}

#[derive(Clone)]
pub struct MPlaylist {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub songs: Vec<String>,
    pub changed: u64,
}

pub struct State {
    pub albums: Vec<MAlbum>,
    pub playlists: Vec<MPlaylist>,
    pub queue: Option<(Vec<String>, Option<String>, u64)>,
    pub bookmarks: HashMap<String, u64>,
    /// Every call by method name, for checking what a sync asked for.
    pub calls: Vec<String>,
    pub next_id: u32,
    pub tick: u64,
    /// Ampache: sessions die after this many calls (0: never).
    pub session_life: u32,
    pub session_calls: u32,
    pub sessions: Vec<String>,
    /// Sent for every song in place of the second of sine, when set.
    pub stream_body: Option<Arc<Vec<u8>>>,
    /// The format each stream call asked for: "raw", "opus:128".
    pub streams: Vec<String>,
}

impl State {
    pub fn count(&self, method: &str) -> usize {
        self.calls.iter().filter(|c| *c == method).count()
    }
    pub fn song(&self, id: &str) -> Option<(&MAlbum, &MSong)> {
        self.albums
            .iter()
            .find_map(|a| a.songs.iter().find(|s| s.id == id).map(|s| (a, s)))
    }
}

/// A FLAC second of sine, made once per test binary.
pub fn audio() -> Arc<Vec<u8>> {
    static A: std::sync::OnceLock<Arc<Vec<u8>>> = std::sync::OnceLock::new();
    A.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.flac");
        let st = Proc::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=f=440:r=44100:d=1")
            .args(["-ac", "2"])
            .arg(&p)
            .status()
            .expect("ffmpeg must be installed");
        assert!(st.success());
        Arc::new(std::fs::read(&p).unwrap())
    })
    .clone()
}

pub fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 40, 120]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// Two albums, five songs.
pub fn library(gonic: bool) -> State {
    let (al, tr) = if gonic { ("al-", "tr-") } else { ("", "") };
    let song = |id: u32, title: &str, track: u32, artist: &str, path: &str| MSong {
        id: format!("{tr}{id}"),
        title: title.into(),
        track,
        artist: artist.into(),
        path: path.into(),
        plays: 0,
        size: 1000,
    };
    State {
        albums: vec![
            MAlbum {
                id: format!("{al}1"),
                name: "Bleach".into(),
                artist: "Nirvana".into(),
                year: 1989,
                genre: "Grunge".into(),
                songs: vec![
                    song(11, "Blew", 1, "Nirvana", "Nirvana/Bleach/01 Blew.flac"),
                    song(
                        12,
                        "Floyd the Barber",
                        2,
                        "Nirvana",
                        "Nirvana/Bleach/02 Floyd the Barber.flac",
                    ),
                    song(
                        13,
                        "About a Girl",
                        3,
                        "Nirvana",
                        "Nirvana/Bleach/03 About a Girl.flac",
                    ),
                ],
            },
            MAlbum {
                id: format!("{al}2"),
                name: "Loveless".into(),
                artist: "My Bloody Valentine".into(),
                year: 1991,
                genre: "Shoegaze".into(),
                songs: vec![
                    song(
                        21,
                        "Only Shallow",
                        1,
                        "My Bloody Valentine",
                        "MBV/Loveless/01 Only Shallow.flac",
                    ),
                    song(
                        22,
                        "Loomer",
                        2,
                        "My Bloody Valentine",
                        "MBV/Loveless/02 Loomer.flac",
                    ),
                ],
            },
        ],
        playlists: Vec::new(),
        queue: None,
        bookmarks: HashMap::new(),
        calls: Vec::new(),
        next_id: 100,
        tick: 0,
        session_life: 0,
        session_calls: 0,
        sessions: Vec::new(),
        stream_body: None,
        streams: Vec::new(),
    }
}

// ----- Subsonic -----

#[derive(Clone, Copy, PartialEq)]
pub enum Flavor {
    Navidrome,
    Gonic,
}

pub struct Subsonic {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}

fn ok(flavor: Flavor, extra: Value) -> Response {
    let mut r = match flavor {
        Flavor::Navidrome => json!({"status":"ok","version":"1.16.1","type":"navidrome",
            "serverVersion":"0.58.0","openSubsonic":true}),
        Flavor::Gonic => json!({"status":"ok","version":"1.15.0","type":"gonic",
            "serverVersion":"0.16.4","openSubsonic":true}),
    };
    if let (Value::Object(m), Value::Object(e)) = (&mut r, extra) {
        m.extend(e);
    }
    Response::json(json!({ "subsonic-response": r }))
}

fn fail(code: i64, msg: &str) -> Response {
    Response::json(
        json!({"subsonic-response":{"status":"failed","version":"1.16.1",
        "error":{"code":code,"message":msg}}}),
    )
}

/// Gonic sends a one-item list as the item itself.
fn many(flavor: Flavor, v: Vec<Value>) -> Value {
    if flavor == Flavor::Gonic && v.len() == 1 {
        v.into_iter().next().unwrap()
    } else {
        Value::Array(v)
    }
}

fn child(flavor: Flavor, a: &MAlbum, s: &MSong) -> Value {
    match flavor {
        Flavor::Navidrome => json!({
            "id": s.id, "parent": a.id, "isDir": false, "title": s.title, "album": a.name,
            "artist": s.artist, "track": s.track, "year": a.year, "genre": a.genre,
            "coverArt": format!("al-{}", a.id), "size": s.size, "contentType": "audio/flac",
            "suffix": "flac", "duration": 1, "bitRate": 900,
            // Navidrome's default: a path made up from the tags.
            "path": format!("{}/{}/{:02} - {}.flac", a.artist, a.name, s.track, s.title),
            "discNumber": 1, "albumId": a.id, "type": "music", "playCount": s.plays,
            "bitDepth": 16, "samplingRate": 44100, "channelCount": 2,
            "genres": [{"name": a.genre}],
            "replayGain": {"trackGain": -6.0, "albumGain": -7.0, "trackPeak": 0.9, "albumPeak": 1.0},
            "displayAlbumArtist": a.artist
        }),
        // Gonic: the real path, no OpenSubsonic extras, no play count.
        Flavor::Gonic => json!({
            "id": s.id, "parent": a.id, "isDir": false, "title": s.title, "album": a.name,
            "albumId": a.id, "artist": s.artist, "coverArt": a.id, "duration": 1,
            "genre": a.genre, "path": s.path, "size": s.size, "suffix": "flac",
            "track": s.track, "discNumber": 1, "type": "music", "year": a.year
        }),
    }
}

fn album_json(flavor: Flavor, a: &MAlbum, with_songs: bool) -> Value {
    let mut v = json!({
        "id": a.id, "name": a.name, "artist": a.artist, "songCount": a.songs.len(),
        "duration": a.songs.len(), "year": a.year, "created": "2024-01-02T03:04:05Z",
        "coverArt": a.id, "genre": a.genre
    });
    if with_songs {
        v["song"] = many(
            flavor,
            a.songs.iter().map(|s| child(flavor, a, s)).collect(),
        );
    }
    v
}

impl Subsonic {
    pub fn start(flavor: Flavor, user: &str, password: &str, api_key: Option<&str>) -> Self {
        let state = Arc::new(Mutex::new(library(flavor == Flavor::Gonic)));
        let (st, user, password, key) = (
            state.clone(),
            user.to_string(),
            password.to_string(),
            api_key.map(str::to_string),
        );
        let url = serve(Arc::new(move |r: &Request| {
            subsonic(flavor, &st, &user, &password, key.as_deref(), r)
        }));
        Self { url, state }
    }
}

fn subsonic(
    flavor: Flavor,
    st: &Mutex<State>,
    user: &str,
    password: &str,
    api_key: Option<&str>,
    r: &Request,
) -> Response {
    let Some(method) = r.path.strip_prefix("/rest/") else {
        return Response::not_found();
    };
    let method = method.trim_end_matches(".view");
    st.lock().unwrap().calls.push(method.to_string());
    if method == "getOpenSubsonicExtensions" {
        let mut exts = vec![json!({"name":"formPost","versions":[1]})];
        if api_key.is_some() {
            exts.push(json!({"name":"apiKeyAuthentication","versions":[1]}));
        }
        if flavor == Flavor::Gonic {
            exts.retain(|e| e["name"] != "formPost");
        }
        return ok(flavor, json!({"openSubsonicExtensions": exts}));
    }
    // Login.
    if let Some(k) = r.get("apiKey") {
        if Some(k) != api_key {
            return fail(44, "Invalid API key");
        }
    } else {
        if r.get("p").is_some() {
            return fail(42, "plain passwords are not allowed here");
        }
        let (Some(t), Some(s)) = (r.get("t"), r.get("s")) else {
            return fail(10, "Required parameter is missing");
        };
        let mut h = Md5::new();
        h.update(password.as_bytes());
        h.update(s.as_bytes());
        if r.get("u") != Some(user) || hex::encode(h.finalize()) != t {
            return fail(40, "Wrong username or password");
        }
    }
    let mut s = st.lock().unwrap();
    s.tick += 1;
    match method {
        "ping" => ok(flavor, json!({})),
        "getAlbumList2" => {
            let size: usize = r.get("size").and_then(|n| n.parse().ok()).unwrap_or(10);
            let offset: usize = r.get("offset").and_then(|n| n.parse().ok()).unwrap_or(0);
            let list: Vec<Value> = s
                .albums
                .iter()
                .skip(offset)
                .take(size)
                .map(|a| album_json(flavor, a, false))
                .collect();
            if flavor == Flavor::Gonic && list.is_empty() {
                // Gonic leaves the empty list out entirely.
                return ok(flavor, json!({"albumList2": {}}));
            }
            ok(flavor, json!({"albumList2": {"album": many(flavor, list)}}))
        }
        "getAlbum" => match s.albums.iter().find(|a| Some(a.id.as_str()) == r.get("id")) {
            Some(a) => ok(flavor, json!({"album": album_json(flavor, a, true)})),
            None => fail(70, "Album not found"),
        },
        "stream" => match r.get("id").and_then(|id| s.song(id)) {
            Some(_) => {
                let f = r.get("format").unwrap_or("").to_string();
                let rec = match r.get("maxBitRate") {
                    Some(k) => format!("{f}:{k}"),
                    None => f,
                };
                s.streams.push(rec);
                let id = r.get("id").unwrap_or("").to_string();
                s.calls.push(format!("stream {id}"));
                let body = s.stream_body.clone().unwrap_or_else(audio);
                Response::bytes("audio/flac", body.to_vec())
            }
            None if r.get("id").is_some_and(|i| i.starts_with("ep")) => {
                Response::bytes("audio/flac", audio().to_vec())
            }
            None => fail(70, "Song not found"),
        },
        "getCoverArt" => Response::bytes("image/png", png()),
        "getPlaylists" => {
            let lists: Vec<Value> = s
                .playlists
                .iter()
                .map(|p| {
                    json!({"id": p.id, "name": p.name, "owner": p.owner,
                    "songCount": p.songs.len(), "changed": format!("2024-01-01T00:00:{:02}Z", p.changed % 60)})
                })
                .collect();
            ok(
                flavor,
                json!({"playlists": {"playlist": many(flavor, lists)}}),
            )
        }
        "getPlaylist" => {
            let Some(p) = s
                .playlists
                .iter()
                .find(|p| Some(p.id.as_str()) == r.get("id"))
            else {
                return fail(70, "Playlist not found");
            };
            let entries: Vec<Value> = p
                .songs
                .iter()
                .filter_map(|id| s.song(id).map(|(a, x)| child(flavor, a, x)))
                .collect();
            ok(
                flavor,
                json!({"playlist": {"id": p.id, "name": p.name, "owner": p.owner,
                "songCount": p.songs.len(), "entry": many(flavor, entries)}}),
            )
        }
        "createPlaylist" => {
            let songs = r.all("songId");
            let tick = s.tick;
            let id = match r.get("playlistId") {
                Some(pid) => {
                    let Some(p) = s.playlists.iter_mut().find(|p| p.id == pid) else {
                        return fail(70, "Playlist not found");
                    };
                    p.songs = songs;
                    p.changed = tick;
                    pid.to_string()
                }
                None => {
                    s.next_id += 1;
                    let id = format!("pl{}", s.next_id);
                    s.playlists.push(MPlaylist {
                        id: id.clone(),
                        name: r.get("name").unwrap_or("").into(),
                        owner: user.into(),
                        songs,
                        changed: tick,
                    });
                    id
                }
            };
            let p = s.playlists.iter().find(|p| p.id == id).unwrap();
            ok(
                flavor,
                json!({"playlist": {"id": p.id, "name": p.name, "songCount": p.songs.len()}}),
            )
        }
        "updatePlaylist" => {
            let tick = s.tick;
            let Some(p) = s
                .playlists
                .iter_mut()
                .find(|p| Some(p.id.as_str()) == r.get("playlistId"))
            else {
                return fail(70, "Playlist not found");
            };
            if let Some(n) = r.get("name") {
                p.name = n.into();
            }
            let mut rm: Vec<usize> = r
                .all("songIndexToRemove")
                .iter()
                .filter_map(|i| i.parse().ok())
                .collect();
            rm.sort_unstable_by(|a, b| b.cmp(a));
            for i in rm {
                if i < p.songs.len() {
                    p.songs.remove(i);
                }
            }
            p.songs.extend(r.all("songIdToAdd"));
            p.changed = tick;
            ok(flavor, json!({}))
        }
        "deletePlaylist" => {
            let id = r.get("id").unwrap_or("").to_string();
            s.playlists.retain(|p| p.id != id);
            ok(flavor, json!({}))
        }
        "getInternetRadioStations" => ok(
            flavor,
            json!({"internetRadioStations": {"internetRadioStation": many(flavor, vec![
                json!({"id": "r1", "name": "Server FM", "streamUrl": "http://127.0.0.1:9/fm"})
            ])}}),
        ),
        "getPodcasts" => ok(
            flavor,
            json!({"podcasts": {"channel": many(flavor, vec![json!({
                "id": "ch1", "url": "https://example.com/feed.xml", "title": "Server Pod",
                "description": "A show", "status": "completed",
                "episode": [
                    {"id": "ep1", "streamId": "ep1s", "channelId": "ch1", "title": "Episode One",
                     "publishDate": "2024-05-01T10:00:00Z", "status": "completed", "duration": 1},
                    {"id": "ep2", "channelId": "ch1", "title": "Not downloaded yet",
                     "status": "skipped"}
                ]
            })])}}),
        ),
        "savePlayQueue" => {
            let ids = r.all("id");
            let pos: u64 = r.get("position").and_then(|p| p.parse().ok()).unwrap_or(0);
            s.queue = Some((ids, r.get("current").map(str::to_string), pos));
            ok(flavor, json!({}))
        }
        "getPlayQueue" => match s.queue.clone() {
            Some((ids, cur, pos)) => {
                let entries: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| s.song(id).map(|(a, x)| child(flavor, a, x)))
                    .collect();
                ok(
                    flavor,
                    json!({"playQueue": {"entry": entries, "current": cur, "position": pos,
                    "changed": "2024-01-01T00:00:00Z", "changedBy": "OtherClient"}}),
                )
            }
            None => ok(flavor, json!({"playQueue": {}})),
        },
        "createBookmark" => {
            let pos: u64 = r.get("position").and_then(|p| p.parse().ok()).unwrap_or(0);
            let id = r.get("id").unwrap_or("").to_string();
            s.bookmarks.insert(id, pos);
            ok(flavor, json!({}))
        }
        _ => fail(0, "unknown method"),
    }
}

// ----- Ampache -----

pub struct Ampache {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}

fn amp_err(code: &str, action: &str, msg: &str) -> Response {
    Response::json(json!({"error": {"errorCode": code, "errorAction": action,
        "errorType": "system", "errorMessage": msg}}))
}

fn amp_song(a: &MAlbum, s: &MSong) -> Value {
    json!({
        "id": s.id, "title": s.title, "name": s.title,
        "artist": {"id": "ar1", "name": s.artist},
        "album": {"id": a.id, "name": a.name},
        "albumartist": {"id": "ar1", "name": a.artist},
        "disk": 1, "track": s.track,
        "filename": format!("/srv/music/{}", s.path),
        "genre": [{"id": "g1", "name": a.genre}],
        "time": 1, "year": a.year, "bitrate": 900000, "rate": 44100, "mode": "vbr",
        "mime": "audio/flac", "size": 1000, "playcount": s.plays, "catalog": 1,
        "replaygain_track_gain": -6.0, "replaygain_track_peak": 0.9
    })
}

impl Ampache {
    pub fn start(user: &str, password: &str) -> Self {
        let state = Arc::new(Mutex::new(library(false)));
        let (st, user, password) = (state.clone(), user.to_string(), password.to_string());
        let url = serve(Arc::new(move |r: &Request| {
            ampache(&st, &user, &password, r)
        }));
        Self { url, state }
    }
}

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn ampache(st: &Mutex<State>, user: &str, password: &str, r: &Request) -> Response {
    if r.path != "/server/json.server.php" {
        return Response::not_found();
    }
    let action = r.get("action").unwrap_or("").to_string();
    let mut s = st.lock().unwrap();
    s.calls.push(action.clone());
    match action.as_str() {
        "ping" if r.get("auth").is_none() => {
            return Response::json(
                json!({"server": "6.6.0", "version": "6.6.0", "compatible": "350001"}),
            );
        }
        "handshake" => {
            let ts = r.get("timestamp").unwrap_or("");
            let want = sha(&format!("{ts}{}", sha(password)));
            if r.get("user") != Some(user) || r.get("auth") != Some(want.as_str()) {
                return amp_err(
                    "4701",
                    "handshake",
                    "Received Invalid Handshake - Incorrect username or password",
                );
            }
            s.next_id += 1;
            let token = format!("tok{}", s.next_id);
            s.sessions.push(token.clone());
            s.session_calls = 0;
            return Response::json(json!({
                "auth": token, "api": "6.6.0",
                "session_expire": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                "update": "2024-01-01T00:00:00+00:00", "songs": 5, "albums": 2
            }));
        }
        _ => {}
    }
    // Every other action needs a live session.
    let live = r
        .get("auth")
        .is_some_and(|a| s.sessions.last().map(String::as_str) == Some(a));
    s.session_calls += 1;
    if !live || (s.session_life > 0 && s.session_calls > s.session_life) {
        s.sessions.clear();
        return amp_err("4701", &action, "Session Expired");
    }
    let offset: usize = r.get("offset").and_then(|n| n.parse().ok()).unwrap_or(0);
    let limit: usize = r.get("limit").and_then(|n| n.parse().ok()).unwrap_or(5000);
    let filter = r.get("filter").unwrap_or("").to_string();
    let tick = s.tick;
    s.tick += 1;
    match action.as_str() {
        "ping" => Response::json(
            json!({"session_expire": "2099-01-01T00:00:00+00:00", "server": "6.6.0"}),
        ),
        "catalogs" => {
            Response::json(json!({"catalog": [{"id": "1", "name": "Music", "path": "/srv/music"}]}))
        }
        "albums" => {
            let list: Vec<Value> = s
                .albums
                .iter()
                .skip(offset)
                .take(limit)
                .map(|a| {
                    json!({"id": a.id, "name": a.name, "artist": {"id": "ar1", "name": a.artist},
                    "time": a.songs.len(), "year": a.year, "songcount": a.songs.len()})
                })
                .collect();
            Response::json(json!({"total_count": s.albums.len(), "album": list}))
        }
        "album_songs" => match s.albums.iter().find(|a| a.id == filter) {
            Some(a) => {
                let list: Vec<Value> = a
                    .songs
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|x| amp_song(a, x))
                    .collect();
                Response::json(json!({"song": list}))
            }
            None => amp_err("4704", &action, "Not Found"),
        },
        "stream" => match r.get("type") {
            Some("song") if r.get("id").and_then(|id| s.song(id)).is_some() => {
                Response::bytes("audio/flac", audio().to_vec())
            }
            Some("podcast_episode") => Response::bytes("audio/flac", audio().to_vec()),
            _ => amp_err("4704", &action, "Not Found"),
        },
        "get_art" => Response::bytes("image/png", png()),
        "playlists" => {
            let list: Vec<Value> = s
                .playlists
                .iter()
                .map(|p| json!({"id": p.id, "name": p.name, "owner": p.owner, "items": p.songs.len(), "type": "private"}))
                .chain(std::iter::once(json!({"id": "smart_1", "name": "Random", "owner": "admin", "items": 0, "type": "public"})))
                .collect();
            Response::json(json!({"playlist": list}))
        }
        "playlist_songs" => {
            if filter.starts_with("smart_") {
                return Response::json(json!({"song": []}));
            }
            let Some(p) = s.playlists.iter().find(|p| p.id == filter) else {
                return amp_err("4704", &action, "Not Found");
            };
            let list: Vec<Value> = p
                .songs
                .iter()
                .filter_map(|id| s.song(id).map(|(a, x)| amp_song(a, x)))
                .collect();
            Response::json(json!({"song": list}))
        }
        "playlist_create" => {
            s.next_id += 1;
            let id = format!("{}", s.next_id);
            s.playlists.push(MPlaylist {
                id: id.clone(),
                name: r.get("name").unwrap_or("").into(),
                owner: user.into(),
                songs: Vec::new(),
                changed: tick,
            });
            // API 6 answers with the playlist itself.
            Response::json(
                json!({"id": id, "name": r.get("name"), "owner": user, "items": 0, "type": "private"}),
            )
        }
        "playlist_add_song" => {
            let song = r.get("song").unwrap_or("").to_string();
            match s.playlists.iter_mut().find(|p| p.id == filter) {
                Some(p) => {
                    p.songs.push(song);
                    Response::json(json!({"success": "song added to playlist"}))
                }
                None => amp_err("4704", &action, "Not Found"),
            }
        }
        "playlist_remove_song" => match s.playlists.iter_mut().find(|p| p.id == filter) {
            Some(p) => {
                if r.get("clear") == Some("1") {
                    p.songs.clear();
                }
                Response::json(json!({"success": "playlist cleared"}))
            }
            None => amp_err("4704", &action, "Not Found"),
        },
        "playlist_edit" => match s.playlists.iter_mut().find(|p| p.id == filter) {
            Some(p) => {
                if let Some(n) = r.get("name") {
                    p.name = n.into();
                }
                Response::json(json!({"success": "playlist changes saved"}))
            }
            None => amp_err("4704", &action, "Not Found"),
        },
        "playlist_delete" => {
            s.playlists.retain(|p| p.id != filter);
            Response::json(json!({"success": "playlist deleted"}))
        }
        "live_streams" => Response::json(json!({"live_stream": [
            {"id": "1", "name": "Ampache FM", "url": "http://127.0.0.1:9/amp", "codec": "mp3"}
        ]})),
        "podcasts" => Response::json(json!({"podcast": [{
            "id": "5", "name": "Amp Pod", "description": "", "feed_url": "https://example.com/amp.xml",
            "podcast_episode": [
                {"id": "51", "title": "Pilot", "pubdate": "2024-05-01T10:00:00+00:00", "state": "completed",
                 "filename": "/srv/podcasts/pilot.mp3", "time": 1, "size": 1000}
            ]
        }]})),
        "bookmark_create" => {
            let pos: u64 = r.get("position").and_then(|p| p.parse().ok()).unwrap_or(0);
            s.bookmarks.insert(filter, pos);
            Response::json(json!({"bookmark": []}))
        }
        _ => amp_err("4705", &action, "Missing Method"),
    }
}

/// A config file with these servers in it, and the passwords in a
/// keyring-less secrets file next to it.
pub fn write_secret(path: &Path, server: &str, user: &str, secret: &str) {
    patina_core::secrets::Secrets::with(None, path.to_path_buf())
        .set(server, user, secret)
        .unwrap();
}

// ----- DLNA -----

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DlnaFlavor {
    /// MiniDLNA: "Browse Folders" at the top, the real folder tree in it.
    Folders,
    /// Jellyfin and friends: no folder view, but Search by class works.
    Search,
    /// Neither: Artists, Albums and All Tracks, every song under each.
    Tree,
}

pub struct DlnaState {
    pub lib: State,
    pub update_id: u32,
    /// "Browse <id>" and "Search" for every call.
    pub calls: Vec<String>,
}

pub struct Dlna {
    /// The description URL.
    pub url: String,
    pub udn: String,
    pub name: String,
    /// Where it answers SSDP searches.
    pub ssdp: std::net::SocketAddr,
    pub state: Arc<Mutex<DlnaState>>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A DIDL child: a container, or a song.
enum Node {
    Dir(String, String),
    Song(String, Box<(MAlbum, MSong)>),
}

impl Dlna {
    pub fn start(flavor: DlnaFlavor, name: &str) -> Self {
        let state = Arc::new(Mutex::new(DlnaState {
            lib: library(false),
            update_id: 1,
            calls: Vec::new(),
        }));
        let udn = format!("uuid:patina-test-{:?}-{}", flavor, name.len());
        let base = Arc::new(Mutex::new(String::new()));
        let st = state.clone();
        let (u, n, b) = (udn.clone(), name.to_string(), base.clone());
        let url = serve(Arc::new(move |r: &Request| {
            let base = b.lock().unwrap().clone();
            dlna(flavor, &st, &u, &n, &base, r)
        }));
        *base.lock().unwrap() = url.clone();
        let desc = format!("{url}/desc.xml");
        // SSDP, answered on a private address instead of the multicast group.
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let ssdp = sock.local_addr().unwrap();
        let (d, u) = (desc.clone(), udn.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                let msg = String::from_utf8_lossy(&buf[..n]);
                if msg.starts_with("M-SEARCH") && msg.contains("ssdp:discover") {
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: {d}\r\n\
                         SERVER: Linux UPnP/1.0 PatinaMock/1\r\n\
                         ST: urn:schemas-upnp-org:device:MediaServer:1\r\n\
                         USN: {u}::urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n"
                    );
                    let _ = sock.send_to(reply.as_bytes(), from);
                }
            }
        });
        Self {
            url: desc,
            udn,
            name: name.into(),
            ssdp,
            state,
        }
    }

    pub fn browses(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.starts_with("Browse") || c.starts_with("Search"))
            .count()
    }
}

fn soap_ok(action: &str, inner: &str) -> Response {
    Response::bytes(
        "text/xml; charset=\"utf-8\"",
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
             <u:{action}Response xmlns:u=\"urn:schemas-upnp-org:service:ContentDirectory:1\">{inner}\
             </u:{action}Response></s:Body></s:Envelope>"
        )
        .into_bytes(),
    )
}

fn soap_fault(code: u32, msg: &str) -> Response {
    Response {
        status: 500,
        ctype: "text/xml".into(),
        body: format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
             <s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail>\
             <UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>{code}</errorCode>\
             <errorDescription>{msg}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"
        )
        .into_bytes(),
    }
}

fn didl_item(flavor: DlnaFlavor, base: &str, id: &str, a: &MAlbum, s: &MSong) -> String {
    let num: String = s.id.chars().filter(char::is_ascii_digit).collect();
    // Search servers name the album artist with a role, like Jellyfin.
    let album_artist = if flavor == DlnaFlavor::Search {
        format!(
            "<upnp:artist role=\"AlbumArtist\">{}</upnp:artist>",
            esc(&a.artist)
        )
    } else {
        String::new()
    };
    // A converted copy first, to check the original wins.
    format!(
        "<item id=\"{id}\" parentID=\"p\" restricted=\"1\"><dc:title>{}</dc:title>\
         <upnp:class>object.item.audioItem.musicTrack</upnp:class>\
         <dc:creator>{}</dc:creator><upnp:artist>{}</upnp:artist>{album_artist}\
         <upnp:album>{}</upnp:album><upnp:genre>{}</upnp:genre><dc:date>{}-01-01</dc:date>\
         <upnp:originalTrackNumber>{}</upnp:originalTrackNumber>\
         <upnp:albumArtURI dlna:profileID=\"PNG_TN\">{base}/art/{}.png</upnp:albumArtURI>\
         <res protocolInfo=\"http-get:*:audio/mpeg:DLNA.ORG_PN=MP3;DLNA.ORG_CI=1\">{base}/transcode/{num}.mp3</res>\
         <res size=\"{}\" duration=\"0:00:01.000\" sampleFrequency=\"44100\" nrAudioChannels=\"2\" \
         protocolInfo=\"http-get:*:audio/x-flac:*\">{base}/media/{num}.flac</res></item>",
        esc(&s.title),
        esc(&s.artist),
        esc(&s.artist),
        esc(&a.name),
        esc(&a.genre),
        a.year,
        s.track,
        a.id,
        audio().len(),
    )
}

/// The children of a container, by flavor.
fn children(flavor: DlnaFlavor, lib: &State, id: &str) -> Option<Vec<Node>> {
    let dir = |id: &str, t: &str| Node::Dir(id.to_string(), t.to_string());
    let artists = || {
        let mut v: Vec<String> = lib.albums.iter().map(|a| a.artist.clone()).collect();
        v.dedup();
        v
    };
    let all_songs = |prefix: &str| {
        lib.albums
            .iter()
            .flat_map(|a| {
                a.songs.iter().map(move |s| {
                    Node::Song(
                        format!("{prefix}{}", s.id),
                        Box::new((a.clone(), s.clone())),
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    let album_songs = |prefix: &str, aid: &str| {
        lib.albums.iter().find(|a| a.id == aid).map(|a| {
            a.songs
                .iter()
                .map(|s| {
                    Node::Song(
                        format!("{prefix}{}", s.id),
                        Box::new((a.clone(), s.clone())),
                    )
                })
                .collect::<Vec<_>>()
        })
    };
    Some(match (flavor, id) {
        (DlnaFlavor::Folders, "0") => vec![
            dir("1", "Music"),
            dir("2", "Video"),
            dir("64", "Browse Folders"),
        ],
        (DlnaFlavor::Folders, "1") => vec![dir("1$7", "All Music")],
        (DlnaFlavor::Folders, "1$7") => all_songs("1$7$"),
        (DlnaFlavor::Folders, "64") => vec![
            dir("64$0", "Music"),
            dir("64$1", "Pictures"),
            dir("64$2", "Video"),
        ],
        (DlnaFlavor::Folders, "64$0") => artists()
            .iter()
            .enumerate()
            .map(|(i, a)| dir(&format!("64$0${i}"), a))
            .collect(),
        (DlnaFlavor::Folders, f) if f.starts_with("64$0$") => {
            let rest = &f["64$0$".len()..];
            match rest.split_once('$') {
                None => {
                    let artist = artists().get(rest.parse::<usize>().ok()?)?.clone();
                    lib.albums
                        .iter()
                        .filter(|a| a.artist == artist)
                        .map(|a| dir(&format!("{f}${}", a.id), &a.name))
                        .collect()
                }
                // Titled by file name, as Gerbera's folder view does; the
                // tags come from search.
                Some((_, aid)) => album_songs(&format!("{f}$"), aid)?
                    .into_iter()
                    .map(|n| match n {
                        Node::Song(id, mut b) => {
                            let file = b.1.path.rsplit('/').next().unwrap_or("").to_string();
                            b.1.title = file
                                .rsplit_once('.')
                                .map_or(file.clone(), |(s, _)| s.to_string());
                            Node::Song(id, b)
                        }
                        dir => dir,
                    })
                    .collect(),
            }
        }
        (DlnaFlavor::Search | DlnaFlavor::Tree, "0") => vec![dir("music", "Music")],
        (DlnaFlavor::Search, "music") => vec![dir("albums", "Albums")],
        (DlnaFlavor::Tree, "music") => vec![
            dir("artists", "Artists"),
            dir("albums", "Albums"),
            dir("all", "All Tracks"),
        ],
        (_, "albums") => lib
            .albums
            .iter()
            .map(|a| dir(&format!("album:{}", a.id), &a.name))
            .collect(),
        (_, a) if a.starts_with("album:") => album_songs("alb:", &a[6..])?,
        (DlnaFlavor::Tree, "artists") => artists()
            .iter()
            .enumerate()
            .map(|(i, a)| dir(&format!("artist:{i}"), a))
            .collect(),
        (DlnaFlavor::Tree, a) if a.starts_with("artist:") => {
            let artist = artists().get(a[7..].parse::<usize>().ok()?)?.clone();
            lib.albums
                .iter()
                .filter(|x| x.artist == artist)
                .map(|x| dir(&format!("album:{}", x.id), &x.name))
                .collect()
        }
        (DlnaFlavor::Tree, "all") => all_songs("all:"),
        _ => return None,
    })
}

fn dlna(
    flavor: DlnaFlavor,
    st: &Mutex<DlnaState>,
    udn: &str,
    name: &str,
    base: &str,
    r: &Request,
) -> Response {
    let path = r.path.as_str();
    if path == "/desc.xml" {
        let service = "<serviceList><service>\
             <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>\
             <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>\
             <controlURL>ctl/cd</controlURL><SCPDURL>/cd.xml</SCPDURL></service></serviceList>";
        let device = format!(
            "<device><deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>\
             <friendlyName>{}</friendlyName><manufacturer>Patina</manufacturer>\
             <modelName>Mock Media Server</modelName><modelNumber>1.0</modelNumber>\
             <UDN>{udn}</UDN>{service}</device>",
            esc(name)
        );
        // The search flavor nests the media server in another device and
        // sets URLBase, as some NAS boxes do.
        let body = if flavor == DlnaFlavor::Search {
            format!(
                "<?xml version=\"1.0\"?><root xmlns=\"urn:schemas-upnp-org:device-1-0\">\
                 <specVersion><major>1</major><minor>0</minor></specVersion><URLBase>{base}/</URLBase>\
                 <device><deviceType>urn:schemas-upnp-org:device:Basic:1</deviceType>\
                 <friendlyName>NAS box</friendlyName><UDN>uuid:outer</UDN>\
                 <deviceList>{device}</deviceList></device></root>"
            )
        } else {
            format!(
                "<?xml version=\"1.0\"?><root xmlns=\"urn:schemas-upnp-org:device-1-0\">\
                 <specVersion><major>1</major><minor>0</minor></specVersion>{device}</root>"
            )
        };
        return Response::bytes("text/xml", body.into_bytes());
    }
    if let Some(n) = path.strip_prefix("/media/") {
        st.lock().unwrap().calls.push(format!("GET {n}"));
        return Response::bytes("audio/x-flac", (*audio()).clone());
    }
    if path.starts_with("/transcode/") {
        return Response::not_found();
    }
    if path.starts_with("/art/") {
        return Response::bytes("image/png", png());
    }
    // The control URL is relative to the description.
    if path != "/ctl/cd" || r.method != "POST" {
        return Response::not_found();
    }
    let action = r
        .headers
        .get("soapaction")
        .and_then(|a| {
            a.trim_matches('"')
                .rsplit_once('#')
                .map(|(_, a)| a.to_string())
        })
        .unwrap_or_default();
    let body = String::from_utf8_lossy(&r.body).to_string();
    let Ok(doc) = roxmltree::Document::parse(&body) else {
        return soap_fault(401, "Invalid Action");
    };
    let arg = |n: &str| {
        doc.descendants()
            .find(|x| x.tag_name().name() == n)
            .and_then(|x| x.text())
            .unwrap_or("")
            .to_string()
    };
    let mut s = st.lock().unwrap();
    match action.as_str() {
        "GetSystemUpdateID" => soap_ok(&action, &format!("<Id>{}</Id>", s.update_id)),
        "GetSearchCapabilities" => soap_ok(
            &action,
            match flavor {
                DlnaFlavor::Tree => "<SearchCaps></SearchCaps>",
                _ => "<SearchCaps>dc:title,upnp:class,upnp:artist</SearchCaps>",
            },
        ),
        "Browse" | "Search" => {
            let nodes = if action == "Browse" {
                let id = arg("ObjectID");
                s.calls.push(format!("Browse {id}"));
                match children(flavor, &s.lib, &id) {
                    Some(n) => n,
                    None => return soap_fault(701, "No such object"),
                }
            } else {
                s.calls.push("Search".into());
                if flavor == DlnaFlavor::Tree {
                    return soap_fault(708, "Unsupported or invalid search criteria");
                }
                assert!(arg("SearchCriteria").contains("object.item.audioItem"));
                children(flavor, &s.lib, "albums")
                    .unwrap()
                    .into_iter()
                    .flat_map(|n| match n {
                        Node::Dir(id, _) => children(flavor, &s.lib, &id).unwrap(),
                        song => vec![song],
                    })
                    .collect()
            };
            let start: usize = arg("StartingIndex").parse().unwrap_or(0);
            let want: usize = arg("RequestedCount").parse().unwrap_or(0);
            // Two at a time at most, as servers may, so paging gets used.
            let want = if want == 0 { 2 } else { want.min(2) };
            let page: Vec<&Node> = nodes.iter().skip(start).take(want).collect();
            let mut didl = String::from(
                "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
                 xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                 xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\" \
                 xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\">",
            );
            for n in &page {
                match n {
                    Node::Dir(id, t) => didl.push_str(&format!(
                        "<container id=\"{}\" parentID=\"p\" restricted=\"1\"><dc:title>{}</dc:title>\
                         <upnp:class>object.container.storageFolder</upnp:class></container>",
                        esc(id),
                        esc(t)
                    )),
                    Node::Song(id, b) => didl.push_str(&didl_item(flavor, base, id, &b.0, &b.1)),
                }
            }
            didl.push_str("</DIDL-Lite>");
            soap_ok(
                &action,
                &format!(
                    "<Result>{}</Result><NumberReturned>{}</NumberReturned>\
                     <TotalMatches>{}</TotalMatches><UpdateID>{}</UpdateID>",
                    esc(&didl),
                    page.len(),
                    nodes.len(),
                    s.update_id
                ),
            )
        }
        _ => soap_fault(401, "Invalid Action"),
    }
}
