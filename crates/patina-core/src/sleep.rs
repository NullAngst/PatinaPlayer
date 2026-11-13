//! The sleep timer: pause after a while, at a time of day, or at the end
//! of the song playing.
//!
//! The player thread runs it (see [`crate::player::Command::SetSleep`]), so
//! it keeps going with the window closed, the screen off, or the terminal
//! app in the background. Over its last [`FADE`] the volume comes down,
//! then playback pauses, and the volume is back where it was for the next
//! play. Bit-perfect output gets no fade, since it doesn't touch the
//! samples; it just pauses.
//!
//! This module reads what people type for it and writes the labels the
//! apps show, so the desktop app, the terminal app and the phone all take
//! and say the same things.

use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local, NaiveTime, TimeZone, Timelike};

/// How long the volume takes to come down before the pause.
pub const FADE: Duration = Duration::from_secs(30);

/// The volume, as a gain, with `left` to go before the pause: 1 until the
/// last [`FADE`], then down in even steps of loudness to -40 dB at the end,
/// where the pause's own short fade takes it the rest of the way.
pub fn fade_gain(left: Duration) -> f64 {
    if left >= FADE {
        return 1.0;
    }
    let db = -40.0 * (1.0 - left.as_secs_f64() / FADE.as_secs_f64());
    10f64.powf(db / 20.0)
}

/// The longest a timer can run.
const MAX: Duration = Duration::from_secs(24 * 3600);

/// When to pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepTimer {
    /// At this moment. Wall-clock time, so time the phone spent asleep
    /// counts.
    At(SystemTime),
    /// When the song playing now ends.
    EndOfSong,
}

impl SleepTimer {
    /// This long from now.
    pub fn after(d: Duration) -> Self {
        SleepTimer::At(SystemTime::now() + d)
    }

    /// Time left, for a timer set to a moment.
    pub fn remaining(&self) -> Option<Duration> {
        match self {
            SleepTimer::At(t) => Some(t.duration_since(SystemTime::now()).unwrap_or_default()),
            SleepTimer::EndOfSong => None,
        }
    }

    /// Short, for the bottom bar or the status line: "41:23", "1:02:10",
    /// "end of song".
    pub fn short(&self) -> String {
        match self.remaining() {
            None => "end of song".into(),
            Some(d) => clock(d),
        }
    }

    /// A sentence, for the timer's own row or a message: "Pauses at 23:47,
    /// in 45 min", "Pauses at the end of this song".
    pub fn describe(&self) -> String {
        match self {
            SleepTimer::EndOfSong => "Pauses at the end of this song".into(),
            SleepTimer::At(t) => {
                let at: DateTime<Local> = (*t).into();
                let left = self.remaining().unwrap_or_default();
                format!("Pauses at {}, in {}", at.format("%H:%M"), words(left))
            }
        }
    }
}

/// "41:23" or "1:02:10", rounded up so it never says 0:00 with time left.
fn clock(d: Duration) -> String {
    let s = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "45 min", "1 h 30 min", "under a minute".
fn words(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60);
    match (m / 60, m % 60) {
        (0, 0) => "under a minute".into(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// What the timer box says when it can't read something.
pub const HELP: &str =
    "Minutes (45), a length (1h30), a time (23:30 or 11:30pm), or \"end\" for the end of this song";

/// Read what was typed. Minutes on their own ("45"), a length ("1h30",
/// "90m", "1.5h", "2 hours"), a time of day ("23:30", "11:30pm", "11pm",
/// "at 7"), or "end" for the end of the song. A time already past today
/// means tomorrow.
pub fn parse(text: &str) -> Result<SleepTimer, String> {
    parse_at(text, Local::now())
}

/// [`parse`], with "now" given, for tests.
pub fn parse_at<Tz: TimeZone>(text: &str, now: DateTime<Tz>) -> Result<SleepTimer, String> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return Err(HELP.into());
    }
    if matches!(
        t.as_str(),
        "end" | "e" | "song" | "end of song" | "end of this song" | "end of the song"
    ) {
        return Ok(SleepTimer::EndOfSong);
    }
    let (clock_text, forced) = match t.strip_prefix("at ") {
        Some(rest) => (rest.trim(), true),
        None => (t.as_str(), false),
    };
    let ampm = clock_text.ends_with("am") || clock_text.ends_with("pm");
    if forced || ampm || clock_text.contains(':') {
        let time = time_of_day(clock_text)
            .ok_or_else(|| format!("\"{}\" isn't a time. {HELP}", text.trim()))?;
        return Ok(SleepTimer::At(next_at(time, now)));
    }
    let d = length(&t).ok_or_else(|| format!("\"{}\" isn't a length. {HELP}", text.trim()))?;
    if d.is_zero() {
        return Err("That's no time at all".into());
    }
    if d > MAX {
        return Err("That's more than a day. Pick 24 hours or less".into());
    }
    Ok(SleepTimer::At(
        SystemTime::from(now.with_timezone(&chrono::Utc)) + d,
    ))
}

/// The next moment the clock reads `time`: today, or tomorrow if that's
/// already gone.
fn next_at<Tz: TimeZone>(time: NaiveTime, now: DateTime<Tz>) -> SystemTime {
    let today = now.date_naive().and_time(time);
    let mut at = now
        .timezone()
        .from_local_datetime(&today)
        .earliest()
        // A time skipped by a clock change: an hour on.
        .unwrap_or_else(|| now.clone() + chrono::Duration::hours(1));
    if at <= now {
        at += chrono::Duration::days(1);
    }
    SystemTime::from(at.with_timezone(&chrono::Utc))
}

/// "23:30", "7:05", "11:30pm", "11pm", "11 pm", "7" (with "at").
fn time_of_day(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    let (body, pm) = if let Some(b) = s.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else if let Some(b) = s.strip_suffix("am") {
        (b.trim(), Some(false))
    } else {
        (s, None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        Some(_) => return None,
        None => (body.parse::<u32>().ok()?, 0),
    };
    let h = match pm {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            match (h, pm) {
                (12, false) => 0,
                (12, true) => 12,
                (h, true) => h + 12,
                (h, false) => h,
            }
        }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0).filter(|t| t.hour() < 24)
}

/// "45", "45m", "45 min", "1h30", "1h 30m", "1.5h", "2 hours", "90s".
/// A bare number is minutes, and so is a bare number after hours.
fn length(s: &str) -> Option<Duration> {
    let mut total = 0f64;
    let mut rest = s.trim();
    let mut last_unit: Option<f64> = None;
    let mut any = false;
    while !rest.is_empty() {
        let n_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if n_end == 0 {
            return None;
        }
        let n: f64 = rest[..n_end].parse().ok()?;
        rest = rest[n_end..].trim_start();
        let u_end = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let unit = match &rest[..u_end] {
            "" => match last_unit {
                // "1h30": the bare number after hours is minutes.
                Some(u) if u >= 3600.0 => 60.0,
                Some(_) => return None,
                None => 60.0,
            },
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            _ => return None,
        };
        if last_unit.is_some_and(|l| unit >= l) {
            // "30m 1h" or "5m 5m": not a length.
            return None;
        }
        last_unit = Some(unit);
        total += n * unit;
        any = true;
        rest = rest[u_end..].trim_start();
    }
    if !any || !total.is_finite() {
        return None;
    }
    // Anything past a day is refused by the caller; capped here so a huge
    // number can't overflow on the way.
    Some(Duration::from_secs_f64(total.min(MAX.as_secs_f64() + 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn now() -> DateTime<FixedOffset> {
        // 22:15 on a Thursday, five hours behind UTC.
        FixedOffset::west_opt(5 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 12, 22, 15, 0)
            .unwrap()
    }

    fn mins(text: &str) -> i64 {
        let n = now();
        match parse_at(text, n).unwrap() {
            SleepTimer::At(t) => {
                let at: DateTime<chrono::Utc> = t.into();
                (at - n.with_timezone(&chrono::Utc)).num_minutes()
            }
            SleepTimer::EndOfSong => -1,
        }
    }

    #[test]
    fn lengths() {
        assert_eq!(length("45"), Some(Duration::from_secs(45 * 60)));
        assert_eq!(length("45m"), Some(Duration::from_secs(45 * 60)));
        assert_eq!(length("45 min"), Some(Duration::from_secs(45 * 60)));
        assert_eq!(length("1h30"), Some(Duration::from_secs(90 * 60)));
        assert_eq!(length("1h 30m"), Some(Duration::from_secs(90 * 60)));
        assert_eq!(length("1.5h"), Some(Duration::from_secs(90 * 60)));
        assert_eq!(length("2 hours"), Some(Duration::from_secs(2 * 3600)));
        assert_eq!(length("90s"), Some(Duration::from_secs(90)));
        assert_eq!(length("30m 1h"), None);
        assert_eq!(length("5m 5m"), None);
        assert_eq!(length("soon"), None);
        assert_eq!(length("1x"), None);
    }

    #[test]
    fn lengths_from_now() {
        assert!((44..=45).contains(&mins("45")));
        assert!((89..=90).contains(&mins("1h30")));
        assert_eq!(mins("end"), -1);
        assert_eq!(mins("End of song"), -1);
    }

    #[test]
    fn times_of_day() {
        // 22:15 now: 23:30 is today, 7:00 is tomorrow.
        assert_eq!(mins("23:30"), 75);
        assert_eq!(mins("11:30pm"), 75);
        assert_eq!(mins("11pm"), 45);
        assert_eq!(mins("11 pm"), 45);
        assert_eq!(mins("at 7"), 8 * 60 + 45);
        assert_eq!(mins("7:00am"), 8 * 60 + 45);
        assert_eq!(mins("12am"), 105);
        // A time a minute ago is tomorrow, not now.
        assert_eq!(mins("22:14"), 24 * 60 - 1);
    }

    #[test]
    fn refusals() {
        for bad in [
            "", "25:00", "13pm", "7:5", "0", "0m", "25h", "tonight", "at noon",
        ] {
            assert!(parse_at(bad, now()).is_err(), "{bad}");
        }
    }

    #[test]
    fn fade_comes_down_over_the_last_stretch() {
        assert_eq!(fade_gain(Duration::from_secs(60)), 1.0);
        assert_eq!(fade_gain(FADE), 1.0);
        let half = fade_gain(FADE / 2);
        assert!((half - 0.1).abs() < 1e-9, "-20 dB halfway, got {half}");
        let end = fade_gain(Duration::ZERO);
        assert!((end - 0.01).abs() < 1e-9, "-40 dB at the end, got {end}");
        // Always going down.
        let mut last = 1.0;
        for ms in (0..30_000).rev().step_by(500) {
            let g = fade_gain(Duration::from_millis(ms));
            assert!(g <= last);
            last = g;
        }
    }

    #[test]
    fn labels() {
        assert_eq!(clock(Duration::from_secs(41 * 60 + 23)), "41:23");
        assert_eq!(clock(Duration::from_millis(3_723_400)), "1:02:04");
        assert_eq!(clock(Duration::from_millis(400)), "0:01");
        assert_eq!(words(Duration::from_secs(45 * 60)), "45 min");
        assert_eq!(words(Duration::from_secs(90 * 60)), "1 h 30 min");
        assert_eq!(words(Duration::from_secs(20)), "1 min");
        assert_eq!(words(Duration::ZERO), "under a minute");
        assert_eq!(SleepTimer::EndOfSong.short(), "end of song");
    }
}
