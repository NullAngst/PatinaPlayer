//! Cover art in the terminal, through kitty, sixel or iTerm2 graphics where
//! the terminal supports them and Unicode half blocks everywhere else.
//! Loading happens on a thread; the draw loop only picks up the result.

use std::path::PathBuf;
use std::sync::mpsc;

use patina_core::TrackInfo;
use patina_core::track::MediaKind;
use ratatui_image::StatefulImage;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Kitty,
    Sixel,
    Iterm2,
    Blocks,
    Off,
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "kitty" => Ok(Self::Kitty),
            "sixel" => Ok(Self::Sixel),
            "iterm2" | "iterm" => Ok(Self::Iterm2),
            "blocks" | "halfblocks" => Ok(Self::Blocks),
            "off" | "none" => Ok(Self::Off),
            _ => Err(format!(
                "unknown art mode '{s}' (auto, kitty, sixel, iterm2, blocks, off)"
            )),
        }
    }
}

/// Pixel size art is loaded at. Big enough for a sharp panel in a
/// terminal with large cells; the protocol scales it to fit.
const LOAD_SIZE: u32 = 512;

pub struct ArtView {
    picker: Option<Picker>,
    /// The file whose art is showing (or loading).
    current: Option<PathBuf>,
    pub image: Option<StatefulProtocol>,
    rx: Option<mpsc::Receiver<(PathBuf, Option<image::RgbaImage>)>>,
}

impl ArtView {
    pub fn new(mode: Mode) -> Self {
        let picker = match mode {
            Mode::Off => None,
            Mode::Blocks => Some(Picker::halfblocks()),
            Mode::Auto => Some(Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())),
            m => {
                let mut p = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
                p.set_protocol_type(match m {
                    Mode::Kitty => ProtocolType::Kitty,
                    Mode::Sixel => ProtocolType::Sixel,
                    Mode::Iterm2 => ProtocolType::Iterm2,
                    _ => ProtocolType::Halfblocks,
                });
                Some(p)
            }
        };
        Self {
            picker,
            current: None,
            image: None,
            rx: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    /// Follow the playing track. Starts a load when it changes, and picks
    /// up a finished load.
    pub fn update(&mut self, track: Option<&TrackInfo>) {
        let Some(picker) = &self.picker else { return };
        if let Some(rx) = &self.rx
            && let Ok((path, img)) = rx.try_recv()
        {
            self.rx = None;
            if self.current.as_ref() == Some(&path) {
                self.image =
                    img.map(|i| picker.new_resize_protocol(image::DynamicImage::ImageRgba8(i)));
            }
        }
        // Only local files have art we can find; streams and episodes
        // without a download have none for now.
        let want = track
            .filter(|t| t.kind != MediaKind::Radio && !t.path.as_os_str().is_empty())
            .map(|t| t.path.clone());
        if want == self.current {
            return;
        }
        self.current = want.clone();
        self.image = None;
        if let Some(path) = want {
            let (tx, rx) = mpsc::channel();
            self.rx = Some(rx);
            std::thread::spawn(move || {
                let img = patina_core::art::load(&path, LOAD_SIZE);
                let _ = tx.send((path, img));
            });
        }
    }

    pub fn widget() -> StatefulImage<StatefulProtocol> {
        StatefulImage::default()
    }
}
