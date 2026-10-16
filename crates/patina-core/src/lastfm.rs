//! Last.fm: who and what is similar (for mixes), and scrobbling.
//!
//! Reading similarity needs only an API key. Scrobbling, Now Playing and
//! love need the key's secret too (calls are signed with it) and a session
//! key, which you get by approving the app on Last.fm's site once.
//!
//! A key and secret can be built in (`PATINA_LASTFM_KEY` and
//! `PATINA_LASTFM_SECRET` at build time, which the release workflow takes
//! from the repository's secrets), and your own in config.toml win over
//! them. Anyone can pull a built-in secret out of the binary; the most it
//! lets them do is make calls as the app, never as you.

use std::time::Duration;

use md5::{Digest, Md5};
use serde::Deserialize;

pub const API: &str = "https://ws.audioscrobbler.com/2.0/";
/// Where you approve the app.
pub const AUTH_PAGE: &str = "https://www.last.fm/api/auth/";

/// The key and secret built into this binary, if any.
pub fn builtin() -> (Option<&'static str>, Option<&'static str>) {
    (
        option_env!("PATINA_LASTFM_KEY").filter(|k| !k.trim().is_empty()),
        option_env!("PATINA_LASTFM_SECRET").filter(|k| !k.trim().is_empty()),
    )
}

/// Your key if you set one, else the built-in one.
pub fn effective_key(yours: &str) -> String {
    let y = yours.trim();
    if !y.is_empty() {
        return y.to_string();
    }
    builtin().0.unwrap_or_default().to_string()
}

/// The secret that goes with the effective key: yours with your key,
/// the built-in one with the built-in key.
pub fn effective_secret(your_key: &str, your_secret: &str) -> String {
    if !your_key.trim().is_empty() {
        return your_secret.trim().to_string();
    }
    builtin().1.unwrap_or_default().to_string()
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum LastFmError {
    #[error("can't reach Last.fm: {0}")]
    Http(String),
    #[error("Last.fm said: {msg} (error {code})")]
    Api { code: i64, msg: String },
    #[error("unexpected answer from Last.fm: {0}")]
    Parse(String),
    #[error("{0}")]
    Setup(&'static str),
}

impl LastFmError {
    /// Worth trying again later: the network, or Last.fm busy or down.
    pub fn transient(&self) -> bool {
        match self {
            LastFmError::Http(_) | LastFmError::Parse(_) => true,
            // 8 operation failed, 11 offline, 16 temporarily unavailable,
            // 29 rate limit.
            LastFmError::Api { code, .. } => matches!(code, 8 | 11 | 16 | 29),
            LastFmError::Setup(_) => false,
        }
    }

    /// The session is no good any more: link again.
    pub fn bad_session(&self) -> bool {
        matches!(
            self,
            LastFmError::Api {
                code: 4 | 9 | 26,
                ..
            }
        )
    }
}

/// One play, as Last.fm takes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Play {
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    /// Seconds.
    pub duration: Option<u32>,
    /// When it started, Unix seconds. Only for scrobbles.
    pub timestamp: i64,
}

impl Play {
    fn params(&self, i: Option<usize>, out: &mut Vec<(String, String)>) {
        let k = |name: &str| match i {
            Some(i) => format!("{name}[{i}]"),
            None => name.to_string(),
        };
        out.push((k("artist"), self.artist.clone()));
        out.push((k("track"), self.track.clone()));
        if let Some(a) = &self.album {
            out.push((k("album"), a.clone()));
        }
        if let Some(a) = &self.album_artist {
            out.push((k("albumArtist"), a.clone()));
        }
        if let Some(n) = self.track_no {
            out.push((k("trackNumber"), n.to_string()));
        }
        if let Some(d) = self.duration {
            out.push((k("duration"), d.to_string()));
        }
        if i.is_some() {
            out.push((k("timestamp"), self.timestamp.to_string()));
        }
    }
}

/// What came of a batch of scrobbles.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sent {
    pub accepted: u32,
    pub ignored: u32,
}

pub struct LastFm {
    base: String,
    key: String,
    secret: String,
    session: String,
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
            secret: String::new(),
            session: String::new(),
            http: crate::net::client_builder()
                .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(15))
                .build()
                .ok()?,
        })
    }

    pub fn with_secret(mut self, secret: &str) -> Self {
        self.secret = secret.trim().to_string();
        self
    }

    pub fn with_session(mut self, session: &str) -> Self {
        self.session = session.trim().to_string();
        self
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn can_sign(&self) -> bool {
        !self.secret.is_empty()
    }

    /// The api_sig: every parameter but `format` and `callback`, sorted by
    /// name, names and values run together, the secret on the end, MD5.
    pub fn sign(&self, params: &[(String, String)]) -> String {
        let mut p: Vec<&(String, String)> = params
            .iter()
            .filter(|(k, _)| k != "format" && k != "callback")
            .collect();
        p.sort_by(|a, b| a.0.cmp(&b.0));
        let mut s = String::new();
        for (k, v) in p {
            s.push_str(k);
            s.push_str(v);
        }
        s.push_str(&self.secret);
        hex::encode(Md5::digest(s.as_bytes()))
    }

    fn parse(body: &[u8]) -> Result<serde_json::Value, LastFmError> {
        let v: serde_json::Value =
            serde_json::from_slice(body).map_err(|e| LastFmError::Parse(e.to_string()))?;
        if let Some(code) = v.get("error").and_then(|c| c.as_i64()) {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("error")
                .to_string();
            return Err(LastFmError::Api { code, msg });
        }
        Ok(v)
    }

    /// An unsigned read.
    fn get(&self, method: &str, args: &[(&str, &str)]) -> Result<serde_json::Value, LastFmError> {
        let limit = args.iter().find(|a| a.0 == "limit").map(|a| a.1);
        let mut q: Vec<(&str, &str)> = vec![
            ("method", method),
            ("api_key", &self.key),
            ("format", "json"),
            ("autocorrect", "1"),
        ];
        q.extend(args.iter().filter(|a| a.0 != "limit").copied());
        if limit.is_some() {
            q.push(("limit", limit.unwrap_or("50")));
        }
        let body = self
            .http
            .get(&self.base)
            .query(&q)
            .send()
            .and_then(|r| r.bytes())
            .map_err(|e| LastFmError::Http(e.to_string()))?;
        Self::parse(&body)
    }

    /// A signed call: GET for the auth reads, POST for anything that
    /// writes.
    fn signed(
        &self,
        method: &str,
        args: Vec<(String, String)>,
        post: bool,
        with_session: bool,
    ) -> Result<serde_json::Value, LastFmError> {
        if self.secret.is_empty() {
            return Err(LastFmError::Setup(
                "scrobbling needs the API key's secret too (Settings, Last.fm)",
            ));
        }
        let mut p = args;
        p.push(("method".into(), method.into()));
        p.push(("api_key".into(), self.key.clone()));
        if with_session {
            if self.session.is_empty() {
                return Err(LastFmError::Setup("not linked to Last.fm"));
            }
            p.push(("sk".into(), self.session.clone()));
        }
        let sig = self.sign(&p);
        p.push(("api_sig".into(), sig));
        p.push(("format".into(), "json".into()));
        let req = if post {
            self.http.post(&self.base).form(&p)
        } else {
            self.http.get(&self.base).query(&p)
        };
        let body = req
            .send()
            .and_then(|r| r.bytes())
            .map_err(|e| LastFmError::Http(e.to_string()))?;
        Self::parse(&body)
    }

    // ----- Linking -----

    /// A token to approve.
    pub fn get_token(&self) -> Result<String, LastFmError> {
        let v = self.signed("auth.getToken", Vec::new(), false, false)?;
        v.get("token")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .ok_or_else(|| LastFmError::Parse("no token".into()))
    }

    /// The page where you approve `token`.
    pub fn auth_url(&self, token: &str) -> String {
        format!("{AUTH_PAGE}?api_key={}&token={token}", self.key)
    }

    /// The session once you've approved: (user name, session key). None
    /// while it's still waiting for you.
    pub fn get_session(&self, token: &str) -> Result<Option<(String, String)>, LastFmError> {
        match self.signed(
            "auth.getSession",
            vec![("token".into(), token.into())],
            false,
            false,
        ) {
            Ok(v) => {
                let s = v
                    .get("session")
                    .ok_or_else(|| LastFmError::Parse("no session".into()))?;
                let name = s.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                let key = s.get("key").and_then(|n| n.as_str()).unwrap_or_default();
                if key.is_empty() {
                    return Err(LastFmError::Parse("empty session key".into()));
                }
                Ok(Some((name.to_string(), key.to_string())))
            }
            // 14: not approved yet.
            Err(LastFmError::Api { code: 14, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // ----- Scrobbling -----

    pub fn now_playing(&self, p: &Play) -> Result<(), LastFmError> {
        let mut args = Vec::new();
        p.params(None, &mut args);
        self.signed("track.updateNowPlaying", args, true, true)
            .map(|_| ())
    }

    /// Up to 50 at a time, which is Last.fm's limit.
    pub fn scrobble(&self, plays: &[Play]) -> Result<Sent, LastFmError> {
        let mut args = Vec::new();
        for (i, p) in plays.iter().take(50).enumerate() {
            p.params(Some(i), &mut args);
        }
        let v = self.signed("track.scrobble", args, true, true)?;
        let attr = v
            .get("scrobbles")
            .and_then(|s| s.get("@attr"))
            .cloned()
            .unwrap_or_default();
        let n = |k: &str| {
            attr.get(k)
                .and_then(|x| {
                    x.as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0) as u32
        };
        Ok(Sent {
            accepted: n("accepted"),
            ignored: n("ignored"),
        })
    }

    pub fn love(&self, artist: &str, track: &str, on: bool) -> Result<(), LastFmError> {
        self.signed(
            if on { "track.love" } else { "track.unlove" },
            vec![
                ("artist".into(), artist.into()),
                ("track".into(), track.into()),
            ],
            true,
            true,
        )
        .map(|_| ())
    }

    /// Whether `user` has loved this track.
    pub fn loved(&self, artist: &str, track: &str, user: &str) -> Result<bool, LastFmError> {
        let v = self.get(
            "track.getInfo",
            &[("artist", artist), ("track", track), ("username", user)],
        )?;
        let l = v.get("track").and_then(|t| t.get("userloved"));
        Ok(match l {
            Some(serde_json::Value::String(s)) => s == "1",
            Some(serde_json::Value::Number(n)) => n.as_i64() == Some(1),
            _ => false,
        })
    }

    // ----- Similar -----

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

/// Split a radio stream title into (artist, title): "Artist - Title" is
/// how nearly every station sends it. None for anything else (station
/// IDs, ads, a bare title), since a guess would scrobble junk.
pub fn split_stream_title(t: &str) -> Option<(String, String)> {
    let t = t.trim();
    let (a, b) = t.split_once(" - ")?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() || a.len() > 200 || b.len() > 200 {
        return None;
    }
    Some((a.to_string(), b.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_no_client() {
        assert!(LastFm::new("  ").is_none());
        assert!(LastFm::new("abc").is_some());
    }

    #[test]
    fn signature_follows_the_documented_recipe() {
        // md5("api_keyxxxmethodauth.getSessiontokenyyyilovecher")
        let lf = LastFm::new("xxx").unwrap().with_secret("ilovecher");
        let p = vec![
            ("token".to_string(), "yyy".to_string()),
            ("method".to_string(), "auth.getSession".to_string()),
            ("api_key".to_string(), "xxx".to_string()),
            ("format".to_string(), "json".to_string()),
        ];
        let want = hex::encode(Md5::digest(
            b"api_keyxxxmethodauth.getSessiontokenyyyilovecher",
        ));
        assert_eq!(lf.sign(&p), want);
    }

    #[test]
    fn stream_titles() {
        assert_eq!(
            split_stream_title("Nirvana - Blew"),
            Some(("Nirvana".into(), "Blew".into()))
        );
        assert_eq!(
            split_stream_title("Sonic Youth - Teen Age Riot - Live"),
            Some(("Sonic Youth".into(), "Teen Age Riot - Live".into()))
        );
        assert_eq!(split_stream_title("Radio Paradise"), None);
        assert_eq!(split_stream_title(" - Blew"), None);
    }

    #[test]
    fn errors_sort_into_retry_and_relink() {
        let e = |code| LastFmError::Api {
            code,
            msg: String::new(),
        };
        assert!(e(11).transient() && e(16).transient() && e(29).transient());
        assert!(!e(6).transient());
        assert!(e(9).bad_session());
        assert!(LastFmError::Http("x".into()).transient());
    }
}
