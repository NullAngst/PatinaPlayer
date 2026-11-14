//! The source browsing shows: all of them, the local folders, or one
//! server. The pick lives in the config, and the library's queries follow
//! it (see [`crate::library::scope`]).

use super::{Browser, Section};
use crate::config;
use crate::library::scope::{Pick, Scope};

/// The sections a scope change reads again from the top.
const BROWSING: [Section; 8] = [
    Section::Folders,
    Section::Artists,
    Section::Albums,
    Section::Songs,
    Section::Genres,
    Section::MostListened,
    Section::TopAlbums,
    Section::TopArtists,
];

/// What the source picker offers: all, local (when there are folders),
/// then each server with songs, by name.
pub fn picks(cfg: &config::Config) -> Vec<Pick> {
    let mut out = vec![Pick::All];
    if !cfg.library.folders.is_empty() {
        out.push(Pick::Local);
    }
    out.extend(
        cfg.servers
            .iter()
            .filter(|s| s.kind.has_songs())
            .map(|s| Pick::Server(s.name.clone())),
    );
    if !out.contains(&cfg.library.scope) {
        out.push(cfg.library.scope.clone());
    }
    out
}

impl Browser {
    /// What the source picker offers.
    pub fn scope_choices(&self) -> Vec<Pick> {
        picks(&self.config)
    }

    pub fn scope(&self) -> &Pick {
        &self.config.library.scope
    }

    /// Hand the scope to the library: this window's connection, and the
    /// process's for the others (the car's browsing, mixes).
    pub(crate) fn apply_scope(&mut self) {
        let scope = Scope {
            pick: self.config.library.scope.clone(),
            order: self.config.library.source_order.clone(),
            away: self.scope_away.clone(),
        };
        if let Some(lib) = &self.lib {
            lib.set_scope(scope.clone());
        }
        crate::library::scope::set_global(scope);
    }

    /// Browse another source. Views inside the old one go back to the top,
    /// since what they showed may not be in the new one.
    pub fn set_scope(&mut self, pick: Pick) {
        if pick == self.config.library.scope {
            return;
        }
        if let Err(e) = config::set_value(&self.config_path, "library", "scope", pick.to_config()) {
            self.note(format!("Couldn't save the source: {e}"));
        }
        self.config.library.scope = pick;
        self.scope_changed();
        let label = self.config.library.scope.label();
        self.note(format!("Showing {label}"));
    }

    /// The next source in the picker, for a key that steps through them.
    pub fn cycle_scope(&mut self) {
        let choices = self.scope_choices();
        let at = choices
            .iter()
            .position(|p| p == &self.config.library.scope)
            .unwrap_or(0);
        let next = choices[(at + 1) % choices.len()].clone();
        self.set_scope(next);
    }

    /// Servers came or went: with all sources shown, an album shared with
    /// a server now away shows from one that can play it.
    pub(crate) fn scope_servers_away(&mut self, mut away: Vec<String>) {
        away.sort();
        if away == self.scope_away {
            return;
        }
        self.scope_away = away;
        self.apply_scope();
        if self.config.library.scope == Pick::All {
            self.refresh_visible();
        }
    }

    /// The scope or the order changed: read the browsing views again.
    pub(crate) fn scope_changed(&mut self) {
        self.apply_scope();
        for s in BROWSING {
            if let Some(stack) = self.stacks.get_mut(&s) {
                stack.truncate(1);
            }
            self.forward.remove(&s);
        }
        self.refresh_all();
        // Smart playlists and mixes count their songs in the sidebar.
        self.rebuild_sidebar();
        self.touch();
    }
}
