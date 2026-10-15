//! The amp stack: a rack of old hardware you switch on and chain, run
//! oversampled between the transition mixer and the EQ. Off by default.
//!
//! [`AmpParams`] is what the renderer runs, plain values it can copy
//! without allocating. [`AmpRt`] runs it. The settings as you keep them
//! (presets, config.toml) are in [`crate::amp`].

pub mod devices;
pub mod oversample;

use serde::{Deserialize, Serialize};

use devices::Devices;
use oversample::Oversampler;

/// Channels the devices keep state for. More than this pass through the
/// stack untouched past the eighth.
pub const MAX_CH: usize = 8;
/// One of each kind of device.
pub const MAX_UNITS: usize = 9;
pub const MAX_PARAMS: usize = 7;
/// The highest rate devices run at. A 192 kHz song gets 4x at most.
pub const MAX_RATE: u32 = 768_000;
/// On and off glide over this long.
const MIX_MS: f64 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    TubePreamp,
    TubePowerAmp,
    Transformer,
    Tape,
    Phono,
    Console,
    Compressor,
    Exciter,
    BassEnhancer,
}

/// One knob.
#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    /// Its name in files.
    pub key: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default: f64,
    /// " dB", " Hz", "%". Empty for a choice.
    pub unit: &'static str,
    /// Non-empty: the knob picks one of these, by index.
    pub choices: &'static [&'static str],
}

const fn knob(
    key: &'static str,
    label: &'static str,
    min: f64,
    max: f64,
    step: f64,
    default: f64,
    unit: &'static str,
) -> ParamSpec {
    ParamSpec {
        key,
        label,
        min,
        max,
        step,
        default,
        unit,
        choices: &[],
    }
}

const fn pick(
    key: &'static str,
    label: &'static str,
    choices: &'static [&'static str],
    default: f64,
) -> ParamSpec {
    ParamSpec {
        key,
        label,
        min: 0.0,
        max: (choices.len() - 1) as f64,
        step: 1.0,
        default,
        unit: "",
        choices,
    }
}

const MIX: ParamSpec = knob("mix", "Mix", 0.0, 100.0, 5.0, 100.0, "%");

const TUBE_PRE: &[ParamSpec] = &[
    knob("drive", "Drive", 0.0, 24.0, 0.5, 4.0, " dB"),
    knob("bias", "Bias", 0.0, 100.0, 5.0, 30.0, "%"),
    knob("warmth", "Warmth", 0.0, 100.0, 5.0, 30.0, "%"),
    MIX,
];
const TUBE_POWER: &[ParamSpec] = &[
    knob("drive", "Drive", 0.0, 24.0, 0.5, 4.0, " dB"),
    knob("sag", "Sag", 0.0, 100.0, 5.0, 30.0, "%"),
    knob("presence", "Presence", -6.0, 6.0, 0.5, 0.0, " dB"),
    MIX,
];
const TRANSFORMER: &[ParamSpec] = &[
    knob("saturation", "Saturation", 0.0, 100.0, 5.0, 30.0, "%"),
    knob("low_cut", "Low cut", 10.0, 300.0, 5.0, 20.0, " Hz"),
    knob("high_rolloff", "High rolloff", 3.0, 24.0, 0.5, 20.0, " kHz"),
    MIX,
];
const TAPE: &[ParamSpec] = &[
    pick("speed", "Speed", &["7.5 ips", "15 ips", "30 ips"], 1.0),
    knob("saturation", "Saturation", 0.0, 100.0, 5.0, 30.0, "%"),
    knob("wow", "Wow", 0.0, 100.0, 5.0, 20.0, "%"),
    knob("flutter", "Flutter", 0.0, 100.0, 5.0, 20.0, "%"),
    knob("hiss", "Hiss", 0.0, 100.0, 5.0, 0.0, "%"),
];
const PHONO: &[ParamSpec] = &[
    knob("mono_below", "Mono below", 0.0, 300.0, 10.0, 100.0, " Hz"),
    knob("rumble", "Rumble", 0.0, 100.0, 5.0, 20.0, "%"),
    knob("crackle", "Crackle", 0.0, 100.0, 5.0, 0.0, "%"),
    knob("warmth", "Warmth", 0.0, 100.0, 5.0, 30.0, "%"),
];
const CONSOLE: &[ParamSpec] = &[
    knob("drive", "Drive", 0.0, 100.0, 5.0, 30.0, "%"),
    knob("crosstalk", "Crosstalk", -60.0, -20.0, 1.0, -40.0, " dB"),
];
const COMPRESSOR: &[ParamSpec] = &[
    pick("style", "Style", &["Opto", "VCA"], 0.0),
    knob("threshold", "Threshold", -40.0, 0.0, 1.0, -18.0, " dB"),
    knob("ratio", "Ratio", 1.0, 10.0, 0.5, 2.0, ":1"),
    knob("attack", "Attack", 0.5, 100.0, 0.5, 10.0, " ms"),
    knob("release", "Release", 10.0, 1000.0, 10.0, 200.0, " ms"),
    knob("makeup", "Makeup", 0.0, 18.0, 0.5, 2.0, " dB"),
    MIX,
];
const EXCITER: &[ParamSpec] = &[
    knob("frequency", "Frequency", 1.0, 10.0, 0.5, 4.0, " kHz"),
    knob("amount", "Amount", 0.0, 100.0, 5.0, 25.0, "%"),
    MIX,
];
const BASS: &[ParamSpec] = &[
    knob("frequency", "Frequency", 40.0, 200.0, 5.0, 90.0, " Hz"),
    knob("amount", "Amount", 0.0, 100.0, 5.0, 30.0, "%"),
];

/// The trims every device has.
pub const TRIM: ParamSpec = knob("trim", "Trim", -12.0, 12.0, 0.5, 0.0, " dB");

impl Kind {
    pub const ALL: [Kind; MAX_UNITS] = [
        Kind::TubePreamp,
        Kind::TubePowerAmp,
        Kind::Transformer,
        Kind::Tape,
        Kind::Phono,
        Kind::Console,
        Kind::Compressor,
        Kind::Exciter,
        Kind::BassEnhancer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::TubePreamp => "Tube preamp",
            Kind::TubePowerAmp => "Tube power amp",
            Kind::Transformer => "Output transformer",
            Kind::Tape => "Tape deck",
            Kind::Phono => "Phono stage",
            Kind::Console => "Console channel",
            Kind::Compressor => "Bus compressor",
            Kind::Exciter => "Exciter",
            Kind::BassEnhancer => "Bass enhancer",
        }
    }

    /// What it does, in a line.
    pub fn blurb(self) -> &'static str {
        match self {
            Kind::TubePreamp => {
                "Triode-style soft saturation, mostly even harmonics: the warm glow."
            }
            Kind::TubePowerAmp => {
                "Push-pull output stage: odd harmonics, and supply sag on loud passages."
            }
            Kind::Transformer => {
                "Iron that thickens and rounds the lows, and rolls off the extremes."
            }
            Kind::Tape => "Tape saturation, head bump, high-frequency loss, wow and flutter.",
            Kind::Phono => "Old turntable character: mono bass, rumble, crackle if you want it.",
            Kind::Console => "Subtle mixing-desk saturation and channel crosstalk.",
            Kind::Compressor => "Glue compression across both channels, opto or VCA style.",
            Kind::Exciter => "New harmonics in the highs, for sparkle.",
            Kind::BassEnhancer => "Harmonics of the bass, so small speakers seem to reach lower.",
        }
    }

    /// Its name in files.
    pub fn key(self) -> &'static str {
        match self {
            Kind::TubePreamp => "tube-preamp",
            Kind::TubePowerAmp => "tube-power-amp",
            Kind::Transformer => "transformer",
            Kind::Tape => "tape",
            Kind::Phono => "phono",
            Kind::Console => "console",
            Kind::Compressor => "compressor",
            Kind::Exciter => "exciter",
            Kind::BassEnhancer => "bass-enhancer",
        }
    }

    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            Kind::TubePreamp => TUBE_PRE,
            Kind::TubePowerAmp => TUBE_POWER,
            Kind::Transformer => TRANSFORMER,
            Kind::Tape => TAPE,
            Kind::Phono => PHONO,
            Kind::Console => CONSOLE,
            Kind::Compressor => COMPRESSOR,
            Kind::Exciter => EXCITER,
            Kind::BassEnhancer => BASS,
        }
    }

    /// Which knob is the mix, if it has one.
    pub fn mix_param(self) -> Option<usize> {
        self.params().iter().position(|p| p.key == "mix")
    }

    /// Whether it bends the waveform, and so needs oversampling. The
    /// stack runs oversampled as a whole either way.
    pub fn nonlinear(self) -> bool {
        !matches!(self, Kind::Phono)
    }
}

/// One device in the stack, as set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub kind: Kind,
    pub on: bool,
    /// In the order of [`Kind::params`], in the units they show in.
    pub p: [f64; MAX_PARAMS],
    pub trim_in: f64,
    pub trim_out: f64,
}

impl Unit {
    /// A device as it comes out of the box: on, at its defaults.
    pub fn new(kind: Kind) -> Self {
        let mut p = [0.0; MAX_PARAMS];
        for (i, s) in kind.params().iter().enumerate() {
            p[i] = s.default;
        }
        Self {
            kind,
            on: true,
            p,
            trim_in: 0.0,
            trim_out: 0.0,
        }
    }

    /// Knobs pulled into their ranges, onto their steps.
    pub fn clamped(mut self) -> Self {
        for (i, s) in self.kind.params().iter().enumerate() {
            self.p[i] = snap(self.p[i], s);
        }
        self.trim_in = snap(self.trim_in, &TRIM);
        self.trim_out = snap(self.trim_out, &TRIM);
        self
    }
}

fn snap(v: f64, s: &ParamSpec) -> f64 {
    let v = if v.is_finite() { v } else { s.default };
    let v = v.clamp(s.min, s.max);
    let k = ((v - s.min) / s.step).round();
    (s.min + k * s.step).clamp(s.min, s.max)
}

/// What the renderer runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmpParams {
    pub on: bool,
    /// 2, 4 or 8.
    pub oversample: u8,
    pub n: usize,
    pub units: [Unit; MAX_UNITS],
}

impl Default for AmpParams {
    fn default() -> Self {
        Self {
            on: false,
            oversample: 4,
            n: 0,
            units: [Unit::new(Kind::TubePreamp); MAX_UNITS],
        }
    }
}

impl AmpParams {
    pub fn units(&self) -> &[Unit] {
        &self.units[..self.n.min(MAX_UNITS)]
    }

    /// On, with at least one device on: the sound changes.
    pub fn changes_sound(&self) -> bool {
        self.on && self.units().iter().any(|u| u.on)
    }

    /// The level change a quiet signal gets through the stack: the trims,
    /// and the makeup of the compressor. For the limiter's look ahead.
    pub fn static_gain(&self) -> f64 {
        if !self.changes_sound() {
            return 1.0;
        }
        let db: f64 = self
            .units()
            .iter()
            .filter(|u| u.on)
            .map(|u| u.trim_in + u.trim_out)
            .sum();
        10f64.powf(db / 20.0)
    }
}

/// The oversampling factor for this rate: what's asked, halved until the
/// devices' rate stays at or under [`MAX_RATE`].
pub fn factor_for(asked: u8, rate: u32) -> usize {
    let mut n = match asked {
        0..=2 => 2,
        3..=4 => 4,
        _ => 8,
    };
    while n > 1 && rate as u64 * n as u64 > MAX_RATE as u64 {
        n /= 2;
    }
    n
}

/// The stack as the renderer runs it, for one stream format.
pub struct AmpRt {
    ch: usize,
    rate: u32,
    /// One oversampler per factor (1, 2, 4, 8), made up front.
    os: [Oversampler; 4],
    cur: usize,
    devices: Devices,
    params: AmpParams,
    /// Factor index waiting for the stack to glide out before it switches.
    pending: Option<usize>,
    mix: f64,
    target: f64,
    mix_step: f64,
    hi: Vec<f64>,
    dry: Vec<f64>,
}

fn os_index(n: usize) -> usize {
    match n {
        1 => 0,
        2 => 1,
        4 => 2,
        _ => 3,
    }
}

impl AmpRt {
    pub fn new(ch: usize, rate: u32) -> Self {
        let ch = ch.max(1);
        let rate = rate.max(1);
        let max_fs = rate as f64 * factor_for(8, rate) as f64;
        let os = [1, 2, 4, 8].map(|n| Oversampler::new(n, ch));
        Self {
            ch,
            rate,
            os,
            cur: os_index(factor_for(4, rate)),
            devices: Devices::new(max_fs),
            params: AmpParams::default(),
            pending: None,
            mix: 0.0,
            target: 0.0,
            mix_step: 1.0 / (rate as f64 * MIX_MS / 1000.0).max(1.0),
            hi: vec![0.0; 8 * ch],
            dry: vec![0.0; ch],
        }
    }

    fn fs(&self) -> f64 {
        self.rate as f64 * self.os[self.cur].factor() as f64
    }

    /// New settings. Knobs glide to their new values; a device switched
    /// on or off fades in or out; a new oversampling factor waits for the
    /// stack to glide out, switches, and glides back.
    pub fn set(&mut self, p: &AmpParams) {
        let want = os_index(factor_for(p.oversample, self.rate));
        if want != self.cur && self.mix > 0.0 {
            self.pending = Some(want);
        } else if want != self.cur {
            self.cur = want;
            self.os[want].reset();
        }
        let fs = self.fs();
        // Devices not in the stack any more go off.
        for k in Kind::ALL {
            if !p.units().iter().any(|u| u.kind == k)
                && let Some(old) = self.params.units().iter().find(|u| u.kind == k)
            {
                let mut off = *old;
                off.on = false;
                self.devices.set(&off, fs, false);
            }
        }
        for u in p.units() {
            let fresh = !self.devices.is_active(u.kind);
            self.devices.set(u, fs, fresh);
        }
        self.params = *p;
        self.target = if p.changes_sound() && self.pending.is_none() {
            1.0
        } else {
            0.0
        };
    }

    /// Forget everything: after a seek or a stop.
    pub fn reset(&mut self) {
        if let Some(i) = self.pending.take() {
            self.cur = i;
            let p = self.params;
            self.set(&p);
        }
        for o in &mut self.os {
            o.reset();
        }
        self.devices.reset();
        self.target = if self.params.changes_sound() {
            1.0
        } else {
            0.0
        };
        self.mix = self.target;
    }

    /// The stack's delay while it's on, in frames.
    pub fn latency(&self) -> usize {
        oversample::TAPS_PER_PHASE
    }

    pub fn static_gain(&self) -> f64 {
        if self.mix <= 0.0 && self.target <= 0.0 {
            1.0
        } else {
            self.params.static_gain()
        }
    }

    pub fn process(&mut self, buf: &mut [f64]) {
        if self.mix <= 0.0 && self.target <= 0.0 {
            if let Some(i) = self.pending.take() {
                // Glided out for a new factor: switch, and back in.
                self.cur = i;
                self.os[i].reset();
                self.devices.reset();
                let p = self.params;
                self.set(&p);
            } else {
                return;
            }
        }
        let ch = self.ch;
        let dch = ch.min(MAX_CH);
        let n = self.os[self.cur].factor();
        let order: [Option<Kind>; MAX_UNITS] = {
            let mut o = [None; MAX_UNITS];
            for (i, u) in self.params.units().iter().enumerate() {
                o[i] = Some(u.kind);
            }
            o
        };
        for frame in buf.chunks_exact_mut(ch) {
            self.dry.copy_from_slice(frame);
            let os = &mut self.os[self.cur];
            os.up(frame, &mut self.hi[..n * ch]);
            for p in 0..n {
                let f = &mut self.hi[p * ch..p * ch + dch];
                for k in order.iter().flatten() {
                    self.devices.run(*k, f);
                }
            }
            os.down(&self.hi[..n * ch], frame);
            if self.mix != self.target {
                if self.mix < self.target {
                    self.mix = (self.mix + self.mix_step).min(self.target);
                } else {
                    self.mix = (self.mix - self.mix_step).max(self.target);
                }
            }
            if self.mix < 1.0 {
                // Gliding in or out: against the undelayed input, so the
                // glide ends on a continuous signal either way.
                for (w, d) in frame.iter_mut().zip(&self.dry) {
                    *w = *d + (*w - *d) * self.mix;
                }
            }
        }
        self.devices.settle();
        if self.mix <= 0.0 && self.target <= 0.0 {
            for o in &mut self.os {
                o.reset();
            }
            self.devices.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f64, rate: u32, secs: f64) -> Vec<f64> {
        (0..(rate as f64 * secs) as usize)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin();
                [s, s]
            })
            .collect()
    }

    fn stack(units: &[Unit]) -> AmpParams {
        let mut p = AmpParams {
            on: true,
            n: units.len(),
            ..Default::default()
        };
        p.units[..units.len()].copy_from_slice(units);
        p
    }

    fn run(p: &AmpParams, input: &[f64], rate: u32) -> Vec<f64> {
        let mut a = AmpRt::new(2, rate);
        a.set(p);
        a.reset();
        let mut out = input.to_vec();
        for c in out.chunks_mut(1024 * 2) {
            a.process(c);
        }
        out
    }

    /// Level of a frequency in the left channel, by correlation.
    fn level(buf: &[f64], freq: f64, rate: u32) -> f64 {
        let l: Vec<f64> = buf.chunks(2).map(|f| f[0]).collect();
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in l.iter().enumerate() {
            let a = 2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64;
            re += s * a.cos();
            im += s * a.sin();
        }
        2.0 * (re * re + im * im).sqrt() / l.len() as f64
    }

    #[test]
    fn off_and_empty_are_untouched() {
        let x = sine(1000.0, 0.5, 44100, 0.5);
        assert_eq!(run(&AmpParams::default(), &x, 44100), x);
        let mut u = Unit::new(Kind::TubePreamp);
        u.on = false;
        assert_eq!(run(&stack(&[u]), &x, 44100), x, "on, but no device on");
    }

    #[test]
    fn every_device_at_its_defaults_stays_subtle() {
        // A 1 kHz tone at -12 dBFS through each device alone: it comes
        // out within 3 dB of where it went in, and whatever harmonics it
        // gained stay well under the tone.
        let rate = 44100;
        let x = sine(1000.0, 0.25, rate, 1.0);
        for k in Kind::ALL {
            let y = run(&stack(&[Unit::new(k)]), &x, rate);
            let tail = &y[(rate as usize / 2) * 2..];
            let f = level(tail, 1000.0, rate);
            let g = 20.0 * (f / 0.25).log10();
            assert!(g.abs() < 3.0, "{k:?}: {g:.2} dB");
            let h2 = level(tail, 2000.0, rate);
            let h3 = level(tail, 3000.0, rate);
            let thd = 20.0 * ((h2 * h2 + h3 * h3).sqrt() / f).log10();
            assert!(thd < -20.0, "{k:?}: harmonics at {thd:.1} dB");
        }
    }

    #[test]
    fn the_tube_preamp_makes_even_harmonics_and_the_power_amp_odd() {
        let rate = 48000;
        let x = sine(500.0, 0.7, rate, 1.0);
        // Moderate drive, default bias: the second harmonic leads, as in a
        // single-ended triode stage. Pushed much harder, any curve ends
        // up mostly odd.
        let mut pre = Unit::new(Kind::TubePreamp);
        pre.p[0] = 6.0;
        pre.p[2] = 0.0;
        let y = run(&stack(&[pre]), &x, rate);
        let t = &y[48000..];
        assert!(
            level(t, 1000.0, rate) > 2.0 * level(t, 1500.0, rate),
            "preamp: 2nd over 3rd"
        );
        let mut pow = Unit::new(Kind::TubePowerAmp);
        pow.p[0] = 12.0;
        let y = run(&stack(&[pow]), &x, rate);
        let t = &y[48000..];
        assert!(
            level(t, 1500.0, rate) > 10.0 * level(t, 1000.0, rate),
            "power amp: 3rd over 2nd"
        );
    }

    #[test]
    fn oversampling_keeps_aliases_down() {
        // A hard-driven 7 kHz tone at 44.1 kHz. At 2x (88.2 kHz) its 11th
        // harmonic, 77 kHz, folds back to 11.2 kHz, inside what you hear.
        // At 8x it has room, and is filtered off on the way down.
        let rate = 44100;
        let x = sine(7000.0, 0.8, rate, 0.5);
        let mut u = Unit::new(Kind::TubePowerAmp);
        u.p[0] = 24.0;
        let alias = |os: u8| {
            let mut p = stack(&[u]);
            p.oversample = os;
            let y = run(&p, &x, rate);
            let t = &y[rate as usize / 4 * 2..];
            20.0 * (level(t, 11200.0, rate) / level(t, 7000.0, rate)).log10()
        };
        let a2 = alias(2);
        let a8 = alias(8);
        assert!(a2 > -60.0, "2x should show the alias: {a2:.1} dB");
        assert!(a8 < a2 - 20.0, "8x {a8:.1} dB vs 2x {a2:.1} dB");
    }

    #[test]
    fn the_compressor_holds_loud_passages_down() {
        let rate = 44100;
        let loud = sine(1000.0, 0.9, rate, 1.0);
        let mut c = Unit::new(Kind::Compressor);
        c.p[0] = 1.0; // VCA
        c.p[1] = -20.0;
        c.p[2] = 4.0;
        c.p[5] = 0.0;
        let y = run(&stack(&[c]), &loud, rate);
        let g = 20.0 * (level(&y[44100..], 1000.0, rate) / 0.9).log10();
        // -1 dB in, threshold -20, 4:1: about 14 dB down.
        assert!((g + 14.0).abs() < 2.5, "{g:.1} dB");
    }

    #[test]
    fn phono_folds_the_bass_to_mono() {
        let rate = 44100;
        // 50 Hz in the left only: below the mono point it's shared.
        let x: Vec<f64> = (0..rate as usize)
            .flat_map(|i| {
                let s = 0.5 * (2.0 * std::f64::consts::PI * 50.0 * i as f64 / rate as f64).sin();
                [s, 0.0]
            })
            .collect();
        let mut u = Unit::new(Kind::Phono);
        u.p[0] = 200.0;
        u.p[1] = 0.0;
        u.p[3] = 0.0;
        let y = run(&stack(&[u]), &x, rate);
        let r: Vec<f64> = y[rate as usize..].chunks(2).map(|f| f[1]).collect();
        let rr: f64 = (r.iter().map(|s| s * s).sum::<f64>() / r.len() as f64).sqrt();
        assert!(rr > 0.12, "right got {rr}");
    }

    #[test]
    fn switching_on_glides_in() {
        // A device switched on mid-stream fades in over 20 ms: no jump
        // bigger than the tone itself makes from one sample to the next.
        let rate = 44100;
        let x = sine(200.0, 0.5, rate, 0.3);
        let mut a = AmpRt::new(2, rate);
        let mut out = x.clone();
        let mut u = Unit::new(Kind::TubePreamp);
        u.p[0] = 18.0;
        for (i, c) in out.chunks_mut(512).enumerate() {
            if i == 20 {
                a.set(&stack(&[u]));
            }
            a.process(c);
        }
        let max_step = out
            .chunks(2)
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        let tone_step = 0.5 * 2.0 * std::f64::consts::PI * 200.0 / rate as f64;
        assert!(max_step < tone_step * 1.5, "{max_step} vs {tone_step}");
    }

    #[test]
    fn factors_follow_the_rate() {
        assert_eq!(factor_for(8, 44100), 8);
        assert_eq!(factor_for(8, 192000), 4);
        assert_eq!(factor_for(4, 384000), 2);
        assert_eq!(factor_for(2, 768000), 1);
    }
}
