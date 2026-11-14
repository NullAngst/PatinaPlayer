//! Other devices: playing on one ([`crate::cast`]), letting another
//! player control this one, and finding one to control
//! ([`crate::control`]).

use std::time::Duration;

use super::Browser;
use super::settings::{Control, Key};
use crate::cast::Target;
use crate::config;
use crate::player::Command;

/// How long a search of the network takes.
const LOOK: Duration = Duration::from_secs(3);

impl Browser {
    // ----- Playing on another device -----

    /// Look for devices to play on, off this thread. What's found shows
    /// in [`Self::cast_targets`] as it comes in.
    pub fn look_for_cast_targets(&mut self) {
        if self.cast_job.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(crate::cast::discover(LOOK));
        });
        self.cast_job = Some(rx);
        self.cast_generation += 1;
    }

    /// A search for devices is going.
    pub fn cast_looking(&self) -> bool {
        self.cast_job.is_some()
    }

    /// Devices to play on: those found, the snapserver typed in Settings,
    /// and the one playing now, if it's gone quiet on the network.
    pub fn cast_targets(&self) -> Vec<Target> {
        let mut v = self.cast_found.clone();
        let typed = self
            .config
            .cast
            .snapcast
            .as_deref()
            .and_then(Target::snapcast);
        for t in typed.into_iter().chain(self.casting()) {
            if !v.iter().any(|f| f.kind == t.kind && f.addr == t.addr) {
                v.push(t);
            }
        }
        v
    }

    /// The device playing now, when it isn't this one.
    pub fn casting(&self) -> Option<Target> {
        self.player.snapshot().cast
    }

    /// Play on `t`, or back here with None.
    pub fn cast_to(&mut self, t: Option<Target>) {
        if self.casting() == t {
            return;
        }
        match &t {
            Some(t) => self.note(format!(
                "Playing on {}. {}",
                t.name,
                match t.kind {
                    crate::cast::Kind::Snapcast => "",
                    _ => "It starts a few seconds behind, and pause takes as long.",
                }
            )),
            None => self.note("Playing here again"),
        }
        self.player.send(Command::SetCast(t));
        self.cast_generation += 1;
        self.touch();
    }

    // ----- Being controlled -----

    /// Start or stop the remote control server to match the config.
    pub(super) fn apply_remote_control(&mut self) {
        let want = self.config.remote.enabled && !self.config.remote.code.is_empty();
        let port = self.config.remote.port;
        let running = self.remote_server.as_ref().map(|s| s.port());
        if !want {
            self.remote_server = None;
            return;
        }
        if running == Some(port) {
            return;
        }
        self.remote_server = None;
        match crate::control::Server::start(
            self.player.clone(),
            port,
            &self.config.remote.code,
            self.remote_asks.0.clone(),
            self.remote_loved.clone(),
        ) {
            Ok(s) => self.remote_server = Some(s),
            Err(e) => self.error(format!("Remote control can't listen on port {port}: {e}")),
        }
    }

    /// A new code for the remote control: whoever had the old one is out.
    pub fn new_remote_code(&mut self) {
        let code = crate::control::new_code();
        if let Err(e) = config::set_value(&self.config_path, "remote", "code", code.as_str()) {
            return self.error(format!("couldn't save to the config: {e}"));
        }
        self.config.remote.code = code.clone();
        // The server keeps the code it started with: start it again.
        self.remote_server = None;
        self.apply_remote_control();
        self.note(format!("The remote control code is now {code}"));
        self.refresh_or_stale(super::Section::Settings);
    }

    pub(super) fn change_cast_setting(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        match (key, new) {
            (Key::CastSnapcast, Control::Text { value, .. }) => {
                let v = value.trim();
                if !v.is_empty() && Target::snapcast(v).is_none() {
                    return Err(format!("\"{v}\" isn't a host or host:port"));
                }
                w(config::set_value(&path, "cast", "snapcast", v))?;
                self.config.cast.snapcast = (!v.is_empty()).then(|| v.to_string());
                self.cast_generation += 1;
            }
            (Key::RemoteEnabled, Control::Toggle(on)) => {
                if *on && self.config.remote.code.is_empty() {
                    let code = crate::control::new_code();
                    w(config::set_value(&path, "remote", "code", code.as_str()))?;
                    self.config.remote.code = code;
                }
                w(config::set_value(&path, "remote", "enabled", *on))?;
                self.config.remote.enabled = *on;
                self.apply_remote_control();
                if *on && self.remote_server.is_some() {
                    self.note(format!(
                        "Other players can control this one with code {}",
                        self.config.remote.code
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ----- Controlling another player -----

    /// Look for players to control, off this thread.
    pub fn look_for_players(&mut self) {
        if self.control_job.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(crate::control::discover(LOOK));
        });
        self.control_job = Some(rx);
        self.control_generation += 1;
    }

    pub fn players_looking(&self) -> bool {
        self.control_job.is_some()
    }

    /// Players found to control, and the one controlled last if it
    /// didn't answer this time.
    pub fn players_found(&self) -> Vec<crate::control::Found> {
        let mut v = self.control_found.clone();
        if let Some(last) = &self.config.remote.control
            && !v.iter().any(|f| &f.addr == last)
        {
            v.push(crate::control::Found {
                name: format!("{last} (last time)"),
                addr: last.clone(),
            });
        }
        v
    }

    /// The code saved for the player at `addr`, if it's the last one.
    pub fn player_code(&self, addr: &str) -> Option<String> {
        (self.config.remote.control.as_deref() == Some(addr)
            && !self.config.remote.control_code.is_empty())
        .then(|| self.config.remote.control_code.clone())
    }

    /// Remember the player controlled and its code, for next time.
    pub fn remember_player(&mut self, addr: &str, code: &str) {
        let path = self.config_path.clone();
        let saved = config::set_value(&path, "remote", "control", addr)
            .and_then(|_| config::set_value(&path, "remote", "control_code", code));
        if let Err(e) = saved {
            self.error(format!("couldn't save to the config: {e}"));
        }
        self.config.remote.control = Some(addr.to_string());
        self.config.remote.control_code = code.to_string();
    }

    // ----- Every poll -----

    pub(super) fn poll_cast(&mut self) {
        if let Some(rx) = &self.cast_job
            && let Ok(found) = rx.try_recv()
        {
            self.cast_job = None;
            self.cast_found = found;
            self.cast_generation += 1;
        }
        if let Some(rx) = &self.control_job
            && let Ok(found) = rx.try_recv()
        {
            self.control_job = None;
            self.control_found = found;
            self.control_generation += 1;
        }
        let asks: Vec<_> = self.remote_asks.1.try_iter().collect();
        for a in asks {
            match a {
                crate::control::Ask::Love => self.toggle_love(),
            }
        }
        *self.remote_loved.lock().unwrap() = self.loved_now;
    }
}
