//! What the Sources pages do besides show settings: signing in to Plex,
//! adding a Plex server by hand or an Icecast server, and the Snapcast
//! rooms.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use super::settings::SnapLive;
use super::{Browser, Section};
use crate::cast::snapcast::{self, SnapClient};
use crate::cast::{Kind, Target};
use crate::remote::plex_tv::{self, Login, Resource};
use crate::remote::{ServerConfig, ServerKind, StreamFormat};

/// How often a PIN is checked while you enter it.
const PIN_POLL: Duration = Duration::from_secs(2);

/// Where a Plex sign-in is.
#[derive(Debug, Clone, PartialEq)]
pub enum PlexSignIn {
    /// Asking plex.tv for a code.
    Starting,
    /// Enter `code` at plex.tv/link; `link` has it filled in.
    Waiting { code: String, link: String },
    /// Signed in as `account`: pick a server to add.
    Choose {
        account: String,
        servers: Vec<PlexChoice>,
    },
}

/// A server on the Plex account.
#[derive(Debug, Clone, PartialEq)]
pub struct PlexChoice {
    pub name: String,
    /// Its machine identifier.
    pub id: String,
    pub owned: bool,
    /// Added here already.
    pub added: bool,
}

enum PlexMsg {
    Pin(String, String),
    Found {
        login: Login,
        account: String,
        servers: Vec<Resource>,
    },
    Failed(String),
}

#[derive(Default)]
pub(super) struct PlexState {
    rx: Option<Receiver<PlexMsg>>,
    cancel: Option<Arc<AtomicBool>>,
    shown: Option<PlexSignIn>,
    found: Option<(Login, String, Vec<Resource>)>,
    /// Bumps whenever `shown` changes, so a window can tell.
    generation: u64,
}

enum SnapMsg {
    Clients(Target, Result<Vec<SnapClient>, String>),
    Routed {
        client: String,
        on: bool,
        res: Result<String, String>,
    },
    Done(Result<(), String>),
}

#[derive(Default)]
pub(super) struct SnapState {
    pub(super) live: SnapLive,
    rx: Option<Receiver<SnapMsg>>,
    /// Where each client was before it went on Patina, to put it back.
    before: HashMap<String, String>,
    asked: Option<Instant>,
}

impl Browser {
    /// A name for a new server that no other has: "Plex", "Plex 2"…
    fn unique_server_name(&self, base: &str) -> String {
        let base: String = base
            .chars()
            .map(|c| if c == '/' || c == '#' { '-' } else { c })
            .collect::<String>()
            .trim()
            .to_string();
        let base = if base.is_empty() {
            "Server".into()
        } else {
            base
        };
        let mut name = base.clone();
        let mut n = 2;
        while self.config.servers.iter().any(|s| s.name == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        name
    }

    // ----- Plex -----

    /// Start signing in to Plex: ask plex.tv for a PIN, then wait for it
    /// to be entered at plex.tv/link. [`Self::plex_sign_in_state`] says
    /// where it's at.
    pub fn plex_sign_in(&mut self) {
        if self.remote.is_none() {
            return self.error("Servers need the library");
        }
        self.cancel_plex_sign_in();
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let pin = match plex_tv::start_pin() {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(PlexMsg::Failed(format!("plex.tv: {e}")));
                    return;
                }
            };
            let _ = tx.send(PlexMsg::Pin(pin.code.clone(), pin.link()));
            let until = Instant::now() + Duration::from_secs(pin.expires_in.clamp(60, 3600) as u64);
            while Instant::now() < until {
                std::thread::sleep(PIN_POLL);
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match plex_tv::check_pin(&pin) {
                    Ok(None) => continue,
                    Ok(Some(login)) => {
                        let account = plex_tv::account(&login).unwrap_or_else(|_| "Plex".into());
                        let msg = match plex_tv::servers(&login) {
                            Ok(servers) => PlexMsg::Found {
                                login,
                                account,
                                servers,
                            },
                            Err(e) => PlexMsg::Failed(format!("plex.tv's server list: {e}")),
                        };
                        let _ = tx.send(msg);
                        return;
                    }
                    Err(crate::remote::RemoteError::Http(e)) => {
                        // A blip; the next check may get through.
                        log::info!("plex.tv PIN check: {e}");
                    }
                    Err(e) => {
                        let _ = tx.send(PlexMsg::Failed(format!("plex.tv: {e}")));
                        return;
                    }
                }
            }
            let _ = tx.send(PlexMsg::Failed(
                "The code ran out before it was entered. Sign in again for a new one.".into(),
            ));
        });
        self.plex.rx = Some(rx);
        self.plex.cancel = Some(cancel);
        self.set_plex_shown(Some(PlexSignIn::Starting));
    }

    /// Where the sign-in is, while there is one.
    pub fn plex_sign_in_state(&self) -> Option<&PlexSignIn> {
        self.plex.shown.as_ref()
    }

    /// Changes whenever [`Self::plex_sign_in_state`] does.
    pub fn plex_generation(&self) -> u64 {
        self.plex.generation
    }

    pub fn cancel_plex_sign_in(&mut self) {
        if let Some(c) = self.plex.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.plex.rx = None;
        self.plex.found = None;
        self.set_plex_shown(None);
    }

    fn set_plex_shown(&mut self, s: Option<PlexSignIn>) {
        self.plex.shown = s;
        self.plex.generation += 1;
        self.touch();
    }

    /// Add one of the servers the sign-in found.
    pub fn add_plex_choice(&mut self, id: &str) {
        let Some((login, account, servers)) = self.plex.found.clone() else {
            return;
        };
        let Some(r) = servers.iter().find(|r| r.id == id) else {
            return;
        };
        if self
            .config
            .servers
            .iter()
            .any(|s| s.kind == ServerKind::Plex && s.udn == r.id)
        {
            return self.note(format!("{} is already added", r.name));
        }
        let Some(remote) = &self.remote else { return };
        let name = self.unique_server_name(&r.name);
        let login = Login {
            server_token: r.token.clone(),
            ..login
        };
        remote.add(
            ServerConfig {
                name,
                url: r.ordered().into_iter().next().unwrap_or_default(),
                user: account,
                kind: ServerKind::Plex,
                stream: StreamFormat::Raw,
                bitrate: 192,
                udn: r.id.clone(),
                extras: false,
            },
            login.to_secret(),
            None,
        );
        self.note(format!("Connecting to {}…", r.name));
        // The list stays up for more picks, with this one marked.
        if let Some(PlexSignIn::Choose { servers, .. }) = self.plex.shown.as_mut()
            && let Some(c) = servers.iter_mut().find(|c| c.id == id)
        {
            c.added = true;
        }
        self.plex.generation += 1;
    }

    /// Add a Plex server by its address and a token, without plex.tv.
    /// An empty name becomes "Plex" (or "Plex 2"…).
    pub fn add_plex_by_token(&mut self, name: &str, url: &str, token: &str) {
        let name = if name.trim().is_empty() {
            self.unique_server_name("Plex")
        } else {
            name.trim().to_string()
        };
        let name = name.as_str();
        if let Err(e) = self.check_server_name(name, None) {
            return self.error(e);
        }
        if url.trim().is_empty() || token.trim().is_empty() {
            return self.error("A Plex server added by hand needs its address and a token");
        }
        let Some(remote) = &self.remote else {
            return self.error("Servers need the library");
        };
        remote.add(
            ServerConfig {
                name: name.into(),
                url: url.trim().into(),
                kind: ServerKind::Plex,
                bitrate: 192,
                ..Default::default()
            },
            token.trim().to_string(),
            None,
        );
        self.note(format!("Connecting to {}…", url.trim()));
    }

    /// Add an Icecast server; its streams show once it answers.
    pub fn add_icecast(&mut self, name: &str, url: &str) {
        let url = url.trim();
        let name = if name.trim().is_empty() {
            self.unique_server_name(
                &url::Url::parse(&crate::remote::icecast::normalize(url))
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_else(|| "Icecast".into()),
            )
        } else {
            name.trim().to_string()
        };
        if let Err(e) = self.check_server_name(&name, None) {
            return self.error(e);
        }
        if url.is_empty() {
            return self.error("An Icecast server needs its address");
        }
        let Some(remote) = &self.remote else {
            return self.error("Servers need the library");
        };
        remote.add(
            ServerConfig {
                name,
                url: url.into(),
                kind: ServerKind::Icecast,
                ..Default::default()
            },
            String::new(),
            None,
        );
        self.note(format!("Asking {url} for its streams…"));
    }

    // ----- Snapcast rooms -----

    /// The snapserver whose rooms the Snapcast page shows: the one playing
    /// now, else the one typed in, else the first one found.
    pub fn snap_target(&self) -> Option<Target> {
        self.casting()
            .filter(|t| t.kind == Kind::Snapcast)
            .or_else(|| {
                self.config
                    .cast
                    .snapcast
                    .as_deref()
                    .and_then(Target::snapcast)
            })
            .or_else(|| {
                self.cast_found
                    .iter()
                    .find(|t| t.kind == Kind::Snapcast)
                    .cloned()
            })
    }

    /// Ask the snapserver for its rooms again.
    pub fn refresh_snap_rooms(&mut self) {
        let Some(t) = self.snap_target() else {
            self.snap.live = SnapLive {
                note: Some(
                    "No snapserver yet. Type one in above, or pick one under Play on.".into(),
                ),
                ..Default::default()
            };
            if self.cast_found.is_empty() && !self.cast_looking() {
                self.look_for_cast_targets();
            }
            return self.refresh_or_stale(Section::Snapcast);
        };
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let r = snapcast::clients(&t);
            let _ = tx.send(SnapMsg::Clients(t, r));
        });
        self.snap.rx = Some(rx);
        self.snap.live.loading = true;
        self.snap.asked = Some(Instant::now());
        self.refresh_or_stale(Section::Snapcast);
    }

    /// Play Patina in one room, or stop. Turning a room on starts playing
    /// on its snapserver if Patina isn't already.
    pub fn set_snap_room(&mut self, client: &str, on: bool) {
        let Some(t) = self.snap_target() else {
            return self.error("No snapserver to ask");
        };
        if on && self.casting().as_ref() != Some(&t) {
            self.cast_to(Some(t.clone()));
        }
        let back = self.snap.before.get(client).cloned();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let id = client.to_string();
        std::thread::spawn(move || {
            let res = snapcast::route(&t, &id, on, back.as_deref());
            let _ = tx.send(SnapMsg::Routed {
                client: id,
                on,
                res,
            });
        });
        self.snap.rx = Some(rx);
        // Shown at once; the answer corrects it if it went wrong.
        if let Some(c) = self.snap.live.clients.iter_mut().find(|c| c.id == client) {
            c.on_patina = on;
        }
        self.refresh_or_stale(Section::Snapcast);
    }

    pub fn set_snap_volume(&mut self, client: &str, percent: i64) {
        let Some(t) = self.snap_target() else { return };
        let percent = percent.clamp(0, 100) as u8;
        let muted = self
            .snap
            .live
            .clients
            .iter()
            .find(|c| c.id == client)
            .is_some_and(|c| c.muted);
        if let Some(c) = self.snap.live.clients.iter_mut().find(|c| c.id == client) {
            c.volume = percent;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let id = client.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(SnapMsg::Done(snapcast::set_volume(&t, &id, percent, muted)));
        });
        self.snap.rx = Some(rx);
        self.refresh_or_stale(Section::Snapcast);
    }

    /// Answers from the threads above.
    pub(super) fn poll_sources(&mut self) {
        if let Some(rx) = &self.plex.rx
            && let Ok(msg) = rx.try_recv()
        {
            match msg {
                PlexMsg::Pin(code, link) => {
                    self.set_plex_shown(Some(PlexSignIn::Waiting { code, link }))
                }
                PlexMsg::Failed(e) => {
                    self.plex.rx = None;
                    self.set_plex_shown(None);
                    self.error(e);
                }
                PlexMsg::Found {
                    login,
                    account,
                    servers,
                } => {
                    self.plex.rx = None;
                    if servers.is_empty() {
                        self.set_plex_shown(None);
                        return self.error(format!(
                            "Signed in as {account}, but there are no Plex servers on that account"
                        ));
                    }
                    let choices: Vec<PlexChoice> = servers
                        .iter()
                        .map(|r| PlexChoice {
                            name: r.name.clone(),
                            id: r.id.clone(),
                            owned: r.owned,
                            added: self
                                .config
                                .servers
                                .iter()
                                .any(|s| s.kind == ServerKind::Plex && s.udn == r.id),
                        })
                        .collect();
                    self.plex.found = Some((login, account.clone(), servers));
                    self.note(format!("Signed in to Plex as {account}"));
                    let fresh: Vec<&PlexChoice> = choices.iter().filter(|c| !c.added).collect();
                    if choices.len() == 1 && fresh.len() == 1 {
                        // One server: no need to ask.
                        let id = fresh[0].id.clone();
                        self.set_plex_shown(None);
                        self.add_plex_choice(&id);
                    } else {
                        self.set_plex_shown(Some(PlexSignIn::Choose {
                            account,
                            servers: choices,
                        }));
                    }
                }
            }
        }
        if let Some(rx) = &self.snap.rx
            && let Ok(msg) = rx.try_recv()
        {
            self.snap.rx = None;
            match msg {
                SnapMsg::Clients(t, r) => {
                    self.snap.live.loading = false;
                    self.snap.live.server = Some(t.name.clone());
                    match r {
                        Ok(c) => {
                            self.snap.live.note = c
                                .is_empty()
                                .then(|| "The snapserver knows no rooms yet.".to_string());
                            self.snap.live.clients = c;
                        }
                        Err(e) => {
                            self.snap.live.clients.clear();
                            self.snap.live.note =
                                Some(format!("Couldn't ask {} for its rooms: {e}", t.name));
                        }
                    }
                }
                SnapMsg::Routed { client, on, res } => match res {
                    Ok(was) => {
                        if on {
                            self.snap.before.insert(client, was);
                        }
                        self.refresh_snap_rooms();
                    }
                    Err(e) => {
                        self.error(format!("Snapcast: {e}"));
                        self.refresh_snap_rooms();
                    }
                },
                SnapMsg::Done(Err(e)) => self.error(format!("Snapcast: {e}")),
                SnapMsg::Done(Ok(())) => {}
            }
            self.refresh_or_stale(Section::Snapcast);
        }
    }

    /// The Snapcast page opened: look at its rooms, unless that was just
    /// done.
    pub(super) fn snap_page_opened(&mut self) {
        if self
            .snap
            .asked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
        {
            self.refresh_snap_rooms();
        }
    }
}
