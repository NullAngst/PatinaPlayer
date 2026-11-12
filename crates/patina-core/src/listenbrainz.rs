//! ListenBrainz: Now Playing and listens, alongside Last.fm or instead of
//! it.
//!
//! The API is a user token in a header and JSON in the body, so there's
//! no linking dance: paste the token from listenbrainz.org/settings in
//! Settings, the app checks it and keeps it in the keyring. Listens go
//! into the library database first and leave once ListenBrainz has taken
//! them, in batches of up to 100 with the time each started, so plays
//! made offline still count. Any server that speaks the same API (a
//! self-hosted ListenBrainz, Maloja, Koito) works by changing the address.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use rusqlite::params;
use serde_json::json;

use crate::lastfm::Play;
use crate::library::Library;
use crate::secrets::Secrets;

/// The public ListenBrainz API.
pub const API: &str = "https://api.listenbrainz.org";
/// Where the token is kept in the secrets store.
pub const SECRET_NAME: &str = "listenbrainz";
/// The keyring entry's user. ListenBrainz has one token per account, and
/// the account name is only known once the token's been checked.
const SECRET_USER: &str = "token";
const BATCH: usize = 100;
const RETRY_MIN: Duration = Duration::from_secs(30);
const RETRY_MAX: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LbStatus {
    /// The account the token belongs to; empty with no working token.
    pub user: String,
    /// A token is saved, checked or not.
    pub has_token: bool,
    pub waiting: i64,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LbEvent {
    /// The token works, for this account.
    Valid(String),
    /// The token was refused, or couldn't be checked.
    Invalid(String),
    Error(String),
}

enum Cmd {
    NowPlaying(Play),
    Listen(Play),
    /// A new token, or None to forget it.
    Token(Option<String>),
    Url(String),
    Flush,
}

#[derive(Debug, thiserror::Error)]
pub enum LbError {
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    /// The token isn't accepted.
    #[error("ListenBrainz doesn't accept the token: {0}")]
    Auth(String),
    /// Too many requests, or the server's having trouble: try later.
    #[error("ListenBrainz is busy: {0}")]
    Busy(String),
    #[error("ListenBrainz said no: {0}")]
    Rejected(String),
}

/// The HTTP side, on its own so tests can point it at a stand-in.
pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    token: String,
}

impl Client {
    pub fn new(base: &str, token: &str) -> Result<Self, LbError> {
        Ok(Self {
            http: crate::net::client_builder()
                .timeout(Duration::from_secs(20))
                .build()?,
            base: base.trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
        })
    }

    fn check(resp: reqwest::blocking::Response) -> Result<serde_json::Value, LbError> {
        let status = resp.status();
        let body: serde_json::Value = resp
            .text()
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(serde_json::Value::Null);
        let msg = body
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or(status.canonical_reason().unwrap_or("error"))
            .to_string();
        match status.as_u16() {
            200..=299 => Ok(body),
            401 | 403 => Err(LbError::Auth(msg)),
            429 | 500..=599 => Err(LbError::Busy(msg)),
            _ => Err(LbError::Rejected(msg)),
        }
    }

    /// The account the token belongs to.
    pub fn validate(&self) -> Result<String, LbError> {
        let resp = self
            .http
            .get(format!("{}/1/validate-token", self.base))
            .header("Authorization", format!("Token {}", self.token))
            .send()?;
        let body = Self::check(resp)?;
        if body.get("valid").and_then(|v| v.as_bool()) == Some(false) {
            return Err(LbError::Auth(
                body.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("invalid token")
                    .to_string(),
            ));
        }
        Ok(body
            .get("user_name")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string())
    }

    fn submit(&self, kind: &str, plays: &[Play]) -> Result<(), LbError> {
        let payload: Vec<serde_json::Value> = plays
            .iter()
            .map(|p| {
                let mut info = json!({
                    "media_player": "PatinaPlayer",
                    "submission_client": "PatinaPlayer",
                    "submission_client_version": env!("CARGO_PKG_VERSION"),
                });
                if let Some(d) = p.duration {
                    info["duration_ms"] = json!(d as u64 * 1000);
                }
                if let Some(n) = p.track_no {
                    info["tracknumber"] = json!(n);
                }
                let mut meta = json!({
                    "artist_name": p.artist,
                    "track_name": p.track,
                    "additional_info": info,
                });
                if let Some(a) = &p.album {
                    meta["release_name"] = json!(a);
                }
                let mut l = json!({ "track_metadata": meta });
                if kind != "playing_now" {
                    l["listened_at"] = json!(p.timestamp);
                }
                l
            })
            .collect();
        let resp = self
            .http
            .post(format!("{}/1/submit-listens", self.base))
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "application/json")
            .body(json!({ "listen_type": kind, "payload": payload }).to_string())
            .send()?;
        Self::check(resp).map(|_| ())
    }

    pub fn now_playing(&self, p: &Play) -> Result<(), LbError> {
        self.submit("playing_now", std::slice::from_ref(p))
    }

    /// One listen as "single", several as "import", as the API asks.
    pub fn listens(&self, plays: &[Play]) -> Result<(), LbError> {
        self.submit(if plays.len() == 1 { "single" } else { "import" }, plays)
    }
}

impl Library {
    pub fn queue_listen(&self, p: &Play) -> crate::library::Result<()> {
        self.conn.execute(
            "INSERT INTO lb_listens (artist, track, album, track_no, duration, played_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                p.artist,
                p.track,
                p.album,
                p.track_no.map(i64::from),
                p.duration.map(i64::from),
                p.timestamp
            ],
        )?;
        Ok(())
    }

    pub fn listens_waiting(&self) -> crate::library::Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM lb_listens", [], |r| r.get(0))?)
    }

    /// The oldest listens waiting, with their row ids.
    pub fn waiting_listens(&self, n: usize) -> crate::library::Result<Vec<(i64, Play)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, artist, track, album, track_no, duration, played_at \
             FROM lb_listens ORDER BY played_at, id LIMIT ?1",
        )?;
        let rows = st.query_map([n as i64], |r| {
            Ok((
                r.get(0)?,
                Play {
                    artist: r.get(1)?,
                    track: r.get(2)?,
                    album: r.get(3)?,
                    album_artist: None,
                    track_no: r.get::<_, Option<i64>>(4)?.map(|n| n as u32),
                    duration: r.get::<_, Option<i64>>(5)?.map(|n| n as u32),
                    timestamp: r.get(6)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn drop_listens(&self, ids: &[i64]) -> crate::library::Result<()> {
        for id in ids {
            self.conn
                .execute("DELETE FROM lb_listens WHERE id = ?1", [id])?;
        }
        Ok(())
    }
}

pub struct ListenBrainz {
    tx: Option<Sender<Cmd>>,
    events: Receiver<LbEvent>,
    status: Arc<Mutex<LbStatus>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ListenBrainz {
    /// Start the thread. `url` empty means the public ListenBrainz.
    pub fn start(
        db: &Path,
        url: &str,
        user: &str,
        secrets: Arc<Secrets>,
    ) -> crate::library::Result<Self> {
        let lib = Library::open(db)?;
        let (tx, rx) = unbounded();
        let (etx, events) = unbounded();
        let status = Arc::new(Mutex::new(LbStatus::default()));
        let token = secrets.get(SECRET_NAME, SECRET_USER).ok().flatten();
        let mut w = Worker {
            lib,
            rx,
            events: etx,
            status: status.clone(),
            secrets,
            base: base_of(url),
            token,
            user: user.to_string(),
            retry_at: Some(Instant::now()),
            backoff: RETRY_MIN,
            refused: false,
        };
        w.publish(None);
        let join = std::thread::Builder::new()
            .name("patina-listenbrainz".into())
            .spawn(move || w.run())
            .expect("spawn ListenBrainz thread");
        Ok(Self {
            tx: Some(tx),
            events,
            status,
            join: Some(join),
        })
    }

    fn send(&self, c: Cmd) {
        if let Some(t) = &self.tx {
            let _ = t.send(c);
        }
    }

    pub fn now_playing(&self, p: Play) {
        self.send(Cmd::NowPlaying(p));
    }

    /// Queue a listen, and send it as soon as ListenBrainz takes it.
    pub fn listen(&self, p: Play) {
        self.send(Cmd::Listen(p));
    }

    /// Check and keep a token; an empty one forgets it.
    pub fn set_token(&self, token: &str) {
        let t = token.trim();
        self.send(Cmd::Token((!t.is_empty()).then(|| t.to_string())));
    }

    pub fn set_url(&self, url: &str) {
        self.send(Cmd::Url(url.to_string()));
    }

    pub fn flush(&self) {
        self.send(Cmd::Flush);
    }

    pub fn events(&self) -> &Receiver<LbEvent> {
        &self.events
    }

    pub fn status(&self) -> LbStatus {
        self.status.lock().unwrap().clone()
    }

    /// Whether listens go anywhere: a token that hasn't been refused.
    pub fn live(&self) -> bool {
        let s = self.status();
        s.has_token && !s.user.is_empty()
    }
}

impl Drop for ListenBrainz {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn base_of(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() {
        API.to_string()
    } else {
        u.trim_end_matches('/').to_string()
    }
}

struct Worker {
    lib: Library,
    rx: Receiver<Cmd>,
    events: Sender<LbEvent>,
    status: Arc<Mutex<LbStatus>>,
    secrets: Arc<Secrets>,
    base: String,
    token: Option<String>,
    /// The account, as last checked. Kept in the config so it shows
    /// before the check comes back.
    user: String,
    retry_at: Option<Instant>,
    backoff: Duration,
    /// The token was refused: nothing goes out until a new one's given.
    refused: bool,
}

impl Worker {
    fn emit(&self, e: LbEvent) {
        let _ = self.events.send(e);
    }

    fn publish(&self, problem: Option<String>) {
        let waiting = self.lib.listens_waiting().unwrap_or(0);
        let mut s = self.status.lock().unwrap();
        s.has_token = self.token.is_some();
        s.user = if self.token.is_some() && !self.refused {
            self.user.clone()
        } else {
            String::new()
        };
        s.waiting = waiting;
        if problem.is_some() || waiting == 0 {
            s.problem = problem;
        }
    }

    fn client(&self) -> Option<Client> {
        let t = self.token.as_ref().filter(|_| !self.refused)?;
        Client::new(&self.base, t).ok()
    }

    fn run(&mut self) {
        loop {
            let now = Instant::now();
            let wake = self.retry_at.unwrap_or(now + Duration::from_secs(3600));
            match self.rx.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(c) => self.handle(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.retry_at.is_some_and(|r| Instant::now() >= r) {
                self.retry_at = None;
                self.flush();
            }
        }
    }

    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::NowPlaying(p) => {
                if let Some(c) = self.client()
                    && let Err(e) = c.now_playing(&p)
                {
                    log::info!("ListenBrainz now playing: {e}");
                    self.on_error(&e);
                }
            }
            Cmd::Listen(p) => {
                if let Err(e) = self.lib.queue_listen(&p) {
                    self.emit(LbEvent::Error(format!("couldn't queue a listen: {e}")));
                }
                if self.retry_at.is_none() {
                    self.flush();
                } else {
                    self.publish(None);
                }
            }
            Cmd::Token(None) => {
                let _ = self.secrets.delete(SECRET_NAME, SECRET_USER);
                self.token = None;
                self.user.clear();
                self.refused = false;
                self.publish(None);
                self.emit(LbEvent::Invalid(String::new()));
            }
            Cmd::Token(Some(t)) => {
                self.check_token(&t);
            }
            Cmd::Url(u) => {
                self.base = base_of(&u);
                self.refused = false;
                if let Some(t) = self.token.clone() {
                    self.check_token(&t);
                }
            }
            Cmd::Flush => {
                self.retry_at = None;
                self.backoff = RETRY_MIN;
                self.flush();
            }
        }
    }

    fn check_token(&mut self, t: &str) {
        let res = Client::new(&self.base, t).and_then(|c| c.validate());
        match res {
            Ok(user) => {
                if let Err(e) = self.secrets.set(SECRET_NAME, SECRET_USER, t) {
                    self.emit(LbEvent::Error(format!("couldn't save the token: {e}")));
                }
                self.token = Some(t.to_string());
                self.user = user.clone();
                self.refused = false;
                self.publish(None);
                self.emit(LbEvent::Valid(user));
                self.retry_at = Some(Instant::now());
            }
            Err(LbError::Auth(why)) => {
                self.publish(Some(why.clone()));
                self.emit(LbEvent::Invalid(format!(
                    "ListenBrainz doesn't accept that token: {why}"
                )));
            }
            Err(e) => {
                // Offline, most likely. Keep it and check again later.
                let _ = self.secrets.set(SECRET_NAME, SECRET_USER, t);
                self.token = Some(t.to_string());
                self.publish(Some(e.to_string()));
                self.emit(LbEvent::Error(format!(
                    "Couldn't check the token with ListenBrainz ({e}). It's kept; listens wait until it can be checked."
                )));
                self.retry_at = Some(Instant::now() + self.backoff);
            }
        }
    }

    fn on_error(&mut self, e: &LbError) {
        if let LbError::Auth(why) = e {
            self.refused = true;
            self.publish(Some(why.clone()));
            self.emit(LbEvent::Invalid(format!(
                "ListenBrainz stopped accepting the token: {why}. Paste a new one in Settings."
            )));
        }
    }

    fn flush(&mut self) {
        // No token, or a refused one: nothing to send, and whatever's
        // shown about why stays shown.
        let Some(client) = self.client() else {
            return;
        };
        // The account name, if it isn't known yet (the token was kept
        // while offline).
        if self.user.is_empty() {
            match client.validate() {
                Ok(u) => {
                    self.user = u.clone();
                    self.emit(LbEvent::Valid(u));
                }
                Err(e) => {
                    self.on_error(&e);
                    if !self.refused {
                        self.later(e.to_string());
                    }
                    return;
                }
            }
        }
        loop {
            let batch = match self.lib.waiting_listens(BATCH) {
                Ok(b) => b,
                Err(e) => {
                    self.publish(Some(e.to_string()));
                    return;
                }
            };
            if batch.is_empty() {
                self.backoff = RETRY_MIN;
                self.publish(None);
                return;
            }
            let ids: Vec<i64> = batch.iter().map(|(id, _)| *id).collect();
            let plays: Vec<Play> = batch.into_iter().map(|(_, p)| p).collect();
            match client.listens(&plays) {
                Ok(()) => {
                    let _ = self.lib.drop_listens(&ids);
                }
                Err(LbError::Rejected(why)) => {
                    // Something in the batch it'll never take. Drop the
                    // oldest one and carry on, so one bad tag can't hold
                    // up every listen after it.
                    log::warn!("ListenBrainz refused a listen: {why}");
                    let _ = self.lib.drop_listens(&ids[..1]);
                }
                Err(e) => {
                    self.on_error(&e);
                    if !self.refused {
                        self.later(e.to_string());
                    }
                    return;
                }
            }
        }
    }

    fn later(&mut self, why: String) {
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(RETRY_MAX);
        self.publish(Some(why));
    }
}
