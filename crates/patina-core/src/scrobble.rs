//! The Last.fm thread: linking your account, Now Playing, the scrobble
//! queue, and love.
//!
//! Every scrobble goes into the library database first and leaves only
//! once Last.fm has taken it, so plays made offline (or while Last.fm is
//! down) are sent later in batches of up to 50, each with the time it was
//! played. A network error or a busy Last.fm means try again later, a
//! little later each time; a session Last.fm no longer accepts means link
//! again, and the queue waits for that.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::lastfm::{LastFm, LastFmError, Play};
use crate::library::Library;
use crate::secrets::Secrets;

/// Where the session key is kept in the secrets store.
pub const SECRET_NAME: &str = "last.fm";
/// How long linking waits for you to approve.
const LINK_TIMEOUT: Duration = Duration::from_secs(300);
const LINK_POLL: Duration = Duration::from_secs(3);
const RETRY_MIN: Duration = Duration::from_secs(30);
const RETRY_MAX: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LastFmSetup {
    pub key: String,
    pub secret: String,
    /// Linked user, if any.
    pub user: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScrobbleEvent {
    /// Approve the app at this address (it's been opened in the browser).
    Approve(String),
    Linked(String),
    LinkFailed(String),
    /// The session ended (unlinked here, or Last.fm dropped it).
    Unlinked(String),
    /// Whether a track is loved on Last.fm.
    Loved {
        artist: String,
        track: String,
        on: bool,
    },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScrobbleStatus {
    /// Linked user name; empty when not linked.
    pub user: String,
    /// Waiting for you to approve in the browser.
    pub linking: bool,
    pub waiting: i64,
    /// Why the queue isn't moving, if it isn't.
    pub problem: Option<String>,
}

enum Cmd {
    NowPlaying(Play),
    Scrobble(Play),
    Love(String, String, bool),
    AskLoved(String, String),
    Link,
    Unlink,
    Setup(LastFmSetup),
    Flush,
}

pub struct ScrobbleService {
    tx: Option<Sender<Cmd>>,
    events: Receiver<ScrobbleEvent>,
    status: Arc<Mutex<ScrobbleStatus>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ScrobbleService {
    pub fn start(
        db: &Path,
        setup: LastFmSetup,
        secrets: Arc<Secrets>,
    ) -> crate::library::Result<Self> {
        Self::start_with(db, setup, secrets, crate::lastfm::API)
    }

    /// For tests: a stand-in Last.fm at `base`.
    pub fn start_with(
        db: &Path,
        setup: LastFmSetup,
        secrets: Arc<Secrets>,
        base: &str,
    ) -> crate::library::Result<Self> {
        let lib = Library::open(db)?;
        let (tx, rx) = unbounded();
        let (etx, events) = unbounded();
        let status = Arc::new(Mutex::new(ScrobbleStatus::default()));
        let mut w = Worker {
            lib,
            rx,
            events: etx,
            status: status.clone(),
            secrets,
            base: base.to_string(),
            setup: LastFmSetup::default(),
            client: None,
            linked: false,
            link: None,
            retry_at: None,
            backoff: RETRY_MIN,
        };
        w.apply(setup);
        let join = std::thread::Builder::new()
            .name("patina-lastfm".into())
            .spawn(move || w.run())
            .expect("spawn Last.fm thread");
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

    /// Queue a play and send it as soon as Last.fm takes it.
    pub fn scrobble(&self, p: Play) {
        self.send(Cmd::Scrobble(p));
    }

    pub fn love(&self, artist: &str, track: &str, on: bool) {
        self.send(Cmd::Love(artist.into(), track.into(), on));
    }

    /// Ask Last.fm whether this is loved; the answer comes as an event.
    pub fn ask_loved(&self, artist: &str, track: &str) {
        self.send(Cmd::AskLoved(artist.into(), track.into()));
    }

    /// Start linking: a token, the approval page, then waiting for you.
    pub fn link(&self) {
        self.send(Cmd::Link);
    }

    pub fn unlink(&self) {
        self.send(Cmd::Unlink);
    }

    /// New key or secret.
    pub fn setup(&self, s: LastFmSetup) {
        self.send(Cmd::Setup(s));
    }

    /// Try sending what's waiting now.
    pub fn flush(&self) {
        self.send(Cmd::Flush);
    }

    pub fn events(&self) -> &Receiver<ScrobbleEvent> {
        &self.events
    }

    pub fn status(&self) -> ScrobbleStatus {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for ScrobbleService {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct Link {
    token: String,
    started: Instant,
    next: Instant,
}

struct Worker {
    lib: Library,
    rx: Receiver<Cmd>,
    events: Sender<ScrobbleEvent>,
    status: Arc<Mutex<ScrobbleStatus>>,
    secrets: Arc<Secrets>,
    base: String,
    setup: LastFmSetup,
    client: Option<LastFm>,
    linked: bool,
    link: Option<Link>,
    retry_at: Option<Instant>,
    backoff: Duration,
}

impl Worker {
    fn emit(&self, e: ScrobbleEvent) {
        let _ = self.events.send(e);
    }

    /// Build the client from the key, secret and saved session.
    fn apply(&mut self, s: LastFmSetup) {
        self.setup = s;
        let session = if self.setup.user.is_empty() {
            None
        } else {
            self.secrets
                .get(SECRET_NAME, &self.setup.user)
                .ok()
                .flatten()
        };
        self.client = LastFm::with_base(&self.base, &self.setup.key)
            .map(|c| c.with_secret(&self.setup.secret));
        self.linked = false;
        if let Some(sk) = session
            && let Some(c) = self.client.take()
        {
            self.client = Some(c.with_session(&sk));
            self.linked = true;
        }
        self.publish(None);
    }

    fn publish(&self, problem: Option<String>) {
        let waiting = self.lib.scrobbles_waiting().unwrap_or(0);
        let mut s = self.status.lock().unwrap();
        s.user = if self.linked {
            self.setup.user.clone()
        } else {
            String::new()
        };
        s.linking = self.link.is_some();
        s.waiting = waiting;
        if problem.is_some() || waiting == 0 {
            s.problem = problem;
        }
    }

    fn run(&mut self) {
        // Anything left from last time goes out first.
        self.retry_at = Some(Instant::now());
        loop {
            let now = Instant::now();
            let mut wake = now + Duration::from_secs(3600);
            if let Some(l) = &self.link {
                wake = wake.min(l.next);
            }
            if let Some(r) = self.retry_at {
                wake = wake.min(r);
            }
            match self.rx.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(c) => self.handle(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.link.as_ref().is_some_and(|l| Instant::now() >= l.next) {
                self.poll_link();
            }
            if self.retry_at.is_some_and(|r| Instant::now() >= r) {
                self.retry_at = None;
                self.flush();
            }
        }
    }

    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Setup(s) => {
                self.apply(s);
                self.retry_at = Some(Instant::now());
            }
            Cmd::NowPlaying(p) => {
                if let Some(c) = self.client.as_ref().filter(|_| self.linked)
                    && let Err(e) = c.now_playing(&p)
                {
                    log::info!("Last.fm now playing: {e}");
                    self.check_session(&e);
                }
            }
            Cmd::Scrobble(p) => {
                if let Err(e) = self.lib.queue_scrobble(&p) {
                    self.emit(ScrobbleEvent::Error(format!(
                        "couldn't queue a scrobble: {e}"
                    )));
                }
                // Straight out, unless Last.fm is being given a rest.
                if self.retry_at.is_none() {
                    self.flush();
                } else {
                    self.publish(None);
                }
            }
            Cmd::Flush => {
                self.retry_at = None;
                self.backoff = RETRY_MIN;
                self.flush();
            }
            Cmd::Love(artist, track, on) => {
                if let Some(c) = self.client.as_ref().filter(|_| self.linked)
                    && let Err(e) = c.love(&artist, &track, on)
                {
                    self.emit(ScrobbleEvent::Error(format!(
                        "couldn't {} it on Last.fm: {e}",
                        if on { "love" } else { "unlove" }
                    )));
                    self.check_session(&e);
                }
            }
            Cmd::AskLoved(artist, track) => {
                if let Some(c) = self.client.as_ref().filter(|_| self.linked)
                    && let Ok(on) = c.loved(&artist, &track, &self.setup.user)
                {
                    self.emit(ScrobbleEvent::Loved { artist, track, on });
                }
            }
            Cmd::Link => self.start_link(),
            Cmd::Unlink => {
                self.link = None;
                if !self.setup.user.is_empty() {
                    let _ = self.secrets.delete(SECRET_NAME, &self.setup.user);
                }
                if let Some(c) = self.client.take() {
                    self.client = Some(c.with_session(""));
                }
                self.linked = false;
                self.publish(None);
                self.emit(ScrobbleEvent::Unlinked(String::new()));
            }
        }
    }

    fn start_link(&mut self) {
        let Some(c) = &self.client else {
            return self.emit(ScrobbleEvent::LinkFailed(
                "there's no Last.fm API key: set one in Settings, Last.fm".into(),
            ));
        };
        if !c.can_sign() {
            return self.emit(ScrobbleEvent::LinkFailed(
                "the API key needs its secret too (Settings, Last.fm)".into(),
            ));
        }
        match c.get_token() {
            Ok(token) => {
                let url = c.auth_url(&token);
                self.link = Some(Link {
                    token,
                    started: Instant::now(),
                    next: Instant::now() + LINK_POLL,
                });
                self.publish(None);
                self.emit(ScrobbleEvent::Approve(url));
            }
            Err(e) => self.emit(ScrobbleEvent::LinkFailed(e.to_string())),
        }
    }

    fn poll_link(&mut self) {
        let Some(l) = &mut self.link else { return };
        if l.started.elapsed() > LINK_TIMEOUT {
            self.link = None;
            self.publish(None);
            return self.emit(ScrobbleEvent::LinkFailed(
                "no approval after five minutes; press Link to try again".into(),
            ));
        }
        l.next = Instant::now() + LINK_POLL;
        let token = l.token.clone();
        let Some(c) = &self.client else { return };
        match c.get_session(&token) {
            Ok(None) => {}
            Ok(Some((user, key))) => {
                self.link = None;
                if let Err(e) = self.secrets.set(SECRET_NAME, &user, &key) {
                    self.publish(None);
                    return self.emit(ScrobbleEvent::LinkFailed(format!(
                        "linked, but the session key couldn't be saved: {e}"
                    )));
                }
                self.setup.user = user.clone();
                let c = self.client.take().map(|c| c.with_session(&key));
                self.client = c;
                self.linked = true;
                self.publish(None);
                self.emit(ScrobbleEvent::Linked(user));
                self.retry_at = Some(Instant::now());
            }
            // 15: the token ran out.
            Err(e) if e.transient() => log::info!("Last.fm link check: {e}"),
            Err(e) => {
                self.link = None;
                self.publish(None);
                self.emit(ScrobbleEvent::LinkFailed(e.to_string()));
            }
        }
    }

    /// The session's no good any more: forget it, so the queue waits for
    /// a new link instead of failing over and over.
    fn check_session(&mut self, e: &LastFmError) {
        if !e.bad_session() {
            return;
        }
        if !self.setup.user.is_empty() {
            let _ = self.secrets.delete(SECRET_NAME, &self.setup.user);
        }
        self.linked = false;
        self.publish(Some("Last.fm ended the session; link again".into()));
        self.emit(ScrobbleEvent::Unlinked(
            "Last.fm doesn't accept this session any more. Link again in Settings; scrobbles wait till then."
                .into(),
        ));
    }

    fn flush(&mut self) {
        let Some(c) = self.client.as_ref().filter(|_| self.linked) else {
            return self.publish(None);
        };
        loop {
            let batch = match self.lib.queued_scrobbles(50) {
                Ok(b) => b,
                Err(e) => return self.publish(Some(e.to_string())),
            };
            if batch.is_empty() {
                self.backoff = RETRY_MIN;
                return self.publish(None);
            }
            let ids: Vec<i64> = batch.iter().map(|q| q.id).collect();
            let plays: Vec<Play> = batch.into_iter().map(|q| q.play).collect();
            match c.scrobble(&plays) {
                Ok(sent) => {
                    if sent.ignored > 0 {
                        log::info!("Last.fm ignored {} scrobble(s)", sent.ignored);
                    }
                    if let Err(e) = self.lib.drop_scrobbles(&ids) {
                        return self.publish(Some(e.to_string()));
                    }
                }
                Err(e) if e.transient() => {
                    let _ = self.lib.bump_scrobble_tries(&ids);
                    self.retry_at = Some(Instant::now() + self.backoff);
                    self.backoff = (self.backoff * 2).min(RETRY_MAX);
                    return self.publish(Some(format!("waiting to send: {e}")));
                }
                Err(e) if e.bad_session() => {
                    let e = e.clone();
                    self.check_session(&e);
                    return;
                }
                Err(e) => {
                    // Something in the batch Last.fm won't ever take.
                    // Holding it would hold every scrobble behind it.
                    let _ = self.lib.drop_scrobbles(&ids);
                    self.emit(ScrobbleEvent::Error(format!(
                        "Last.fm turned down {} scrobble(s): {e}",
                        ids.len()
                    )));
                }
            }
        }
    }
}
