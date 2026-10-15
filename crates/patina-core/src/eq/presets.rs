//! EQ presets: the built-in genre ones, and your own as TOML files in
//! `~/.config/patinaplayer/presets/eq/`, one per preset, so they're easy
//! to copy between machines or share.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{EqSettings, Mode};
use crate::dsp::biquad::Band;

/// Built-in presets: a 10-band curve each (31 Hz to 16 kHz). Gentle on
/// purpose; they're starting points.
pub const BUILTIN: &[(&str, [f64; 10])] = &[
    ("Flat", [0.0; 10]),
    (
        "Acoustic",
        [3.0, 3.0, 2.0, 0.5, 1.0, 1.0, 2.0, 2.5, 2.0, 1.0],
    ),
    (
        "Bass boost",
        [6.0, 5.0, 3.5, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Classical",
        [3.0, 2.0, 1.0, 0.0, 0.0, 0.0, -1.0, -0.5, 1.5, 2.5],
    ),
    (
        "Electronic",
        [5.0, 4.0, 1.5, 0.0, -2.0, 0.5, 0.0, 1.5, 4.0, 4.5],
    ),
    (
        "Hip-hop",
        [5.0, 4.5, 2.0, 2.5, -1.0, -1.0, 1.0, 0.0, 1.5, 2.5],
    ),
    ("Jazz", [3.0, 2.0, 1.0, 1.5, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0]),
    (
        "Loudness",
        [5.0, 4.0, 1.5, 0.0, -1.0, 0.0, -0.5, 1.0, 4.0, 4.5],
    ),
    (
        "Metal",
        [4.0, 3.0, 1.0, -1.0, -2.0, 0.0, 2.0, 3.0, 3.5, 3.0],
    ),
    ("Pop", [-1.0, 0.0, 1.5, 3.0, 4.0, 3.0, 1.5, 0.0, -0.5, -1.0]),
    ("Rock", [4.5, 3.5, 2.0, 0.5, -1.0, -1.0, 1.0, 2.5, 3.5, 4.5]),
    (
        "Spoken word",
        [-4.0, -3.0, -1.0, 1.0, 3.0, 4.0, 3.5, 2.0, 0.0, -2.0],
    ),
    (
        "Treble boost",
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.5, 5.0, 6.0],
    ),
    (
        "Vocal",
        [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 1.0, 0.0, -1.0],
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetInfo {
    pub name: String,
    pub builtin: bool,
}

/// A preset as its file holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct PresetFile {
    name: String,
    mode: Mode,
    graphic_bands: usize,
    graphic: Vec<f64>,
    parametric: Vec<Band>,
    preamp: f64,
    auto_preamp: bool,
}

impl Default for PresetFile {
    fn default() -> Self {
        let d = EqSettings::default();
        Self {
            name: String::new(),
            mode: d.mode,
            graphic_bands: d.graphic_bands,
            graphic: d.graphic,
            parametric: d.parametric,
            preamp: d.preamp,
            auto_preamp: d.auto_preamp,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{path}: {msg}")]
    Parse { path: PathBuf, msg: String },
    #[error("there's no preset called \"{0}\"")]
    Missing(String),
    #[error("\"{0}\" is a built-in preset; save under another name")]
    Builtin(String),
    #[error("a preset needs a name")]
    NoName,
}

/// Where your presets go.
pub fn dir() -> PathBuf {
    crate::config::config_dir().join("presets").join("eq")
}

/// A file name for a preset name: kept readable, with the characters that
/// can't be in a file name swapped out.
fn file_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim_start_matches('.');
    format!("{s}.toml")
}

pub fn builtin(name: &str) -> Option<EqSettings> {
    BUILTIN
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(n, g)| EqSettings {
            graphic: g.to_vec(),
            preset: n.to_string(),
            ..Default::default()
        })
}

pub fn is_builtin(name: &str) -> bool {
    builtin(name).is_some()
}

/// Presets in a folder. Built-ins first, then yours by name.
pub struct Presets {
    dir: PathBuf,
}

impl Presets {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> Vec<PresetInfo> {
        let mut v: Vec<PresetInfo> = BUILTIN
            .iter()
            .map(|(n, _)| PresetInfo {
                name: n.to_string(),
                builtin: true,
            })
            .collect();
        let mut mine: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .filter_map(|e| self.read_file(&e.path()).ok().map(|p| p.name))
            .filter(|n| !is_builtin(n))
            .collect();
        mine.sort_by_key(|n| n.to_lowercase());
        mine.dedup();
        v.extend(mine.into_iter().map(|name| PresetInfo {
            name,
            builtin: false,
        }));
        v
    }

    fn read_file(&self, path: &Path) -> Result<PresetFile, PresetError> {
        let text = std::fs::read_to_string(path)?;
        let mut p: PresetFile = toml::from_str(&text).map_err(|e| PresetError::Parse {
            path: path.to_path_buf(),
            msg: e.to_string(),
        })?;
        if p.name.trim().is_empty() {
            p.name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        Ok(p)
    }

    /// A preset's settings. `on` is left on; the caller keeps its own.
    pub fn load(&self, name: &str) -> Result<EqSettings, PresetError> {
        if let Some(b) = builtin(name) {
            return Ok(b);
        }
        let path = self.dir.join(file_name(name));
        if !path.exists() {
            return Err(PresetError::Missing(name.to_string()));
        }
        let p = self.read_file(&path)?;
        Ok(EqSettings {
            on: true,
            mode: p.mode,
            graphic_bands: p.graphic_bands,
            graphic: p.graphic,
            parametric: p.parametric,
            preamp: p.preamp,
            auto_preamp: p.auto_preamp,
            preset: p.name,
        }
        .normalized())
    }

    pub fn exists(&self, name: &str) -> bool {
        is_builtin(name) || self.dir.join(file_name(name)).exists()
    }

    /// Save as a preset of your own, replacing one of the same name.
    pub fn save(&self, name: &str, s: &EqSettings) -> Result<(), PresetError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PresetError::NoName);
        }
        if is_builtin(name) {
            return Err(PresetError::Builtin(name.to_string()));
        }
        std::fs::create_dir_all(&self.dir)?;
        let f = PresetFile {
            name: name.to_string(),
            mode: s.mode,
            graphic_bands: s.graphic_bands,
            graphic: s.graphic.clone(),
            parametric: s.parametric.clone(),
            preamp: s.preamp,
            auto_preamp: s.auto_preamp,
        };
        let text = toml::to_string(&f).map_err(|e| PresetError::Parse {
            path: self.dir.clone(),
            msg: e.to_string(),
        })?;
        let path = self.dir.join(file_name(name));
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("# PatinaPlayer EQ preset\n{text}"))?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<(), PresetError> {
        if is_builtin(name) {
            return Err(PresetError::Builtin(name.to_string()));
        }
        let path = self.dir.join(file_name(name));
        if !path.exists() {
            return Err(PresetError::Missing(name.to_string()));
        }
        std::fs::remove_file(path)?;
        Ok(())
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), PresetError> {
        let s = self.load(from)?;
        if is_builtin(from) {
            return Err(PresetError::Builtin(from.to_string()));
        }
        self.save(to, &s)?;
        if file_name(from) != file_name(to) {
            self.delete(from)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::biquad::Kind;

    #[test]
    fn save_load_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let p = Presets::new(dir.path().join("eq"));
        let mut s = EqSettings {
            mode: Mode::Parametric,
            parametric: vec![Band {
                kind: Kind::LowShelf,
                freq: 105.0,
                gain: 4.5,
                q: 0.7,
            }],
            preamp: -3.0,
            ..Default::default()
        };
        p.save("HD 650 / AutoEq", &s).unwrap();
        let got = p.load("HD 650 / AutoEq").unwrap();
        s.preset = "HD 650 / AutoEq".into();
        assert!(got.same_sound(&s));
        assert_eq!(got.preset, "HD 650 / AutoEq");
        let names: Vec<String> = p.list().into_iter().map(|i| i.name).collect();
        assert_eq!(names[0], "Flat");
        assert_eq!(names.last().unwrap(), "HD 650 / AutoEq");
        assert!(matches!(p.save("rock", &s), Err(PresetError::Builtin(_))));
        p.rename("HD 650 / AutoEq", "HD650").unwrap();
        assert!(p.exists("HD650") && !p.exists("HD 650 / AutoEq"));
        p.delete("HD650").unwrap();
        assert!(matches!(p.load("HD650"), Err(PresetError::Missing(_))));
    }

    #[test]
    fn builtins_load_as_ten_band() {
        let p = Presets::new("/nonexistent");
        let r = p.load("rock").unwrap();
        assert_eq!(r.graphic_bands, 10);
        assert_eq!(r.preset, "Rock");
        assert!(p.list().iter().all(|i| i.builtin));
    }
}
