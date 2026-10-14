//! PatinaPlayer TUI. Browses the library index and plays through the core.

mod app;
mod art;
mod theme;
mod ui;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use patina_core::Backend;
use patina_core::browse::Section;
use patina_core::session::{self, Options, Session};

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

    /// Cover art: auto, kitty, sixel, iterm2, blocks or off.
    #[arg(long, default_value = "auto")]
    art: art::Mode,

    /// Write a log here. The terminal is busy drawing, so logs never go
    /// to stderr while the TUI runs.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Folder management: edit the config and leave.
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

    if let Some(path) = &args.log {
        let file = std::fs::File::create(path)
            .with_context(|| format!("can't write log to {}", path.display()))?;
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init();
    }

    let session = match Session::open(Options {
        config: args.config.clone(),
        no_library: args.no_library,
        backend: args.backend,
        device: args.device.clone(),
        buffer_ms: args.buffer_ms,
        volume: args.volume,
        fade_ms: args.fade_ms,
        shuffle: args.shuffle,
        paths: args.paths.clone(),
    }) {
        Ok(s) => s,
        Err(session::SessionError::NoFiles) => bail!("no audio files found in the paths given"),
        Err(e) => return Err(e.into()),
    };
    let start = if session.playing_files {
        Section::Queue
    } else {
        Section::Folders
    };

    let mut terminal = ratatui::init();
    // Ask the terminal what it can draw. This has to happen inside the
    // alternate screen and before the event loop starts reading input.
    let art = art::ArtView::new(args.art);
    // The query switches raw mode off when it's done, which would leave
    // keys line-buffered. Put it back.
    let _ = ratatui::crossterm::terminal::enable_raw_mode();
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let Session {
        browser, fade_ms, ..
    } = session;
    let mut app = app::App::new(browser, art);
    app.b.switch(start);
    let result = app.run(&mut terminal);
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    patina_core::session::shutdown(&mut app.b, fade_ms);
    result
}
