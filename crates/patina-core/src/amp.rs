//! The amp stack as you set it: which devices, in what order, with what
//! knobs, saved in config.toml's `[amp]` and as presets in
//! `presets/amp/`. What the renderer runs is [`crate::dsp::amp`].
//!
//! In files, each device is a `[[units]]` table with its kind, whether
//! it's on, its trims, and its knobs by name. Choices are written as
//! their labels ("15 ips", "Opto"). Anything missing takes its default,
//! anything out of range is pulled in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dsp::amp::{AmpParams, Kind, MAX_UNITS, Unit};

/// Everything about the stack you can set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "SettingsFile", into = "SettingsFile")]
pub struct AmpSettings {
    /// Off bypasses the whole stack.
    pub on: bool,
    /// 2, 4 or 8.
    pub oversample: u8,
    /// The preset it came from; empty for your own.
    pub preset: String,
    /// In signal order. Each kind at most once.
    pub units: Vec<Unit>,
}

impl Default for AmpSettings {
    fn default() -> Self {
        Self {
            on: false,
            oversample: 4,
            preset: String::new(),
            units: Vec::new(),
        }
    }
}

impl AmpSettings {
    pub fn normalized(mut self) -> Self {
        self.oversample = match self.oversample {
            0..=2 => 2,
            3..=4 => 4,
            _ => 8,
        };
        let mut seen = Vec::new();
        self.units.retain(|u| {
            let fresh = !seen.contains(&u.kind);
            seen.push(u.kind);
            fresh
        });
        self.units.truncate(MAX_UNITS);
        for u in &mut self.units {
            *u = u.clamped();
        }
        self
    }

    pub fn params(&self) -> AmpParams {
        let mut p = AmpParams {
            on: self.on,
            oversample: self.oversample,
            n: self.units.len().min(MAX_UNITS),
            ..Default::default()
        };
        for (i, u) in self.units.iter().take(MAX_UNITS).enumerate() {
            p.units[i] = *u;
        }
        p
    }

    /// Same devices, order and knobs: the on switch and name aside.
    pub fn same_sound(&self, other: &AmpSettings) -> bool {
        self.oversample == other.oversample && self.units == other.units
    }

    /// Kinds not in the stack yet.
    pub fn missing(&self) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|k| !self.units.iter().any(|u| u.kind == *k))
            .collect()
    }

    /// Whether anything in it would change the sound.
    pub fn changes_sound(&self) -> bool {
        self.params().changes_sound()
    }
}

/// A device as files hold it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UnitFile {
    kind: Kind,
    #[serde(default = "yes")]
    on: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    trim_in: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    trim_out: f64,
    #[serde(flatten)]
    knobs: BTreeMap<String, toml::Value>,
}

fn yes() -> bool {
    true
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl From<UnitFile> for Unit {
    fn from(f: UnitFile) -> Unit {
        let mut u = Unit::new(f.kind);
        u.on = f.on;
        u.trim_in = f.trim_in;
        u.trim_out = f.trim_out;
        for (i, s) in f.kind.params().iter().enumerate() {
            let Some(v) = f.knobs.get(s.key) else {
                continue;
            };
            let num = match v {
                toml::Value::Float(x) => Some(*x),
                toml::Value::Integer(x) => Some(*x as f64),
                toml::Value::String(t) => s
                    .choices
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case(t.trim()))
                    .map(|i| i as f64),
                _ => None,
            };
            if let Some(n) = num {
                u.p[i] = n;
            }
        }
        u.clamped()
    }
}

impl From<Unit> for UnitFile {
    fn from(u: Unit) -> UnitFile {
        let mut knobs = BTreeMap::new();
        for (i, s) in u.kind.params().iter().enumerate() {
            let v = if s.choices.is_empty() {
                let x = u.p[i];
                if x.fract() == 0.0 && x.abs() < 1e9 {
                    toml::Value::Integer(x as i64)
                } else {
                    toml::Value::Float(x)
                }
            } else {
                toml::Value::String(s.choices[(u.p[i] as usize).min(s.choices.len() - 1)].into())
            };
            knobs.insert(s.key.to_string(), v);
        }
        UnitFile {
            kind: u.kind,
            on: u.on,
            trim_in: u.trim_in,
            trim_out: u.trim_out,
            knobs,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SettingsFile {
    on: bool,
    oversample: u8,
    #[serde(skip_serializing_if = "String::is_empty")]
    preset: String,
    units: Vec<UnitFile>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        AmpSettings::default().into()
    }
}

impl From<SettingsFile> for AmpSettings {
    fn from(f: SettingsFile) -> Self {
        AmpSettings {
            on: f.on,
            oversample: f.oversample,
            preset: f.preset,
            units: f.units.into_iter().map(Unit::from).collect(),
        }
        .normalized()
    }
}

impl From<AmpSettings> for SettingsFile {
    fn from(s: AmpSettings) -> Self {
        SettingsFile {
            on: s.on,
            oversample: s.oversample,
            preset: s.preset,
            units: s.units.into_iter().map(UnitFile::from).collect(),
        }
    }
}

/// What config.toml's `[amp]` holds: the stack for devices with none of
/// their own, and which output device gets which preset.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AmpConfig {
    #[serde(flatten)]
    pub settings: AmpSettings,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<String, String>,
}

// ----- Presets -----

fn with(kind: Kind, knobs: &[(usize, f64)]) -> Unit {
    let mut u = Unit::new(kind);
    for (i, v) in knobs {
        u.p[*i] = *v;
    }
    u
}

/// The starting set, from the design doc.
pub fn builtin(name: &str) -> Option<AmpSettings> {
    let (n, units): (&str, Vec<Unit>) = match name.trim().to_lowercase().as_str() {
        "warm glow" => (
            "Warm Glow",
            vec![
                with(Kind::TubePreamp, &[(0, 3.0), (1, 35.0), (2, 35.0)]),
                with(Kind::Transformer, &[(0, 30.0)]),
            ],
        ),
        "reel to reel" => (
            "Reel to Reel",
            vec![
                with(Kind::Tape, &[(0, 1.0), (1, 35.0), (2, 15.0), (3, 15.0)]),
                with(Kind::Console, &[(0, 20.0), (1, -45.0)]),
            ],
        ),
        "late night" => (
            "Late Night",
            vec![
                with(
                    Kind::Compressor,
                    &[
                        (0, 0.0),
                        (1, -26.0),
                        (2, 3.0),
                        (3, 15.0),
                        (4, 300.0),
                        (5, 6.0),
                    ],
                ),
                with(Kind::BassEnhancer, &[(0, 80.0), (1, 35.0)]),
            ],
        ),
        "living room console" => (
            "Living Room Console",
            vec![
                with(Kind::TubePreamp, &[(0, 4.0)]),
                with(Kind::TubePowerAmp, &[(0, 4.0), (1, 30.0)]),
                with(Kind::Transformer, &[(0, 35.0), (2, 18.0)]),
            ],
        ),
        "am radio" => (
            "AM Radio",
            vec![
                with(Kind::Transformer, &[(0, 50.0), (1, 250.0), (2, 4.5)]),
                with(
                    Kind::Compressor,
                    &[
                        (0, 1.0),
                        (1, -30.0),
                        (2, 6.0),
                        (3, 2.0),
                        (4, 120.0),
                        (5, 9.0),
                    ],
                ),
                with(Kind::TubePowerAmp, &[(0, 15.0), (1, 60.0)]),
            ],
        ),
        _ => return None,
    };
    Some(AmpSettings {
        on: true,
        oversample: 4,
        preset: n.into(),
        units,
    })
}

pub const BUILTIN: [&str; 5] = [
    "Warm Glow",
    "Reel to Reel",
    "Late Night",
    "Living Room Console",
    "AM Radio",
];

pub fn is_builtin(name: &str) -> bool {
    builtin(name).is_some()
}

pub fn dir() -> PathBuf {
    crate::config::config_dir().join("presets").join("amp")
}

pub use crate::eq::presets::{PresetError, PresetInfo};

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
    format!("{}.toml", s.trim_start_matches('.'))
}

#[derive(Serialize, Deserialize)]
struct PresetFile {
    #[serde(default)]
    name: String,
    #[serde(flatten)]
    stack: AmpSettings,
}

/// Amp presets in a folder: the built-ins first, then yours.
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

    fn read(&self, path: &Path) -> Result<PresetFile, PresetError> {
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

    pub fn list(&self) -> Vec<PresetInfo> {
        let mut v: Vec<PresetInfo> = BUILTIN
            .iter()
            .map(|n| PresetInfo {
                name: n.to_string(),
                builtin: true,
            })
            .collect();
        let mut mine: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .filter_map(|e| self.read(&e.path()).ok().map(|p| p.name))
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

    /// A preset's stack, switched on.
    pub fn load(&self, name: &str) -> Result<AmpSettings, PresetError> {
        if let Some(b) = builtin(name) {
            return Ok(b);
        }
        let path = self.dir.join(file_name(name));
        if !path.exists() {
            return Err(PresetError::Missing(name.to_string()));
        }
        let p = self.read(&path)?;
        Ok(AmpSettings {
            on: true,
            preset: p.name,
            ..p.stack
        })
    }

    pub fn exists(&self, name: &str) -> bool {
        is_builtin(name) || self.dir.join(file_name(name)).exists()
    }

    pub fn save(&self, name: &str, s: &AmpSettings) -> Result<(), PresetError> {
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
            stack: AmpSettings {
                on: true,
                preset: String::new(),
                ..s.clone()
            },
        };
        let text = toml::to_string(&f).map_err(|e| PresetError::Parse {
            path: self.dir.clone(),
            msg: e.to_string(),
        })?;
        let path = self.dir.join(file_name(name));
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("# PatinaPlayer amp stack preset\n{text}"))?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_goes_through_a_file_and_back() {
        let mut tape = Unit::new(Kind::Tape);
        tape.p[0] = 2.0;
        tape.trim_out = -1.5;
        let mut comp = Unit::new(Kind::Compressor);
        comp.p[0] = 1.0;
        comp.on = false;
        let s = AmpSettings {
            on: true,
            oversample: 8,
            preset: String::new(),
            units: vec![tape, comp, Unit::new(Kind::TubePreamp)],
        };
        let text = toml::to_string(&AmpConfig {
            settings: s.clone(),
            devices: BTreeMap::new(),
        })
        .unwrap();
        assert!(text.contains("speed = \"30 ips\""), "{text}");
        assert!(text.contains("style = \"VCA\""), "{text}");
        let back: AmpConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.settings, s);
    }

    #[test]
    fn files_are_forgiving() {
        let s: AmpSettings = toml::from_str(
            r#"
on = true
oversample = 3
[[units]]
kind = "tube-preamp"
drive = 99
[[units]]
kind = "tape"
speed = "7.5 IPS"
[[units]]
kind = "tube-preamp"
"#,
        )
        .unwrap();
        assert_eq!(s.oversample, 4);
        assert_eq!(s.units.len(), 2, "a kind only once");
        assert_eq!(s.units[0].p[0], 24.0, "pulled into range");
        assert_eq!(s.units[0].p[1], 30.0, "missing takes the default");
        assert_eq!(s.units[1].p[0], 0.0);
    }

    #[test]
    fn the_readme_example_reads() {
        let c = crate::config::Config::parse(
            r#"
[amp]
on = true
oversample = 4
preset = "Warm Glow"

[[amp.units]]
kind = "tube-preamp"
drive = 3
bias = 35

[[amp.units]]
kind = "transformer"
on = false
trim_out = -1.5

[amp.devices]
"alsa_output.pci-0000_00_1f.3.analog-stereo" = "Speakers stack"
"#,
            Path::new("x"),
        )
        .unwrap();
        let a = &c.amp;
        assert_eq!(a.settings.units.len(), 2);
        assert_eq!(a.settings.units[0].p[0], 3.0);
        assert!(!a.settings.units[1].on);
        assert_eq!(a.settings.units[1].trim_out, -1.5);
        assert_eq!(a.devices.len(), 1);
    }

    #[test]
    fn presets_save_load_and_builtins() {
        let dir = tempfile::tempdir().unwrap();
        let p = Presets::new(dir.path());
        for n in BUILTIN {
            let s = p.load(n).unwrap();
            assert!(s.changes_sound(), "{n}");
            assert_eq!(s.preset, n);
        }
        let s = builtin("warm glow").unwrap();
        p.save("Mine", &s).unwrap();
        let got = p.load("Mine").unwrap();
        assert!(got.same_sound(&s));
        assert_eq!(got.preset, "Mine");
        assert!(matches!(
            p.save("AM Radio", &s),
            Err(PresetError::Builtin(_))
        ));
        assert_eq!(p.list().last().unwrap().name, "Mine");
        p.delete("Mine").unwrap();
        assert!(!p.exists("Mine"));
    }
}
