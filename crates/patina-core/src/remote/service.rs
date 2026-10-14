//! Keeping servers connected and synced, in the background.
//!
//! One thread per server: it logs in (password from the keyring), syncs,
//! then waits, syncing again every half hour or when asked. Anything that
//! talks to the server on your behalf (saving a playlist, saving the play
//! queue) goes through that server's thread too, so the UI never waits on
//! the network and two requests to one server never race.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use super::sync::{self, SyncError};
use super::{
    Client, RemoteError, RemoteStation, SavedQueue, ServerConfig, ServerInfo, ServerKind, registry,
};
use crate::library::Library;
use crate::secrets::{Secrets, Stored};

/// How often a connected server is synced again on its own.
const RESYNC: Duration = Duration::from_secs(30 * 60);
/// Waits between tries while a server can't be reached.
const RETRY: [u64; 5] = [15, 30, 60, 120, 300];

#[derive(Debug, Clone, PartialEq)]
pub enum ServerState {
    Connecting,
    Syncing {
        done: usize,
        total: usize,
    },
    Ready,
    /// Can't reach it. The next try is on its own.
    Offline(String),
    /// No password saved for it.
    NoPassword,
    BadLogin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    pub name: String,
    pub state: ServerState,
    pub info: Option<ServerInfo>,
    /// Unix seconds of the last finished sync.
    pub last_sync: Option<i64>,
    pub stations: Vec<RemoteStation>,
}

impl ServerStatus {
    /// "Navidrome 0.58.0", "syncing 40 of 300 albums", "offline: ..."
    pub fn summary(&self) -> String {
        let product = self
            .info
            .as_ref()
            .map(|i| i.product.clone())
            .unwrap_or_default();
        let state = match &self.state {
            ServerState::Connecting => "connecting".to_string(),
            ServerState::Syncing { total: 0, .. } => "syncing".into(),
            ServerState::Syncing { done, total } => format!("syncing, {done} of {total} albums"),
            ServerState::Ready => "connected".into(),
            ServerState::Offline(e) => format!("offline: {e}"),
            ServerState::NoPassword => "needs its password".into(),
            ServerState::BadLogin => "wrong user name or password".into(),
        };
        if product.is_empty() {
            state
        } else {
            format!("{product}, {state}")
        }
    }
}

#[derive(Debug, Clone)]
pub enum RemoteEvent {
    /// The index changed. Views should re-query.
    Changed,
    /// A server's status changed.
    Status,
    Error(String),
    /// A server was added (or edited) and logged in fine.
    Added {
        cfg: ServerConfig,
        info: ServerInfo,
        stored: Stored,
        replaces: Option<String>,
    },
    AddFailed(String),
    QueueLoaded {
        server: String,
        queue: SavedQueue,
    },
    /// Something you asked for is done.
    Done(String),
}

/// Work for one server.
#[derive(Debug, Clone)]
pub enum Job {
    CreatePlaylist {
        name: String,
        songs: Vec<String>,
    },
    /// Set a playlist's songs to exactly these.
    ReplacePlaylist {
        id: String,
        songs: Vec<String>,
    },
    RenamePlaylist {
        id: String,
        name: String,
    },
    DeletePlaylist {
        id: String,
    },
    SaveQueue {
        ids: Vec<String>,
        current: Option<String>,
        pos: Duration,
    },
    LoadQueue,
    Bookmark {
        id: String,
        pos: Duration,
    },
}

enum Cmd {
    Servers(Vec<ServerConfig>),
    Sync(Option<String>),
    Add {
        cfg: ServerConfig,
        secret: String,
        replaces: Option<String>,
    },
    Job(String, Job),
    Quit,
}

enum WCmd {
    /// Sync now. `true` reads every album again.
    Sync(bool),
    Job(Job),
    Stop,
}

pub struct RemoteService {
    tx: Sender<Cmd>,
    events: Receiver<RemoteEvent>,
    status: Arc<Mutex<HashMap<String, ServerStatus>>>,
    order: Arc<Mutex<Vec<String>>>,
    secrets: Arc<Secrets>,
}

struct Ctx {
    db: PathBuf,
    secrets: Arc<Secrets>,
    events: Sender<RemoteEvent>,
    status: Arc<Mutex<HashMap<String, ServerStatus>>>,
}

impl Ctx {
    fn set_state(&self, name: &str, f: impl FnOnce(&mut ServerStatus)) {
        let mut st = self.status.lock().unwrap();
        let s = st.entry(name.to_string()).or_insert_with(|| ServerStatus {
            name: name.to_string(),
            state: ServerState::Connecting,
            info: None,
            last_sync: None,
            stations: Vec::new(),
        });
        f(s);
        drop(st);
        let _ = self.events.send(RemoteEvent::Status);
    }
}

impl RemoteService {
    pub fn start(db: &Path, servers: Vec<ServerConfig>, secrets: Arc<Secrets>) -> Self {
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let status = Arc::new(Mutex::new(HashMap::new()));
        let order = Arc::new(Mutex::new(Vec::new()));
        let ctx = Arc::new(Ctx {
            db: db.to_path_buf(),
            secrets: secrets.clone(),
            events: ev_tx,
            status: status.clone(),
        });
        let o2 = order.clone();
        std::thread::Builder::new()
            .name("patina-servers".into())
            .spawn(move || manager(rx, ctx, o2))
            .expect("spawn server manager");
        let _ = tx.send(Cmd::Servers(servers));
        Self {
            tx,
            events: ev_rx,
            status,
            order,
            secrets,
        }
    }

    pub fn events(&self) -> &Receiver<RemoteEvent> {
        &self.events
    }

    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// Every server's status, in config order.
    pub fn statuses(&self) -> Vec<ServerStatus> {
        let st = self.status.lock().unwrap();
        self.order
            .lock()
            .unwrap()
            .iter()
            .filter_map(|n| st.get(n).cloned())
            .collect()
    }

    pub fn status(&self, name: &str) -> Option<ServerStatus> {
        self.status.lock().unwrap().get(name).cloned()
    }

    /// The configured servers changed: start, stop or reconnect to match.
    /// Servers no longer listed are forgotten, songs and all.
    pub fn set_servers(&self, servers: Vec<ServerConfig>) {
        let _ = self.tx.send(Cmd::Servers(servers));
    }

    /// Sync one server now, or all of them.
    pub fn sync(&self, server: Option<&str>) {
        let _ = self.tx.send(Cmd::Sync(server.map(str::to_string)));
    }

    /// Log in to a new server (or an edited one) and save its password.
    /// The answer comes back as Added or AddFailed.
    pub fn add(&self, cfg: ServerConfig, secret: String, replaces: Option<String>) {
        let _ = self.tx.send(Cmd::Add {
            cfg,
            secret,
            replaces,
        });
    }

    pub fn job(&self, server: &str, job: Job) {
        let _ = self.tx.send(Cmd::Job(server.to_string(), job));
    }
}

impl Drop for RemoteService {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
}

fn manager(rx: Receiver<Cmd>, ctx: Arc<Ctx>, order: Arc<Mutex<Vec<String>>>) {
    let mut workers: HashMap<String, (ServerConfig, Sender<WCmd>)> = HashMap::new();
    for cmd in rx {
        match cmd {
            Cmd::Servers(list) => {
                *order.lock().unwrap() = list.iter().map(|s| s.name.clone()).collect();
                let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
                let gone: Vec<String> = workers
                    .keys()
                    .filter(|n| !names.contains(&n.as_str()))
                    .cloned()
                    .collect();
                for name in gone {
                    if let Some((_, w)) = workers.remove(&name) {
                        let _ = w.send(WCmd::Stop);
                    }
                    forget(&ctx, &name);
                }
                // Servers in the index that aren't configured any more
                // (removed while the app was closed) go too.
                if let Ok(lib) = Library::open(&ctx.db)
                    && let Ok(srcs) = lib.sources()
                {
                    for s in srcs {
                        if let Some(n) = super::server_of(&s.root)
                            && !names.contains(&n)
                        {
                            forget(&ctx, n);
                        }
                    }
                }
                for cfg in list {
                    let same = workers.get(&cfg.name).is_some_and(|(c, _)| *c == cfg);
                    if same {
                        continue;
                    }
                    if let Some((_, w)) = workers.remove(&cfg.name) {
                        let _ = w.send(WCmd::Stop);
                    }
                    let (wtx, wrx) = unbounded();
                    let (c2, ctx2) = (cfg.clone(), ctx.clone());
                    std::thread::Builder::new()
                        .name(format!("patina-server-{}", cfg.name))
                        .spawn(move || worker(c2, wrx, ctx2))
                        .expect("spawn server thread");
                    workers.insert(cfg.name.clone(), (cfg, wtx));
                }
            }
            Cmd::Sync(None) => {
                for (_, w) in workers.values() {
                    let _ = w.send(WCmd::Sync(false));
                }
            }
            Cmd::Sync(Some(name)) => {
                // Asked for by name, from Settings: read it all.
                if let Some((_, w)) = workers.get(&name) {
                    let _ = w.send(WCmd::Sync(true));
                }
            }
            Cmd::Job(name, job) => match workers.get(&name) {
                Some((_, w)) => {
                    let _ = w.send(WCmd::Job(job));
                }
                None => {
                    let _ = ctx
                        .events
                        .send(RemoteEvent::Error(format!("{name} isn't set up")));
                }
            },
            Cmd::Add {
                cfg,
                secret,
                replaces,
            } => {
                let ctx = ctx.clone();
                std::thread::spawn(move || add(cfg, secret, replaces, &ctx));
            }
            Cmd::Quit => break,
        }
    }
    for (_, (_, w)) in workers {
        let _ = w.send(WCmd::Stop);
    }
}

fn forget(ctx: &Ctx, name: &str) {
    registry().remove(name);
    ctx.status.lock().unwrap().remove(name);
    match Library::open(&ctx.db) {
        Ok(mut lib) => {
            if let Err(e) = sync::drop_source(&mut lib, name) {
                log::warn!("couldn't forget {name}: {e}");
            }
        }
        Err(e) => log::warn!("couldn't forget {name}: {e}"),
    }
    let _ = ctx.events.send(RemoteEvent::Changed);
}

fn add(mut cfg: ServerConfig, secret: String, replaces: Option<String>, ctx: &Ctx) {
    if cfg.kind == ServerKind::Dlna {
        let res = super::dlna::Dlna::connect(cfg.url.trim(), &cfg.udn);
        let _ = ctx.events.send(match res {
            Ok(c) => RemoteEvent::Added {
                info: c.info().clone(),
                cfg,
                stored: Stored::NotNeeded,
                replaces,
            },
            Err(e) => RemoteEvent::AddFailed(format!("{}: {e}", cfg.name)),
        });
        return;
    }
    let res = super::detect(&cfg.url, &cfg.user, &secret).and_then(|(kind, info)| {
        cfg.kind = kind;
        cfg.url = super::normalize_url(&cfg.url);
        ctx.secrets
            .set(&cfg.secret_key(), &cfg.user, &secret)
            .map(|stored| (info, stored))
            .map_err(|e| RemoteError::Http(e.to_string()))
    });
    let _ = ctx.events.send(match res {
        Ok((info, stored)) => RemoteEvent::Added {
            cfg,
            info,
            stored,
            replaces,
        },
        Err(e) => RemoteEvent::AddFailed(format!("{}: {e}", cfg.name)),
    });
}

struct Worker {
    cfg: ServerConfig,
    ctx: Arc<Ctx>,
    lib: Library,
    client: Option<Arc<dyn Client>>,
    next_connect: Instant,
    next_sync: Instant,
    tries: usize,
    cancel: AtomicBool,
    /// The next sync reads every album, not only changed ones.
    full: bool,
}

fn worker(cfg: ServerConfig, rx: Receiver<WCmd>, ctx: Arc<Ctx>) {
    let lib = match Library::open(&ctx.db) {
        Ok(l) => l,
        Err(e) => {
            let _ = ctx
                .events
                .send(RemoteEvent::Error(format!("{}: {e}", cfg.name)));
            return;
        }
    };
    registry().set_db(&cfg.name, Library::open(&ctx.db).ok());
    let mut w = Worker {
        cfg,
        ctx,
        lib,
        client: None,
        next_connect: Instant::now(),
        next_sync: Instant::now(),
        tries: 0,
        cancel: AtomicBool::new(false),
        full: false,
    };
    w.ctx
        .set_state(&w.cfg.name, |s| s.state = ServerState::Connecting);
    loop {
        let now = Instant::now();
        if w.client.is_none() && now >= w.next_connect {
            w.connect();
        }
        if w.client.is_some() && Instant::now() >= w.next_sync {
            w.sync();
        }
        let wake = if w.client.is_some() {
            w.next_sync
        } else {
            w.next_connect
        };
        let wait = wake.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait.max(Duration::from_millis(10))) {
            Ok(WCmd::Sync(full)) => {
                w.full = full;
                w.next_sync = Instant::now();
                if w.client.is_none() {
                    w.next_connect = Instant::now();
                    w.tries = 0;
                }
            }
            Ok(WCmd::Job(job)) => w.job(job),
            Ok(WCmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    registry().set_client(&w.cfg.name, None);
}

impl Worker {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn connect(&mut self) {
        self.ctx
            .set_state(&self.cfg.name, |s| s.state = ServerState::Connecting);
        let saved = if self.cfg.kind.logs_in() {
            self.ctx.secrets.get(&self.cfg.secret_key(), &self.cfg.user)
        } else {
            Ok(Some(String::new()))
        };
        let secret = match saved {
            Ok(Some(s)) => s,
            Ok(None) => {
                self.ctx
                    .set_state(&self.cfg.name, |s| s.state = ServerState::NoPassword);
                // Nothing to retry with until a password is given.
                self.next_connect = Instant::now() + Duration::from_secs(365 * 24 * 3600);
                return;
            }
            Err(e) => {
                self.offline(e.to_string());
                return;
            }
        };
        match super::connect(&self.cfg, &secret) {
            Ok(c) => {
                registry().set_client(&self.cfg.name, Some(c.clone()));
                let info = c.info().clone();
                self.client = Some(c);
                self.tries = 0;
                self.next_sync = Instant::now();
                self.ctx.set_state(&self.cfg.name, |s| {
                    s.state = ServerState::Ready;
                    s.info = Some(info);
                });
            }
            Err(RemoteError::Auth) => {
                self.ctx
                    .set_state(&self.cfg.name, |s| s.state = ServerState::BadLogin);
                self.next_connect = Instant::now() + Duration::from_secs(365 * 24 * 3600);
            }
            Err(e) => self.offline(e.to_string()),
        }
    }

    fn offline(&mut self, why: String) {
        let wait = RETRY[self.tries.min(RETRY.len() - 1)];
        self.tries += 1;
        self.next_connect = Instant::now() + Duration::from_secs(wait);
        self.client = None;
        registry().set_client(&self.cfg.name, None);
        self.ctx
            .set_state(&self.cfg.name, |s| s.state = ServerState::Offline(why));
    }

    fn sync(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let name = self.cfg.name.clone();
        let kind: ServerKind = self.cfg.kind;
        self.ctx.set_state(&name, |s| {
            s.state = ServerState::Syncing { done: 0, total: 0 }
        });
        let ctx = self.ctx.clone();
        let progress = |done: usize, total: usize| {
            ctx.set_state(&name, |s| s.state = ServerState::Syncing { done, total });
            if done > 0 {
                let _ = ctx.events.send(RemoteEvent::Changed);
            }
        };
        let full = std::mem::take(&mut self.full);
        let res = sync::sync_library(
            &mut self.lib,
            &name,
            kind,
            client.as_ref(),
            full,
            &progress,
            &self.cancel,
        );
        match res {
            Ok(stats) => {
                if stats.changed() {
                    let _ = self.ctx.events.send(RemoteEvent::Changed);
                }
            }
            Err(SyncError::Remote(RemoteError::Auth)) => {
                self.client = None;
                registry().set_client(&name, None);
                self.ctx
                    .set_state(&name, |s| s.state = ServerState::BadLogin);
                self.next_connect = Instant::now() + Duration::from_secs(365 * 24 * 3600);
                return;
            }
            Err(SyncError::Remote(e @ RemoteError::Http(_))) => {
                self.offline(e.to_string());
                return;
            }
            Err(e) => {
                let _ = self
                    .ctx
                    .events
                    .send(RemoteEvent::Error(format!("syncing {name}: {e}")));
            }
        }
        // The rest is smaller and less important: a failure in one doesn't
        // stop the others.
        match sync::sync_playlists(&mut self.lib, &name, kind, client.as_ref()) {
            Ok(true) => {
                let _ = self.ctx.events.send(RemoteEvent::Changed);
            }
            Ok(false) => {}
            Err(e) => log::warn!("{name} playlists: {e}"),
        }
        let stations = client.radio().unwrap_or_else(|e| {
            log::info!("{name} radio: {e}");
            Vec::new()
        });
        match sync::sync_podcasts(&mut self.lib, &name, client.as_ref()) {
            Ok(_) => {
                let _ = self.ctx.events.send(RemoteEvent::Changed);
            }
            Err(e) => log::info!("{name} podcasts: {e}"),
        }
        self.ctx.set_state(&name, |s| {
            s.state = ServerState::Ready;
            s.last_sync = Some(crate::library::now_secs());
            s.stations = stations;
        });
        self.next_sync = Instant::now() + RESYNC;
    }

    fn job(&mut self, job: Job) {
        let Some(c) = self.client.clone() else {
            let _ = self.ctx.events.send(RemoteEvent::Error(format!(
                "{} isn't connected right now",
                self.name()
            )));
            return;
        };
        let name = self.cfg.name.clone();
        let playlists_changed = matches!(
            job,
            Job::CreatePlaylist { .. }
                | Job::ReplacePlaylist { .. }
                | Job::RenamePlaylist { .. }
                | Job::DeletePlaylist { .. }
        );
        let res: Result<Option<RemoteEvent>, RemoteError> = match job {
            Job::CreatePlaylist { name: list, songs } => c
                .create_playlist(&list, &songs)
                .map(|_| Some(RemoteEvent::Done(format!("Made \"{list}\" on {name}")))),
            Job::ReplacePlaylist { id, songs } => c.replace_playlist(&id, &songs).map(|_| None),
            Job::RenamePlaylist { id, name: n } => c.rename_playlist(&id, &n).map(|_| None),
            Job::DeletePlaylist { id } => c.delete_playlist(&id).map(|_| None),
            Job::SaveQueue { ids, current, pos } => {
                c.save_queue(&ids, current.as_deref(), pos).map(|_| None)
            }
            Job::LoadQueue => c.load_queue().map(|q| match q {
                Some(queue) => Some(RemoteEvent::QueueLoaded {
                    server: name.clone(),
                    queue,
                }),
                None => Some(RemoteEvent::Done(format!("{name} has no saved queue"))),
            }),
            Job::Bookmark { id, pos } => c.bookmark(&id, pos).map(|_| None),
        };
        match res {
            Ok(ev) => {
                if let Some(ev) = ev {
                    let _ = self.ctx.events.send(ev);
                }
            }
            Err(e) => {
                let _ = self
                    .ctx
                    .events
                    .send(RemoteEvent::Error(format!("{name}: {e}")));
            }
        }
        if playlists_changed {
            // Read them back, so the copy here matches the server.
            match sync::sync_playlists(&mut self.lib, &name, self.cfg.kind, c.as_ref()) {
                Ok(_) => {
                    let _ = self.ctx.events.send(RemoteEvent::Changed);
                }
                Err(e) => log::warn!("{name} playlists: {e}"),
            }
        }
    }
}
