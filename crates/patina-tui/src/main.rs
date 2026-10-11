//! PatinaPlayer TUI. Milestone 1: play local files and folders from the
//! command line, with the queue and the bottom bar.

mod app;
mod theme;
mod ui;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use patina_core::{Backend, Command, OutputConfig, Player, PlayerConfig, track};

#[derive(Parser, Debug)]
#[command(
    name = "patinaplayer-tui",
    version,
    about = "PatinaPlayer, terminal frontend"
)]
struct Args {
    /// Files and folders to play. Folders are scanned recursively.
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Output backend: auto, pipewire, alsa or null.
    #[arg(long, default_value = "auto")]
    backend: Backend,

    /// ALSA device ("default", "hw:1,0") or PipeWire node name.
    #[arg(long)]
    device: Option<String>,

    /// Output buffer in milliseconds, 50 to 1000.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u32).range(50..=1000))]
    buffer_ms: u32,

    /// Starting volume, 0 to 100.
    #[arg(long, default_value_t = 80, value_parser = clap::value_parser!(u8).range(0..=100))]
    volume: u8,

    /// Fade on pause, resume, seek and stop, in milliseconds. 0 is off.
    #[arg(long, default_value_t = 100)]
    fade_ms: u32,

    /// Start with shuffle on.
    #[arg(long)]
    shuffle: bool,

    /// Write a log here. The terminal is busy drawing, so logs never go
    /// to stderr while the TUI runs.
    #[arg(long)]
    log: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(path) = &args.log {
        let file = std::fs::File::create(path)
            .with_context(|| format!("can't write log to {}", path.display()))?;
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init();
    }

    let files = track::expand_paths(&args.paths);
    if files.is_empty() {
        bail!("no audio files found in the paths given");
    }

    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: args.backend,
            device: args.device.clone(),
            buffer_ms: args.buffer_ms,
        },
        volume: args.volume as f64 / 100.0,
        fade_ms: args.fade_ms,
    });
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

    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let result = app::App::new(player.clone()).run(&mut terminal);
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    player.send(Command::Quit);
    // Give the core a moment to fade out instead of cutting the sound.
    std::thread::sleep(std::time::Duration::from_millis(
        args.fade_ms.min(300) as u64 + 50,
    ));
    result
}
