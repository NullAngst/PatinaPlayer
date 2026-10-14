//! PatinaPlayer desktop app. The window is Slint; everything behind it
//! (library, queue, playback, radio, podcasts) is the same core the TUI
//! runs on, through the shared Browser.

mod art;
mod bridge;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use patina_core::Backend;
use patina_core::session::{self, Options, Session};

slint::include_modules!();

#[derive(Parser, Debug)]
#[command(name = "patinaplayer", version, about = "PatinaPlayer, desktop app")]
struct Args {
    /// Files or folders to play right now, without adding them to the
    /// library.
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

    /// Write a log here as well as to stderr.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.add_folder.is_some() || args.remove_folder.is_some() {
        let lines = session::edit_folders(
            args.config.as_deref(),
            args.add_folder.as_deref(),
            args.remove_folder.as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
        for l in lines {
            println!("{l}");
        }
        return Ok(());
    }

    let mut logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));
    if let Some(path) = &args.log {
        let file = std::fs::File::create(path)
            .with_context(|| format!("can't write log to {}", path.display()))?;
        logger
            .filter_level(log::LevelFilter::Info)
            .target(env_logger::Target::Pipe(Box::new(file)));
    }
    logger.init();

    let session = Session::open(Options {
        config: args.config.clone(),
        no_library: args.no_library,
        backend: args.backend,
        device: args.device.clone(),
        buffer_ms: args.buffer_ms,
        volume: args.volume,
        fade_ms: args.fade_ms,
        shuffle: args.shuffle,
        paths: args.paths.clone(),
    })?;
    let fade_ms = session.fade_ms;
    let playing_files = session.playing_files;

    let ui = AppWindow::new().context("can't open the window")?;
    let bridge = bridge::install(&ui, session.browser, playing_files);
    ui.run().context("the window closed with an error")?;
    bridge.shutdown(fade_ms);
    Ok(())
}
