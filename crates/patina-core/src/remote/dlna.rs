//! DLNA (UPnP AV) media servers: MiniDLNA, Gerbera, Jellyfin, Plex,
//! Serviio, Twonky and most NAS boxes.
//!
//! They're found on the local network with an SSDP search, and read with
//! the ContentDirectory service. There's no login: anything on the network
//! that can reach the server can browse it, which is how DLNA works.
//!
//! What a server offers to browse varies a lot, so the library is read the
//! best way the server allows:
//!
//! 1. A folder view ("Browse Folders", "Folders", "By Folder"), walked to
//!    the bottom. That gives the real folder tree and every song once.
//! 2. Otherwise a ContentDirectory search for every audio item, where the
//!    server supports searching by class.
//! 3. Otherwise the whole tree from the top, keeping each song once (the
//!    same song shows under Artist, Album, Genre and All Tracks).
//!
//! Servers say when their content changes with a SystemUpdateID; while it
//! stays the same, a sync doesn't walk the tree again.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::http;
use super::{
    Client, RemoteAlbum, RemoteError, RemotePlaylist, RemotePodcast, RemoteSong, RemoteStation,
    Result, SavedQueue, ServerInfo, ServerKind, StreamFormat,
};

const CDS: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";
const MEDIA_SERVER: &str = "urn:schemas-upnp-org:device:MediaServer:1";
const SSDP: &str = "239.255.255.250:1900";
const PAGE: usize = 200;
const AUDIO_SEARCH: &str = "upnp:class derivedfrom \"object.item.audioItem\"";
/// Deeper than any real music folder tree, shallow enough to stop a loop.
const MAX_DEPTH: usize = 16;
/// A crawl is reused while the server's update id holds, up to this long,
/// since some servers never bump it.
const CRAWL_LIFE: Duration = Duration::from_secs(6 * 3600);

/// A server that answered on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    /// Its device description URL; what the config keeps as `url`.
    pub location: String,
    /// Its unique id, which stays the same when its address changes.
    pub udn: String,
    pub model: String,
}

impl Found {
    /// The address it answered from, for telling two boxes apart.
    pub fn host(&self) -> String {
        url::Url::parse(&self.location)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// One line for a picker: name, model, address.
    pub fn label(&self) -> String {
        if self.model.is_empty() {
            format!("{}  ·  {}", self.name, self.host())
        } else {
            format!("{}  ·  {}  ·  {}", self.name, self.model, self.host())
        }
    }
}

/// Look for media servers on the local network for `wait`.
pub fn discover(wait: Duration) -> Vec<Found> {
    let target: SocketAddr = SSDP.parse().expect("address");
    discover_at(target, wait)
}

/// [`discover`] asking one address instead of the multicast group. For
/// tests, and for a server on another subnet.
pub fn discover_at(target: SocketAddr, wait: Duration) -> Vec<Found> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = sock.set_multicast_ttl_v4(2);
    let msg = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {MEDIA_SERVER}\r\n\r\n"
    );
    // UDP gets lost; ask twice.
    let _ = sock.send_to(msg.as_bytes(), target);
    std::thread::sleep(Duration::from_millis(100));
    let _ = sock.send_to(msg.as_bytes(), target);
    let deadline = Instant::now() + wait;
    let mut locations: Vec<String> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = sock.set_read_timeout(Some(left.max(Duration::from_millis(10))));
        let Ok((n, _)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        if let Some(loc) = header(&text, "location")
            && !locations.contains(&loc)
        {
            locations.push(loc);
        }
    }
    let mut out: Vec<Found> = Vec::new();
    for loc in locations {
        if let Ok(d) = describe(&loc)
            && !out.iter().any(|f| f.udn == d.udn)
        {
            out.push(Found {
                name: d.name,
                location: loc,
                udn: d.udn,
                model: d.model,
            });
        }
    }
    out
}

fn header(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

/// What a device description says.
#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub udn: String,
    pub model: String,
    /// Absolute ContentDirectory control URL.
    pub control: String,
}

/// Read a device description and find its ContentDirectory.
pub fn describe(location: &str) -> Result<Device> {
    let c = http::client()?;
    let body = c
        .get(location)
        .timeout(Duration::from_secs(5))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(http::http_err)?;
    let doc = roxmltree::Document::parse(&body)
        .map_err(|e| RemoteError::Parse(format!("device description: {e}")))?;
    let text = |node: roxmltree::Node, name: &str| -> Option<String> {
        node.descendants()
            .find(|n| n.tag_name().name() == name)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
    };
    let root = doc.root_element();
    // The media server device, which may sit inside another device.
    let device = root
        .descendants()
        .find(|n| {
            n.tag_name().name() == "device"
                && text(*n, "deviceType").is_some_and(|t| t.contains("MediaServer"))
        })
        .ok_or_else(|| RemoteError::Parse("that isn't a media server".into()))?;
    let service = device
        .descendants()
        .find(|n| {
            n.tag_name().name() == "service"
                && text(*n, "serviceType").is_some_and(|t| t.contains(":ContentDirectory:"))
        })
        .ok_or(RemoteError::Unsupported("browsing (no ContentDirectory)"))?;
    let control = text(service, "controlURL").unwrap_or_default();
    let base = text(root, "URLBase").filter(|b| !b.is_empty());
    let base = url::Url::parse(base.as_deref().unwrap_or(location))
        .map_err(|e| RemoteError::Parse(e.to_string()))?;
    let control = base
        .join(&control)
        .map_err(|e| RemoteError::Parse(e.to_string()))?
        .to_string();
    // MiniDLNA calls itself "Windows Media Connect compatible (MiniDLNA)";
    // the part in brackets is the name worth showing.
    let model_name = text(device, "modelName").map(|m| match (m.find('('), m.rfind(')')) {
        (Some(a), Some(b)) if b > a + 1 => m[a + 1..b].to_string(),
        _ => m,
    });
    let model = [model_name, text(device, "modelNumber")]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Device {
        name: text(device, "friendlyName").unwrap_or_else(|| "DLNA server".into()),
        udn: text(device, "UDN").unwrap_or_default(),
        model,
        control,
    })
}

/// One DIDL-Lite entry.
#[derive(Debug, Clone, Default)]
struct Entry {
    id: String,
    container: bool,
    class: String,
    title: String,
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    genre: Option<String>,
    date: Option<String>,
    track: Option<u32>,
    art: Option<String>,
    res: Option<Res>,
}

#[derive(Debug, Clone, Default)]
struct Res {
    url: String,
    mime: String,
    duration: Option<Duration>,
    size: Option<i64>,
    rate: Option<u32>,
    bits: Option<u32>,
    channels: Option<u16>,
    /// DLNA.ORG_CI=1: the server converted it.
    converted: bool,
}

/// "0:03:25.500" to a duration.
fn parse_duration(s: &str) -> Option<Duration> {
    let mut secs = 0.0;
    for part in s.trim().split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    (secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

fn parse_didl(xml: &str) -> Result<Vec<Entry>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| RemoteError::Parse(format!("DIDL-Lite: {e}")))?;
    let mut out = Vec::new();
    for node in doc.root_element().children().filter(|n| n.is_element()) {
        let container = match node.tag_name().name() {
            "container" => true,
            "item" => false,
            _ => continue,
        };
        let mut e = Entry {
            id: node.attribute("id").unwrap_or("").to_string(),
            container,
            ..Default::default()
        };
        let mut all_res: Vec<Res> = Vec::new();
        for c in node.children().filter(|n| n.is_element()) {
            let t = || {
                c.text()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            match c.tag_name().name() {
                "title" => e.title = t().unwrap_or_default(),
                "class" => e.class = t().unwrap_or_default(),
                "artist" => {
                    // upnp:artist role="AlbumArtist" is the album artist.
                    if c.attribute("role")
                        .is_some_and(|r| r.eq_ignore_ascii_case("AlbumArtist"))
                    {
                        e.album_artist = e.album_artist.or(t());
                    } else {
                        e.artist = e.artist.or(t());
                    }
                }
                "creator" => e.artist = e.artist.or(t()),
                "albumArtist" => e.album_artist = e.album_artist.or(t()),
                "album" => e.album = t(),
                "genre" => e.genre = e.genre.or(t()),
                "date" => e.date = t(),
                "originalTrackNumber" => e.track = t().and_then(|n| n.parse().ok()),
                "albumArtURI" => e.art = e.art.or(t()),
                "res" => {
                    let proto = c.attribute("protocolInfo").unwrap_or("");
                    let mime = proto.split(':').nth(2).unwrap_or("").to_string();
                    let num = |a: &str| c.attribute(a).and_then(|v| v.trim().parse::<f64>().ok());
                    if let Some(url) = t() {
                        all_res.push(Res {
                            url,
                            mime,
                            duration: c.attribute("duration").and_then(parse_duration),
                            size: num("size").map(|n| n as i64),
                            rate: num("sampleFrequency").map(|n| n as u32),
                            bits: num("bitsPerSample").map(|n| n as u32),
                            channels: num("nrAudioChannels").map(|n| n as u16),
                            converted: proto.contains("DLNA.ORG_CI=1"),
                        });
                    }
                }
                _ => {}
            }
        }
        // The original file over a converted copy, and audio over
        // anything else (some servers list a cover as a res too).
        all_res.sort_by_key(|r| (r.converted, !r.mime.starts_with("audio/")));
        e.res = all_res.into_iter().next();
        out.push(e);
    }
    Ok(out)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A file extension from a mime type, for the codec badge.
fn suffix(mime: &str, url: &str) -> String {
    let from_url = url
        .split(['?', '#'])
        .next()
        .and_then(|p| p.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()));
    if let Some(e) = from_url {
        return e;
    }
    match mime.to_ascii_lowercase().as_str() {
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/ogg" | "audio/x-ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/x-aiff" | "audio/aiff" => "aiff",
        "audio/x-ms-wma" => "wma",
        "audio/l16" => "pcm",
        _ => "",
    }
    .to_string()
}

fn stable_id(parts: &[&str]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Each album's songs, by album id.
type SongsByAlbum = HashMap<String, Vec<RemoteSong>>;

struct Crawl {
    update_id: Option<String>,
    at: Instant,
    albums: Vec<RemoteAlbum>,
    songs: SongsByAlbum,
}

pub struct Dlna {
    http: reqwest::blocking::Client,
    control: String,
    info: ServerInfo,
    crawl: Mutex<Option<Crawl>>,
}

impl Dlna {
    /// Connect by device description URL. If that's gone (the server's
    /// address or port changed), look for the server by its id.
    pub fn connect(location: &str, udn: &str) -> Result<Self> {
        let dev = match describe(location) {
            Ok(d) => d,
            Err(e) if !udn.is_empty() => discover(Duration::from_secs(3))
                .into_iter()
                .find(|f| f.udn == udn)
                .map(|f| describe(&f.location))
                .unwrap_or(Err(e))?,
            Err(e) => return Err(e),
        };
        let product = if dev.model.is_empty() {
            "DLNA".to_string()
        } else {
            format!("DLNA, {}", dev.model)
        };
        let s = Self {
            http: http::client()?,
            control: dev.control,
            info: ServerInfo {
                kind: ServerKind::Dlna,
                product,
                open_subsonic: false,
                api_version: String::new(),
                play_queue: false,
            },
            crawl: Mutex::new(None),
        };
        // Make sure it answers before calling it connected.
        s.update_id()?;
        Ok(s)
    }

    /// Call a ContentDirectory action and read the named fields out of
    /// the answer.
    fn soap<const N: usize>(
        &self,
        action: &str,
        args: &str,
        names: [&str; N],
    ) -> Result<[Option<String>; N]> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
             <u:{action} xmlns:u=\"{CDS}\">{args}</u:{action}></s:Body></s:Envelope>"
        );
        let resp = self
            .http
            .post(&self.control)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header("SOAPACTION", format!("\"{CDS}#{action}\""))
            .body(body)
            .send()
            .map_err(http::http_err)?;
        let ok = resp.status().is_success();
        let text = resp.text().map_err(http::http_err)?;
        let doc = roxmltree::Document::parse(&text)
            .map_err(|e| RemoteError::Parse(format!("{action}: {e}")))?;
        let field = |name: &str| {
            doc.descendants()
                .find(|n| n.tag_name().name() == name)
                .and_then(|n| n.text())
                .map(str::to_string)
        };
        if !ok {
            let code = field("errorCode")
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0);
            let message = field("errorDescription").unwrap_or_else(|| "UPnP error".into());
            return Err(RemoteError::Server { code, message });
        }
        Ok(names.map(field))
    }

    fn update_id(&self) -> Result<Option<String>> {
        let [id] = self.soap("GetSystemUpdateID", "", ["Id"])?;
        Ok(id)
    }

    fn search_caps(&self) -> String {
        self.soap("GetSearchCapabilities", "", ["SearchCaps"])
            .ok()
            .and_then(|[c]| c)
            .unwrap_or_default()
    }

    /// The server's own address, for turning stored references back into
    /// URLs.
    fn origin(&self) -> String {
        url::Url::parse(&self.control)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_default()
    }

    /// A URL on the server as it's kept in the index: without the address,
    /// so the songs stay the same songs when the server's address changes.
    /// Servers often name themselves by a LAN address that differs from the
    /// one we reached them on, so any host on the same scheme and port
    /// counts as the server.
    fn local_ref(&self, url: &str) -> String {
        let (Ok(u), Ok(c)) = (url::Url::parse(url), url::Url::parse(&self.control)) else {
            return url.to_string();
        };
        if u.scheme() == c.scheme() && u.port_or_known_default() == c.port_or_known_default() {
            let mut r = u.path().to_string();
            if let Some(q) = u.query() {
                r.push('?');
                r.push_str(q);
            }
            r
        } else {
            url.to_string()
        }
    }

    /// A stored reference back to a full URL.
    fn full_url(&self, r: &str) -> String {
        if r.starts_with('/') {
            format!("{}{r}", self.origin())
        } else {
            r.to_string()
        }
    }

    /// One page of `Browse` or `Search`: (entries, total if known).
    fn page(&self, search: Option<&str>, id: &str, start: usize) -> Result<(Vec<Entry>, usize)> {
        let want = ["Result", "TotalMatches"];
        let [result, total] = match search {
            None => self.soap(
                "Browse",
                &format!(
                    "<ObjectID>{}</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag>\
                     <Filter>*</Filter><StartingIndex>{start}</StartingIndex>\
                     <RequestedCount>{PAGE}</RequestedCount><SortCriteria></SortCriteria>",
                    xml_escape(id)
                ),
                want,
            )?,
            Some(criteria) => self.soap(
                "Search",
                &format!(
                    "<ContainerID>{}</ContainerID><SearchCriteria>{}</SearchCriteria>\
                     <Filter>*</Filter><StartingIndex>{start}</StartingIndex>\
                     <RequestedCount>{PAGE}</RequestedCount><SortCriteria></SortCriteria>",
                    xml_escape(id),
                    xml_escape(criteria)
                ),
                want,
            )?,
        };
        let result = result.unwrap_or_default();
        let total = total.and_then(|t| t.trim().parse().ok()).unwrap_or(0);
        let entries = if result.trim().is_empty() {
            Vec::new()
        } else {
            parse_didl(&result)?
        };
        Ok((entries, total))
    }

    /// Every entry, all pages.
    fn all(&self, search: Option<&str>, id: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        loop {
            let (page, total) = self.page(search, id, out.len())?;
            let n = page.len();
            out.extend(page);
            // Some servers report 0 for the total; stop on a short page.
            if n == 0 || (total > 0 && out.len() >= total) || (total == 0 && n < PAGE) {
                break;
            }
        }
        Ok(out)
    }

    /// The server's folder view, if it has one: (id, path so far).
    fn folder_view(&self) -> Result<Option<String>> {
        let is_folders = |t: &str| {
            let t = t.to_ascii_lowercase();
            // MiniDLNA, Serviio, Twonky and Gerbera, in that order.
            [
                "browse folders",
                "folders",
                "by folder",
                "folder",
                "pc directory",
            ]
            .contains(&t.as_str())
        };
        let top = self.all(None, "0")?;
        if let Some(f) = top.iter().find(|e| e.container && is_folders(&e.title)) {
            // MiniDLNA's top "Browse Folders" holds Music, Video and
            // Pictures folders; the music one is under it.
            let inner = self.all(None, &f.id)?;
            if let Some(m) = inner
                .iter()
                .find(|e| e.container && e.title.eq_ignore_ascii_case("music"))
            {
                return Ok(Some(m.id.clone()));
            }
            return Ok(Some(f.id.clone()));
        }
        if let Some(music) = top
            .iter()
            .find(|e| e.container && e.title.eq_ignore_ascii_case("music"))
        {
            let inner = self.all(None, &music.id)?;
            if let Some(f) = inner.iter().find(|e| e.container && is_folders(&e.title)) {
                return Ok(Some(f.id.clone()));
            }
        }
        Ok(None)
    }

    /// Walk containers from `id`, collecting audio items with the folder
    /// path they were found under.
    /// `art` is the folder's cover, for songs that have none of their own.
    fn walk(
        &self,
        id: &str,
        art: Option<&str>,
        path: &mut Vec<String>,
        seen: &mut HashSet<String>,
        out: &mut Vec<(Vec<String>, Entry)>,
    ) -> Result<()> {
        if path.len() > MAX_DEPTH {
            return Ok(());
        }
        let children = self.all(None, id)?;
        // A cover.jpg or folder.jpg beside the songs, as Gerbera lists them.
        let folder_art = children.iter().find_map(|e| {
            let name = e.title.to_ascii_lowercase();
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
            (e.class.starts_with("object.item.imageItem")
                && ["cover", "folder", "front", "albumart", "album"].contains(&stem))
            .then(|| e.res.as_ref().map(|r| self.local_ref(&r.url)))
            .flatten()
        });
        let art = folder_art.as_deref().or(art);
        for mut e in children {
            if e.container {
                // Pictures and video have no business in a music library.
                if path.is_empty()
                    && ["video", "videos", "pictures", "photos", "images"]
                        .contains(&e.title.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                if !seen.insert(format!("c:{}", e.id)) {
                    continue;
                }
                path.push(e.title.clone());
                self.walk(&e.id, e.art.as_deref().or(art), path, seen, out)?;
                path.pop();
            } else if e.class.starts_with("object.item.audioItem")
                && let Some(r) = &e.res
                && seen.insert(format!("r:{}", r.url))
            {
                if e.art.is_none() {
                    e.art = art.map(str::to_string);
                }
                out.push((path.clone(), e));
            }
        }
        Ok(())
    }

    fn crawl(&self) -> Result<(Vec<RemoteAlbum>, SongsByAlbum)> {
        // 1: the folder view; 2: search; 3: everything.
        let mut found: Vec<(Vec<String>, Entry)> = Vec::new();
        let mut seen = HashSet::new();
        let mut folders = false;
        if let Some(root) = self.folder_view()? {
            self.walk(&root, None, &mut Vec::new(), &mut seen, &mut found)?;
            folders = true;
            // Gerbera's folder view starts at the filesystem root. Drop the
            // folders every song shares, keeping at least each song's own.
            let shortest = found.iter().map(|(p, _)| p.len()).min().unwrap_or(0);
            let mut common = 0;
            while common + 1 < shortest
                && found.iter().all(|(p, _)| p[common] == found[0].0[common])
            {
                common += 1;
            }
            for (p, _) in &mut found {
                p.drain(..common);
            }
            // Gerbera's folder view titles songs by file name. Its search
            // has the tags, so where a server searches, the tags come from
            // there and only the folders from here.
            if self.search_caps().contains("upnp:class")
                && let Ok(items) = self.all(Some(AUDIO_SEARCH), "0")
            {
                let tagged: HashMap<String, Entry> = items
                    .into_iter()
                    .filter_map(|e| Some((e.res.as_ref()?.url.clone(), e)))
                    .collect();
                for (_, e) in &mut found {
                    if let Some(t) = e.res.as_ref().and_then(|r| tagged.get(&r.url)) {
                        let (res, art) = (e.res.take(), e.art.take());
                        *e = t.clone();
                        e.res = res;
                        e.art = e.art.take().or(art);
                    }
                }
            }
        } else if self.search_caps().contains("upnp:class") {
            let items = self.all(Some(AUDIO_SEARCH), "0")?;
            for e in items {
                if let Some(r) = &e.res
                    && seen.insert(format!("r:{}", r.url))
                {
                    found.push((Vec::new(), e));
                }
            }
        } else {
            self.walk("0", None, &mut Vec::new(), &mut seen, &mut found)?;
        }
        let mut songs: HashMap<String, Vec<RemoteSong>> = HashMap::new();
        for (path, mut e) in found {
            let Some(r) = e.res.take() else { continue };
            // Gerbera titles songs "Artist - Title".
            if let Some(a) = &e.artist
                && let Some(t) = e.title.strip_prefix(a.as_str())
                && let Some(t) = t.strip_prefix(" - ")
                && !t.trim().is_empty()
            {
                e.title = t.trim().to_string();
            }
            let ext = suffix(&r.mime, &r.url);
            let album_artist = e.album_artist.clone().or(e.artist.clone());
            let album_id = match &e.album {
                Some(a) => stable_id(&[album_artist.as_deref().unwrap_or(""), a]),
                None => stable_id(&["folder", &path.join("/")]),
            };
            let kbps = match (r.size, r.duration) {
                (Some(sz), Some(d)) if d.as_secs_f64() > 0.0 => {
                    Some((sz as f64 * 8.0 / d.as_secs_f64() / 1000.0).round() as u32)
                }
                _ => None,
            };
            // The folder path, with a file name made from the title.
            let rel = folders.then(|| {
                let mut p = path.clone();
                let num = e.track.map(|n| format!("{n:02} ")).unwrap_or_default();
                let ext = if ext.is_empty() { "audio" } else { &ext };
                p.push(format!("{num}{}.{ext}", e.title.replace('/', "\u{2215}")));
                p.join("/")
            });
            let song = RemoteSong {
                // The item id changes between views on some servers; the
                // stream URL is what identifies the song.
                id: self.local_ref(&r.url),
                title: e.title.clone(),
                artist: e.artist.clone(),
                album_artist,
                album: e.album.clone(),
                album_id: album_id.clone(),
                track_no: e.track,
                disc_no: None,
                year: e.date.as_deref().and_then(|d| d.get(..4)?.parse().ok()),
                genres: e.genre.clone().into_iter().collect(),
                duration: r.duration,
                suffix: ext,
                sample_rate: r.rate,
                bits: r.bits,
                channels: r.channels,
                kbps,
                size: r.size,
                path: rel,
                cover: e.art.as_deref().map(|a| self.local_ref(a)),
                ..Default::default()
            };
            songs.entry(album_id).or_default().push(song);
        }
        let albums = songs
            .iter()
            .map(|(id, list)| {
                let first = &list[0];
                let mut ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
                ids.sort_unstable();
                RemoteAlbum {
                    rating: None,
                    id: id.clone(),
                    name: first.album.clone().unwrap_or_default(),
                    artist: first.album_artist.clone().unwrap_or_default(),
                    song_count: list.len() as u32,
                    duration_s: list
                        .iter()
                        .filter_map(|s| s.duration)
                        .map(|d| d.as_secs())
                        .sum(),
                    year: first.year,
                    // Which songs it holds, so a swap of one file for
                    // another counts as a change.
                    changed: Some(stable_id(&ids)),
                }
            })
            .collect();
        Ok((albums, songs))
    }
}

impl Client for Dlna {
    fn info(&self) -> &ServerInfo {
        &self.info
    }

    fn albums(&self) -> Result<Vec<RemoteAlbum>> {
        let update = self.update_id().ok().flatten();
        {
            let c = self.crawl.lock().unwrap();
            if let Some(c) = c.as_ref()
                && update.is_some()
                && c.update_id == update
                && c.at.elapsed() < CRAWL_LIFE
            {
                return Ok(c.albums.clone());
            }
        }
        let (albums, songs) = self.crawl()?;
        *self.crawl.lock().unwrap() = Some(Crawl {
            update_id: update,
            at: Instant::now(),
            albums: albums.clone(),
            songs,
        });
        Ok(albums)
    }

    fn album_songs(&self, album_id: &str) -> Result<Vec<RemoteSong>> {
        if self.crawl.lock().unwrap().is_none() {
            self.albums()?;
        }
        Ok(self
            .crawl
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.songs.get(album_id).cloned())
            .unwrap_or_default())
    }

    fn stream_url_as(&self, song_id: &str, _: StreamFormat, _: u32) -> Result<(String, bool)> {
        // The id is the server's own URL for the file, less its address.
        // DLNA has no conversion to ask for; it's always the original.
        Ok((self.full_url(song_id), true))
    }

    fn stream_setting(&self) -> (StreamFormat, u32) {
        (StreamFormat::Raw, 0)
    }

    fn ping(&self) -> Result<()> {
        self.update_id().map(|_| ())
    }

    fn episode_url(&self, _: &str) -> Result<String> {
        Err(RemoteError::Unsupported("podcasts"))
    }

    fn cover(&self, cover_id: &str, _size: u32) -> Result<Vec<u8>> {
        let (b, ctype) = http::get_bytes(&self.http, &self.full_url(cover_id), &[])?;
        if !ctype.starts_with("image/") && !b.starts_with(b"\xff\xd8") && !b.starts_with(b"\x89PNG")
        {
            return Err(RemoteError::Parse("no image".into()));
        }
        Ok(b)
    }

    fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        Ok(Vec::new())
    }

    fn playlist_songs(&self, _: &str) -> Result<Vec<RemoteSong>> {
        Ok(Vec::new())
    }

    fn create_playlist(&self, _: &str, _: &[String]) -> Result<String> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn replace_playlist(&self, _: &str, _: &[String]) -> Result<()> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn rename_playlist(&self, _: &str, _: &str) -> Result<()> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn delete_playlist(&self, _: &str) -> Result<()> {
        Err(RemoteError::Unsupported("playlists"))
    }

    fn radio(&self) -> Result<Vec<RemoteStation>> {
        Ok(Vec::new())
    }

    fn podcasts(&self) -> Result<Vec<RemotePodcast>> {
        Ok(Vec::new())
    }

    fn save_queue(&self, _: &[String], _: Option<&str>, _: Duration) -> Result<()> {
        Err(RemoteError::Unsupported("a saved play queue"))
    }

    fn load_queue(&self) -> Result<Option<SavedQueue>> {
        Err(RemoteError::Unsupported("a saved play queue"))
    }

    fn bookmark(&self, _: &str, _: Duration) -> Result<()> {
        Err(RemoteError::Unsupported("bookmarks"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minidlna_items_parse() {
        // As MiniDLNA 1.3.3 sends them.
        let xml = r#"<DIDL-Lite xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dlna="urn:schemas-dlna-org:metadata-1-0/">
<item id="64$1$0$0" parentID="64$1$0" restricted="1"><dc:title>Blak 1</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class><dc:creator>Gary Clark Jr.</dc:creator><dc:date>2012-01-01</dc:date><upnp:artist>Gary Clark Jr.</upnp:artist><upnp:album>Blak and Blu</upnp:album><upnp:genre>Blues Rock</upnp:genre><upnp:originalTrackNumber>1</upnp:originalTrackNumber><upnp:albumArtURI dlna:profileID="JPEG_TN">http://192.0.2.2:8200/AlbumArt/3-25.jpg</upnp:albumArtURI><res size="481051" duration="0:00:30.053" bitrate="128000" sampleFrequency="44100" nrAudioChannels="2" protocolInfo="http-get:*:audio/mpeg:DLNA.ORG_PN=MP3;DLNA.ORG_OP=01;DLNA.ORG_CI=0">http://192.0.2.2:8200/MediaItems/25.mp3</res></item>
<container id="64$1" parentID="64" restricted="1" childCount="2"><dc:title>Blak and Blu</dc:title><upnp:class>object.container.storageFolder</upnp:class></container></DIDL-Lite>"#;
        let e = parse_didl(xml).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].title, "Blak 1");
        assert_eq!(e[0].track, Some(1));
        assert_eq!(
            e[0].art.as_deref(),
            Some("http://192.0.2.2:8200/AlbumArt/3-25.jpg")
        );
        let r = e[0].res.as_ref().unwrap();
        assert_eq!(r.duration, Some(Duration::from_millis(30053)));
        assert_eq!((r.rate, r.channels), (Some(44100), Some(2)));
        assert!(e[1].container);
        assert_eq!(suffix(&r.mime, &r.url), "mp3");
        assert_eq!(suffix("audio/x-flac", "http://h/stream?id=4"), "flac");
    }

    #[test]
    fn converted_copies_lose_to_originals() {
        let xml = r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
<item id="1"><dc:title>T</dc:title><upnp:class>object.item.audioItem</upnp:class>
<res protocolInfo="http-get:*:audio/mpeg:DLNA.ORG_CI=1">http://h/t.mp3</res>
<res protocolInfo="http-get:*:audio/flac:*">http://h/t.flac</res></item></DIDL-Lite>"#;
        let e = parse_didl(xml).unwrap();
        assert_eq!(e[0].res.as_ref().unwrap().url, "http://h/t.flac");
        assert_eq!(parse_duration("1:02:03"), Some(Duration::from_secs(3723)));
    }

    #[test]
    fn ssdp_headers() {
        let r = "HTTP/1.1 200 OK\r\nST: urn:x\r\nLocation: http://192.168.1.5:8200/rootDesc.xml\r\n\r\n";
        assert_eq!(
            header(r, "location").as_deref(),
            Some("http://192.168.1.5:8200/rootDesc.xml")
        );
    }
}
