//! Importing AutoEq and Equalizer APO files: the ParametricEQ.txt and
//! GraphicEQ.txt AutoEq publishes for thousands of headphones, and APO
//! config files written the same way.
//!
//! Parametric filters come in as parametric bands, exactly. A GraphicEQ
//! curve (hundreds of points) is fitted to the 31-band graphic EQ, which
//! follows it closely but not exactly. Anything APO does that this EQ
//! doesn't (per-channel filters, includes, expressions) is skipped and
//! named in the warnings.

use super::{EqSettings, GRAPHIC_RANGE, MAX_PARAMETRIC, Mode, clamp_band, fit_graphic};
use crate::dsp::biquad::{Band, Kind};

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub settings: EqSettings,
    /// What was left out or changed, one line each.
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ImportError {
    #[error("no EQ filters in that file (looked for Filter: and GraphicEQ: lines)")]
    Nothing,
    #[error("line {line}: {msg}")]
    Bad { line: usize, msg: String },
}

/// A number with an optional unit: "105", "-2.3", "6.7dB".
fn num(s: &str) -> Option<f64> {
    let t = s
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .replace(',', ".");
    t.parse().ok().filter(|v: &f64| v.is_finite())
}

/// Q for a bandwidth in octaves.
fn q_from_bw(oct: f64) -> f64 {
    let n = 2f64.powf(oct.max(0.01));
    n.sqrt() / (n - 1.0)
}

/// One APO filter, from the words after "Filter n:". Ok(None) for a filter
/// that's switched off.
fn filter(words: &[&str], line: usize) -> Result<Option<Band>, String> {
    let mut it = words.iter().map(|w| w.trim()).filter(|w| !w.is_empty());
    let state = it.next().ok_or("empty filter")?;
    if state.eq_ignore_ascii_case("OFF") {
        return Ok(None);
    }
    if !state.eq_ignore_ascii_case("ON") {
        return Err(format!("expected ON or OFF, got \"{state}\""));
    }
    let ty = it.next().ok_or("no filter type")?.to_ascii_uppercase();
    let rest: Vec<&str> = it.collect();
    let kind = match ty.as_str() {
        "PK" | "PEQ" | "MODAL" => Kind::Peak,
        "LS" | "LSC" | "LSQ" => Kind::LowShelf,
        "HS" | "HSC" | "HSQ" => Kind::HighShelf,
        "LP" | "LPQ" => Kind::LowPass,
        "HP" | "HPQ" => Kind::HighPass,
        "NO" => Kind::Notch,
        other => return Err(format!("filter type {other} isn't supported (line {line})")),
    };
    let mut b = Band {
        kind,
        freq: 0.0,
        gain: 0.0,
        q: std::f64::consts::FRAC_1_SQRT_2,
    };
    if kind == Kind::Notch {
        // APO's default notch is narrow.
        b.q = 30.0;
    }
    let mut i = 0;
    let mut have_freq = false;
    while i < rest.len() {
        let w = rest[i].to_ascii_lowercase();
        let next = rest.get(i + 1).copied().unwrap_or("");
        match w.as_str() {
            "fc" => {
                b.freq = num(next).ok_or_else(|| format!("bad frequency \"{next}\""))?;
                have_freq = true;
                i += 2;
            }
            "gain" => {
                b.gain = num(next).ok_or_else(|| format!("bad gain \"{next}\""))?;
                i += 2;
            }
            "q" => {
                b.q = num(next).ok_or_else(|| format!("bad Q \"{next}\""))?;
                i += 2;
            }
            "bw" => {
                // "BW Oct 1.0"
                let v = rest.get(i + 2).copied().unwrap_or(next);
                let oct = num(v).ok_or_else(|| format!("bad bandwidth \"{v}\""))?;
                b.q = q_from_bw(oct);
                i += if num(next).is_some() { 2 } else { 3 };
            }
            // Units and slope words ("Hz", "dB", "12dB") carry nothing more.
            _ => i += 1,
        }
    }
    if !have_freq {
        return Err("no Fc".into());
    }
    Ok(Some(b))
}

/// Read an AutoEq or Equalizer APO file.
pub fn parse(text: &str) -> Result<Imported, ImportError> {
    let mut bands = Vec::new();
    let mut preamp: Option<f64> = None;
    let mut graphic: Option<Vec<(f64, f64)>> = None;
    let mut warnings = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((key, val)) = l.split_once(':') else {
            continue;
        };
        let k = key.trim().to_ascii_lowercase();
        if k == "preamp" {
            let v =
                val.split_whitespace()
                    .next()
                    .and_then(num)
                    .ok_or_else(|| ImportError::Bad {
                        line,
                        msg: format!("bad preamp \"{}\"", val.trim()),
                    })?;
            preamp = Some(preamp.unwrap_or(0.0) + v);
        } else if k == "filter" || k.starts_with("filter ") {
            let words: Vec<&str> = val.split_whitespace().collect();
            match filter(&words, line) {
                Ok(Some(b)) => bands.push(b),
                Ok(None) => {}
                Err(msg) if msg.contains("isn't supported") => warnings.push(msg),
                Err(msg) => return Err(ImportError::Bad { line, msg }),
            }
        } else if k == "graphiceq" {
            let mut pts = Vec::new();
            for pair in val.split(';') {
                let mut w = pair.split_whitespace();
                let (Some(f), Some(g)) = (w.next().and_then(num), w.next().and_then(num)) else {
                    continue;
                };
                if f > 0.0 {
                    pts.push((f, g));
                }
            }
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            if pts.is_empty() {
                return Err(ImportError::Bad {
                    line,
                    msg: "GraphicEQ has no points".into(),
                });
            }
            graphic = Some(pts);
        } else if matches!(
            k.as_str(),
            "channel" | "include" | "device" | "stage" | "copy" | "eval" | "convolution" | "delay"
        ) {
            warnings.push(format!(
                "line {line}: {} isn't supported here, skipped",
                key.trim()
            ));
        }
    }

    if let Some(pts) = graphic {
        if !bands.is_empty() {
            warnings.push("the file has both filters and a GraphicEQ; took the GraphicEQ".into());
        }
        let (gains, clipped) = fit_graphic(&pts, 31);
        if clipped {
            warnings.push(format!(
                "the curve goes past ±{GRAPHIC_RANGE} dB; the graphic EQ stops there"
            ));
        }
        let settings = EqSettings {
            on: true,
            mode: Mode::Graphic,
            graphic_bands: 31,
            graphic: gains,
            parametric: Vec::new(),
            preamp: preamp.unwrap_or(0.0),
            // A GraphicEQ file carries no preamp of its own.
            auto_preamp: preamp.is_none(),
            preset: String::new(),
        };
        return Ok(Imported { settings, warnings });
    }

    if bands.is_empty() {
        return Err(ImportError::Nothing);
    }
    if bands.len() > MAX_PARAMETRIC {
        warnings.push(format!(
            "{} filters; the parametric EQ takes {MAX_PARAMETRIC}, so the rest were left out",
            bands.len()
        ));
        bands.truncate(MAX_PARAMETRIC);
    }
    let clamped: Vec<Band> = bands.iter().map(|b| clamp_band(*b)).collect();
    if clamped != bands {
        warnings.push("some values were past what the EQ allows and were brought in".into());
    }
    let settings = EqSettings {
        on: true,
        mode: Mode::Parametric,
        parametric: clamped,
        preamp: preamp.unwrap_or(0.0),
        // AutoEq's preamp is worked out for its filters; keep it as it is
        // rather than adding ours. With none given, ours takes over.
        auto_preamp: preamp.is_none(),
        preset: String::new(),
        ..Default::default()
    }
    .normalized();
    Ok(Imported { settings, warnings })
}

/// A preset name from an import's file name: "Sennheiser HD 650
/// ParametricEQ.txt" is "Sennheiser HD 650".
pub fn name_from_path(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".into());
    let mut s = stem.as_str();
    for suffix in [
        " ParametricEQ",
        " GraphicEQ",
        " FixedBandEQ",
        "_ParametricEQ",
        "_GraphicEQ",
    ] {
        if let Some(t) = s.strip_suffix(suffix) {
            s = t;
        }
    }
    let s = s.trim();
    if s.is_empty()
        || s.eq_ignore_ascii_case("ParametricEQ")
        || s.eq_ignore_ascii_case("GraphicEQ")
        || s.eq_ignore_ascii_case("config")
    {
        "Imported".into()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTOEQ: &str = "Preamp: -6.4 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.7 dB Q 0.70
Filter 2: ON PK Fc 213 Hz Gain -2.3 dB Q 0.85
Filter 3: ON PK Fc 2960 Hz Gain 3.4 dB Q 2.35
Filter 4: OFF PK Fc 5000 Hz Gain 9 dB Q 1
Filter 5: ON HSC Fc 10000 Hz Gain -2.2 dB Q 0.70
";

    #[test]
    fn autoeq_parametric() {
        let r = parse(AUTOEQ).unwrap();
        let s = r.settings;
        assert_eq!(s.mode, Mode::Parametric);
        assert_eq!(s.preamp, -6.4);
        assert!(!s.auto_preamp);
        assert_eq!(s.parametric.len(), 4);
        assert_eq!(
            s.parametric[0],
            Band {
                kind: Kind::LowShelf,
                freq: 105.0,
                gain: 6.7,
                q: 0.70
            }
        );
        assert_eq!(s.parametric[3].kind, Kind::HighShelf);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn apo_variants() {
        let t = "# my config
Device: all
Channel: L
Preamp: -3 dB
Filter: ON PK Fc 1000 Hz Gain 3 dB BW Oct 1.0
Filter 2: ON HP Fc 30 Hz
Filter 3: ON NO Fc 50 Hz
Filter 4: ON BP Fc 50 Hz
";
        let r = parse(t).unwrap();
        let p = &r.settings.parametric;
        assert_eq!(p.len(), 3);
        assert!((p[0].q - std::f64::consts::SQRT_2).abs() < 0.001);
        assert_eq!(p[1].kind, Kind::HighPass);
        assert_eq!(p[2].q, 30.0);
        assert_eq!(r.warnings.len(), 3, "{:?}", r.warnings);
    }

    #[test]
    fn graphic_eq_is_fitted_to_31_bands() {
        let t = "GraphicEQ: 20 6; 100 6; 1000 0; 3000 -4; 20000 -4";
        let r = parse(t).unwrap();
        let s = r.settings;
        assert_eq!(s.graphic_bands, 31);
        assert!(s.auto_preamp);
        assert!((s.response_at(50.0) - 6.0).abs() < 1.0);
        assert!((s.response_at(8000.0) + 4.0).abs() < 1.0);
    }

    #[test]
    fn errors() {
        assert_eq!(parse("hello"), Err(ImportError::Nothing));
        assert!(matches!(
            parse("Filter 1: ON PK Gain 3 dB"),
            Err(ImportError::Bad { line: 1, .. })
        ));
    }

    #[test]
    fn names() {
        let n = |s: &str| name_from_path(std::path::Path::new(s));
        assert_eq!(
            n("/x/Sennheiser HD 650 ParametricEQ.txt"),
            "Sennheiser HD 650"
        );
        assert_eq!(n("/x/ParametricEQ.txt"), "Imported");
        assert_eq!(n("/x/my eq.txt"), "my eq");
    }
}
