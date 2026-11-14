//! Remote control over the local network: PatinaPlayer on a phone (or
//! another computer) driving this one. What's playing and its cover,
//! play, pause, skip, seek, volume, shuffle, repeat, the heart, and the
//! queue, where picking a song plays it.
//!
//! The player being controlled runs a [`Server`]: a small HTTP server
//! speaking JSON, off until you turn it on, and only for whoever gives
//! its code. It announces itself with mDNS (`_patinaplayer._tcp`), so the
//! other side can list it without typing an address. The other side is a
//! [`Client`].
//!
//! The code keeps out the curious on a shared network, nothing more: it
//! crosses the network in the clear, as everything on plain HTTP does.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};

use crate::player::Player;
use crate::queue::Repeat;
use crate::track::MediaKind;
use crate::{Command, PlayState};

const SERVICE: &str = "_patinaplayer._tcp.local.";
/// The most of the queue the other side gets: around the song playing.
const QUEUE_BACK: usize = 20;
const QUEUE_AHEAD: usize = 280;

/// What's playing, as the other side shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The controlled player's name: its machine's.
    pub name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// "playing", "paused" or "stopped".
    pub state: String,
    pub position_ms: u64,
    /// 0 for a live stream.
    pub duration_ms: u64,
    /// Slider position, 0 to 1.
    pub volume: f64,
    pub muted: bool,
    pub shuffle: bool,
    /// "off", "all" or "one".
    pub repeat: String,
    /// Hearted, not, or can't be.
    pub loved: Option<bool>,
    pub live: bool,
    /// Changes when the cover does; empty with none.
    pub art: String,
    /// Changes when the queue does.
    pub queue: u64,
    pub current: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: u64,
    pub title: String,
    pub artist: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Queue {
    pub current: Option<u64>,
    pub items: Vec<QueueItem>,
}

/// What the other side can ask for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    Toggle,
    Next,
    Prev,
    Seek { ms: u64 },
    Volume { value: f64 },
    Mute,
    Shuffle,
    Repeat,
    Love,
    PlayEntry { id: u64 },
}

/// Asks that need more than the player: the browser does them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Love,
}

/// This machine's name, for the other side's list.
pub fn machine_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "localhost")
        .unwrap_or_else(|| {
            if cfg!(target_os = "android") {
                "Android phone".into()
            } else {
                "PatinaPlayer".into()
            }
        })
}

/// A fresh six-digit code.
pub fn new_code() -> String {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the system's randomness.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:06}", h.finish() % 1_000_000)
}

// ----- The server -----

/// Wrong codes lately: after too many, everyone waits a minute.
struct Guard {
    misses: u32,
    since: Instant,
}

struct ServerCtx {
    player: Player,
    code: String,
    name: String,
    asks: Sender<Ask>,
    loved: Arc<Mutex<Option<bool>>>,
    guard: Mutex<Guard>,
}

/// Lets other players control this one. Stops when dropped.
pub struct Server {
    stop: Arc<AtomicBool>,
    port: u16,
    mdns: Option<(mdns_sd::ServiceDaemon, String)>,
}

impl Server {
    /// Listen on `port` (0 for any free one) on every address, and say so
    /// on the network. `loved` is the playing song's heart, kept up to
    /// date by the browser; `asks` takes what only it can do.
    pub fn start(
        player: Player,
        port: u16,
        code: &str,
        asks: Sender<Ask>,
        loved: Arc<Mutex<Option<bool>>>,
    ) -> std::io::Result<Server> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let name = machine_name();
        let ctx = Arc::new(ServerCtx {
            player,
            code: code.to_string(),
            name: name.clone(),
            asks,
            loved,
            guard: Mutex::new(Guard {
                misses: 0,
                since: Instant::now(),
            }),
        });
        let s2 = stop.clone();
        std::thread::Builder::new()
            .name("patina-remote".into())
            .spawn(move || {
                while !s2.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((sock, _)) => {
                            let ctx = ctx.clone();
                            let _ = std::thread::Builder::new()
                                .name("patina-remote-req".into())
                                .spawn(move || {
                                    let _ = sock.set_nonblocking(false);
                                    handle(sock, &ctx);
                                });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(200)),
                    }
                }
            })?;
        let mdns = advertise(&name, port);
        log::info!("remote control: listening on port {port}");
        Ok(Server { stop, port, mdns })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

fn advertise(name: &str, port: u16) -> Option<(mdns_sd::ServiceDaemon, String)> {
    let daemon = mdns_sd::ServiceDaemon::new().ok()?;
    let host = format!(
        "{}.local.",
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    );
    let info = mdns_sd::ServiceInfo::new(SERVICE, name, &host, "", port, &[("name", name)][..])
        .ok()?
        .enable_addr_auto();
    let full = info.get_fullname().to_string();
    match daemon.register(info) {
        Ok(()) => Some((daemon, full)),
        Err(e) => {
            log::info!("remote control: can't announce it on the network: {e}");
            let _ = daemon.shutdown();
            None
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some((d, full)) = self.mdns.take() {
            let _ = d.unregister(&full);
            let _ = d.shutdown();
        }
    }
}

struct Request {
    method: String,
    path: String,
    code: Option<String>,
    body: Vec<u8>,
}

fn read_request(sock: &TcpStream) -> Option<Request> {
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new(sock);
    let mut first = String::new();
    r.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let (mut code, mut len) = (None, 0usize);
    for _ in 0..100 {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("x-code") {
                code = Some(v.to_string());
            } else if k.eq_ignore_ascii_case("content-length") {
                len = v.parse().unwrap_or(0);
            }
        }
    }
    if len > 64 * 1024 {
        return None;
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        code,
        body,
    })
}

fn respond(sock: &mut TcpStream, status: &str, kind: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = sock.write_all(head.as_bytes());
    let _ = sock.write_all(body);
}

fn json<T: Serialize>(sock: &mut TcpStream, v: &T) {
    let body = serde_json::to_vec(v).unwrap_or_default();
    respond(sock, "200 OK", "application/json", &body);
}

/// The same answer however much of the code is right, so timing gives
/// nothing away.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn handle(mut sock: TcpStream, ctx: &ServerCtx) {
    let Some(req) = read_request(&sock) else {
        return;
    };
    let path = req.path.split('?').next().unwrap_or("");
    // Who's there: no code needed, so a list can show the name.
    if path == "/api/hello" {
        return json(
            &mut sock,
            &serde_json::json!({ "app": "PatinaPlayer", "name": ctx.name }),
        );
    }
    {
        let mut g = ctx.guard.lock().unwrap();
        if g.since.elapsed() > Duration::from_secs(60) {
            g.misses = 0;
            g.since = Instant::now();
        }
        if g.misses >= 10 {
            drop(g);
            return respond(
                &mut sock,
                "429 Too Many Requests",
                "text/plain",
                b"wait a minute",
            );
        }
        let ok = req.code.as_deref().is_some_and(|c| same(c, &ctx.code));
        if !ok {
            g.misses += 1;
            drop(g);
            std::thread::sleep(Duration::from_millis(500));
            return respond(&mut sock, "401 Unauthorized", "text/plain", b"wrong code");
        }
    }
    match (req.method.as_str(), path) {
        ("GET", "/api/state") => json(&mut sock, &state(ctx)),
        ("GET", "/api/queue") => json(&mut sock, &queue(&ctx.player)),
        ("GET", "/api/art") => match art_file(&ctx.player) {
            Some((bytes, kind)) => respond(&mut sock, "200 OK", kind, &bytes),
            None => respond(&mut sock, "404 Not Found", "text/plain", b""),
        },
        ("POST", "/api/cmd") => match serde_json::from_slice::<Cmd>(&req.body) {
            Ok(c) => {
                run(ctx, c);
                json(&mut sock, &serde_json::json!({ "ok": true }));
            }
            Err(e) => respond(
                &mut sock,
                "400 Bad Request",
                "text/plain",
                e.to_string().as_bytes(),
            ),
        },
        _ => respond(&mut sock, "404 Not Found", "text/plain", b""),
    }
}

fn state(ctx: &ServerCtx) -> State {
    let p = &ctx.player;
    let snap = p.snapshot();
    let q = p.queue();
    let (title, artist, album, art) = match &snap.track {
        Some(t) => {
            let title = match (&snap.live_title, t.kind) {
                (Some(lt), MediaKind::Radio) if !lt.trim().is_empty() => lt.clone(),
                _ => t.display_title(),
            };
            let art = if t.path.as_os_str().is_empty() {
                String::new()
            } else {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                t.path.hash(&mut h);
                format!("{:016x}", h.finish())
            };
            (
                title,
                t.artist.clone().unwrap_or_default(),
                t.album.clone().unwrap_or_default(),
                art,
            )
        }
        None => Default::default(),
    };
    State {
        name: ctx.name.clone(),
        title,
        artist,
        album,
        state: match snap.state {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Stopped => "stopped",
        }
        .into(),
        position_ms: snap.position.as_millis() as u64,
        duration_ms: if snap.live {
            0
        } else {
            snap.duration.map_or(0, |d| d.as_millis() as u64)
        },
        volume: snap.volume,
        muted: snap.muted,
        shuffle: snap.shuffle,
        repeat: match snap.repeat {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
        .into(),
        loved: *ctx.loved.lock().unwrap(),
        live: snap.live,
        art,
        queue: q.version,
        current: snap.current,
    }
}

fn queue(p: &Player) -> Queue {
    let current = p.snapshot().current;
    let q = p.queue();
    let order = q.play_order();
    let at = current.and_then(|c| q.position(c)).unwrap_or(0);
    let from = at.saturating_sub(QUEUE_BACK);
    let to = (at + QUEUE_AHEAD + 1).min(order.len());
    Queue {
        current,
        items: order[from.min(to)..to]
            .iter()
            .filter_map(|&id| q.get(id))
            .map(|e| QueueItem {
                id: e.id,
                title: e.info.display_title(),
                artist: e.info.artist.clone().unwrap_or_default(),
            })
            .collect(),
    }
}

fn art_file(p: &Player) -> Option<(Vec<u8>, &'static str)> {
    let t = p.snapshot().track?;
    if t.path.as_os_str().is_empty() {
        return None;
    }
    let f = crate::art::cached_file(&t.path, 600)?;
    let kind = match f.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        _ => "image/jpeg",
    };
    Some((std::fs::read(f).ok()?, kind))
}

fn run(ctx: &ServerCtx, c: Cmd) {
    let p = &ctx.player;
    match c {
        Cmd::Toggle => p.send(Command::TogglePause),
        Cmd::Next => p.send(Command::Next),
        Cmd::Prev => p.send(Command::Back),
        Cmd::Seek { ms } => p.send(Command::Seek(Duration::from_millis(ms))),
        Cmd::Volume { value } => p.send(Command::SetVolume(value.clamp(0.0, 1.0))),
        Cmd::Mute => p.send(Command::ToggleMute),
        Cmd::Shuffle => p.send(Command::ToggleShuffle),
        Cmd::Repeat => p.send(Command::CycleRepeat),
        Cmd::PlayEntry { id } => p.send(Command::PlayEntry(id)),
        Cmd::Love => {
            let _ = ctx.asks.send(Ask::Love);
        }
    }
}

// ----- The client -----

/// A player on the network that can be controlled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    /// "host:port".
    pub addr: String,
}

/// Players that announced themselves within `wait`.
pub fn discover(wait: Duration) -> Vec<Found> {
    let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
        return Vec::new();
    };
    let Ok(rx) = daemon.browse(SERVICE) else {
        let _ = daemon.shutdown();
        return Vec::new();
    };
    let deadline = Instant::now() + wait;
    let mut out: Vec<Found> = Vec::new();
    let me = machine_name();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(s)) => {
                let Some(ip) = s.get_addresses_v4().into_iter().next() else {
                    continue;
                };
                let name = s
                    .get_property_val_str("name")
                    .map(str::to_string)
                    .unwrap_or_else(|| s.host.trim_end_matches(".local.").to_string());
                let addr = format!("{ip}:{}", s.port);
                if name != me && !out.iter().any(|f| f.addr == addr) {
                    out.push(Found { name, addr });
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    out
}

/// Talks to a [`Server`].
#[derive(Clone)]
pub struct Client {
    base: String,
    code: String,
    http: reqwest::blocking::Client,
}

impl Client {
    /// `addr` is "host" or "host:port".
    pub fn new(addr: &str, code: &str) -> Result<Client, String> {
        let addr = addr.trim();
        let addr = if addr
            .rsplit_once(':')
            .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        {
            addr.to_string()
        } else {
            format!("{addr}:{}", crate::config::REMOTE_PORT)
        };
        let http = crate::net::client_builder()
            .timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Client {
            base: format!("http://{addr}"),
            code: code.trim().to_string(),
            http,
        })
    }

    /// "host:port", as given to [`Client::new`].
    pub fn addr(&self) -> &str {
        self.base.trim_start_matches("http://")
    }

    fn get(&self, path: &str) -> Result<reqwest::blocking::Response, String> {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .header("X-Code", &self.code)
            .send()
            .map_err(|e| format!("can't reach {}: {}", self.addr(), root_cause(&e)))?;
        check(r)
    }

    pub fn state(&self) -> Result<State, String> {
        let body = self.get("/api/state")?.bytes().map_err(|e| e.to_string())?;
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }

    pub fn queue(&self) -> Result<Queue, String> {
        let body = self.get("/api/queue")?.bytes().map_err(|e| e.to_string())?;
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }

    /// The cover of what's playing, if it has one.
    pub fn art(&self) -> Result<Option<Vec<u8>>, String> {
        let r = self
            .http
            .get(format!("{}/api/art", self.base))
            .header("X-Code", &self.code)
            .send()
            .map_err(|e| e.to_string())?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let r = check(r)?;
        r.bytes()
            .map(|b| Some(b.to_vec()))
            .map_err(|e| e.to_string())
    }

    pub fn send(&self, c: &Cmd) -> Result<(), String> {
        let r = self
            .http
            .post(format!("{}/api/cmd", self.base))
            .header("X-Code", &self.code)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(c).map_err(|e| e.to_string())?)
            .send()
            .map_err(|e| format!("can't reach {}: {}", self.addr(), root_cause(&e)))?;
        check(r).map(|_| ())
    }
}

fn check(r: reqwest::blocking::Response) -> Result<reqwest::blocking::Response, String> {
    match r.status().as_u16() {
        200..=299 => Ok(r),
        401 => Err("wrong code".into()),
        429 => Err("too many wrong codes: wait a minute".into()),
        s => Err(format!("it answered {s}")),
    }
}

fn root_cause(e: &reqwest::Error) -> String {
    let mut src: &dyn std::error::Error = e;
    while let Some(s) = src.source() {
        src = s;
    }
    src.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_read_as_they_are_sent() {
        for c in [
            Cmd::Toggle,
            Cmd::Seek { ms: 61_000 },
            Cmd::Volume { value: 0.5 },
            Cmd::PlayEntry { id: 7 },
        ] {
            let s = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Cmd>(&s).unwrap(), c);
        }
        assert_eq!(
            serde_json::from_str::<Cmd>(r#"{"cmd":"next"}"#).unwrap(),
            Cmd::Next
        );
    }

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..20 {
            let c = new_code();
            assert_eq!(c.len(), 6);
            assert!(c.chars().all(|c| c.is_ascii_digit()));
        }
        assert!(same("123456", "123456"));
        assert!(!same("123456", "123457"));
        assert!(!same("123456", "12345"));
    }
}
