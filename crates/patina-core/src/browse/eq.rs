//! The EQ from the browser's side: the EQ view's rows, every change to
//! the EQ, presets, imports, and switching EQ when the output device
//! changes.
//!
//! Which EQ plays: a device you linked a preset to gets that preset;
//! every other device gets the EQ in config.toml's `[eq]`. A change made
//! while a linked device plays is saved to its preset, so each device
//! keeps its own. On and off (bypass) is one switch for all of them.

use std::path::Path;
use std::time::{Duration, Instant};

use super::Browser;
use super::settings::{Change, Control, Key, Setting};
use crate::Command;
use crate::audio::output::BitPerfect;
use crate::config;
use crate::dsp::biquad::{Band, Kind};
use crate::eq::presets::{self, Presets};
use crate::eq::{
    self, EqConfig, EqSettings, GRAPHIC_RANGE, MAX_PARAMETRIC, Mode, OutputDevice, freq_label,
};

/// Changes are written this long after the last one, so a drag doesn't
/// write the file a hundred times.
const SAVE_AFTER: Duration = Duration::from_millis(600);

pub const EQ_BIT_PERFECT: &str =
    "Bit-perfect is on, and it skips the EQ. Set bit-perfect to Off or Auto to use it.";

/// The EQ as the browser keeps it.
pub struct EqState {
    /// What config.toml holds: the EQ for unlinked devices, and the links.
    pub cfg: EqConfig,
    /// What's playing now.
    pub active: EqSettings,
    /// The preset `active` came from, as it was loaded, to tell whether
    /// it's been changed since.
    loaded: Option<EqSettings>,
    /// Where the sound is going.
    pub device: Option<OutputDevice>,
    /// The device's own preset, when it has one.
    pub linked: Option<String>,
    pub presets: Presets,
    dirty: Option<Instant>,
}

impl EqState {
    pub fn new(cfg: EqConfig, presets: Presets) -> Self {
        let active = cfg.settings.clone();
        let loaded = presets.load(&active.preset).ok();
        Self {
            cfg,
            active,
            loaded,
            device: None,
            linked: None,
            presets,
            dirty: None,
        }
    }

    /// "Rock", or "Rock (changed)" once you've moved something.
    pub fn preset_label(&self) -> String {
        let name = &self.active.preset;
        if name.is_empty() {
            return "Your own".into();
        }
        match &self.loaded {
            Some(l) if l.same_sound(&self.active) => name.clone(),
            _ => format!("{name} (changed)"),
        }
    }

    pub fn changed(&self) -> bool {
        !self
            .loaded
            .as_ref()
            .is_some_and(|l| l.same_sound(&self.active))
    }
}

impl Browser {
    fn eq_greyed(&self) -> bool {
        self.config.output.bit_perfect == BitPerfect::On
    }

    /// Send the EQ to the player, and save it soon.
    fn eq_apply(&mut self) {
        self.player
            .send(Command::SetEq(Box::new(self.eq.active.params())));
        self.eq.dirty = Some(Instant::now());
        self.refresh_section(super::Section::Eq);
        self.touch();
    }

    /// Write what changed, once the changes stop for a moment.
    pub(super) fn eq_tick(&mut self) {
        let snap_dev = self.player.snapshot().output_device;
        if snap_dev.is_some() && snap_dev != self.eq.device {
            self.eq_device_changed(snap_dev);
        }
        if let Some(t) = self.eq.dirty
            && t.elapsed() >= SAVE_AFTER
        {
            self.eq_save_now();
        }
    }

    /// Write any change not saved yet, now. Also called on quit.
    pub fn eq_save_now(&mut self) {
        if self.eq.dirty.take().is_none() {
            return;
        }
        let a = self.eq.active.clone();
        self.eq.cfg.settings.on = a.on;
        if let Some(name) = self.eq.linked.clone() {
            match self.eq.presets.save(&name, &a) {
                // The device's preset is what plays now: nothing changed
                // from it any more.
                Ok(()) => self.eq.loaded = Some(a.clone()),
                Err(e) => self.error(format!("couldn't save the EQ for this device: {e}")),
            }
        } else {
            self.eq.cfg.settings = a;
        }
        if let Err(e) = config::set_eq(&self.config_path, &self.eq.cfg) {
            self.error(format!("couldn't save the EQ: {e}"));
        }
        self.config.eq = self.eq.cfg.clone();
    }

    fn eq_device_changed(&mut self, dev: Option<OutputDevice>) {
        self.eq_save_now();
        let on = self.eq.active.on;
        self.eq.device = dev.clone();
        let Some(dev) = dev else { return };
        let link = self.eq.cfg.devices.get(&dev.key).cloned();
        match link {
            Some(name) => match self.eq.presets.load(&name) {
                Ok(s) => {
                    self.eq.linked = Some(name.clone());
                    self.eq.loaded = Some(s.clone());
                    self.eq.active = EqSettings { on, ..s };
                    self.note(format!("EQ: {name}, for {}", dev.name));
                }
                Err(e) => {
                    self.error(format!("the EQ linked to {} is gone: {e}", dev.name));
                    self.eq.linked = None;
                }
            },
            None if self.eq.linked.is_some() => {
                self.eq.linked = None;
                let s = self.eq.cfg.settings.clone();
                self.eq.loaded = self.eq.presets.load(&s.preset).ok();
                self.eq.active = EqSettings { on, ..s };
                self.note(format!("EQ: back to your usual EQ, for {}", dev.name));
            }
            None => {}
        }
        self.player
            .send(Command::SetEq(Box::new(self.eq.active.params())));
        self.refresh_section(super::Section::Eq);
        self.touch();
    }

    // ----- Changes -----

    pub fn eq_settings(&self) -> &EqSettings {
        &self.eq.active
    }

    fn eq_edit(&mut self, f: impl FnOnce(&mut EqSettings)) {
        if self.eq_greyed() {
            return self.note(EQ_BIT_PERFECT);
        }
        f(&mut self.eq.active);
        self.eq.active = std::mem::take(&mut self.eq.active).normalized();
        self.eq_apply();
    }

    pub fn eq_toggle(&mut self) {
        if self.eq_greyed() {
            return self.note(EQ_BIT_PERFECT);
        }
        self.eq.active.on = !self.eq.active.on;
        self.note(if self.eq.active.on {
            "EQ on"
        } else {
            "EQ bypassed"
        });
        self.eq_apply();
    }

    pub fn eq_set_mode(&mut self, m: Mode) {
        self.eq_edit(|s| s.mode = m);
    }

    pub fn eq_set_graphic_bands(&mut self, n: usize) {
        self.eq_edit(|s| s.set_graphic_bands(n));
    }

    pub fn eq_set_gain(&mut self, band: usize, db: f64) {
        self.eq_edit(|s| match s.mode {
            Mode::Graphic => {
                if let Some(g) = s.graphic.get_mut(band) {
                    *g = (db.clamp(-GRAPHIC_RANGE, GRAPHIC_RANGE) * 10.0).round() / 10.0;
                }
            }
            Mode::Parametric => {
                if let Some(b) = s.parametric.get_mut(band) {
                    b.gain = (db * 10.0).round() / 10.0;
                }
            }
        });
    }

    pub fn eq_nudge_gain(&mut self, band: usize, by: f64) {
        let cur = match self.eq.active.mode {
            Mode::Graphic => self.eq.active.graphic.get(band).copied(),
            Mode::Parametric => self.eq.active.parametric.get(band).map(|b| b.gain),
        };
        if let Some(c) = cur {
            self.eq_set_gain(band, c + by);
        }
    }

    pub fn eq_set_band(&mut self, i: usize, b: Band) {
        self.eq_edit(|s| {
            if let Some(slot) = s.parametric.get_mut(i) {
                *slot = b;
            }
        });
    }

    /// Move a parametric band's frequency by `steps` twelfths of an octave.
    pub fn eq_nudge_freq(&mut self, i: usize, steps: f64) {
        if let Some(mut b) = self.eq.active.parametric.get(i).copied() {
            b.freq = (b.freq * 2f64.powf(steps / 12.0)).round();
            self.eq_set_band(i, b);
        }
    }

    pub fn eq_add_band(&mut self) {
        if self.eq.active.parametric.len() >= MAX_PARAMETRIC {
            return self.note(format!("The parametric EQ takes {MAX_PARAMETRIC} bands"));
        }
        self.eq_edit(|s| {
            s.mode = Mode::Parametric;
            // Somewhere not taken yet, as a starting point.
            let freq = [1000.0, 100.0, 4000.0, 300.0, 10000.0, 50.0, 2000.0, 600.0]
                .into_iter()
                .find(|f| !s.parametric.iter().any(|b| (b.freq - f).abs() < 1.0))
                .unwrap_or(1000.0);
            s.parametric.push(Band {
                kind: Kind::Peak,
                freq,
                gain: 0.0,
                q: 1.0,
            });
        });
    }

    pub fn eq_remove_band(&mut self, i: usize) {
        self.eq_edit(|s| {
            if i < s.parametric.len() {
                s.parametric.remove(i);
            }
        });
    }

    pub fn eq_set_preamp(&mut self, db: f64) {
        self.eq_edit(|s| s.preamp = (db * 10.0).round() / 10.0);
    }

    pub fn eq_set_auto_preamp(&mut self, on: bool) {
        self.eq_edit(|s| s.auto_preamp = on);
    }

    /// Every graphic band back to 0, every parametric one gone, preamp 0.
    pub fn eq_flat(&mut self) {
        self.eq_load_preset("Flat");
    }

    // ----- Presets -----

    pub fn eq_presets(&self) -> Vec<presets::PresetInfo> {
        self.eq.presets.list()
    }

    pub fn eq_load_preset(&mut self, name: &str) {
        if self.eq_greyed() {
            return self.note(EQ_BIT_PERFECT);
        }
        match self.eq.presets.load(name) {
            Ok(s) => {
                let on = self.eq.active.on;
                self.eq.loaded = Some(s.clone());
                self.eq.active = EqSettings { on, ..s };
                self.eq_apply();
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn eq_save_preset(&mut self, name: &str) {
        let name = name.trim();
        let mut s = self.eq.active.clone();
        s.preset = name.to_string();
        match self.eq.presets.save(name, &s) {
            Ok(()) => {
                self.eq.active.preset = name.to_string();
                self.eq.loaded = Some(s);
                self.note(format!("Saved preset \"{name}\""));
                self.eq_apply();
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    /// Delete one of your presets, and any device links to it.
    pub fn eq_delete_preset(&mut self, name: &str) {
        match self.eq.presets.delete(name) {
            Ok(()) => {
                self.eq.cfg.devices.retain(|_, p| p != name);
                if self.eq.linked.as_deref() == Some(name) {
                    self.eq.linked = None;
                }
                if self.eq.active.preset == name {
                    self.eq.loaded = None;
                }
                self.note(format!("Deleted preset \"{name}\""));
                self.eq.dirty = Some(Instant::now() - SAVE_AFTER);
                self.eq_apply();
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    /// Import an AutoEq or Equalizer APO file as a preset, and use it.
    pub fn eq_import(&mut self, path: &Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => return self.error(format!("can't read {}: {e}", path.display())),
        };
        let imp = match eq::import::parse(&text) {
            Ok(i) => i,
            Err(e) => return self.error(format!("{}: {e}", path.display())),
        };
        let base = eq::import::name_from_path(path);
        let mut name = base.clone();
        let mut n = 2;
        while presets::is_builtin(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        for w in &imp.warnings {
            self.note(w.clone());
        }
        let mut s = imp.settings;
        s.preset = name.clone();
        if let Err(e) = self.eq.presets.save(&name, &s) {
            return self.error(e.to_string());
        }
        let kind = match s.mode {
            Mode::Graphic => "31-band graphic",
            Mode::Parametric => "parametric",
        };
        self.note(format!("Imported \"{name}\" ({kind})"));
        self.eq_load_preset(&name);
    }

    // ----- Devices -----

    /// Give the device playing now its own EQ: what's set now, saved as a
    /// preset named after it.
    pub fn eq_link_device(&mut self) {
        let Some(dev) = self.eq.device.clone() else {
            return self.note("Play something first, so I know which device it is");
        };
        if self.eq.linked.is_some() {
            return self.note(format!("{} has its own EQ already", dev.name));
        }
        let mut name = dev.name.clone();
        if presets::is_builtin(&name) {
            name.push_str(" EQ");
        }
        let mut s = self.eq.active.clone();
        s.preset = name.clone();
        if let Err(e) = self.eq.presets.save(&name, &s) {
            return self.error(e.to_string());
        }
        self.eq.cfg.devices.insert(dev.key.clone(), name.clone());
        self.eq.linked = Some(name.clone());
        self.eq.active.preset = name.clone();
        self.eq.loaded = Some(s);
        self.note(format!(
            "{} keeps this EQ now (preset \"{name}\"). Other devices keep theirs.",
            dev.name
        ));
        self.eq.dirty = Some(Instant::now() - SAVE_AFTER);
        self.eq_apply();
    }

    /// The device goes back to the usual EQ. Its preset stays.
    pub fn eq_unlink_device(&mut self) {
        let Some(dev) = self.eq.device.clone() else {
            return;
        };
        if self.eq.cfg.devices.remove(&dev.key).is_none() {
            return;
        }
        self.eq.linked = None;
        let on = self.eq.active.on;
        let s = self.eq.cfg.settings.clone();
        self.eq.loaded = self.eq.presets.load(&s.preset).ok();
        self.eq.active = EqSettings { on, ..s };
        self.note(format!("{} uses your usual EQ again", dev.name));
        self.eq.dirty = Some(Instant::now() - SAVE_AFTER);
        self.eq_apply();
    }

    // ----- The EQ view's rows -----

    pub(super) fn change_eq(&mut self, key: &Key, change: Change) {
        if self.eq_greyed() && !matches!(key, Key::EqImport) {
            return self.note(EQ_BIT_PERFECT);
        }
        let row = eq_rows(&self.eq, false)
            .into_iter()
            .map(|(_, s)| s)
            .find(|s| &s.key == key);
        let Some(row) = row else { return };
        // Parametric bands are text rows; h and l move their gain.
        if let (Key::EqBand(i), Change::Step(d)) = (key, &change) {
            return self.eq_nudge_gain(*i, 0.5 * *d as f64);
        }
        if let (Key::EqBand(i), Change::Set(text)) = (key, &change) {
            return match parse_band(text) {
                Ok(b) => self.eq_set_band(*i, b),
                Err(e) => self.error(e),
            };
        }
        match key {
            Key::EqAddBand => return self.eq_add_band(),
            Key::EqDevice => {
                return if self.eq.linked.is_some() {
                    self.eq_unlink_device()
                } else {
                    self.eq_link_device()
                };
            }
            Key::EqDeletePreset => {
                let n = self.eq.active.preset.clone();
                return self.eq_delete_preset(&n);
            }
            _ => {}
        }
        let Some(new) = super::settings::stepped(&row.control, &change) else {
            return;
        };
        match (key, new) {
            (Key::EqOn, Control::Toggle(_)) => self.eq_toggle(),
            (Key::EqMode, Control::Choice { options, index }) => {
                self.eq_set_mode(if options[index].0 == "parametric" {
                    Mode::Parametric
                } else {
                    Mode::Graphic
                })
            }
            (Key::EqBands, Control::Choice { options, index }) => {
                self.eq_set_graphic_bands(options[index].0.parse().unwrap_or(10))
            }
            (Key::EqPreset, Control::Choice { options, index }) => {
                let name = options[index].0.clone();
                if !name.is_empty() {
                    self.eq_load_preset(&name);
                }
            }
            (Key::EqPreamp, Control::Number { value, scale, .. }) => {
                self.eq_set_preamp(value as f64 / scale as f64)
            }
            (Key::EqAutoPreamp, Control::Toggle(on)) => self.eq_set_auto_preamp(on),
            (Key::EqGraphic(i), Control::Number { value, scale, .. }) => {
                self.eq_set_gain(*i, value as f64 / scale as f64)
            }
            _ => {}
        }
    }

    pub(super) fn change_channel(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        let c = &mut self.config.channel;
        match (key, new) {
            (Key::ChannelOn, Control::Toggle(on)) => {
                w(config::set_value(&path, "channel", "on", *on))?;
                c.on = *on;
            }
            (Key::ChannelMono, Control::Toggle(on)) => {
                w(config::set_value(&path, "channel", "mono", *on))?;
                c.mono = *on;
            }
            (Key::ChannelWidth, Control::Number { value, .. }) => {
                let v = (*value).clamp(0, 200);
                w(config::set_value(&path, "channel", "width", v))?;
                c.width = v as u32;
            }
            (Key::ChannelCrossfeed, Control::Choice { options, index }) => {
                let name = options[*index].0.clone();
                let cf = crate::dsp::channel::Crossfeed::parse(&name).unwrap_or_default();
                w(config::set_value(
                    &path,
                    "channel",
                    "crossfeed",
                    cf.as_str(),
                ))?;
                c.crossfeed = cf;
            }
            (Key::ChannelBalance, Control::Number { value, .. }) => {
                let v = (*value).clamp(-100, 100);
                w(config::set_value(&path, "channel", "balance", v))?;
                c.balance = v as i32;
            }
            _ => return Ok(()),
        }
        self.player
            .send(Command::SetChannel(self.config.channel.params()));
        Ok(())
    }

    pub(super) fn change_limiter(&mut self, key: &Key, new: &Control) -> Result<(), String> {
        let path = self.config_path.clone();
        let w = |r: Result<bool, config::ConfigError>| r.map(|_| ()).map_err(|e| e.to_string());
        match (key, new) {
            (Key::Limiter, Control::Toggle(on)) => {
                w(config::set_value(&path, "limiter", "on", *on))?;
                self.config.limiter.on = *on;
            }
            (Key::LimiterCeiling, Control::Number { value, scale, .. }) => {
                let db = *value as f64 / *scale as f64;
                w(config::set_value(&path, "limiter", "ceiling_db", db))?;
                self.config.limiter.ceiling_db = db;
            }
            _ => return Ok(()),
        }
        self.player.send(Command::SetLimiter {
            on: self.config.limiter.on,
            ceiling_db: self.config.limiter.ceiling_db,
        });
        Ok(())
    }
}

/// A parametric band as text: "PK 105 Hz 6.7 dB Q 0.7", the way AutoEq
/// writes them, or just "LS 100 4". Types: PK, LS, HS, LP, HP, NO.
pub fn parse_band(text: &str) -> Result<Band, String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_end_matches(',').to_string())
        .collect();
    let mut it = words.iter().peekable();
    let ty = it
        .next()
        .ok_or("type a band like: PK 1000 Hz +3 dB Q 1.0")?
        .to_ascii_uppercase();
    let kind = match ty.as_str() {
        "PK" | "PEAK" | "PEQ" => Kind::Peak,
        "LS" | "LSC" | "LOWSHELF" => Kind::LowShelf,
        "HS" | "HSC" | "HIGHSHELF" => Kind::HighShelf,
        "LP" | "LPQ" | "LOWPASS" => Kind::LowPass,
        "HP" | "HPQ" | "HIGHPASS" => Kind::HighPass,
        "NO" | "NOTCH" => Kind::Notch,
        o => {
            return Err(format!(
                "unknown band type \"{o}\" (PK, LS, HS, LP, HP, NO)"
            ));
        }
    };
    let mut nums = Vec::new();
    let mut q = None;
    while let Some(w) = it.next() {
        let lw = w.to_ascii_lowercase();
        if lw == "q" {
            q = it.next().and_then(|v| v.parse::<f64>().ok());
            continue;
        }
        if lw == "hz" || lw == "db" || lw == "fc" || lw == "gain" {
            continue;
        }
        let t = lw.trim_end_matches("hz").trim_end_matches("db");
        let (t, mult) = match t.strip_suffix('k') {
            Some(x) => (x, 1000.0),
            None => (t, 1.0),
        };
        match t.parse::<f64>() {
            Ok(v) => nums.push(v * mult),
            Err(_) => return Err(format!("can't read \"{w}\"")),
        }
    }
    let freq = *nums.first().ok_or("a band needs a frequency")?;
    let gain = nums.get(1).copied().unwrap_or(0.0);
    let q = q
        .or_else(|| nums.get(2).copied())
        .unwrap_or(std::f64::consts::FRAC_1_SQRT_2);
    Ok(eq::clamp_band(Band {
        kind,
        freq,
        gain,
        q,
    }))
}

/// A band as the row shows it, and as it reads back in.
pub fn band_text(b: &Band) -> String {
    let f = if b.freq >= 1000.0 {
        format!("{} Hz", (b.freq * 10.0).round() / 10.0)
    } else {
        format!("{} Hz", b.freq.round())
    };
    if b.kind.has_gain() {
        format!("{} {f} {:+.1} dB Q {:.2}", b.kind.short(), b.gain, b.q)
    } else {
        format!("{} {f} Q {:.2}", b.kind.short(), b.q)
    }
}

/// The EQ view as rows, grouped.
pub fn eq_rows(st: &EqState, greyed: bool) -> Vec<(Option<&'static str>, Setting)> {
    let a = &st.active;
    let mut v: Vec<(Option<&'static str>, Setting)> = Vec::new();
    let off = greyed.then_some(EQ_BIT_PERFECT);
    let mut push =
        |g: &'static str, key: Key, label: String, help: &'static str, control: Control| {
            v.push((
                Some(g),
                Setting {
                    key,
                    label,
                    help,
                    control,
                    disabled: off,
                },
            ))
        };
    push(
        "EQ",
        Key::EqOn,
        "EQ".into(),
        "Off bypasses it, for a quick A/B. o in the TUI.",
        Control::Toggle(a.on),
    );
    push(
        "EQ",
        Key::EqMode,
        "Mode".into(),
        "Graphic: fixed bands, ±12 dB each. Parametric: up to 20 bands of any shape, frequency and width.",
        Control::Choice {
            options: vec![
                ("graphic".into(), "Graphic".into()),
                ("parametric".into(), "Parametric".into()),
            ],
            index: usize::from(a.mode == Mode::Parametric),
        },
    );
    if a.mode == Mode::Graphic {
        push(
            "EQ",
            Key::EqBands,
            "Bands".into(),
            "Octaves (10), two-thirds octaves (15) or third octaves (31). The curve carries over.",
            Control::Choice {
                options: vec![
                    ("10".into(), "10".into()),
                    ("15".into(), "15".into()),
                    ("31".into(), "31".into()),
                ],
                index: match a.graphic_bands {
                    15 => 1,
                    31 => 2,
                    _ => 0,
                },
            },
        );
    }
    let list = st.presets.list();
    let mut options: Vec<(String, String)> = Vec::new();
    let label = st.preset_label();
    let current = list
        .iter()
        .position(|p| p.name == a.preset && !st.changed());
    if current.is_none() {
        options.push((String::new(), label));
    }
    options.extend(list.iter().map(|p| {
        let l = if p.builtin {
            p.name.clone()
        } else {
            format!("{} (yours)", p.name)
        };
        (p.name.clone(), l)
    }));
    push(
        "EQ",
        Key::EqPreset,
        "Preset".into(),
        "Built-in starting points, and your own. Import brings in AutoEq and Equalizer APO files.",
        Control::Choice {
            options,
            index: current.unwrap_or(0),
        },
    );
    push(
        "EQ",
        Key::EqPreamp,
        "Preamp".into(),
        "Gain before the bands.",
        Control::Number {
            value: (a.preamp * 10.0).round() as i64,
            min: -(eq::PREAMP_RANGE as i64) * 10,
            max: eq::PREAMP_RANGE as i64 * 10,
            step: 5,
            scale: 10,
            unit: " dB",
        },
    );
    push(
        "EQ",
        Key::EqAutoPreamp,
        format!(
            "Auto preamp ({:+.1} dB now)",
            // + 0.0 turns -0.0 into 0.0, so it never shows "-0.0".
            if a.auto_preamp {
                -a.max_boost() + 0.0
            } else {
                0.0
            }
        ),
        "Takes the biggest boost off the preamp too, so the EQ can't push the sound into the limiter.",
        Control::Toggle(a.auto_preamp),
    );
    let (dev_label, dev_action) = match (&st.device, &st.linked) {
        (Some(d), Some(_)) => (format!("Device: {}", d.name), "Use my usual EQ here"),
        (Some(d), None) => (format!("Device: {}", d.name), "Keep this EQ for it"),
        (None, _) => ("Device: not known yet".to_string(), "Keep this EQ for it"),
    };
    push(
        "EQ",
        Key::EqDevice,
        dev_label,
        "A device you give its own EQ gets it whenever the sound goes there (headphones, speakers). Changes save to that device's preset.",
        Control::Action(dev_action),
    );
    push(
        "EQ",
        Key::EqSavePreset,
        "Save as a preset".into(),
        "Keep this EQ under a name, in presets/eq/ next to config.toml.",
        Control::Action("Save as"),
    );
    push(
        "EQ",
        Key::EqImport,
        "Import AutoEq or APO".into(),
        "A ParametricEQ.txt or GraphicEQ.txt from AutoEq, or an Equalizer APO config.",
        Control::Action("Import"),
    );
    if !a.preset.is_empty() && !presets::is_builtin(&a.preset) && st.presets.exists(&a.preset) {
        push(
            "EQ",
            Key::EqDeletePreset,
            format!("Delete \"{}\"", a.preset),
            "Deletes the preset file. The EQ as it is now stays.",
            Control::Action("Delete"),
        );
    }
    match a.mode {
        Mode::Graphic => {
            for (i, (f, g)) in eq::centres(a.graphic_bands)
                .iter()
                .zip(&a.graphic)
                .enumerate()
            {
                push(
                    "Bands",
                    Key::EqGraphic(i),
                    format!("{} Hz", freq_label(*f)),
                    "h and l move it half a dB.",
                    Control::Number {
                        value: (g * 10.0).round() as i64,
                        min: -(GRAPHIC_RANGE as i64) * 10,
                        max: GRAPHIC_RANGE as i64 * 10,
                        step: 5,
                        scale: 10,
                        unit: " dB",
                    },
                );
            }
        }
        Mode::Parametric => {
            for (i, b) in a.parametric.iter().enumerate() {
                push(
                    "Bands",
                    Key::EqBand(i),
                    format!("Band {}", i + 1),
                    "Type it AutoEq style: PK 105 Hz 6.7 dB Q 0.7 (PK, LS, HS, LP, HP, NO). h and l move the gain.",
                    Control::Text {
                        value: band_text(b),
                        empty: "",
                    },
                );
            }
            if a.parametric.len() < MAX_PARAMETRIC {
                push(
                    "Bands",
                    Key::EqAddBand,
                    "Add a band".into(),
                    "A new peak band at 0 dB, to shape from there.",
                    Control::Action("Add band"),
                );
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_read_back_from_text() {
        let b = parse_band("PK 105 Hz 6.7 dB Q 0.7").unwrap();
        assert_eq!((b.kind, b.freq, b.gain, b.q), (Kind::Peak, 105.0, 6.7, 0.7));
        let b = parse_band("ls 2k -3").unwrap();
        assert_eq!((b.kind, b.freq, b.gain), (Kind::LowShelf, 2000.0, -3.0));
        let b = parse_band("HP 30hz").unwrap();
        assert_eq!(b.kind, Kind::HighPass);
        let back = parse_band(&band_text(&b)).unwrap();
        assert_eq!((back.kind, back.freq), (b.kind, b.freq));
        assert!((back.q - b.q).abs() < 0.01);
        assert!(parse_band("XX 100").is_err());
        assert!(parse_band("PK loud").is_err());
    }
}
