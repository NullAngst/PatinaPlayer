//! PatinaPlayer TUI. Browses the library index and plays through the core.

mod app;
mod theme;
mod ui;
mod views;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use clap::Parser;
use patina_core::config::{self, Config};
use patina_core::library::{Library, LibraryService};
use patina_core::player::Hooks;
use patina_core::{Backend, Command, OutputConfig, Player, PlayerConfig, TrackInfo, track};

#[derive(Parser, Debug)]
#[command(
    name = "patinaplayer-tui",
    version,
    about = "PatinaPlayer, terminal frontend"
)]
struct Args {
    /// Files or folders to play right now. They go in the queue without
    /// being added to the library. Leave this off to browse the library.
    paths: Vec<PathBuf>,

    /// Add a folder to the library in config.toml, then exit.
    #[arg(long, value_name = "DIR")]
    add_folder: Option<PathBuf>,

    /// Remove a folder from the library in config.toml, then exit.
    #[arg(long, value_name = "DIR")]
    remove_folder: Option<PathBuf>,

    /// Use this config file instead of ~/.config/patinaplayer/config.toml.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Don't open the library at all.
    #[arg(long)]
    no_library: bool,

    /// Output backend: auto, pipewire, alsa or null. Overrides the config.
    #[arg(long)]
    backend: Option<Backend>,

    /// ALSA device ("default", "hw:1,0") or PipeWire node name.
    #[arg(long)]
    device: Option<String>,

    /// Output buffer in milliseconds, 50 to 1000.
    #[arg(long, value_parser = clap::value_parser!(u32).range(50..=1000))]
    buffer_ms: Option<u32>,

    /// Starting volume, 0 to 100.
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    volume: Option<u8>,

    /// Fade on pause, resume, seek and stop, in milliseconds. 0 is off.
    #[arg(long)]
    fade_ms: Option<u32>,

    /// Start with shuffle on.
    #[arg(long)]
    shuffle: bool,

    /// Write a log here. The terminal is busy drawing, so logs never go
    /// to stderr while the TUI runs.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
}

fn absolute(p: &Path) -> Result<PathBuf> {
    let expanded = config::expand_tilde(&p.to_string_lossy());
    std::fs::canonicalize(&expanded)
        .or_else(|_| std::path::absolute(&expanded))
        .with_context(|| format!("can't resolve {}", p.display()))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config.clone().unwrap_or_else(config::config_path);

    // Folder management: edit the config and leave.
    if args.add_folder.is_some() || args.remove_folder.is_some() {
        if let Some(dir) = &args.add_folder {
            let dir = absolute(dir)?;
            if !dir.is_dir() {
                bail!("{} isn't a folder", dir.display());
            }
            let changed = config::edit_library_folder(&config_path, &dir, true)?;
            println!(
                "{} {} {}",
                if changed { "Added" } else { "Already have" },
                dir.display(),
                if changed {
                    "to the library"
                } else {
                    "in the library"
                }
            );
        }
        if let Some(dir) = &args.remove_folder {
            let dir = absolute(dir).unwrap_or_else(|_| dir.clone());
            let changed = config::edit_library_folder(&config_path, &dir, false)?;
            println!(
                "{} {}",
                if changed {
                    "Removed"
                } else {
                    "Not in the library:"
                },
                dir.display()
            );
        }
        println!("Config: {}", config_path.display());
        return Ok(());
    }

    let cfg = if args.config.is_some() {
        Config::load_from(&config_path)?
    } else {
        Config::load_or_create()?
    };

    if let Some(path) = &args.log {
        let file = std::fs::File::create(path)
            .with_context(|| format!("can't write log to {}", path.display()))?;
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init();
    }

    let output = OutputConfig {
        backend: args.backend.unwrap_or(cfg.output.backend),
        device: args.device.clone().or(cfg.output.device.clone()),
        buffer_ms: args.buffer_ms.unwrap_or(cfg.output.buffer_ms),
    };
    let volume = args.volume.unwrap_or(cfg.playback.volume);
    let fade_ms = args.fade_ms.unwrap_or(cfg.playback.fade_ms);

    // Library: the index for the views, the service for scanning, and a
    // separate connection on the core thread for play counts.
    let db_path = config::library_db_path();
    let (lib, service, hook_lib) = if args.no_library {
        (None, None, None)
    } else {
        let lib = Library::open(&db_path)
            .with_context(|| format!("can't open the library at {}", db_path.display()))?;
        let svc = LibraryService::start(&db_path, cfg.library.folders.clone(), cfg.library.watch)?;
        let hook = Library::open(&db_path)?;
        (Some(lib), Some(svc), Some(Mutex::new(hook)))
    };

    let played = hook_lib.map(|l| {
        Box::new(move |t: &TrackInfo| {
            if let Err(e) = l.lock().unwrap().record_play(&t.path) {
                log::warn!("couldn't record play: {e}");
            }
        }) as patina_core::player::PlayedHook
    });
    let player = Player::spawn_with(
        PlayerConfig {
            output: output.clone(),
            volume: volume as f64 / 100.0,
            fade_ms,
        },
        Hooks {
            played,
            ..Default::default()
        },
    );

    let files = track::expand_paths(&args.paths);
    if !args.paths.is_empty() && files.is_empty() {
        bail!("no audio files found in the paths given");
    }
    if !files.is_empty() {
        if args.shuffle {
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
    } else if args.shuffle {
        player.send(Command::ToggleShuffle);
    }

    let settings = views::Settings {
        lines: settings_lines(
            &config_path,
            &cfg,
            &output,
            volume,
            fade_ms,
            &db_path,
            args.no_library,
        ),
    };
    let start = if args.paths.is_empty() {
        views::Section::Folders
    } else {
        views::Section::Queue
    };

    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let mut app = app::App::new(player.clone(), lib, service, settings);
    app.switch(start);
    let result = app.run(&mut terminal);
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    player.send(Command::Quit);
    // Give the core a moment to fade out instead of cutting the sound.
    std::thread::sleep(std::time::Duration::from_millis(
        fade_ms.min(300) as u64 + 50,
    ));
    result
}

fn settings_lines(
    config_path: &Path,
    cfg: &Config,
    output: &OutputConfig,
    volume: u8,
    fade_ms: u32,
    db: &Path,
    no_library: bool,
) -> Vec<(String, String)> {
    let mut v = vec![("Config file".to_string(), config_path.display().to_string())];
    if no_library {
        v.push(("Library".into(), "off (--no-library)".into()));
    } else if cfg.library.folders.is_empty() {
        v.push((
            "Library folders".into(),
            "none. Add one with --add-folder DIR".into(),
        ));
    } else {
        for (i, f) in cfg.library.folders.iter().enumerate() {
            let label = if i == 0 { "Library folders" } else { "" };
            v.push((label.into(), f.display().to_string()));
        }
    }
    v.push((
        "Watch for changes".into(),
        if cfg.library.watch { "on" } else { "off" }.into(),
    ));
    v.push(("Library index".into(), db.display().to_string()));
    v.push((
        "Output".into(),
        format!(
            "{:?}{}",
            output.backend,
            output
                .device
                .as_ref()
                .map(|d| format!(", device {d}"))
                .unwrap_or_default()
        )
        .to_lowercase(),
    ));
    v.push(("Buffer".into(), format!("{} ms", output.buffer_ms)));
    v.push(("Starting volume".into(), format!("{volume}%")));
    v.push((
        "Fades".into(),
        if fade_ms == 0 {
            "off".into()
        } else {
            format!("{fade_ms} ms")
        },
    ));
    v.push(("".into(), "".into()));
    v.push((
        "Changing settings".into(),
        "edit the config file and restart, since there's no settings screen yet".into(),
    ));
    v
}
