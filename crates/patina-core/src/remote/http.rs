//! HTTP and JSON bits both server kinds share.

use std::time::Duration;

use serde::{Deserialize, Deserializer};

use super::{RemoteError, Result};

pub fn client() -> Result<reqwest::blocking::Client> {
    crate::net::client_builder()
        .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        // Big libraries make for slow first pages on small servers.
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| RemoteError::Http(e.to_string()))
}

/// The error text without the URL reqwest likes to include, since ours
/// carry credentials.
pub fn http_err(e: reqwest::Error) -> RemoteError {
    let e = e.without_url();
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(s) = src {
        msg = format!("{msg}: {s}");
        src = s.source();
    }
    RemoteError::Http(msg)
}

pub fn get_bytes(
    c: &reqwest::blocking::Client,
    url: &str,
    query: &[(&str, String)],
) -> Result<(Vec<u8>, String)> {
    let resp = c.get(url).query(query).send().map_err(http_err)?;
    let status = resp.status();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.bytes().map_err(http_err)?.to_vec();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // Some servers answer bad logins with a 401 and a JSON body; let
        // the caller read the body if there is one.
        if !ctype.contains("json") {
            return Err(RemoteError::Auth);
        }
    } else if !status.is_success() && !ctype.contains("json") {
        return Err(RemoteError::Http(format!("HTTP {status}")));
    }
    Ok((body, ctype))
}

/// Ids come as strings from most servers and as numbers from some.
pub fn id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        S(String),
        I(i64),
        F(f64),
    }
    Ok(match Id::deserialize(d)? {
        Id::S(s) => s,
        Id::I(i) => i.to_string(),
        Id::F(f) => format!("{f}"),
    })
}

pub fn opt_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct W(#[serde(deserialize_with = "id")] String);
    Ok(Option::<W>::deserialize(d)?
        .map(|w| w.0)
        .filter(|s| !s.is_empty()))
}

/// A number that may come as a string ("320"), a number, or null.
pub fn num<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        F(f64),
        S(String),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<N>::deserialize(d)? {
        Some(N::F(f)) => Some(f),
        Some(N::S(s)) => s.trim().parse().ok(),
        Some(N::Other(_)) | None => None,
    })
}

/// A list the server may send as a single object when there's one item,
/// or leave out when there are none.
pub fn list<'de, D, T>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum L<T> {
        Many(Vec<T>),
        One(T),
    }
    Ok(match Option::<L<T>>::deserialize(d)? {
        Some(L::Many(v)) => v,
        Some(L::One(t)) => vec![t],
        None => Vec::new(),
    })
}

/// ISO 8601 or RFC 3339 dates, or Unix seconds, to Unix seconds.
pub fn date_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
                .map(|d| d.and_utc().timestamp())
                .ok()
        })
}

/// A short random string for Subsonic salts.
pub fn salt() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}", h.finish())
}

/// "flac" and friends, for the lossless flag on server songs.
pub fn lossless_suffix(suffix: &str) -> bool {
    matches!(
        suffix.to_ascii_lowercase().as_str(),
        "flac" | "alac" | "wav" | "aif" | "aiff" | "ape" | "wv" | "dsf" | "dff" | "tta"
    )
}

/// The codec name the badge shows, from a file extension.
pub fn codec_name(suffix: &str) -> String {
    match suffix.to_ascii_lowercase().as_str() {
        "flac" => "FLAC".into(),
        "mp3" => "MP3".into(),
        "m4a" | "aac" | "mp4" => "AAC".into(),
        "alac" => "ALAC".into(),
        "ogg" | "oga" => "Vorbis".into(),
        "opus" => "Opus".into(),
        "wav" => "WAV".into(),
        "aif" | "aiff" => "AIFF".into(),
        "ape" => "APE".into(),
        "wv" => "WavPack".into(),
        "dsf" | "dff" => "DSD".into(),
        "" => String::new(),
        other => other.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_secs("1700000000"), Some(1_700_000_000));
        assert_eq!(date_secs("2023-11-14T22:13:20Z"), Some(1_700_000_000));
        assert_eq!(date_secs("2023-11-14T22:13:20.000Z"), Some(1_700_000_000));
        assert_eq!(date_secs("2023-11-14 22:13:20"), Some(1_700_000_000));
        assert_eq!(date_secs("soon"), None);
    }

    #[test]
    fn ids_and_lists() {
        #[derive(Deserialize)]
        struct T {
            #[serde(deserialize_with = "id")]
            a: String,
            #[serde(default, deserialize_with = "list")]
            b: Vec<u32>,
            #[serde(default, deserialize_with = "num")]
            c: Option<f64>,
        }
        let t: T = serde_json::from_str(r#"{"a": 12, "b": 3, "c": "4.5"}"#).unwrap();
        assert_eq!((t.a.as_str(), t.b, t.c), ("12", vec![3], Some(4.5)));
        let t: T = serde_json::from_str(r#"{"a": "x"}"#).unwrap();
        assert!(t.b.is_empty() && t.c.is_none());
    }
}
