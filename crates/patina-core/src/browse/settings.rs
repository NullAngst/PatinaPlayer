//! The Settings view as data: every option in config.toml, with the kind
//! of control it needs and how a change applies. Both frontends draw these
//! rows and send changes back through [`super::Browser::change_setting`],
//! which writes the file (comments kept) and applies the change live where
//! it can.

use std::path::PathBuf;

use crate::audio::output::exclusive::HwDevice;
use crate::audio::output::{Backend, BitPerfect};
use crate::config::{Config, Theme};
use crate::remote::{ServerStatus, StreamFormat};

/// Which option a row is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A library folder; its row removes it.
    Folder(PathBuf),
    AddFolder,
    Watch,
    Theme,
    StartVolume,
    FadeMs,
    Backend,
    Device,
    BufferMs,
    BitPerfect,
    BitPerfectDevice,
    PodcastSpeed,
    PodcastTrim,
    /// A music server, by name: sync, edit, remove.
    Server(String),
    /// What a server streams: the original file, or a transcode.
    ServerStream(String),
    AddServer,
    /// Read-only facts: where files are.
    Info(&'static str),
}

/// How a row is edited.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Toggle(bool),
    /// Pick one. `options` are (value written to the file, label shown).
    Choice {
        options: Vec<(String, String)>,
        index: usize,
    },
    /// A whole number shown as `value / scale` with a unit: podcast speed
    /// is stored as tenths and shown as "1.2x".
    Number {
        value: i64,
        min: i64,
        max: i64,
        step: i64,
        scale: i64,
        unit: &'static str,
    },
    /// Free text, like a device name. Empty means the default.
    Text {
        value: String,
        empty: &'static str,
    },
    /// A library folder, removable.
    Folder,
    /// A button.
    Action(&'static str),
    /// A music server: its status line. Sync, Edit and Remove go with it.
    Server(String),
    Info(String),
}

impl Control {
    /// The value as the row shows it.
    pub fn display(&self) -> String {
        match self {
            Control::Toggle(true) => "on".into(),
            Control::Toggle(false) => "off".into(),
            Control::Choice { options, index } => {
                options.get(*index).map(|o| o.1.clone()).unwrap_or_default()
            }
            Control::Number {
                value, scale, unit, ..
            } => {
                if *scale == 1 {
                    format!("{value}{unit}")
                } else {
                    format!("{:.1}{unit}", *value as f64 / *scale as f64)
                }
            }
            Control::Text { value, empty } => {
                if value.is_empty() {
                    (*empty).to_string()
                } else {
                    value.clone()
                }
            }
            Control::Folder => String::new(),
            Control::Action(a) => (*a).to_string(),
            Control::Server(s) => s.clone(),
            Control::Info(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: Key,
    pub label: String,
    /// One line on what it does, for a tooltip or the TUI's hint line.
    pub help: &'static str,
    pub control: Control,
}

/// A change to a setting, as a frontend expresses it.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Flip a toggle.
    Toggle,
    /// Next or previous choice, or a number up or down by its step.
    Step(i32),
    /// Pick a choice by index.
    Choose(usize),
    /// Typed text, or a typed number.
    Set(String),
}

/// What Settings shows besides the config: where things live, and the
/// sound cards there are to pick from.
#[derive(Debug, Clone, Default)]
pub struct SettingsInfo {
    pub config_path: PathBuf,
    pub db_path: PathBuf,
    pub offline_dir: PathBuf,
    pub no_library: bool,
}

/// The warning shown before bit-perfect is turned on.
pub const BIT_PERFECT_WARNING: &str = "Bit-perfect takes exclusive control of this device. \
Other apps won't be able to play sound through it until PatinaPlayer stops or you turn this off.";

/// Stream choices for a server: (value, label). The value is
/// "format" or "format:kbps".
pub const STREAM_CHOICES: [(&str, &str); 6] = [
    ("raw", "Original file"),
    ("opus:96", "Opus 96 kbps"),
    ("opus:128", "Opus 128 kbps"),
    ("opus:192", "Opus 192 kbps"),
    ("mp3:192", "MP3 192 kbps"),
    ("mp3:320", "MP3 320 kbps"),
];

/// Read a stream choice back.
pub fn parse_stream(v: &str) -> (StreamFormat, Option<u32>) {
    let (f, k) = v.split_once(':').unwrap_or((v, ""));
    (f.parse().unwrap_or_default(), k.parse().ok())
}

/// Rows, grouped under headers, for the Settings view.
pub fn rows(
    cfg: &Config,
    info: &SettingsInfo,
    podcast_tempo: f64,
    podcast_trim: bool,
    devices: &[HwDevice],
    servers: &[ServerStatus],
) -> Vec<(Option<&'static str>, Setting)> {
    let mut v: Vec<(Option<&'static str>, Setting)> = Vec::new();
    let mut push = |group: &'static str, s: Setting| v.push((Some(group), s));

    // Library
    if !info.no_library {
        for f in &cfg.library.folders {
            push(
                "Library",
                Setting {
                    key: Key::Folder(f.clone()),
                    label: f.display().to_string(),
                    help: "A library folder. Removing it takes its tracks out of the index; the files stay where they are.",
                    control: Control::Folder,
                },
            );
        }
        push(
            "Library",
            Setting {
                key: Key::AddFolder,
                label: "Add a music folder".into(),
                help: "Index another folder. The scan starts right away.",
                control: Control::Action("Add folder"),
            },
        );
        push(
            "Library",
            Setting {
                key: Key::Watch,
                label: "Watch for changes".into(),
                help: "Pick up added, changed and removed files while the player runs.",
                control: Control::Toggle(cfg.library.watch),
            },
        );
    }

    // Servers
    if !info.no_library {
        for sc in &cfg.servers {
            let status = servers
                .iter()
                .find(|s| s.name == sc.name)
                .map(ServerStatus::summary)
                .unwrap_or_else(|| "starting".into());
            let who = if sc.user.is_empty() {
                "API key".to_string()
            } else {
                sc.user.clone()
            };
            push(
                "Servers",
                Setting {
                    key: Key::Server(sc.name.clone()),
                    label: sc.name.clone(),
                    help: "Sync reads the server's library again now. Edit changes the address, user or password.",
                    control: Control::Server(format!("{} · {who} · {status}", sc.url)),
                },
            );
            let current = match sc.stream {
                StreamFormat::Raw => "raw".to_string(),
                f => format!("{}:{}", f.as_str(), sc.bitrate),
            };
            let mut options: Vec<(String, String)> = STREAM_CHOICES
                .iter()
                .map(|(v, l)| (v.to_string(), l.to_string()))
                .collect();
            if !options.iter().any(|o| o.0 == current) {
                options.push((
                    current.clone(),
                    format!("{} {} kbps", sc.stream.as_str(), sc.bitrate),
                ));
            }
            let index = options.iter().position(|o| o.0 == current).unwrap_or(0);
            push(
                "Servers",
                Setting {
                    key: Key::ServerStream(sc.name.clone()),
                    label: format!("{}: streaming", sc.name),
                    help: "The original file keeps the sound untouched (and lets bit-perfect work). A transcode saves data on a slow line.",
                    control: Control::Choice { options, index },
                },
            );
        }
        push(
            "Servers",
            Setting {
                key: Key::AddServer,
                label: "Add a music server".into(),
                help: "Navidrome, Gonic, Airsonic and other Subsonic servers, or Ampache. The password goes in your keyring.",
                control: Control::Action("Add server"),
            },
        );
    }

    // Appearance
    push(
        "Appearance",
        Setting {
            key: Key::Theme,
            label: "Theme".into(),
            help: "How the app looks. Runout is dark verdigris and copper; the kawaii themes are pastel and rounded.",
            control: Control::Choice {
                options: Theme::ALL
                    .iter()
                    .map(|t| (t.as_str().to_string(), t.label().to_string()))
                    .collect(),
                index: Theme::ALL
                    .iter()
                    .position(|t| *t == cfg.appearance.theme)
                    .unwrap_or(0),
            },
        },
    );

    // Playback
    push(
        "Playback",
        Setting {
            key: Key::StartVolume,
            label: "Starting volume".into(),
            help: "Volume the player starts at.",
            control: Control::Number {
                value: cfg.playback.volume as i64,
                min: 0,
                max: 100,
                step: 5,
                scale: 1,
                unit: "%",
            },
        },
    );
    push(
        "Playback",
        Setting {
            key: Key::FadeMs,
            label: "Fades".into(),
            help: "Fade on pause, resume, seek and stop. 0 turns it off.",
            control: Control::Number {
                value: cfg.playback.fade_ms as i64,
                min: 0,
                max: 2000,
                step: 50,
                scale: 1,
                unit: " ms",
            },
        },
    );

    // Output
    let backends = [
        (Backend::Auto, "auto", "PipeWire, else ALSA"),
        (Backend::PipeWire, "pipewire", "PipeWire"),
        (Backend::Alsa, "alsa", "ALSA"),
        (Backend::Null, "null", "None (silent)"),
    ];
    push(
        "Output",
        Setting {
            key: Key::Backend,
            label: "Output".into(),
            help: "Where sound goes when it isn't playing bit-perfect.",
            control: Control::Choice {
                options: backends
                    .iter()
                    .map(|(_, v, l)| (v.to_string(), l.to_string()))
                    .collect(),
                index: backends
                    .iter()
                    .position(|(b, _, _)| *b == cfg.output.backend)
                    .unwrap_or(0),
            },
        },
    );
    push(
        "Output",
        Setting {
            key: Key::Device,
            label: "Device".into(),
            help: "An ALSA device (\"default\", \"hw:1,0\") or a PipeWire node name.",
            control: Control::Text {
                value: cfg.output.device.clone().unwrap_or_default(),
                empty: "system default",
            },
        },
    );
    push(
        "Output",
        Setting {
            key: Key::BufferMs,
            label: "Buffer".into(),
            help: "Output latency. Raise it if you hear dropouts.",
            control: Control::Number {
                value: cfg.output.buffer_ms as i64,
                min: 50,
                max: 1000,
                step: 50,
                scale: 1,
                unit: " ms",
            },
        },
    );

    // Bit-perfect
    let modes = [
        (BitPerfect::Off, "Off"),
        (BitPerfect::On, "On, for local files"),
        (BitPerfect::Auto, "Auto, whenever nothing changes the sound"),
    ];
    push(
        "Bit-perfect",
        Setting {
            key: Key::BitPerfect,
            label: "Bit-perfect".into(),
            help: "Local files go straight to the DAC, untouched. Takes the card for itself while playing.",
            control: Control::Choice {
                options: modes
                    .iter()
                    .map(|(m, l)| (m.as_str().to_string(), l.to_string()))
                    .collect(),
                index: modes
                    .iter()
                    .position(|(m, _)| *m == cfg.output.bit_perfect)
                    .unwrap_or(0),
            },
        },
    );
    let mut opts: Vec<(String, String)> = vec![(String::new(), "First sound card".into())];
    opts.extend(devices.iter().map(|d| (d.id.clone(), d.name.clone())));
    if let Some(cur) = &cfg.output.bit_perfect_device
        && !devices.iter().any(|d| &d.id == cur)
    {
        opts.push((cur.clone(), format!("{cur} (not connected)")));
    }
    let index = cfg
        .output
        .bit_perfect_device
        .as_ref()
        .and_then(|cur| opts.iter().position(|o| &o.0 == cur))
        .unwrap_or(0);
    push(
        "Bit-perfect",
        Setting {
            key: Key::BitPerfectDevice,
            label: "Bit-perfect device".into(),
            help: "The sound card bit-perfect playback takes over.",
            control: Control::Choice {
                options: opts,
                index,
            },
        },
    );

    // Podcasts
    push(
        "Podcasts",
        Setting {
            key: Key::PodcastSpeed,
            label: "Podcast speed".into(),
            help: "Speed with pitch kept. Podcasts only; music and radio never change speed.",
            control: Control::Number {
                value: (podcast_tempo * 10.0).round() as i64,
                min: 5,
                max: 30,
                step: 1,
                scale: 10,
                unit: "x",
            },
        },
    );
    push(
        "Podcasts",
        Setting {
            key: Key::PodcastTrim,
            label: "Trim silences".into(),
            help: "Cut pauses over 0.7 s down to a quarter second. Podcasts only.",
            control: Control::Toggle(podcast_trim),
        },
    );

    // Files
    let file = |label: &'static str, p: &PathBuf| Setting {
        key: Key::Info(label),
        label: label.into(),
        help: "",
        control: Control::Info(p.display().to_string()),
    };
    push("Files", file("Config file", &info.config_path));
    if !info.no_library {
        push("Files", file("Library index", &info.db_path));
        push("Files", file("Downloaded episodes", &info.offline_dir));
    }
    v
}

/// The new value for a numeric or choice control after `change`, or None
/// if it doesn't apply.
pub fn stepped(control: &Control, change: &Change) -> Option<Control> {
    match (control, change) {
        (Control::Toggle(b), Change::Toggle | Change::Step(_)) => Some(Control::Toggle(!b)),
        (Control::Choice { options, index }, Change::Step(d)) => {
            let n = options.len() as i64;
            if n == 0 {
                return None;
            }
            let i = (*index as i64 + *d as i64).rem_euclid(n) as usize;
            Some(Control::Choice {
                options: options.clone(),
                index: i,
            })
        }
        (Control::Choice { options, .. }, Change::Choose(i)) if *i < options.len() => {
            Some(Control::Choice {
                options: options.clone(),
                index: *i,
            })
        }
        (
            Control::Number {
                value,
                min,
                max,
                step,
                scale,
                unit,
            },
            change,
        ) => {
            let v = match change {
                Change::Step(d) => value + step * *d as i64,
                Change::Set(t) => {
                    let t = t.trim().trim_end_matches(unit.trim()).trim();
                    let f: f64 = t.parse().ok()?;
                    (f * *scale as f64).round() as i64
                }
                _ => return None,
            };
            Some(Control::Number {
                value: v.clamp(*min, *max),
                min: *min,
                max: *max,
                step: *step,
                scale: *scale,
                unit,
            })
        }
        (Control::Text { empty, .. }, Change::Set(t)) => Some(Control::Text {
            value: t.trim().to_string(),
            empty,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(value: i64) -> Control {
        Control::Number {
            value,
            min: 5,
            max: 30,
            step: 1,
            scale: 10,
            unit: "x",
        }
    }

    #[test]
    fn numbers_step_clamp_and_parse() {
        assert_eq!(stepped(&num(10), &Change::Step(1)), Some(num(11)));
        assert_eq!(stepped(&num(30), &Change::Step(1)), Some(num(30)));
        assert_eq!(
            stepped(&num(10), &Change::Set("1.5x".into())),
            Some(num(15))
        );
        assert_eq!(stepped(&num(10), &Change::Set("nope".into())), None);
        assert_eq!(num(12).display(), "1.2x");
    }

    #[test]
    fn choices_wrap() {
        let c = Control::Choice {
            options: vec![("a".into(), "A".into()), ("b".into(), "B".into())],
            index: 1,
        };
        assert_eq!(stepped(&c, &Change::Step(1)).unwrap().display(), "A");
        assert_eq!(stepped(&c, &Change::Step(-1)).unwrap().display(), "A");
    }
}
