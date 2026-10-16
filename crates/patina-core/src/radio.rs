//! Internet radio: turning whatever URL a station gives you into streams
//! FFmpeg can play.
//!
//! Stations hand out three kinds of links. A direct stream (Icecast,
//! SHOUTcast, plain HTTP audio) plays as is. A playlist (.pls, .m3u, a
//! non-HLS .m3u8) is a list of mirrors to try in order. An HLS playlist
//! (.m3u8 with segments) goes straight to FFmpeg, which handles it itself.

use std::time::Duration;

use crate::audio::decoder::{OpenOptions, Source};

#[derive(Debug, thiserror::Error)]
pub enum RadioError {
    #[error("{0}")]
    Http(String),
    #[error("the playlist has no stream URLs in it")]
    EmptyPlaylist,
    #[error("{0}")]
    Url(#[from] url::ParseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Direct,
    Playlist,
    Hls,
}

/// What a station URL turned out to be, and the streams to try in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub kind: Kind,
    pub streams: Vec<String>,
}

impl Resolved {
    /// "Playlist, 3 mirrors" and the like, for the station editor.
    pub fn summary(&self) -> String {
        match self.kind {
            Kind::Direct => "Direct stream".into(),
            Kind::Hls => "HLS stream".into(),
            Kind::Playlist => {
                let n = self.streams.len();
                format!("Playlist, {n} mirror{}", if n == 1 { "" } else { "s" })
            }
        }
    }
}

/// Largest playlist body we'll read. Anything bigger is a stream.
const MAX_PLAYLIST: usize = 64 * 1024;
const MAX_DEPTH: usize = 3;

fn client() -> Result<reqwest::blocking::Client, RadioError> {
    crate::net::client_builder()
        .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| RadioError::Http(e.to_string()))
}

/// Resolve a station URL. Network round trips happen here, so call it off
/// the UI thread.
pub fn resolve(url: &str) -> Result<Resolved, RadioError> {
    let c = client()?;
    resolve_depth(&c, url, 0)
}

fn resolve_depth(
    c: &reqwest::blocking::Client,
    url: &str,
    depth: usize,
) -> Result<Resolved, RadioError> {
    let direct = || Resolved {
        kind: Kind::Direct,
        streams: vec![url.to_string()],
    };
    // Only http(s) can be looked at; anything else is FFmpeg's problem.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Ok(direct());
    }
    let resp = match c.get(url).send() {
        Ok(r) => r,
        // SHOUTcast v1 answers "ICY 200 OK", which isn't HTTP. If the
        // connection worked but the reply didn't parse, it's a stream.
        Err(e) if !e.is_connect() && !e.is_timeout() => return Ok(direct()),
        Err(e) => return Err(RadioError::Http(e.to_string())),
    };
    if !resp.status().is_success() {
        return Err(RadioError::Http(format!("{url}: HTTP {}", resp.status())));
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let final_url = resp.url().to_string();
    let path = resp.url().path().to_ascii_lowercase();

    let by_type = playlist_kind_from_type(&ctype);
    let by_ext = playlist_kind_from_ext(&path);
    let looks_audio =
        ctype.starts_with("audio/") && by_type.is_none() || ctype == "application/ogg";
    if looks_audio && by_ext.is_none() {
        return Ok(direct());
    }

    // Read a bounded prefix. A stream mislabelled as text would otherwise
    // never end.
    let body = read_prefix(resp, MAX_PLAYLIST)?;
    let text = String::from_utf8_lossy(&body);
    let kind = sniff(&text).or(by_type).or(by_ext);
    let Some(kind) = kind else {
        return Ok(direct());
    };
    let entries = match kind {
        PlaylistKind::Pls => parse_pls(&text),
        PlaylistKind::M3u => {
            if is_hls(&text) {
                return Ok(Resolved {
                    kind: Kind::Hls,
                    streams: vec![final_url],
                });
            }
            parse_m3u(&text)
        }
    };
    let base = url::Url::parse(&final_url)?;
    let mut streams = Vec::new();
    for e in entries {
        let abs = base.join(&e)?.to_string();
        // Playlists that point at playlists: follow them, a few deep.
        if depth < MAX_DEPTH && playlist_kind_from_ext(&abs.to_ascii_lowercase()).is_some() {
            match resolve_depth(c, &abs, depth + 1) {
                Ok(r) if r.kind == Kind::Hls => streams.push(abs),
                Ok(r) => streams.extend(r.streams),
                Err(e) => log::info!("skipping mirror {abs}: {e}"),
            }
        } else if !streams.contains(&abs) {
            streams.push(abs);
        }
    }
    if streams.is_empty() {
        return Err(RadioError::EmptyPlaylist);
    }
    Ok(Resolved {
        kind: Kind::Playlist,
        streams,
    })
}

fn read_prefix(mut resp: reqwest::blocking::Response, max: usize) -> Result<Vec<u8>, RadioError> {
    use std::io::Read;
    let mut buf = vec![0u8; 16 * 1024];
    let mut out = Vec::new();
    while out.len() < max {
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if out.is_empty() => return Err(RadioError::Http(e.to_string())),
            Err(_) => break,
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaylistKind {
    Pls,
    M3u,
}

fn playlist_kind_from_type(ct: &str) -> Option<PlaylistKind> {
    let ct = ct.split(';').next().unwrap_or("").trim();
    match ct {
        "audio/x-scpls" | "application/pls+xml" | "audio/scpls" => Some(PlaylistKind::Pls),
        "audio/x-mpegurl"
        | "audio/mpegurl"
        | "application/x-mpegurl"
        | "application/vnd.apple.mpegurl"
        | "audio/m3u"
        | "audio/x-m3u" => Some(PlaylistKind::M3u),
        _ => None,
    }
}

fn playlist_kind_from_ext(path: &str) -> Option<PlaylistKind> {
    let p = path.split(['?', '#']).next().unwrap_or(path);
    if p.ends_with(".pls") {
        Some(PlaylistKind::Pls)
    } else if p.ends_with(".m3u") || p.ends_with(".m3u8") {
        Some(PlaylistKind::M3u)
    } else {
        None
    }
}

/// Decide from the body itself. Servers mislabel playlists all the time.
fn sniff(text: &str) -> Option<PlaylistKind> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.get(..10)
        .is_some_and(|h| h.eq_ignore_ascii_case("[playlist]"))
    {
        Some(PlaylistKind::Pls)
    } else if t.starts_with("#EXTM3U")
        || t.starts_with("#EXT-X-")
        || (!t.is_empty()
            && !t.contains('\0')
            && t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .all(|l| l.starts_with("http://") || l.starts_with("https://")))
    {
        Some(PlaylistKind::M3u)
    } else {
        None
    }
}

/// HLS media and master playlists carry these tags; plain .m3u8 mirror
/// lists don't.
pub fn is_hls(text: &str) -> bool {
    text.contains("#EXT-X-TARGETDURATION")
        || text.contains("#EXT-X-STREAM-INF")
        || text.contains("#EXT-X-MEDIA-SEQUENCE")
}

/// `FileN=` entries of a .pls, in N order.
pub fn parse_pls(text: &str) -> Vec<String> {
    let mut files: Vec<(u32, String)> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            if !k.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("file")) {
                return None;
            }
            let num: u32 = k[4..].trim().parse().ok()?;
            let v = v.trim();
            (!v.is_empty()).then(|| (num, v.to_string()))
        })
        .collect();
    files.sort_by_key(|(n, _)| *n);
    files.into_iter().map(|(_, v)| v).collect()
}

/// Non-comment lines of an .m3u.
pub fn parse_m3u(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// What the station editor shows after a test: the resolve summary plus
/// the codec and bitrate of the first stream that opened.
#[derive(Debug, Clone, PartialEq)]
pub struct StationCheck {
    pub resolved: Resolved,
    /// Which stream answered, as an index into `resolved.streams`.
    pub working: Option<usize>,
    pub codec: Option<String>,
    pub kbps: Option<u32>,
    /// icy-name, if the stream sent one.
    pub name: Option<String>,
    /// What the audio actually decodes to, after a second of it.
    pub rate: Option<u32>,
    pub channels: Option<u16>,
    pub error: Option<String>,
}

impl StationCheck {
    /// "Playlist, 3 mirrors, AAC 128 kbps"
    pub fn summary(&self) -> String {
        let mut s = self.resolved.summary();
        match (&self.codec, self.kbps) {
            (Some(c), Some(k)) => s.push_str(&format!(", {c} {k} kbps")),
            (Some(c), None) => s.push_str(&format!(", {c}")),
            _ => {}
        }
        if let (Some(r), Some(ch)) = (self.rate, self.channels) {
            let khz = if r % 1000 == 0 {
                format!("{}", r / 1000)
            } else {
                format!("{:.1}", r as f64 / 1000.0)
            };
            let layout = match ch {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                n => format!("{n} ch"),
            };
            s.push_str(&format!(", {khz} kHz {layout}"));
        }
        if let Some(e) = &self.error {
            s.push_str(&format!(", {e}"));
        }
        s
    }
}

/// Decode `want` of audio from a stream, following any format change, or
/// say why not. Gives up after 15 seconds of trying.
fn listen(src: &mut Source, want: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut got = 0f64;
    let mut buf = Vec::new();
    while got < want.as_secs_f64() {
        if std::time::Instant::now() > deadline {
            return Err("too slow to deliver audio".into());
        }
        buf.clear();
        match src.decode_into(&mut buf) {
            Ok(0) => return Err("the stream ended".into()),
            Ok(n) => got += n as f64 / src.format().rate.max(1) as f64,
            Err(e) => return Err(e.to_string()),
        }
        src.take_format_change();
    }
    Ok(())
}

/// Resolve and try to open a station, the way playing it would.
pub fn check(url: &str) -> Result<StationCheck, RadioError> {
    let resolved = resolve(url)?;
    let mut last_err = None;
    for (i, s) in resolved.streams.iter().enumerate() {
        match Source::open_with(
            s,
            OpenOptions {
                live: true,
                ..Default::default()
            },
        ) {
            Ok(mut src) => {
                // Opening only proves the server answers. Play a second of
                // it, the way the player would, to prove it decodes.
                if let Err(e) = listen(&mut src, Duration::from_secs(1)) {
                    last_err = Some(format!("answers but doesn't play: {e}"));
                    continue;
                }
                let fmt = src.format();
                let info = src.info();
                return Ok(StationCheck {
                    working: Some(i),
                    rate: Some(fmt.rate),
                    channels: Some(fmt.channels),
                    codec: Some(info.codec.clone()).filter(|c| !c.is_empty()),
                    kbps: info.kbps,
                    name: info.album.clone(),
                    error: None,
                    resolved,
                });
            }
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    Ok(StationCheck {
        resolved,
        working: None,
        codec: None,
        kbps: None,
        name: None,
        rate: None,
        channels: None,
        error: Some(match last_err {
            Some(e) => format!("no stream answered ({e})"),
            None => "no stream answered".into(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pls() {
        let t = "[playlist]\nNumberOfEntries=2\nFile2=http://b/stream\nTitle1=x\nFile1=http://a/stream\nLength1=-1\n";
        assert_eq!(parse_pls(t), vec!["http://a/stream", "http://b/stream"]);
        assert_eq!(sniff(t), Some(PlaylistKind::Pls));
    }

    #[test]
    fn m3u_and_hls() {
        let m = "#EXTM3U\n#EXTINF:-1,Station\nhttp://a/1\n\nhttp://b/2\n";
        assert_eq!(parse_m3u(m), vec!["http://a/1", "http://b/2"]);
        assert!(!is_hls(m));
        let h = "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nseg1.aac\n";
        assert!(is_hls(h));
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nlow.m3u8\n";
        assert!(is_hls(master));
        assert_eq!(sniff("http://x/a\nhttp://y/b\n"), Some(PlaylistKind::M3u));
        assert_eq!(sniff("ID3\u{3}\0\0garbage"), None);
    }

    #[test]
    fn types_and_extensions() {
        assert_eq!(
            playlist_kind_from_type("audio/x-scpls; charset=utf-8"),
            Some(PlaylistKind::Pls)
        );
        assert_eq!(
            playlist_kind_from_type("application/vnd.apple.mpegurl"),
            Some(PlaylistKind::M3u)
        );
        assert_eq!(playlist_kind_from_type("audio/mpeg"), None);
        assert_eq!(
            playlist_kind_from_ext("/listen.pls?sid=1"),
            Some(PlaylistKind::Pls)
        );
        assert_eq!(
            playlist_kind_from_ext("/live/stream.m3u8"),
            Some(PlaylistKind::M3u)
        );
        assert_eq!(playlist_kind_from_ext("/stream"), None);
    }

    #[test]
    fn summaries() {
        let r = Resolved {
            kind: Kind::Playlist,
            streams: vec!["a".into(), "b".into(), "c".into()],
        };
        let c = StationCheck {
            resolved: r,
            working: Some(0),
            codec: Some("AAC".into()),
            kbps: Some(128),
            name: None,
            rate: Some(48000),
            channels: Some(2),
            error: None,
        };
        assert_eq!(
            c.summary(),
            "Playlist, 3 mirrors, AAC 128 kbps, 48 kHz stereo"
        );
    }
}
