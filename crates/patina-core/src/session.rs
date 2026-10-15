//! Startup shared by both frontends: read the config, open the library and
//! podcast services, start the player with its play-count and resume hooks,
//! and hand back a ready [`Browser`]. The TUI and GUI only differ in how
//! they parse their command line and draw.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::browse::{Browser, SettingsInfo, Startup};
use crate::config::{self, Config};
use crate::library::{Library, LibraryService};
use crate::player::{Hooks, PlayedHook, ProgressHook};
use crate::podcast::PodcastService;
use crate::track::{self, MediaKind};
use crate::{Backend, Command, OutputConfig, Player, PlayerConfig, TrackInfo};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{0}")]
    Config(#[from] config::ConfigError),
    #[error("can't open the library at {path}: {source}")]
    Library {
        path: PathBuf,
        source: crate::library::LibError,
    },
    #[error("no audio files found in the paths given")]
    NoFiles,
}

/// Command line options both frontends take. Anything left `None` comes
/// from the config file.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub config: Option<PathBuf>,
    pub no_library: bool,
    pub backend: Option<Backend>,
    pub device: Option<String>,
    pub buffer_ms: Option<u32>,
    pub volume: Option<u8>,
    pub fade_ms: Option<u32>,
    pub shuffle: bool,
    /// Files or folders to queue and play right away.
    pub paths: Vec<PathBuf>,
}

pub struct Session {
    pub browser: Browser,
    pub config: Config,
    pub config_path: PathBuf,
    pub fade_ms: u32,
    /// True when files were given on the command line.
    pub playing_files: bool,
}

impl Session {
    pub fn open(opts: Options) -> Result<Self, SessionError> {
        let config_path = opts.config.clone().unwrap_or_else(config::config_path);
        let cfg = if opts.config.is_some() {
            Config::load_from(&config_path)?
        } else {
            Config::load_or_create()?
        };

        let output = OutputConfig {
            backend: opts.backend.unwrap_or(cfg.output.backend),
            device: opts.device.clone().or(cfg.output.device.clone()),
            buffer_ms: opts.buffer_ms.unwrap_or(cfg.output.buffer_ms),
            bit_perfect: cfg.output.bit_perfect,
            bit_perfect_device: cfg.output.bit_perfect_device.clone(),
            ..cfg.output.clone()
        };
        let volume = opts.volume.unwrap_or(cfg.playback.volume);
        let fade_ms = opts.fade_ms.unwrap_or(cfg.playback.fade_ms);

        // Library: the index for the views, the service for scanning, and a
        // separate connection on the core thread for play counts and
        // podcast positions.
        let db_path = config::library_db_path();
        let lib_err = |source| SessionError::Library {
            path: db_path.clone(),
            source,
        };
        let (lib, service, hook_lib) = if opts.no_library {
            (None, None, None)
        } else {
            let lib = Library::open(&db_path).map_err(lib_err)?;
            let svc =
                LibraryService::start(&db_path, cfg.library.folders.clone(), cfg.library.watch)
                    .map_err(lib_err)?;
            let hook = Library::open(&db_path).map_err(lib_err)?;
            (Some(lib), Some(svc), Some(Arc::new(Mutex::new(hook))))
        };

        let played = hook_lib.clone().map(|l| {
            Box::new(move |t: &TrackInfo| {
                // Only library files have play counts.
                if t.kind == MediaKind::File
                    && let Err(e) = l.lock().unwrap().record_play(&t.path)
                {
                    log::warn!("couldn't record play: {e}");
                }
            }) as PlayedHook
        });
        let progress = hook_lib.map(|l| {
            Box::new(move |t: &TrackInfo, pos: Duration, finished: bool| {
                if let Some(id) = t.episode
                    && let Err(e) = l.lock().unwrap().set_episode_progress(id, pos, finished)
                {
                    log::warn!("couldn't save episode position: {e}");
                }
            }) as ProgressHook
        });
        let pod = if opts.no_library {
            None
        } else {
            match PodcastService::start(&db_path, cfg.podcasts.feeds.clone()) {
                Ok(p) => Some(p),
                Err(e) => {
                    log::warn!("podcasts unavailable: {e}");
                    None
                }
            }
        };
        let player = Player::spawn_with(
            PlayerConfig {
                output: output.clone(),
                volume: volume as f64 / 100.0,
                fade_ms,
                podcast_tempo: cfg.podcasts.speed,
                podcast_trim: cfg.podcasts.trim_silence,
                leveling: cfg.leveling.clone(),
                eq: cfg.eq.settings.params(),
                amp: cfg.amp.settings.params(),
                limiter_on: cfg.limiter.on,
                limiter_ceiling_db: cfg.limiter.ceiling_db,
            },
            Hooks {
                played,
                progress,
                ..Default::default()
            },
        );

        let files = track::expand_paths(&opts.paths);
        if !opts.paths.is_empty() && files.is_empty() {
            player.send(Command::Quit);
            return Err(SessionError::NoFiles);
        }
        let playing_files = !files.is_empty();
        if playing_files {
            if opts.shuffle {
                // Shuffle first so the starting track is a random one too.
                player.send(Command::Append(files));
                player.send(Command::ToggleShuffle);
                player.send(Command::Play);
            } else {
                player.send(Command::SetQueue {
                    paths: files,
                    start: 0,
                });
            }
        } else if opts.shuffle {
            player.send(Command::ToggleShuffle);
        }

        // Music servers: passwords from the keyring, syncing in the
        // background into the same index.
        let remote = (!opts.no_library).then(|| {
            crate::remote::RemoteService::start(
                &db_path,
                cfg.servers.clone(),
                Arc::new(crate::secrets::Secrets::open()),
            )
        });
        // The cache: every server song plays through it, and prefetch,
        // pinned songs and podcast downloads share its queue.
        let cache = if opts.no_library {
            None
        } else {
            match crate::cache::CacheService::start(
                &db_path,
                cfg.cache.clone(),
                cfg.servers.iter().map(|s| s.name.clone()).collect(),
            ) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::warn!("the song cache is unavailable: {e}");
                    None
                }
            }
        };
        let mixes = if opts.no_library {
            None
        } else {
            crate::mix::MixService::start(
                &db_path,
                &crate::lastfm::effective_key(&cfg.lastfm.api_key),
            )
            .map_err(|e| log::warn!("mixes are unavailable: {e}"))
            .ok()
        };
        // Leveling: the decoder's own read connection for gains, and the
        // thread that measures songs with none.
        let level = if opts.no_library {
            crate::level::lookup::install(None);
            None
        } else {
            crate::level::lookup::install(Some(&db_path));
            crate::level::LevelService::start(&db_path)
                .map_err(|e| log::warn!("leveling can't measure songs: {e}"))
                .ok()
        };
        // Last.fm: the scrobble queue lives in the library database.
        let scrobble = if opts.no_library {
            None
        } else {
            let c = &cfg.lastfm;
            crate::scrobble::ScrobbleService::start(
                &db_path,
                crate::scrobble::LastFmSetup {
                    key: crate::lastfm::effective_key(&c.api_key),
                    secret: crate::lastfm::effective_secret(&c.api_key, &c.api_secret),
                    user: c.user.clone(),
                },
                Arc::new(crate::secrets::Secrets::open()),
            )
            .map_err(|e| log::warn!("Last.fm is unavailable: {e}"))
            .ok()
        };
        let browser = Browser::new(Startup {
            player,
            lib,
            service,
            pod,
            remote,
            cache,
            mixes,
            level,
            scrobble,
            config: cfg.clone(),
            settings_info: SettingsInfo {
                config_path: config_path.clone(),
                db_path: db_path.clone(),
                offline_dir: config::offline_dir().join("podcasts"),
                cache_dir: crate::cache::cache_root(),
                pinned_dir: crate::cache::offline_root(),
                no_library: opts.no_library,
            },
        });
        Ok(Self {
            browser,
            config: cfg,
            config_path,
            fade_ms,
            playing_files,
        })
    }
}

/// Stop the player, giving it time to fade out instead of cutting the
/// sound. Both frontends call this on the way out.
pub fn shutdown(browser: &mut Browser, fade_ms: u32) {
    // The play queue goes to its server first, if it's a server's.
    browser.before_quit();
    // An EQ change from the last half second isn't written yet.
    browser.eq_save_now();
    browser.amp_save_now();
    let player = &browser.player;
    player.send(Command::Quit);
    std::thread::sleep(Duration::from_millis(u64::from(fade_ms.min(300)) + 50));
}

/// Resolve a path from the command line, with `~` expanded.
pub fn absolute(p: &Path) -> std::io::Result<PathBuf> {
    let expanded = config::expand_tilde(&p.to_string_lossy());
    std::fs::canonicalize(&expanded).or_else(|_| std::path::absolute(&expanded))
}

/// `--add-folder` / `--remove-folder`: edit the config and report.
pub fn edit_folders(
    config: Option<&Path>,
    add: Option<&Path>,
    remove: Option<&Path>,
) -> Result<Vec<String>, String> {
    let config_path = config
        .map(Path::to_path_buf)
        .unwrap_or_else(config::config_path);
    let mut out = Vec::new();
    if let Some(dir) = add {
        let dir = absolute(dir).map_err(|e| format!("can't resolve {}: {e}", dir.display()))?;
        if !dir.is_dir() {
            return Err(format!("{} isn't a folder", dir.display()));
        }
        let changed =
            config::edit_library_folder(&config_path, &dir, true).map_err(|e| e.to_string())?;
        out.push(if changed {
            format!("Added {} to the library", dir.display())
        } else {
            format!("Already have {} in the library", dir.display())
        });
    }
    if let Some(dir) = remove {
        let dir = absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        let changed =
            config::edit_library_folder(&config_path, &dir, false).map_err(|e| e.to_string())?;
        out.push(if changed {
            format!("Removed {}", dir.display())
        } else {
            format!("Not in the library: {}", dir.display())
        });
    }
    out.push(format!("Config: {}", config_path.display()));
    Ok(out)
}
