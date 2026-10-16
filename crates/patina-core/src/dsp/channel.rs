//! Channel tools: mono, stereo width, crossfeed and balance, in that
//! order, after the EQ. Stereo only; anything else passes through.
//!
//! Every control glides over a few milliseconds, so moving one while music
//! plays never clicks, and the whole stage has an on switch that glides
//! the same way (for A/B).
//!
//! Crossfeed is the simple Bauer-style kind headphone listeners know from
//! bs2b: each ear gets a low-passed share of the other channel, and loses
//! the same low-passed share of its own, so a mono sound comes out exactly
//! as it went in and only the stereo difference in the bass is narrowed.
//! That takes the edge off hard-panned old stereo on headphones.

use serde::{Deserialize, Serialize};

/// Controls glide over this long.
const GLIDE_MS: f64 = 15.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Crossfeed {
    #[default]
    Off,
    /// 700 Hz, about -9.5 dB of feed.
    Light,
    /// 700 Hz, -6 dB.
    Medium,
    /// 650 Hz, -4.5 dB.
    Strong,
}

impl Crossfeed {
    pub const ALL: [Crossfeed; 4] = [
        Crossfeed::Off,
        Crossfeed::Light,
        Crossfeed::Medium,
        Crossfeed::Strong,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Crossfeed::Off => "off",
            Crossfeed::Light => "light",
            Crossfeed::Medium => "medium",
            Crossfeed::Strong => "strong",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Crossfeed::Off => "Off",
            Crossfeed::Light => "Light",
            Crossfeed::Medium => "Medium",
            Crossfeed::Strong => "Strong",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// Cutoff in Hz, and how much of the low-passed difference crosses.
    fn shape(self) -> (f64, f64) {
        // Feed level g as a gain; the crossing share is g / (1 + g).
        let share = |db: f64| {
            let g = 10f64.powf(db / 20.0);
            g / (1.0 + g)
        };
        match self {
            Crossfeed::Off => (700.0, 0.0),
            Crossfeed::Light => (700.0, share(-9.5)),
            Crossfeed::Medium => (700.0, share(-6.0)),
            Crossfeed::Strong => (650.0, share(-4.5)),
        }
    }
}

/// The settings as the renderer runs them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelParams {
    pub on: bool,
    /// Sum to mono.
    pub mono: bool,
    /// Stereo width: 0 is mono, 1 untouched, up to 2 for twice as wide.
    pub width: f64,
    pub crossfeed: Crossfeed,
    /// -1 is left only, 0 centered, 1 right only. The far side is turned
    /// down; nothing is ever turned up.
    pub balance: f64,
}

impl Default for ChannelParams {
    fn default() -> Self {
        Self {
            on: false,
            mono: false,
            width: 1.0,
            crossfeed: Crossfeed::Off,
            balance: 0.0,
        }
    }
}

impl ChannelParams {
    /// On, and set to do something.
    pub fn changes_sound(&self) -> bool {
        self.on
            && (self.mono
                || (self.width - 1.0).abs() > 1e-9
                || self.crossfeed != Crossfeed::Off
                || self.balance.abs() > 1e-9)
    }

    /// The values the processor glides toward. Off is the identity.
    fn goals(&self) -> [f64; 5] {
        if !self.on {
            return [0.0, 1.0, 0.0, 1.0, 1.0];
        }
        let b = self.balance.clamp(-1.0, 1.0);
        let (_, share) = self.crossfeed.shape();
        [
            if self.mono { 1.0 } else { 0.0 },
            self.width.clamp(0.0, 2.0),
            share,
            if b > 0.0 { 1.0 - b } else { 1.0 },
            if b < 0.0 { 1.0 + b } else { 1.0 },
        ]
    }
}

/// The running stage. Small and allocation-free, so the limiter's look
/// ahead can run through a copy.
#[derive(Clone)]
pub struct ChannelRt {
    ch: usize,
    rate: u32,
    /// mono amount, width, crossfeed share, left gain, right gain.
    cur: [f64; 5],
    goal: [f64; 5],
    step: f64,
    /// One-pole low-pass for the crossfeed, per channel.
    lp_a: f64,
    lp: [f64; 2],
    cutoff: f64,
}

impl ChannelRt {
    pub fn new(ch: usize, rate: u32) -> Self {
        let mut s = Self {
            ch,
            rate,
            cur: [0.0, 1.0, 0.0, 1.0, 1.0],
            goal: [0.0, 1.0, 0.0, 1.0, 1.0],
            step: 1.0 / (rate as f64 * GLIDE_MS / 1000.0).max(1.0),
            lp_a: 0.0,
            lp: [0.0; 2],
            cutoff: 0.0,
        };
        s.set_cutoff(700.0);
        s
    }

    fn set_cutoff(&mut self, hz: f64) {
        if hz != self.cutoff {
            self.cutoff = hz;
            self.lp_a = (-2.0 * std::f64::consts::PI * hz / self.rate as f64).exp();
        }
    }

    pub fn set(&mut self, p: &ChannelParams) {
        self.goal = p.goals();
        if p.crossfeed != Crossfeed::Off {
            self.set_cutoff(p.crossfeed.shape().0);
        }
    }

    /// Jump to the settings and forget the filter: after a seek or stop.
    pub fn reset(&mut self) {
        self.cur = self.goal;
        self.lp = [0.0; 2];
    }

    fn idle(&self) -> bool {
        self.cur == [0.0, 1.0, 0.0, 1.0, 1.0] && self.goal == self.cur
    }

    pub fn process(&mut self, buf: &mut [f64]) {
        if self.ch != 2 || self.idle() {
            return;
        }
        let a = self.lp_a;
        for f in buf.chunks_exact_mut(2) {
            for (c, g) in self.cur.iter_mut().zip(self.goal) {
                if *c < g {
                    *c = (*c + self.step).min(g);
                } else if *c > g {
                    *c = (*c - self.step).max(g);
                }
            }
            let [mono, width, share, gl, gr] = self.cur;
            let (mut l, mut r) = (f[0], f[1]);
            // Mono and width, as mid and side.
            let m = 0.5 * (l + r);
            let s = 0.5 * (l - r) * (1.0 - mono) * width;
            l = m + s;
            r = m - s;
            // Crossfeed. The filter keeps running while off so it's warm
            // when it fades in.
            self.lp[0] = l + a * (self.lp[0] - l);
            self.lp[1] = r + a * (self.lp[1] - r);
            if share > 0.0 {
                let d = share * (self.lp[1] - self.lp[0]);
                l += d;
                r -= d;
            }
            f[0] = l * gl;
            f[1] = r * gr;
        }
        for v in &mut self.lp {
            if v.abs() < 1e-25 {
                *v = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, hz: f64, l: f64, r: f64) -> Vec<f64> {
        (0..n)
            .flat_map(|i| {
                let s = (2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin();
                [l * s, r * s]
            })
            .collect()
    }

    fn peak(b: &[f64], c: usize) -> f64 {
        b[b.len() / 2..]
            .chunks_exact(2)
            .fold(0.0f64, |m, f| m.max(f[c].abs()))
    }

    fn run(p: ChannelParams, mut b: Vec<f64>) -> Vec<f64> {
        let mut rt = ChannelRt::new(2, 48000);
        rt.set(&p);
        rt.process(&mut b);
        b
    }

    #[test]
    fn off_is_untouched() {
        let b = tone(4800, 440.0, 0.5, -0.2);
        let p = ChannelParams {
            on: false,
            mono: true,
            balance: 0.5,
            ..Default::default()
        };
        assert_eq!(run(p, b.clone()), b);
        assert!(!p.changes_sound());
    }

    #[test]
    fn mono_sums() {
        let p = ChannelParams {
            on: true,
            mono: true,
            ..Default::default()
        };
        let b = run(p, tone(9600, 440.0, 0.6, 0.2));
        assert!((peak(&b, 0) - 0.4).abs() < 1e-3);
        assert!((peak(&b, 1) - 0.4).abs() < 1e-3);
    }

    #[test]
    fn width_zero_is_mono_and_two_widens() {
        let mut p = ChannelParams {
            on: true,
            width: 0.0,
            ..Default::default()
        };
        let b = run(p, tone(9600, 440.0, 0.6, 0.2));
        assert!((peak(&b, 0) - peak(&b, 1)).abs() < 1e-3);
        p.width = 2.0;
        let b = run(p, tone(9600, 440.0, 0.6, 0.2));
        // Mid 0.4, side 0.2 doubled: 0.8 and 0.0.
        assert!((peak(&b, 0) - 0.8).abs() < 1e-3);
        assert!(peak(&b, 1) < 1e-3);
    }

    #[test]
    fn balance_turns_the_far_side_down() {
        let p = ChannelParams {
            on: true,
            balance: 0.5,
            ..Default::default()
        };
        let b = run(p, tone(9600, 440.0, 0.5, 0.5));
        assert!((peak(&b, 0) - 0.25).abs() < 1e-3);
        assert!((peak(&b, 1) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn crossfeed_leaves_mono_alone_and_narrows_bass() {
        let p = ChannelParams {
            on: true,
            crossfeed: Crossfeed::Medium,
            ..Default::default()
        };
        // A centered tone is untouched.
        let b = run(p, tone(9600, 100.0, 0.5, 0.5));
        assert!((peak(&b, 0) - 0.5).abs() < 2e-3);
        // Left-only bass bleeds into the right; left-only treble mostly
        // doesn't.
        let lo = run(p, tone(48000, 100.0, 0.5, 0.0));
        let hi = run(p, tone(48000, 8000.0, 0.5, 0.0));
        assert!(peak(&lo, 1) > 0.12, "{}", peak(&lo, 1));
        assert!(peak(&hi, 1) < 0.02, "{}", peak(&hi, 1));
    }

    #[test]
    fn changes_glide() {
        let mut rt = ChannelRt::new(2, 48000);
        rt.set(&ChannelParams {
            on: true,
            balance: 1.0,
            ..Default::default()
        });
        let mut b = tone(4800, 440.0, 0.5, 0.5);
        rt.process(&mut b);
        // The left side fades over 15 ms rather than vanishing at once.
        let early = b[..40]
            .chunks_exact(2)
            .fold(0.0f64, |m, f| m.max(f[0].abs()));
        assert!(early > 0.0);
        assert!(peak(&b, 0) < 1e-9);
    }
}
