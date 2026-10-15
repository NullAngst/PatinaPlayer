//! The Amp view: the stack as rows, the same way the EQ view works. Both
//! frontends draw these rows; the desktop app also draws the rack above
//! them, from the same state.

use std::time::{Duration, Instant};

use super::Browser;
use super::settings::{Change, Control, Key, Setting};
use crate::amp::{self, AmpConfig, AmpSettings, Presets};
use crate::audio::output::BitPerfect;
use crate::config;
use crate::dsp::amp::{Kind, ParamSpec, TRIM, Unit};
use crate::eq::OutputDevice;
use crate::player::Command;

const SAVE_AFTER: Duration = Duration::from_millis(600);

pub const AMP_BIT_PERFECT: &str =
    "Bit-perfect is on, and it skips the amp stack. Set bit-perfect to Off or Auto to use it.";

/// The stack as the browser keeps it.
pub struct AmpState {
    pub cfg: AmpConfig,
    pub active: AmpSettings,
    loaded: Option<AmpSettings>,
    pub device: Option<OutputDevice>,
    pub linked: Option<String>,
    pub presets: Presets,
    dirty: Option<Instant>,
}

impl AmpState {
    pub fn new(cfg: AmpConfig, presets: Presets) -> Self {
        let active = cfg.settings.clone();
        let loaded = (!active.preset.is_empty())
            .then(|| presets.load(&active.preset).ok())
            .flatten();
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

    pub fn changed(&self) -> bool {
        !self
            .loaded
            .as_ref()
            .is_some_and(|l| l.same_sound(&self.active))
    }

    pub fn preset_label(&self) -> String {
        let name = &self.active.preset;
        if name.is_empty() {
            return if self.active.units.is_empty() {
                "Empty".into()
            } else {
                "Your own".into()
            };
        }
        if self.changed() {
            format!("{name} (changed)")
        } else {
            name.clone()
        }
    }
}

/// A knob's value as a row's number: whole steps, scaled.
fn scale_of(s: &ParamSpec) -> i64 {
    if s.step.fract() == 0.0 { 1 } else { 10 }
}

fn knob_control(s: &ParamSpec, v: f64) -> Control {
    if !s.choices.is_empty() {
        return Control::Choice {
            options: s
                .choices
                .iter()
                .enumerate()
                .map(|(i, c)| (i.to_string(), c.to_string()))
                .collect(),
            index: (v.round() as usize).min(s.choices.len() - 1),
        };
    }
    let k = scale_of(s);
    Control::Number {
        value: (v * k as f64).round() as i64,
        min: (s.min * k as f64).round() as i64,
        max: (s.max * k as f64).round() as i64,
        step: (s.step * k as f64).round() as i64,
        scale: k,
        unit: s.unit,
    }
}

/// A knob's position from 0 to 1, for the rack's dials.
pub fn knob_pos(s: &ParamSpec, v: f64) -> f64 {
    if s.max <= s.min {
        0.0
    } else {
        ((v - s.min) / (s.max - s.min)).clamp(0.0, 1.0)
    }
}

/// A knob's value as shown: "4.0 dB", "15 ips", "30%".
pub fn knob_text(s: &ParamSpec, v: f64) -> String {
    if !s.choices.is_empty() {
        return s.choices[(v.round() as usize).min(s.choices.len() - 1)].to_string();
    }
    if s.step.fract() == 0.0 {
        format!("{}{}", v.round() as i64, s.unit)
    } else {
        format!("{:.1}{}", v + 0.0, s.unit)
    }
}

pub fn amp_rows(st: &AmpState, greyed: bool) -> Vec<(Option<&'static str>, Setting)> {
    let a = &st.active;
    let mut v: Vec<(Option<&'static str>, Setting)> = Vec::new();
    let off = greyed.then_some(AMP_BIT_PERFECT);
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
        "Amp stack",
        Key::AmpOn,
        "Amp stack".into(),
        "Off bypasses the whole rack, for a quick A/B. o in the TUI.",
        Control::Toggle(a.on),
    );
    let list = st.presets.list();
    let mut options: Vec<(String, String)> = Vec::new();
    let current = list
        .iter()
        .position(|p| p.name == a.preset && !st.changed());
    if current.is_none() {
        options.push((String::new(), st.preset_label()));
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
        "Amp stack",
        Key::AmpPreset,
        "Preset".into(),
        "Warm Glow, Reel to Reel, Late Night, Living Room Console, AM Radio, and your own.",
        Control::Choice {
            options,
            index: current.unwrap_or(0),
        },
    );
    push(
        "Amp stack",
        Key::AmpOversample,
        "Oversampling".into(),
        "The devices run at this many times the song's rate, so their harmonics don't fold back as harsh aliasing. Tops out at 768 kHz.",
        Control::Choice {
            options: vec![
                ("2".into(), "2x".into()),
                ("4".into(), "4x".into()),
                ("8".into(), "8x".into()),
            ],
            index: match a.oversample {
                2 => 0,
                8 => 2,
                _ => 1,
            },
        },
    );
    let missing = a.missing();
    if !missing.is_empty() {
        let mut opts = vec![(String::new(), "Pick one".to_string())];
        opts.extend(
            missing
                .iter()
                .map(|k| (k.key().to_string(), k.label().to_string())),
        );
        push(
            "Amp stack",
            Key::AmpAdd,
            "Add a device".into(),
            "Goes at the end of the rack, on, at its defaults. Move it where you want it.",
            Control::Choice {
                options: opts,
                index: 0,
            },
        );
    }
    push(
        "Amp stack",
        Key::AmpSavePreset,
        "Save as a preset".into(),
        "Saves the rack as it is, under a name you give it.",
        Control::Action("Save"),
    );
    if !a.preset.is_empty() && !amp::is_builtin(&a.preset) && st.presets.exists(&a.preset) {
        push(
            "Amp stack",
            Key::AmpDeletePreset,
            format!("Delete \"{}\"", a.preset),
            "Removes your preset. The rack stays as it is.",
            Control::Action("Delete"),
        );
    }
    if let Some(dev) = &st.device {
        let (label, act) = if st.linked.is_some() {
            (
                format!("{} has its own stack", dev.name),
                "Use the usual one",
            )
        } else {
            (format!("Give {} its own stack", dev.name), "Keep this one")
        };
        push(
            "Amp stack",
            Key::AmpDevice,
            label,
            "Each output can keep its own rack, like the EQ: your speakers get Warm Glow, your headphones stay clean.",
            Control::Action(act),
        );
    }

    let n = a.units.len();
    for (i, u) in a.units.iter().enumerate() {
        let g = u.kind.label();
        push(
            g,
            Key::AmpUnitOn(u.kind),
            g.into(),
            u.kind.blurb(),
            Control::Toggle(u.on),
        );
        for (pi, s) in u.kind.params().iter().enumerate() {
            push(
                g,
                Key::AmpParam(u.kind, pi),
                s.label.into(),
                "",
                knob_control(s, u.p[pi]),
            );
        }
        push(
            g,
            Key::AmpTrimIn(u.kind),
            "Input trim".into(),
            "Level into the device: more drives it harder.",
            knob_control(&TRIM, u.trim_in),
        );
        push(
            g,
            Key::AmpTrimOut(u.kind),
            "Output trim".into(),
            "Level out of the device.",
            knob_control(&TRIM, u.trim_out),
        );
        if i > 0 {
            push(
                g,
                Key::AmpMoveUp(u.kind),
                "Earlier in the chain".into(),
                "Swaps it with the device before it. Order changes the sound.",
                Control::Action("Move up"),
            );
        }
        if i + 1 < n {
            push(
                g,
                Key::AmpMoveDown(u.kind),
                "Later in the chain".into(),
                "Swaps it with the device after it.",
                Control::Action("Move down"),
            );
        }
        push(
            g,
            Key::AmpRemove(u.kind),
            "Take it out of the rack".into(),
            "Its settings go with it. Add it again for the defaults.",
            Control::Action("Remove"),
        );
    }
    v
}

impl Browser {
    fn amp_greyed(&self) -> bool {
        self.config.output.bit_perfect == BitPerfect::On
    }

    pub fn amp_settings(&self) -> &AmpSettings {
        &self.amp.active
    }

    pub fn amp_state(&self) -> &AmpState {
        &self.amp
    }

    fn amp_apply(&mut self) {
        self.player
            .send(Command::SetAmp(Box::new(self.amp.active.params())));
        self.amp.dirty = Some(Instant::now());
        self.refresh_section(super::Section::Amp);
        self.touch();
    }

    /// Follow the output device, and write changes once they stop.
    pub(super) fn amp_tick(&mut self) {
        let snap_dev = self.player.snapshot().output_device;
        if snap_dev.is_some() && snap_dev != self.amp.device {
            self.amp_device_changed(snap_dev);
        }
        if let Some(t) = self.amp.dirty
            && t.elapsed() >= SAVE_AFTER
        {
            self.amp_save_now();
        }
    }

    pub fn amp_save_now(&mut self) {
        if self.amp.dirty.take().is_none() {
            return;
        }
        let a = self.amp.active.clone();
        self.amp.cfg.settings.on = a.on;
        if let Some(name) = self.amp.linked.clone() {
            match self.amp.presets.save(&name, &a) {
                Ok(()) => self.amp.loaded = Some(a.clone()),
                Err(e) => self.error(format!("couldn't save the stack for this device: {e}")),
            }
        } else {
            self.amp.cfg.settings = a;
        }
        if let Err(e) = config::set_amp(&self.config_path, &self.amp.cfg) {
            self.error(format!("couldn't save the amp stack: {e}"));
        }
        self.config.amp = self.amp.cfg.clone();
    }

    fn amp_device_changed(&mut self, dev: Option<OutputDevice>) {
        self.amp_save_now();
        let on = self.amp.active.on;
        self.amp.device = dev.clone();
        let Some(dev) = dev else { return };
        match self.amp.cfg.devices.get(&dev.key).cloned() {
            Some(name) => match self.amp.presets.load(&name) {
                Ok(s) => {
                    self.amp.linked = Some(name.clone());
                    self.amp.loaded = Some(s.clone());
                    self.amp.active = AmpSettings { on, ..s };
                    self.note(format!("Amp stack: {name}, for {}", dev.name));
                }
                Err(e) => {
                    self.error(format!("the amp stack linked to {} is gone: {e}", dev.name));
                    self.amp.linked = None;
                }
            },
            None if self.amp.linked.is_some() => {
                self.amp.linked = None;
                let s = self.amp.cfg.settings.clone();
                self.amp.loaded = self.amp.presets.load(&s.preset).ok();
                self.amp.active = AmpSettings { on, ..s };
                self.note(format!(
                    "Amp stack: back to your usual one, for {}",
                    dev.name
                ));
            }
            None => {}
        }
        self.player
            .send(Command::SetAmp(Box::new(self.amp.active.params())));
        self.refresh_section(super::Section::Amp);
        self.touch();
    }

    // ----- Changes -----

    pub fn amp_toggle(&mut self) {
        if self.amp_greyed() {
            return self.note(AMP_BIT_PERFECT);
        }
        self.amp.active.on = !self.amp.active.on;
        if self.amp.active.on && self.amp.active.units.is_empty() {
            self.note("The rack is empty: add a device or pick a preset in the Amp view");
        }
        self.amp_apply();
    }

    fn amp_unit(&mut self, k: Kind) -> Option<&mut Unit> {
        self.amp.active.units.iter_mut().find(|u| u.kind == k)
    }

    pub fn amp_set_param(&mut self, k: Kind, i: usize, v: f64) {
        if let Some(u) = self.amp_unit(k) {
            u.p[i] = v;
            *u = u.clamped();
            self.amp_apply();
        }
    }

    pub fn amp_set_unit_on(&mut self, k: Kind, on: bool) {
        if let Some(u) = self.amp_unit(k) {
            u.on = on;
            // Switching a device on is asking for the stack.
            if on {
                self.amp.active.on = true;
            }
            self.amp_apply();
        }
    }

    pub fn amp_add(&mut self, k: Kind) {
        if self.amp.active.units.iter().any(|u| u.kind == k) {
            return;
        }
        self.amp.active.units.push(Unit::new(k));
        self.amp.active.on = true;
        self.amp_apply();
    }

    pub fn amp_remove(&mut self, k: Kind) {
        self.amp.active.units.retain(|u| u.kind != k);
        self.amp_apply();
    }

    /// Move a device `by` places along the chain.
    pub fn amp_move(&mut self, k: Kind, by: isize) {
        let u = &mut self.amp.active.units;
        let Some(i) = u.iter().position(|x| x.kind == k) else {
            return;
        };
        let j = i as isize + by;
        if j < 0 || j as usize >= u.len() {
            return;
        }
        u.swap(i, j as usize);
        self.amp_apply();
    }

    pub fn amp_load_preset(&mut self, name: &str) {
        match self.amp.presets.load(name) {
            Ok(s) => {
                self.amp.loaded = Some(s.clone());
                self.amp.active = AmpSettings { on: true, ..s };
                self.amp_apply();
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn amp_save_preset(&mut self, name: &str) {
        let name = name.trim();
        let s = AmpSettings {
            preset: name.to_string(),
            ..self.amp.active.clone()
        };
        match self.amp.presets.save(name, &s) {
            Ok(()) => {
                self.amp.active.preset = name.to_string();
                self.amp.loaded = Some(s);
                self.note(format!("Saved the amp preset \"{name}\""));
                self.amp_apply();
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn amp_delete_preset(&mut self, name: &str) {
        if self.amp.cfg.devices.values().any(|v| v == name) {
            return self.note(format!(
                "\"{name}\" is an output's own stack. Use the usual one there first."
            ));
        }
        match self.amp.presets.delete(name) {
            Ok(()) => {
                self.amp.active.preset.clear();
                self.amp.loaded = None;
                self.note(format!("Deleted \"{name}\""));
                self.amp_apply();
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn amp_presets(&self) -> Vec<amp::PresetInfo> {
        self.amp.presets.list()
    }

    pub fn amp_link_device(&mut self) {
        let Some(dev) = self.amp.device.clone() else {
            return self.note("Play something first, so I know which device it is");
        };
        if self.amp.linked.is_some() {
            return;
        }
        let mut name = format!("{} stack", dev.name);
        if amp::is_builtin(&name) {
            name.push_str(" (own)");
        }
        let s = AmpSettings {
            preset: name.clone(),
            ..self.amp.active.clone()
        };
        if let Err(e) = self.amp.presets.save(&name, &s) {
            return self.error(e.to_string());
        }
        self.amp.cfg.devices.insert(dev.key.clone(), name.clone());
        self.amp.linked = Some(name.clone());
        self.amp.active.preset = name.clone();
        self.amp.loaded = Some(s);
        self.note(format!(
            "{} keeps this stack now (preset \"{name}\"). Other outputs keep theirs.",
            dev.name
        ));
        self.amp.dirty = Some(Instant::now() - SAVE_AFTER);
        self.amp_apply();
    }

    pub fn amp_unlink_device(&mut self) {
        let Some(dev) = self.amp.device.clone() else {
            return;
        };
        if self.amp.cfg.devices.remove(&dev.key).is_none() {
            return;
        }
        self.amp.linked = None;
        let on = self.amp.active.on;
        let s = self.amp.cfg.settings.clone();
        self.amp.loaded = self.amp.presets.load(&s.preset).ok();
        self.amp.active = AmpSettings { on, ..s };
        self.note(format!("{} uses your usual stack again", dev.name));
        self.amp.dirty = Some(Instant::now() - SAVE_AFTER);
        self.amp_apply();
    }

    pub(super) fn change_amp(&mut self, key: &Key, change: Change) {
        if self.amp_greyed() {
            return self.note(AMP_BIT_PERFECT);
        }
        let row = amp_rows(&self.amp, false)
            .into_iter()
            .map(|(_, s)| s)
            .find(|s| &s.key == key);
        let Some(row) = row else { return };
        match key {
            Key::AmpDevice => {
                return if self.amp.linked.is_some() {
                    self.amp_unlink_device()
                } else {
                    self.amp_link_device()
                };
            }
            Key::AmpDeletePreset => {
                let n = self.amp.active.preset.clone();
                return self.amp_delete_preset(&n);
            }
            Key::AmpMoveUp(k) => return self.amp_move(*k, -1),
            Key::AmpMoveDown(k) => return self.amp_move(*k, 1),
            Key::AmpRemove(k) => return self.amp_remove(*k),
            // Saving asks for a name; the frontends do that.
            Key::AmpSavePreset => return,
            _ => {}
        }
        let Some(new) = super::settings::stepped(&row.control, &change) else {
            return;
        };
        match (key, new) {
            (Key::AmpOn, Control::Toggle(_)) => self.amp_toggle(),
            (Key::AmpOversample, Control::Choice { options, index }) => {
                self.amp.active.oversample = options[index].0.parse().unwrap_or(4);
                self.amp_apply();
            }
            (Key::AmpPreset, Control::Choice { options, index }) => {
                let name = options[index].0.clone();
                if !name.is_empty() {
                    self.amp_load_preset(&name);
                }
            }
            (Key::AmpAdd, Control::Choice { options, index }) => {
                let k = Kind::ALL.into_iter().find(|k| k.key() == options[index].0);
                if let Some(k) = k {
                    self.amp_add(k);
                }
            }
            (Key::AmpUnitOn(k), Control::Toggle(on)) => self.amp_set_unit_on(*k, on),
            (Key::AmpParam(k, i), Control::Number { value, scale, .. }) => {
                self.amp_set_param(*k, *i, value as f64 / scale as f64)
            }
            (Key::AmpParam(k, i), Control::Choice { index, .. }) => {
                self.amp_set_param(*k, *i, index as f64)
            }
            (Key::AmpTrimIn(k), Control::Number { value, scale, .. }) => {
                if let Some(u) = self.amp_unit(*k) {
                    u.trim_in = value as f64 / scale as f64;
                    *u = u.clamped();
                    self.amp_apply();
                }
            }
            (Key::AmpTrimOut(k), Control::Number { value, scale, .. }) => {
                if let Some(u) = self.amp_unit(*k) {
                    u.trim_out = value as f64 / scale as f64;
                    *u = u.clamped();
                    self.amp_apply();
                }
            }
            _ => {}
        }
    }
}
