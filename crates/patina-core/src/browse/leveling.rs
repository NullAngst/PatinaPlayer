//! Leveling from the browser's side: telling the measuring thread what's
//! coming up, its settings, and the whole-library run.

use super::Browser;
use super::settings::{Control, Key};
use crate::Command;
use crate::config;
use crate::level::{LevelEvent, service::AHEAD};

impl Browser {
    /// The queue moved: the song playing and the next few get measured
    /// first, if they need it.
    pub(super) fn feed_level(&mut self) {
        let Some(svc) = &self.level else { return };
        let infos: Vec<crate::track::TrackInfo> = {
            let q = self.player.queue();
            q.current()
                .into_iter()
                .chain(q.ahead(AHEAD))
                .filter_map(|id| q.get(id).map(|e| e.info.clone()))
                .collect()
        };
        let keys: Vec<Vec<u8>> = infos
            .iter()
            .filter_map(crate::library::loudness::song_key)
            .collect();
        if keys != self.sent_ahead {
            svc.ahead(infos);
            self.sent_ahead = keys;
        }
    }

    pub(super) fn poll_level(&mut self) {
        let Some(svc) = &self.level else { return };
        let events: Vec<LevelEvent> = svc.events().try_iter().collect();
        for ev in events {
            match ev {
                LevelEvent::Measured(_) => {}
                LevelEvent::LibraryDone { measured, failed } => {
                    self.note(match (measured, failed) {
                        (0, 0) => "Every song already has a gain".to_string(),
                        (m, 0) => format!("Measured {}", super::count(m)),
                        (m, f) => format!("Measured {}; {f} couldn't be read", super::count(m)),
                    });
                    self.sync_settings();
                }
            }
        }
    }

    /// Start or stop measuring the whole library.
    pub fn toggle_measure_library(&mut self) {
        let Some(svc) = &self.level else {
            return self.note("Leveling needs the library");
        };
        if svc.status().library_run {
            svc.stop_library();
            self.note("Stopped measuring");
        } else {
            svc.measure_library();
            self.note("Measuring untagged songs in the background");
        }
        self.sync_settings();
    }

    /// Whether a whole-library run is going. None without the service.
    pub fn measuring_library(&self) -> Option<bool> {
        self.level.as_ref().map(|s| s.status().library_run)
    }

    pub(super) fn change_level_setting(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        let mut c = self.config.leveling.clone();
        match (key, new) {
            (Key::LevelMode, Control::Choice { options, index }) => {
                let v = &options[*index].0;
                w(config::set_value(&path, "leveling", "mode", v.as_str()))?;
                c.mode = v.parse().unwrap_or_default();
            }
            (Key::LevelTarget, Control::Number { value, .. }) => {
                w(config::set_value(&path, "leveling", "target_lufs", *value))?;
                c.target = *value as f64;
            }
            (Key::LevelFallback, Control::Number { value, .. }) => {
                w(config::set_value(&path, "leveling", "fallback_db", *value))?;
                c.fallback = *value as f64;
            }
            (Key::ClipGuard, Control::Toggle(on)) => {
                w(config::set_value(&path, "leveling", "clip_guard", *on))?;
                c.clip_guard = *on;
            }
            (Key::LiveMusic, Control::Toggle(on)) => {
                w(config::set_value(&path, "leveling", "live_music", *on))?;
                c.live_music = *on;
            }
            (Key::LiveRadio, Control::Toggle(on)) => {
                w(config::set_value(&path, "leveling", "live_radio", *on))?;
                c.live_streams = *on;
            }
            _ => return Ok(()),
        }
        self.config.leveling = c.clone();
        self.player.send(Command::SetLeveling(c));
        Ok(())
    }
}
