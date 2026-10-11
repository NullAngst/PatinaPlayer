//! Settings file and the standard places PatinaPlayer keeps things.
//!
//! The config is plain TOML at `~/.config/patinaplayer/config.toml`. It is
//! read with serde and edited with toml_edit, so a change made by the app
//! (adding a library folder, say) keeps your comments and layout.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::audio::output::{Backend, OutputConfig};

pub const APP_DIR: &str = "patinaplayer";

/// `~/.config/patinaplayer`
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| home().join(".config"))
        .join(APP_DIR)
}

/// `~/.local/share/patinaplayer`, where the library index and pinned
/// offline files live. Kept out of `~/.cache` on purpose, since cleaners
/// like BleachBit wipe that.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home().join(".local/share"))
        .join(APP_DIR)
}

/// `~/.cache/patinaplayer`
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home().join(".cache"))
        .join(APP_DIR)
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn library_db_path() -> PathBuf {
    data_dir().join("library.db")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Expand a leading `~/` to the home directory.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Write a path back with `~/` when it lives under home, so the file stays
/// readable and portable between accounts.
fn contract_tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        Ok(_) => "~".into(),
        Err(_) => p.display().to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub library: LibraryConfig,
    pub output: OutputConfig,
    pub playback: PlaybackConfig,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibraryConfig {
    /// Local folder roots. Each one shows as its own source.
    pub folders: Vec<PathBuf>,
    /// Watch the folders for changes while running.
    pub watch: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackConfig {
    /// Starting volume, 0 to 100.
    pub volume: u8,
    pub fade_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library: LibraryConfig {
                folders: Vec::new(),
                watch: true,
            },
            output: OutputConfig::default(),
            playback: PlaybackConfig {
                volume: 80,
                fade_ms: 100,
            },
        }
    }
}

// Raw serde shapes. Everything optional, so a half-written file still
// loads and falls back to defaults for what it leaves out.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    library: Option<RawLibrary>,
    output: Option<RawOutput>,
    playback: Option<RawPlayback>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLibrary {
    folders: Option<Vec<String>>,
    watch: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    backend: Option<String>,
    device: Option<String>,
    buffer_ms: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPlayback {
    volume: Option<u8>,
    fade_ms: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("can't read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {msg}")]
    Parse { path: PathBuf, msg: String },
}

impl Config {
    /// Parse config text. `path` is only used in error messages.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            msg: e.to_string(),
        })?;
        let d = Config::default();
        let lib = raw.library.unwrap_or_default();
        let out = raw.output.unwrap_or_default();
        let pb = raw.playback.unwrap_or_default();
        let backend = match out.backend {
            Some(b) => b.parse::<Backend>().map_err(|msg| ConfigError::Parse {
                path: path.to_path_buf(),
                msg: format!("output.backend: {msg}"),
            })?,
            None => d.output.backend,
        };
        Ok(Self {
            library: LibraryConfig {
                folders: lib
                    .folders
                    .unwrap_or_default()
                    .iter()
                    .map(|f| expand_tilde(f))
                    .collect(),
                watch: lib.watch.unwrap_or(d.library.watch),
            },
            output: OutputConfig {
                backend,
                device: out.device.filter(|d| !d.is_empty()),
                buffer_ms: out.buffer_ms.unwrap_or(d.output.buffer_ms).clamp(50, 1000),
            },
            playback: PlaybackConfig {
                volume: pb.volume.unwrap_or(d.playback.volume).min(100),
                fade_ms: pb.fade_ms.unwrap_or(d.playback.fade_ms).min(2000),
            },
        })
    }

    /// Load the config, writing a commented default first if there is none.
    pub fn load_or_create() -> Result<Self, ConfigError> {
        let path = config_path();
        if !path.exists() {
            let text = default_text();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, &text).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
            return Self::parse(&text, &path);
        }
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text, path)
    }
}

/// The file written on first run. Your music folder goes in if the
/// system knows where it is.
pub fn default_text() -> String {
    let folder = dirs::audio_dir()
        .filter(|d| d.is_dir())
        .map(|d| format!("\"{}\"", contract_tilde(&d)))
        .unwrap_or_default();
    format!(
        r#"# PatinaPlayer settings. Edit freely; comments are kept when the app
# changes something here itself.

[library]
# Local folders to index. Each one shows up as its own source.
folders = [{folder}]
# Pick up added, changed and removed files while the player runs.
watch = true

[output]
# auto tries PipeWire, then ALSA. Also: pipewire, alsa, null.
backend = "auto"
# ALSA device ("default", "hw:1,0") or PipeWire node name. Empty is the
# system default.
device = ""
# Output buffer in milliseconds, 50 to 1000.
buffer_ms = 200

[playback]
# Starting volume, 0 to 100.
volume = 80
# Fade on pause, resume, seek and stop, in milliseconds. 0 turns it off.
fade_ms = 100
"#
    )
}

/// Add or remove a library folder in the config file, keeping the rest of
/// the file as you wrote it. Returns false if nothing needed changing.
pub fn edit_library_folder(path: &Path, folder: &Path, add: bool) -> Result<bool, ConfigError> {
    let text = if path.exists() {
        std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?
    } else {
        default_text()
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| ConfigError::Parse {
                path: path.to_path_buf(),
                msg: e.to_string(),
            })?;
    let lib = doc
        .entry("library")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or_else(|| ConfigError::Parse {
            path: path.to_path_buf(),
            msg: "[library] is not a table".into(),
        })?;
    let arr = lib
        .entry("folders")
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
        .ok_or_else(|| ConfigError::Parse {
            path: path.to_path_buf(),
            msg: "library.folders is not a list".into(),
        })?;

    let same = |v: &toml_edit::Value| {
        v.as_str()
            .map(|s| expand_tilde(s) == folder)
            .unwrap_or(false)
    };
    let changed = if add {
        if arr.iter().any(same) {
            false
        } else {
            arr.push(contract_tilde(folder));
            true
        }
    } else {
        let before = arr.len();
        arr.retain(|v| !same(v));
        arr.len() != before
    };
    if changed {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, doc.to_string()).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_text_parses() {
        let c = Config::parse(&default_text(), Path::new("x")).unwrap();
        assert_eq!(c.playback.volume, 80);
        assert!(c.library.watch);
    }

    #[test]
    fn partial_file_falls_back() {
        let c = Config::parse("[output]\nbuffer_ms = 5000\n", Path::new("x")).unwrap();
        assert_eq!(c.output.buffer_ms, 1000, "clamped");
        assert_eq!(c.playback.fade_ms, 100);
    }

    #[test]
    fn typo_is_an_error() {
        assert!(Config::parse("[output]\nbufer_ms = 5\n", Path::new("x")).is_err());
        assert!(Config::parse("[output]\nbackend = \"oss\"\n", Path::new("x")).is_err());
    }

    #[test]
    fn tilde_expands() {
        let c = Config::parse("[library]\nfolders = [\"~/Music\"]\n", Path::new("x")).unwrap();
        assert_eq!(c.library.folders[0], home().join("Music"));
    }

    #[test]
    fn add_folder_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "# mine\n[library]\nfolders = [] # roots\n").unwrap();
        assert!(edit_library_folder(&p, Path::new("/srv/music"), true).unwrap());
        assert!(!edit_library_folder(&p, Path::new("/srv/music"), true).unwrap());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("# mine") && text.contains("# roots"),
            "{text}"
        );
        assert!(text.contains("/srv/music"));
        assert!(edit_library_folder(&p, Path::new("/srv/music"), false).unwrap());
        let c = Config::load_from(&p).unwrap();
        assert!(c.library.folders.is_empty());
    }
}
