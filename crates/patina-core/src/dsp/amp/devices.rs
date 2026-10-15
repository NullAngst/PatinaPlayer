//! The amp stack's devices, as they run: at the oversampled rate, a frame
//! at a time, so the ones that care about stereo (the phono stage's mono
//! bass, the console's crosstalk, the compressor's linked detector) see
//! both channels together.
//!
//! These are character effects tuned to the textbook behaviour of each
//! kind of hardware, not models of any branded unit. Each is normalised so
//! a quiet signal comes out at the level it went in: drive adds harmonics
//! and squeezes peaks, it doesn't turn the music up.

use super::{Kind, MAX_CH, Unit};
use crate::dsp::biquad::{Band, Coeffs, Kind as Shape, State};

#[inline]
fn db(x: f64) -> f64 {
    10f64.powf(x / 20.0)
}

/// A gain that glides to its target over about 10 ms, so dragging a
/// knob doesn't zipper.
#[derive(Clone, Copy, Default)]
struct Smooth {
    v: f64,
    target: f64,
    k: f64,
}

impl Smooth {
    fn set(&mut self, target: f64, fs: f64, jump: bool) {
        self.target = target;
        self.k = 1.0 - (-1.0 / (0.010 * fs)).exp();
        if jump {
            self.v = target;
        }
    }
    #[inline]
    fn next(&mut self) -> f64 {
        self.v += (self.target - self.v) * self.k;
        self.v
    }
}

/// One biquad per channel.
#[derive(Clone, Copy)]
struct Filter {
    c: Coeffs,
    s: [State; MAX_CH],
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            c: Coeffs::IDENTITY,
            s: [State::default(); MAX_CH],
        }
    }
}

impl Filter {
    fn design(&mut self, kind: Shape, freq: f64, gain: f64, q: f64, fs: f64) {
        self.c = Coeffs::design(
            &Band {
                kind,
                freq,
                gain,
                q,
            },
            fs as u32,
        );
    }
    #[inline]
    fn run(&mut self, c: usize, x: f64) -> f64 {
        self.s[c].run(&self.c, x)
    }
    fn reset(&mut self) {
        self.s = [State::default(); MAX_CH];
    }
    fn settle(&mut self) {
        for s in &mut self.s {
            s.settle();
        }
    }
}

/// Takes out the DC an asymmetric curve leaves behind.
#[derive(Clone, Copy, Default)]
struct DcBlock {
    r: f64,
    x1: [f64; MAX_CH],
    y1: [f64; MAX_CH],
}

impl DcBlock {
    fn set(&mut self, fs: f64) {
        self.r = 1.0 - 2.0 * std::f64::consts::PI * 8.0 / fs;
    }
    #[inline]
    fn run(&mut self, c: usize, x: f64) -> f64 {
        let y = x - self.x1[c] + self.r * self.y1[c];
        self.x1[c] = x;
        self.y1[c] = if y.abs() < 1e-25 { 0.0 } else { y };
        y
    }
    fn reset(&mut self) {
        self.x1 = [0.0; MAX_CH];
        self.y1 = [0.0; MAX_CH];
    }
}

/// xorshift64*, for noise, crackle and the tape's drift.
#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    #[inline]
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    /// -1 to 1.
    #[inline]
    fn bi(&mut self) -> f64 {
        2.0 * self.next() - 1.0
    }
}

/// A one-pole envelope follower with separate attack and release.
#[derive(Clone, Copy, Default)]
struct Env {
    v: f64,
    a: f64,
    r: f64,
}

impl Env {
    fn set(&mut self, attack_s: f64, release_s: f64, fs: f64) {
        self.a = 1.0 - (-1.0 / (attack_s.max(1e-5) * fs)).exp();
        self.r = 1.0 - (-1.0 / (release_s.max(1e-5) * fs)).exp();
    }
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let k = if x > self.v { self.a } else { self.r };
        self.v += (x - self.v) * k;
        self.v
    }
}

/// What every device shares: input and output trims, its own on/off
/// glide, and the mix knob.
#[derive(Clone, Copy, Default)]
struct Common {
    tin: Smooth,
    tout: Smooth,
    mix: Smooth,
    /// 0 off, 1 on, gliding between over 20 ms.
    on: f64,
    on_target: f64,
    on_step: f64,
}

impl Common {
    fn set(&mut self, u: &Unit, mix: f64, fs: f64, jump: bool) {
        self.tin.set(db(u.trim_in), fs, jump);
        self.tout.set(db(u.trim_out), fs, jump);
        self.mix.set(mix, fs, jump);
        self.on_target = if u.on { 1.0 } else { 0.0 };
        self.on_step = 1.0 / (0.020 * fs);
        if jump {
            self.on = self.on_target;
        }
    }
    fn active(&self) -> bool {
        self.on > 0.0 || self.on_target > 0.0
    }
}

/// One of each device. A stack holds each kind at most once, so all nine
/// live here from the start, and reordering or switching one on never
/// allocates on the output thread.
pub struct Devices {
    pub(super) tube_pre: TubePre,
    pub(super) tube_power: TubePower,
    pub(super) transformer: Transformer,
    pub(super) tape: Tape,
    pub(super) phono: Phono,
    pub(super) console: Console,
    pub(super) comp: Compressor,
    pub(super) exciter: Exciter,
    pub(super) bass: Bass,
    common: [Common; 9],
}

impl Devices {
    pub fn new(max_fs: f64) -> Self {
        Self {
            tube_pre: TubePre::default(),
            tube_power: TubePower::default(),
            transformer: Transformer::default(),
            tape: Tape::new(max_fs),
            phono: Phono::default(),
            console: Console::default(),
            comp: Compressor::default(),
            exciter: Exciter::default(),
            bass: Bass::default(),
            common: [Common::default(); 9],
        }
    }

    /// New settings for one device at rate `fs`. `jump` skips the glides,
    /// for a fresh start.
    pub fn set(&mut self, u: &Unit, fs: f64, jump: bool) {
        let p = &u.p;
        let mix = match u.kind.mix_param() {
            Some(i) => p[i] / 100.0,
            None => 1.0,
        };
        self.common[u.kind as usize].set(u, mix, fs, jump);
        match u.kind {
            Kind::TubePreamp => self.tube_pre.set(p, fs, jump),
            Kind::TubePowerAmp => self.tube_power.set(p, fs, jump),
            Kind::Transformer => self.transformer.set(p, fs),
            Kind::Tape => self.tape.set(p, fs),
            Kind::Phono => self.phono.set(p, fs),
            Kind::Console => self.console.set(p, fs, jump),
            Kind::Compressor => self.comp.set(p, fs, jump),
            Kind::Exciter => self.exciter.set(p, fs),
            Kind::BassEnhancer => self.bass.set(p, fs),
        }
    }

    pub fn is_active(&self, k: Kind) -> bool {
        self.common[k as usize].active()
    }

    pub fn reset(&mut self) {
        self.tube_pre.reset();
        self.tube_power.reset();
        self.transformer.reset();
        self.tape.reset();
        self.phono.reset();
        self.console.reset();
        self.comp.reset();
        self.exciter.reset();
        self.bass.reset();
        for c in &mut self.common {
            c.on = c.on_target;
            c.tin.v = c.tin.target;
            c.tout.v = c.tout.target;
            c.mix.v = c.mix.target;
        }
    }

    pub fn settle(&mut self) {
        self.tube_pre.warm.settle();
        self.tube_pre.body.settle();
        self.tube_power.presence.settle();
        self.transformer.lc.settle();
        self.transformer.hr.settle();
        self.tape.bump.settle();
        self.tape.hf.settle();
        self.tape.hiss_hp.settle();
        self.phono.side_hp.settle();
        self.phono.warm.settle();
        self.phono.rumble.settle();
        self.phono.rumble2.settle();
        self.console.bleed.settle();
        self.exciter.hp1.settle();
        self.exciter.hp2.settle();
        self.bass.lp.settle();
        self.bass.hp.settle();
        self.bass.lp2.settle();
    }

    /// Run one high-rate frame through device `k`.
    #[inline]
    pub fn run(&mut self, k: Kind, f: &mut [f64]) {
        let ch = f.len().min(MAX_CH);
        let cm = &mut self.common[k as usize];
        if cm.on < cm.on_target {
            cm.on = (cm.on + cm.on_step).min(cm.on_target);
        } else if cm.on > cm.on_target {
            cm.on = (cm.on - cm.on_step).max(cm.on_target);
        }
        let on = cm.on;
        let (tin, tout, mix) = (cm.tin.next(), cm.tout.next(), cm.mix.next());
        if on <= 0.0 {
            return;
        }
        let mut dry = [0.0; MAX_CH];
        dry[..ch].copy_from_slice(&f[..ch]);
        for s in f[..ch].iter_mut() {
            *s *= tin;
        }
        let pre = {
            let mut a = [0.0; MAX_CH];
            a[..ch].copy_from_slice(&f[..ch]);
            a
        };
        match k {
            Kind::TubePreamp => self.tube_pre.run(&mut f[..ch]),
            Kind::TubePowerAmp => self.tube_power.run(&mut f[..ch]),
            Kind::Transformer => self.transformer.run(&mut f[..ch]),
            Kind::Tape => self.tape.run(&mut f[..ch]),
            Kind::Phono => self.phono.run(&mut f[..ch]),
            Kind::Console => self.console.run(&mut f[..ch]),
            Kind::Compressor => self.comp.run(&mut f[..ch]),
            Kind::Exciter => self.exciter.run(&mut f[..ch]),
            Kind::BassEnhancer => self.bass.run(&mut f[..ch]),
        }
        for c in 0..ch {
            // The mix blends against the trimmed input; the device's own
            // on/off blends against what came in untouched.
            let wet = pre[c] + (f[c] - pre[c]) * mix;
            let out = wet * tout;
            f[c] = dry[c] + (out - dry[c]) * on;
        }
    }

    pub fn tape_delay_frames(&self) -> f64 {
        self.tape.base
    }
}

// ----- Tube preamp -----

/// A triode stage: a soft curve with a bias that tilts it, so the top and
/// bottom of the wave bend differently. That's the even harmonics, the
/// second above all, which is the "warm" in warm tube sound.
#[derive(Default)]
pub struct TubePre {
    drive: Smooth,
    bias: f64,
    /// Small-signal slope of the curve at the bias point, to normalise.
    t0: f64,
    slope: f64,
    warm: Filter,
    body: Filter,
    dc: DcBlock,
}

impl TubePre {
    fn set(&mut self, p: &[f64], fs: f64, jump: bool) {
        // Full scale reaches half the drive into the curve.
        self.drive.set(0.5 * db(p[0]), fs, jump);
        self.bias = p[1] / 100.0;
        self.t0 = self.bias.tanh();
        self.slope = 1.0 - self.t0 * self.t0;
        let w = p[2] / 100.0;
        self.warm
            .design(Shape::HighShelf, 7000.0, -5.0 * w, 0.7, fs);
        self.body.design(Shape::LowShelf, 180.0, 1.5 * w, 0.7, fs);
        self.dc.set(fs);
    }
    fn reset(&mut self) {
        self.warm.reset();
        self.body.reset();
        self.dc.reset();
        self.drive.v = self.drive.target;
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        let g = self.drive.next().max(1e-6);
        for (c, s) in f.iter_mut().enumerate() {
            let y = ((g * *s + self.bias).tanh() - self.t0) / (g * self.slope);
            let y = self.dc.run(c, y);
            let y = self.body.run(c, y);
            *s = self.warm.run(c, y);
        }
    }
}

// ----- Tube power amp -----

/// A push-pull output stage: the two halves cancel the even harmonics, so
/// what's left is odd, and a power supply that sags when the music gets
/// loud, squeezing the peaks a little more.
#[derive(Default)]
pub struct TubePower {
    drive: Smooth,
    sag: f64,
    env: Env,
    presence: Filter,
}

impl TubePower {
    fn set(&mut self, p: &[f64], fs: f64, jump: bool) {
        self.drive.set(0.5 * db(p[0]), fs, jump);
        self.sag = p[1] / 100.0;
        self.env.set(0.005, 0.25, fs);
        self.presence
            .design(Shape::HighShelf, 3500.0, p[2], 0.7, fs);
    }
    fn reset(&mut self) {
        self.env.v = 0.0;
        self.presence.reset();
        self.drive.v = self.drive.target;
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        let g = self.drive.next().max(1e-6);
        let peak = f.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        let e = self.env.run(peak);
        // Up to about 3 dB of droop on loud passages at full sag.
        let sag = 1.0 / (1.0 + 0.6 * self.sag * e);
        for (c, s) in f.iter_mut().enumerate() {
            let y = (g * sag * *s).tanh() / g;
            *s = self.presence.run(c, y);
        }
    }
}

// ----- Output transformer -----

/// Iron saturates at the bottom end first, so the lows thicken and round
/// off while the rest passes clean; the windings roll off the extreme
/// lows and highs.
#[derive(Default)]
pub struct Transformer {
    sat: f64,
    split: f64,
    low: [f64; MAX_CH],
    lc: Filter,
    hr: Filter,
}

impl Transformer {
    fn set(&mut self, p: &[f64], fs: f64) {
        self.sat = 1.0 + 6.0 * p[0] / 100.0;
        self.split = 1.0 - (-2.0 * std::f64::consts::PI * 150.0 / fs).exp();
        self.lc.design(Shape::HighPass, p[1], 0.0, 0.707, fs);
        self.hr.design(Shape::LowPass, p[2] * 1000.0, 0.0, 0.6, fs);
    }
    fn reset(&mut self) {
        self.low = [0.0; MAX_CH];
        self.lc.reset();
        self.hr.reset();
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        for (c, s) in f.iter_mut().enumerate() {
            self.low[c] += (*s - self.low[c]) * self.split;
            let lo = self.low[c];
            let hi = *s - lo;
            let lo = (self.sat * lo).tanh() / self.sat;
            let y = self.lc.run(c, hi + lo);
            *s = self.hr.run(c, y);
        }
    }
}

// ----- Tape deck -----

const SPEEDS: [(f64, f64, f64); 3] = [
    // head bump Hz, bump dB, high-frequency loss Hz
    (45.0, 2.5, 11000.0),
    (70.0, 2.0, 16000.0),
    (110.0, 1.5, 21000.0),
];

/// Tape: a soft squeeze on loud passages, the head bump in the bass, the
/// top end the speed allows, and the speed wandering a little (slow wow,
/// fast flutter), through a short modulated delay.
pub struct Tape {
    sat: f64,
    wow: f64,
    flutter: f64,
    hiss: f64,
    bump: Filter,
    hf: Filter,
    hiss_hp: Filter,
    /// Delay line per channel.
    line: Vec<f64>,
    len: usize,
    pos: usize,
    /// Centre delay, in high-rate frames.
    pub(super) base: f64,
    fs: f64,
    ph_wow: f64,
    ph_flut: f64,
    drift: f64,
    rng: Rng,
}

/// Longest the modulated delay can reach, in seconds.
const TAPE_MAX_S: f64 = 0.004;

impl Tape {
    fn new(max_fs: f64) -> Self {
        let len = (TAPE_MAX_S * max_fs) as usize + 8;
        Self {
            sat: 1.0,
            wow: 0.0,
            flutter: 0.0,
            hiss: 0.0,
            bump: Filter::default(),
            hf: Filter::default(),
            hiss_hp: Filter::default(),
            line: vec![0.0; len * MAX_CH],
            len,
            pos: 0,
            base: 0.0,
            fs: 48000.0,
            ph_wow: 0.0,
            ph_flut: 0.0,
            drift: 0.0,
            rng: Rng(0x7A9E_D00D_1234_5678),
        }
    }
    fn set(&mut self, p: &[f64], fs: f64) {
        let (bf, bg, hf) = SPEEDS[(p[0].round() as usize).min(2)];
        self.bump.design(Shape::Peak, bf, bg, 1.0, fs);
        self.hf.design(Shape::LowPass, hf, 0.0, 0.6, fs);
        self.hiss_hp.design(Shape::HighPass, 1500.0, 0.0, 0.7, fs);
        self.sat = 1.0 + 3.0 * p[1] / 100.0;
        // Peak speed deviation at 100%: 0.15% wow at 0.5 Hz, 0.08%
        // flutter at 6.5 Hz. A delay swinging by d at f Hz bends the speed
        // by 2·pi·f·d.
        self.wow = 0.0015 * p[2] / 100.0 / (2.0 * std::f64::consts::PI * 0.5) * fs;
        self.flutter = 0.0008 * p[3] / 100.0 / (2.0 * std::f64::consts::PI * 6.5) * fs;
        self.hiss = p[4] / 100.0 * db(-58.0);
        self.fs = fs;
        // Centre the swing so it never reaches below a frame.
        self.base = (self.wow + self.flutter + 2.0).min(TAPE_MAX_S * fs * 0.9);
    }
    fn reset(&mut self) {
        self.line.fill(0.0);
        self.bump.reset();
        self.hf.reset();
        self.hiss_hp.reset();
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        let two_pi = 2.0 * std::f64::consts::PI;
        self.ph_wow = (self.ph_wow + 0.5 / self.fs) % 1.0;
        self.ph_flut = (self.ph_flut + 6.5 / self.fs) % 1.0;
        // A slow random walk on top, so the wow isn't a perfect sine.
        self.drift = 0.9999 * self.drift + 0.0001 * self.rng.bi();
        let d = self.base
            + self.wow * ((two_pi * self.ph_wow).sin() + 3.0 * self.drift)
            + self.flutter * (two_pi * self.ph_flut).sin();
        let d = d.clamp(1.0, (self.len - 3) as f64);
        self.pos = (self.pos + 1) % self.len;
        let di = d.floor() as usize;
        let fr = d - di as f64;
        let i0 = (self.pos + self.len - di) % self.len;
        let i1 = (i0 + self.len - 1) % self.len;
        let noise = if self.hiss > 0.0 { self.rng.bi() } else { 0.0 };
        for (c, s) in f.iter_mut().enumerate() {
            let x = (self.sat * *s).tanh() / self.sat;
            let x = self.bump.run(c, x);
            let x = self.hf.run(c, x);
            let line = &mut self.line[c * self.len..(c + 1) * self.len];
            line[self.pos] = x;
            let y = line[i0] * (1.0 - fr) + line[i1] * fr;
            let h = if self.hiss > 0.0 {
                self.hiss_hp.run(c, noise) * self.hiss
            } else {
                0.0
            };
            *s = y + h;
        }
    }
}

// ----- Phono stage -----

/// An old turntable's character: the bass folded to mono (cutters did
/// that so the stylus wouldn't jump), a little rumble from the motor, the
/// odd crackle of dust, and a softer top.
pub struct Phono {
    side_hp: Filter,
    mono: bool,
    warm: Filter,
    rumble: Filter,
    rumble2: Filter,
    rumble_amt: f64,
    crackle_p: f64,
    crackle_amt: f64,
    /// A click in progress: level, decay per frame.
    click: [f64; MAX_CH],
    decay: f64,
    rng: Rng,
}

impl Default for Phono {
    fn default() -> Self {
        Self {
            side_hp: Filter::default(),
            mono: false,
            warm: Filter::default(),
            rumble: Filter::default(),
            rumble2: Filter::default(),
            rumble_amt: 0.0,
            crackle_p: 0.0,
            crackle_amt: 0.0,
            click: [0.0; MAX_CH],
            decay: 0.0,
            rng: Rng(0x5EED_CAFE_F00D_0001),
        }
    }
}

impl Phono {
    fn set(&mut self, p: &[f64], fs: f64) {
        self.mono = p[0] >= 10.0;
        if self.mono {
            self.side_hp.design(Shape::HighPass, p[0], 0.0, 0.707, fs);
        }
        self.rumble.design(Shape::LowPass, 25.0, 0.0, 0.707, fs);
        self.rumble2.design(Shape::HighPass, 8.0, 0.0, 0.707, fs);
        // Filtered noise loses most of its level in the 8-25 Hz band;
        // this lands full rumble around -50 dBFS.
        self.rumble_amt = p[1] / 100.0 * 0.08;
        // Up to about 15 crackles a second at full.
        self.crackle_p = p[2] / 100.0 * 15.0 / fs;
        self.crackle_amt = p[2] / 100.0 * 0.06;
        self.decay = (-1.0 / (0.0003 * fs)).exp();
        self.warm
            .design(Shape::HighShelf, 6000.0, -4.0 * p[3] / 100.0, 0.7, fs);
    }
    fn reset(&mut self) {
        self.side_hp.reset();
        self.warm.reset();
        self.rumble.reset();
        self.rumble2.reset();
        self.click = [0.0; MAX_CH];
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        if self.mono && f.len() == 2 {
            let mid = 0.5 * (f[0] + f[1]);
            let side = self.side_hp.run(0, 0.5 * (f[0] - f[1]));
            f[0] = mid + side;
            f[1] = mid - side;
        }
        let r = if self.rumble_amt > 0.0 {
            let n = self.rng.bi();
            self.rumble2.run(0, self.rumble.run(0, n)) * self.rumble_amt
        } else {
            0.0
        };
        if self.crackle_p > 0.0 && self.rng.next() < self.crackle_p {
            let a = self.crackle_amt * (0.3 + 0.7 * self.rng.next());
            let sign = if self.rng.next() < 0.5 { -1.0 } else { 1.0 };
            let lean = self.rng.next();
            for (c, k) in self.click.iter_mut().enumerate().take(f.len()) {
                // A little louder in one channel than the other.
                let w = if c == 0 {
                    0.5 + lean * 0.5
                } else {
                    1.0 - lean * 0.5
                };
                *k += sign * a * w;
            }
        }
        for (c, s) in f.iter_mut().enumerate() {
            let y = self.warm.run(c, *s);
            *s = y + r + self.click[c];
            self.click[c] *= self.decay;
        }
    }
}

// ----- Console channel -----

/// A mixing desk channel: a whisper of saturation, and a little of each
/// channel bleeding into the other, mostly in the highs, the way it does
/// between neighbouring channel strips.
#[derive(Default)]
pub struct Console {
    drive: Smooth,
    bleed_g: f64,
    bleed: Filter,
}

impl Console {
    fn set(&mut self, p: &[f64], fs: f64, jump: bool) {
        self.drive.set(1.0 + 2.0 * p[0] / 100.0, fs, jump);
        self.bleed_g = db(p[1]);
        self.bleed.design(Shape::HighPass, 1000.0, 0.0, 0.7, fs);
    }
    fn reset(&mut self) {
        self.bleed.reset();
        self.drive.v = self.drive.target;
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        let g = self.drive.next();
        for s in f.iter_mut() {
            *s = (g * *s).tanh() / g;
        }
        if f.len() == 2 {
            let bl = self.bleed.run(0, f[1]) * self.bleed_g;
            let br = self.bleed.run(1, f[0]) * self.bleed_g;
            f[0] += bl;
            f[1] += br;
        }
    }
}

// ----- Bus compressor -----

/// Glue: a gentle compressor across both channels, linked so the image
/// doesn't wander. Opto style follows the level smoothly and lets go
/// slower the harder it worked, like the light cell in an LA-2A-type
/// unit; VCA follows peaks and lets go evenly.
#[derive(Default)]
pub struct Compressor {
    opto: bool,
    thr: f64,
    ratio: f64,
    att: f64,
    rel: f64,
    makeup: Smooth,
    /// Gain reduction in dB, smoothed.
    gr: f64,
    /// Level detector (power for opto).
    det: f64,
    det_k: f64,
    fs: f64,
}

impl Compressor {
    fn set(&mut self, p: &[f64], fs: f64, jump: bool) {
        self.opto = p[0] < 0.5;
        self.thr = p[1];
        self.ratio = p[2].max(1.0);
        self.att = 1.0 - (-1.0 / (p[3].max(0.1) / 1000.0 * fs)).exp();
        self.rel = p[4].max(10.0) / 1000.0;
        self.makeup.set(db(p[5]), fs, jump);
        // RMS window for opto: about 10 ms.
        self.det_k = 1.0 - (-1.0 / (0.010 * fs)).exp();
        self.fs = fs;
    }
    fn reset(&mut self) {
        self.gr = 0.0;
        self.det = 0.0;
        self.makeup.v = self.makeup.target;
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        let level = if self.opto {
            let pw = f.iter().map(|s| s * s).fold(0.0, f64::max);
            self.det += (pw - self.det) * self.det_k;
            10.0 * self.det.max(1e-12).log10()
        } else {
            let pk = f.iter().fold(0.0f64, |m, s| m.max(s.abs()));
            20.0 * pk.max(1e-6).log10()
        };
        // Soft knee, 6 dB wide.
        let over = level - self.thr;
        let knee = 6.0;
        let slope = 1.0 - 1.0 / self.ratio;
        let want = if over <= -knee / 2.0 {
            0.0
        } else if over >= knee / 2.0 {
            over * slope
        } else {
            slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
        };
        let k = if want > self.gr {
            self.att
        } else {
            // Opto lets go slower the deeper it went.
            let rel = if self.opto {
                self.rel * (1.0 + self.gr / 6.0)
            } else {
                self.rel
            };
            1.0 - (-1.0 / (rel * self.fs)).exp()
        };
        self.gr += (want - self.gr) * k;
        let g = db(-self.gr) * self.makeup.next();
        for s in f.iter_mut() {
            *s *= g;
        }
    }
}

// ----- Exciter -----

/// Sparkle: the highs above a corner frequency are run through a curve to
/// make new harmonics of their own, and those are added back, so cymbals
/// and breath get brighter without a plain treble boost.
#[derive(Default)]
pub struct Exciter {
    amt: f64,
    hp1: Filter,
    hp2: Filter,
    dc: DcBlock,
}

impl Exciter {
    fn set(&mut self, p: &[f64], fs: f64) {
        let fc = p[0] * 1000.0;
        self.hp1.design(Shape::HighPass, fc, 0.0, 0.707, fs);
        self.hp2.design(Shape::HighPass, fc, 0.0, 0.707, fs);
        self.amt = p[1] / 100.0;
        self.dc.set(fs);
    }
    fn reset(&mut self) {
        self.hp1.reset();
        self.hp2.reset();
        self.dc.reset();
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        for (c, s) in f.iter_mut().enumerate() {
            let h = self.hp1.run(c, *s);
            // Asymmetric, so both odd and even harmonics come out; the
            // linear part is taken away, leaving only what's new.
            let a = 4.0;
            let shaped =
                ((a * h + 0.3).tanh() - 0.3f64.tanh()) / a - h * (1.0 - 0.3f64.tanh().powi(2));
            let shaped = self.dc.run(c, shaped);
            let harm = self.hp2.run(c, shaped);
            *s += 6.0 * self.amt * harm;
        }
    }
}

// ----- Bass enhancer -----

/// For small speakers and earbuds that can't move enough air for the
/// lowest notes: harmonics of the bass are made and added an octave or two
/// up, and the ear fills in the fundamental that isn't really there.
#[derive(Default)]
pub struct Bass {
    amt: f64,
    lp: Filter,
    hp: Filter,
    lp2: Filter,
    dc: DcBlock,
}

impl Bass {
    fn set(&mut self, p: &[f64], fs: f64) {
        let fc = p[0];
        self.lp.design(Shape::LowPass, fc, 0.0, 0.707, fs);
        self.hp.design(Shape::HighPass, fc * 1.5, 0.0, 0.707, fs);
        self.lp2.design(Shape::LowPass, fc * 5.0, 0.0, 0.707, fs);
        self.amt = p[1] / 100.0;
        self.dc.set(fs);
    }
    fn reset(&mut self) {
        self.lp.reset();
        self.hp.reset();
        self.lp2.reset();
        self.dc.reset();
    }
    #[inline]
    fn run(&mut self, f: &mut [f64]) {
        for (c, s) in f.iter_mut().enumerate() {
            let lo = self.lp.run(c, *s);
            // Full-wave rectified: every harmonic is even, the octave
            // strongest.
            let h = self.dc.run(c, lo.abs());
            let h = self.lp2.run(c, self.hp.run(c, h));
            *s += 2.5 * self.amt * h;
        }
    }
}
