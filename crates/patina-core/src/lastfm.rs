//! Last.fm, for now only to ask who and what is similar: the fallback for
//! mixes when a server has no similarity data, and the only source for
//! local folders besides your own tags. Needs an API key (free, from
//! last.fm/api/account/create). Scrobbling joins this in milestone 11.

use std::time::Duration;

use serde::Deserialize;

pub const API: &str = "https://ws.audioscrobbler.com/2.0/";

#[derive(Debug, thiserror::Error)]
pub enum LastFmError {
    #[error("can't reach Last.fm: {0}")]
    Http(String),
    #[error("Last.fm said: {0}")]
    Api(String),
}

pub struct LastFm {
    base: String,
    key: String,
    http: reqwest::blocking::Client,
}

impl LastFm {
    /// None without a key: there's nothing to ask with.
    pub fn new(key: &str) -> Option<Self> {
        Self::with_base(API, key)
    }

    /// For tests: a stand-in server at `base`.
    pub fn with_base(base: &str, key: &str) -> Option<Self> {
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        Some(Self {
            base: base.to_string(),
            key: key.to_string(),
            http: reqwest::blocking::Client::builder()
                .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(10))
                .build()
                .ok()?,
        })
    }

    fn get(&self, method: &str, args: &[(&str, &str)]) -> Result<serde_json::Value, LastFmError> {
        let limit = args.iter().find(|a| a.0 == "limit").map(|a| a.1);
        let mut q: Vec<(&str, &str)> = vec![
            ("method", method),
            ("api_key", &self.key),
            ("format", "json"),
            ("autocorrect", "1"),
        ];
        q.extend(args.iter().filter(|a| a.0 != "limit").copied());
        q.push(("limit", limit.unwrap_or("50")));
        let body = self
            .http
            .get(&self.base)
            .query(&q)
            .send()
            .and_then(|r| r.bytes())
            .map_err(|e| LastFmError::Http(e.to_string()))?;
        let v: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| LastFmError::Api(format!("unexpected answer: {e}")))?;
        if let Some(code) = v.get("error") {
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("error");
            return Err(LastFmError::Api(format!("{msg} (error {code})")));
        }
        Ok(v)
    }

    /// Artists similar to `artist`, most similar first.
    pub fn similar_artists(&self, artist: &str, limit: usize) -> Result<Vec<String>, LastFmError> {
        #[derive(Deserialize, Default)]
        struct A {
            #[serde(default)]
            name: String,
        }
        #[derive(Deserialize, Default)]
        struct Wrap {
            #[serde(default)]
            artist: OneOrMany<A>,
        }
        let v = self.get(
            "artist.getsimilar",
            &[("artist", artist), ("limit", &limit.to_string())],
        )?;
        let w: Wrap = serde_json::from_value(v.get("similarartists").cloned().unwrap_or_default())
            .unwrap_or_default();
        Ok(w.artist
            .0
            .into_iter()
            .map(|a| a.name)
            .filter(|n| !n.is_empty())
            .collect())
    }

    /// Songs similar to this one, as (artist, title), most similar first.
    pub fn similar_tracks(
        &self,
        artist: &str,
        title: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>, LastFmError> {
        #[derive(Deserialize, Default)]
        struct Who {
            #[serde(default)]
            name: String,
        }
        #[derive(Deserialize, Default)]
        struct T {
            #[serde(default)]
            name: String,
            #[serde(default)]
            artist: Who,
        }
        #[derive(Deserialize, Default)]
        struct Wrap {
            #[serde(default)]
            track: OneOrMany<T>,
        }
        let v = self.get(
            "track.getsimilar",
            &[
                ("artist", artist),
                ("track", title),
                ("limit", &limit.to_string()),
            ],
        )?;
        let w: Wrap = serde_json::from_value(v.get("similartracks").cloned().unwrap_or_default())
            .unwrap_or_default();
        Ok(w.track
            .0
            .into_iter()
            .filter(|t| !t.name.is_empty() && !t.artist.name.is_empty())
            .map(|t| (t.artist.name, t.name))
            .collect())
    }
}

/// Last.fm sends a lone object where a list has one item.
#[derive(Debug, Default)]
struct OneOrMany<T>(Vec<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum L<T> {
            Many(Vec<T>),
            One(T),
        }
        Ok(OneOrMany(match Option::<L<T>>::deserialize(d)? {
            Some(L::Many(v)) => v,
            Some(L::One(t)) => vec![t],
            None => Vec::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_no_client() {
        assert!(LastFm::new("  ").is_none());
        assert!(LastFm::new("abc").is_some());
    }
}
