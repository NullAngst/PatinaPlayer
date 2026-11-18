//! Listening to a Snapcast stream here, the way a snapclient does.
//!
//! Patina already plays *to* a snapserver ([`crate::cast::snapcast`]).
//! This is the other way round: it connects to the snapserver's stream
//! port (1704) as one more client, says hello, and plays the audio the
//! server sends. Stations in Radio can point at one, as
//! `snapcast://host:1704/<stream id>`.
//!
//! Snapcast's binary protocol: every message starts with a 26-byte header
//! (type, id, refers-to, sent and received times, payload size, all
//! little-endian). The server sends a Codec Header first, then Wire
//! Chunks of encoded audio. For FLAC, Ogg and PCM the codec header is the
//! start of an ordinary FLAC, Ogg or WAV stream and the chunks are what
//! follows, so header and chunks together are a stream FFmpeg reads as is.
//! Opus comes as bare packets, which are put into Ogg pages here.
//!
//! What's left out: the clock sync. A snapclient plays each chunk at the
//! time stamped on it, so every room is in step; this plays them as they
//! come, about a buffer's length (a second, by default) ahead of the
//! rooms. Snapweb's volume and mute for this client do nothing either,
//! since the app's own volume is what you have here.
//!
//! The snapserver puts a new client in a group of its own on its default
//! stream. Picking the station's stream goes through the control API
//! ([`crate::cast::snapcast::listen_route`]): before connecting when the
//! server knows this client from before, right after hello the first time.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The snapserver's port for clients.
pub const STREAM_PORT: u16 = 1704;
const SCHEME: &str = "snapcast";
/// What this app's client ids start with, so the Snapcast page can leave
/// it out of the rooms Patina plays to.
pub const ID_PREFIX: &str = "PatinaPlayer-listen-";

const HEADER: usize = 26;
const CODEC_HEADER: u16 = 1;
const WIRE_CHUNK: u16 = 2;
const TIME: u16 = 4;
const HELLO: u16 = 5;
/// Nothing the server sends is anywhere near this.
const MAX_PAYLOAD: usize = 16 << 20;

/// A station's address for a stream on a snapserver at `host`.
pub fn station_url(host: &str, stream: &str) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let mut u = url::Url::parse(&format!("{SCHEME}://{host}:{STREAM_PORT}/"))
        .unwrap_or_else(|_| url::Url::parse("snapcast://localhost:1704/").expect("a fixed URL"));
    if let Ok(mut p) = u.path_segments_mut() {
        p.clear().push(stream);
    }
    u.to_string()
}

/// Whether a station's address is a Snapcast stream.
pub fn is_url(url: &str) -> bool {
    url.trim()
        .get(..SCHEME.len() + 3)
        .is_some_and(|s| s.eq_ignore_ascii_case("snapcast://"))
}

/// A station address taken apart: host, port, stream id. The stream is
/// empty for "whatever the server gives this client".
pub fn parse(url: &str) -> Option<(String, u16, String)> {
    let u = url::Url::parse(url.trim()).ok()?;
    if !u.scheme().eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let host = u.host_str()?.trim_matches(['[', ']']).to_string();
    let stream = u
        .path_segments()
        .and_then(|mut s| s.next())
        .map(percent_decode)
        .unwrap_or_default();
    Some((host, u.port().unwrap_or(STREAM_PORT), stream))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// This device's client id, made once and kept, so the snapserver knows
/// it again (and remembers which stream it was on).
pub fn client_id() -> String {
    let path: PathBuf = crate::config::data_dir().join("snapclient-id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if s.starts_with(ID_PREFIX) {
            return s.to_string();
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&SystemTime::now(), &mut h);
    std::hash::Hash::hash(&std::process::id(), &mut h);
    std::hash::Hash::hash(&host_name(), &mut h);
    let id = format!("{ID_PREFIX}{:016x}", std::hash::Hasher::finish(&h));
    let _ = std::fs::create_dir_all(crate::config::data_dir());
    let _ = std::fs::write(&path, &id);
    id
}

fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "localhost")
        .unwrap_or_else(|| {
            if cfg!(target_os = "android") {
                "Android".into()
            } else {
                "PatinaPlayer".into()
            }
        })
}

fn header(kind: u16, size: usize) -> [u8; HEADER] {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut h = [0u8; HEADER];
    h[0..2].copy_from_slice(&kind.to_le_bytes());
    // id and refersTo stay 0.
    h[6..10].copy_from_slice(&(now.as_secs() as i32).to_le_bytes());
    h[10..14].copy_from_slice(&(now.subsec_micros() as i32).to_le_bytes());
    // Received time: 0, the server fills it in.
    h[22..26].copy_from_slice(&(size as u32).to_le_bytes());
    h
}

fn hello(id: &str) -> Vec<u8> {
    let body = serde_json::json!({
        "Arch": std::env::consts::ARCH,
        "ClientName": "PatinaPlayer",
        "HostName": format!("{} (PatinaPlayer)", host_name()),
        "ID": id,
        "Instance": 1,
        "MAC": "00:00:00:00:00:00",
        "OS": std::env::consts::OS,
        "SnapStreamProtocolVersion": 2,
        "Version": env!("CARGO_PKG_VERSION"),
    })
    .to_string();
    let mut payload = (body.len() as u32).to_le_bytes().to_vec();
    payload.extend_from_slice(body.as_bytes());
    let mut msg = header(HELLO, payload.len()).to_vec();
    msg.extend(payload);
    msg
}

/// A time request: the server answers it, and it shows the client is
/// still there. The latency field is 0.
fn time_request() -> Vec<u8> {
    let mut msg = header(TIME, 8).to_vec();
    msg.extend([0u8; 8]);
    msg
}

/// Fill `buf` from the socket. Its reads time out every half second so a
/// stop is noticed; a timeout part way through just carries on.
fn fill(s: &mut TcpStream, buf: &mut [u8], abort: &AtomicBool) -> std::io::Result<()> {
    let mut got = 0;
    while got < buf.len() {
        if abort.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "stopped",
            ));
        }
        match s.read(&mut buf[got..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "the snapserver closed the connection",
                ));
            }
            Ok(n) => got += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// One message: its type and payload.
fn read_msg(s: &mut TcpStream, abort: &AtomicBool) -> std::io::Result<(u16, Vec<u8>)> {
    let mut h = [0u8; HEADER];
    fill(s, &mut h, abort)?;
    let kind = u16::from_le_bytes([h[0], h[1]]);
    let size = u32::from_le_bytes([h[22], h[23], h[24], h[25]]) as usize;
    if size > MAX_PAYLOAD {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a {size}-byte message: that isn't a snapserver"),
        ));
    }
    let mut payload = vec![0u8; size];
    fill(s, &mut payload, abort)?;
    Ok((kind, payload))
}

/// A length-prefixed piece of a payload, and what's after it.
fn take(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    let rest = &b[4..];
    (rest.len() >= n).then(|| rest.split_at(n))
}

/// A codec header: the codec's name and its own header.
fn codec_header(payload: &[u8]) -> Option<(String, Vec<u8>)> {
    let (name, rest) = take(payload)?;
    let (data, _) = take(rest)?;
    Some((String::from_utf8_lossy(name).into_owned(), data.to_vec()))
}

/// A wire chunk's audio.
fn chunk_audio(payload: &[u8]) -> Option<&[u8]> {
    take(payload.get(8..)?).map(|(a, _)| a)
}

/// How the audio is turned into a stream FFmpeg can read.
enum Wrap {
    /// FLAC, Ogg, PCM: as it comes.
    Plain,
    Opus(OggOpus),
}

/// A Snapcast stream, as one file-like stream of bytes for the decoder.
/// Ends (reads 0) when the server goes away or switches this client to a
/// stream with another codec header; the player then reconnects, as for
/// any station that drops.
pub struct Listener {
    sock: TcpStream,
    abort: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    wrap: Wrap,
    pending: Vec<u8>,
    at: usize,
    ended: bool,
}

impl Listener {
    fn next_audio(&mut self) -> std::io::Result<bool> {
        loop {
            let (kind, payload) = match read_msg(&mut self.sock, &self.abort) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(e) => return Err(e),
            };
            match kind {
                WIRE_CHUNK => {
                    let Some(audio) = chunk_audio(&payload) else {
                        continue;
                    };
                    self.pending = match &mut self.wrap {
                        Wrap::Plain => audio.to_vec(),
                        Wrap::Opus(o) => o.packet(audio),
                    };
                    self.at = 0;
                    return Ok(true);
                }
                // Another stream, maybe another codec: start over.
                CODEC_HEADER => return Ok(false),
                // Server settings, time answers: nothing to do.
                _ => {}
            }
        }
    }
}

impl Read for Listener {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.pending.len() {
            if self.ended || !self.next_audio()? {
                self.ended = true;
                return Ok(0);
            }
        }
        let n = buf.len().min(self.pending.len() - self.at);
        buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

fn connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("can't find {host}")))?;
    let s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    s.set_nodelay(true)?;
    Ok(s)
}

/// Read until a codec header comes, giving up after `wait`.
fn wait_header(
    s: &mut TcpStream,
    abort: &AtomicBool,
    wait: Duration,
) -> Result<(String, Vec<u8>), String> {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        let (kind, payload) = read_msg(s, abort).map_err(|e| e.to_string())?;
        if kind == CODEC_HEADER {
            return codec_header(&payload).ok_or_else(|| "a codec header that doesn't read".into());
        }
    }
    Err("the snapserver sent no audio".into())
}

/// Connect to the stream a station URL names. The codec's file name
/// ("snapcast.flac") comes with it, to help FFmpeg tell the format.
pub fn open(url: &str, abort: Arc<AtomicBool>) -> Result<(Listener, &'static str), String> {
    let (host, port, stream) =
        parse(url).ok_or_else(|| format!("not a Snapcast address: {url}"))?;
    let id = client_id();
    // Known from before: on the right stream before the first byte.
    let known = if stream.is_empty() {
        Some(false)
    } else {
        match crate::cast::snapcast::listen_route(&host, &id, &stream) {
            Ok(r) => r.map(|_| true),
            Err(e) => {
                log::info!("snapcast: can't pick the stream {stream} on {host}: {e}");
                Some(false)
            }
        }
    };
    let mut sock = connect(&host, port).map_err(|e| format!("{host}:{port}: {e}"))?;
    sock.write_all(&hello(&id))
        .map_err(|e| format!("{host}:{port}: {e}"))?;
    let mut codec = wait_header(&mut sock, &abort, Duration::from_secs(8))?;
    if known.is_none() {
        // New to the server: it's on the default stream now. Move it, and
        // take the header of the stream it moves to.
        match crate::cast::snapcast::listen_route(&host, &id, &stream) {
            Ok(Some(true)) => codec = wait_header(&mut sock, &abort, Duration::from_secs(5))?,
            Ok(_) => {}
            Err(e) => log::info!("snapcast: can't pick the stream {stream} on {host}: {e}"),
        }
    }
    let (name, data) = codec;
    let (wrap, start, file) = match name.as_str() {
        "flac" => (Wrap::Plain, data, "snapcast.flac"),
        "ogg" => (Wrap::Plain, data, "snapcast.ogg"),
        "pcm" => (Wrap::Plain, data, "snapcast.wav"),
        "opus" => {
            let (rate, channels) = opus_format(&data);
            let mut o = OggOpus::new(channels, rate);
            let head = o.head();
            (Wrap::Opus(o), head, "snapcast.opus")
        }
        other => {
            return Err(format!(
                "the stream is in {other}, which PatinaPlayer can't read"
            ));
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    // A time request every second or so, as a snapclient sends, so the
    // server sees this one is still listening.
    if let Ok(mut w) = sock.try_clone() {
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("patina-snapclient".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(1));
                    if w.write_all(&time_request()).is_err() {
                        return;
                    }
                }
            })
            .ok();
    }
    log::info!("snapcast: listening to {stream} on {host}:{port} ({name})");
    Ok((
        Listener {
            sock,
            abort,
            stop,
            wrap,
            pending: start,
            at: 0,
            ended: false,
        },
        file,
    ))
}

/// Snapcast's own Opus header: "OPUS" as a number, the rate, the bits
/// and the channels.
fn opus_format(h: &[u8]) -> (u32, u8) {
    let rate = h
        .get(4..8)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .filter(|r| *r > 0)
        .unwrap_or(48_000);
    let channels = h
        .get(10..12)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .filter(|c| (1..=2).contains(c))
        .unwrap_or(2) as u8;
    (rate, channels)
}

/// Opus packets put into Ogg pages, one packet a page: what FFmpeg (and
/// every other player) reads as a .opus file.
struct OggOpus {
    channels: u8,
    rate: u32,
    serial: u32,
    seq: u32,
    granule: u64,
}

impl OggOpus {
    fn new(channels: u8, rate: u32) -> Self {
        Self {
            channels,
            rate,
            serial: 0x5041_5449,
            seq: 0,
            granule: 0,
        }
    }

    /// The two header pages: OpusHead and OpusTags.
    fn head(&mut self) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(self.channels);
        head.extend(0u16.to_le_bytes());
        head.extend(self.rate.to_le_bytes());
        head.extend(0i16.to_le_bytes());
        head.push(0);
        let mut tags = b"OpusTags".to_vec();
        let vendor = b"PatinaPlayer";
        tags.extend((vendor.len() as u32).to_le_bytes());
        tags.extend(vendor);
        tags.extend(0u32.to_le_bytes());
        let mut out = self.page(0x02, 0, &head);
        out.extend(self.page(0, 0, &tags));
        out
    }

    fn packet(&mut self, p: &[u8]) -> Vec<u8> {
        self.granule += opus_samples(p);
        let g = self.granule;
        self.page(0, g, p)
    }

    fn page(&mut self, flags: u8, granule: u64, packet: &[u8]) -> Vec<u8> {
        let mut lacing = vec![255u8; packet.len() / 255];
        lacing.push((packet.len() % 255) as u8);
        let mut p = Vec::with_capacity(27 + lacing.len() + packet.len());
        p.extend(b"OggS");
        p.push(0);
        p.push(flags);
        p.extend(granule.to_le_bytes());
        p.extend(self.serial.to_le_bytes());
        p.extend(self.seq.to_le_bytes());
        p.extend([0u8; 4]);
        p.push(lacing.len() as u8);
        p.extend(&lacing);
        p.extend(packet);
        let crc = ogg_crc(&p);
        p[22..26].copy_from_slice(&crc.to_le_bytes());
        self.seq += 1;
        p
    }
}

/// How many samples (at 48 kHz) an Opus packet holds, from its first byte.
fn opus_samples(p: &[u8]) -> u64 {
    let Some(&toc) = p.first() else { return 0 };
    let config = toc >> 3;
    let frame: u64 = match config {
        0..=11 => [480, 960, 1920, 2880][usize::from(config % 4)],
        12..=15 => [480, 960][usize::from(config % 2)],
        _ => [120, 240, 480, 960][usize::from(config % 4)],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u64::from(p.get(1).map_or(0, |c| c & 0x3f)),
    };
    frame * frames
}

/// Ogg's page checksum: CRC-32 with polynomial 0x04c11db7, no reflection,
/// starting from 0.
fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn station_urls_go_both_ways() {
        let u = station_url("192.168.1.20", "Kitchen radio");
        assert_eq!(u, "snapcast://192.168.1.20:1704/Kitchen%20radio");
        assert!(is_url(&u));
        assert!(!is_url("http://x/stream"));
        assert_eq!(
            parse(&u),
            Some(("192.168.1.20".into(), 1704, "Kitchen radio".into()))
        );
        assert_eq!(
            parse("snapcast://box"),
            Some(("box".into(), 1704, String::new()))
        );
        assert_eq!(parse("http://box/x"), None);
    }

    #[test]
    fn opus_packet_lengths() {
        // CELT 20 ms, one frame.
        assert_eq!(opus_samples(&[31 << 3]), 960);
        // SILK 60 ms, two frames.
        assert_eq!(opus_samples(&[(3 << 3) | 1]), 5760);
        // Code 3: the count is in the next byte.
        assert_eq!(opus_samples(&[(18 << 3) | 3, 4]), 4 * 480);
    }

    #[test]
    fn ogg_pages_check_out() {
        // CRC-32 with Ogg's parameters over the usual check string.
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
        let mut o = OggOpus::new(2, 48_000);
        let head = o.head();
        assert_eq!(&head[..4], b"OggS");
        assert_eq!(head[5], 0x02);
        let p = o.packet(&[(31 << 3), 1, 2, 3]);
        assert_eq!(u64::from_le_bytes(p[6..14].try_into().unwrap()), 960);
        assert_eq!(u32::from_le_bytes(p[18..22].try_into().unwrap()), 2);
    }

    /// Opus packets as Snapcast sends them (bare, one a chunk), put into
    /// Ogg here, decode. The packets come out of a real Ogg Opus file
    /// FFmpeg makes, so this needs the `ffmpeg` command with libopus; it
    /// skips itself without one.
    #[test]
    fn rewrapped_opus_decodes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.opus");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=f=440:r=48000:d=2")
            .args(["-ac", "2", "-c:a", "libopus", "-b:a", "96k"])
            .arg(&src)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("no ffmpeg with libopus: skipped");
            return;
        }
        // The packets, from the file's pages: laced segments, a packet
        // ending at a segment under 255.
        let data = std::fs::read(&src).unwrap();
        let mut packets = Vec::new();
        let mut cur = Vec::new();
        let mut i = 0;
        while i + 27 <= data.len() && &data[i..i + 4] == b"OggS" {
            let n = data[i + 26] as usize;
            let lacing = &data[i + 27..i + 27 + n];
            let mut at = i + 27 + n;
            for &l in lacing {
                cur.extend_from_slice(&data[at..at + l as usize]);
                at += l as usize;
                if l < 255 {
                    packets.push(std::mem::take(&mut cur));
                }
            }
            i = at;
        }
        // OpusHead and OpusTags first.
        assert!(packets.len() > 10);
        let mut o = OggOpus::new(2, 48_000);
        let mut out = o.head();
        for p in &packets[2..] {
            out.extend(o.packet(p));
        }
        let mut s = crate::audio::decoder::Source::open_stream(
            std::io::Cursor::new(out),
            "snapcast.opus",
            crate::audio::decoder::OpenOptions::default(),
        )
        .expect("FFmpeg reads the rewrapped stream");
        assert_eq!(s.info().codec.to_lowercase(), "opus");
        let mut frames = 0usize;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match s.decode_into(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => frames += n,
            }
        }
        // About two seconds of it.
        assert!(frames > 48_000, "{frames} frames");
    }

    /// A stand-in snapserver: takes the hello, sends a FLAC codec header
    /// and two chunks, then a new codec header.
    #[test]
    fn reads_header_and_chunks_as_one_stream() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut h = [0u8; HEADER];
            s.read_exact(&mut h).unwrap();
            assert_eq!(u16::from_le_bytes([h[0], h[1]]), HELLO);
            let n = u32::from_le_bytes(h[22..26].try_into().unwrap()) as usize;
            let mut body = vec![0u8; n];
            s.read_exact(&mut body).unwrap();
            let json = String::from_utf8_lossy(&body[4..]).into_owned();
            assert!(json.contains(ID_PREFIX), "{json}");
            let send = |s: &mut TcpStream, kind: u16, payload: Vec<u8>| {
                s.write_all(&header(kind, payload.len())).unwrap();
                s.write_all(&payload).unwrap();
            };
            let lp = |b: &[u8]| {
                let mut v = (b.len() as u32).to_le_bytes().to_vec();
                v.extend(b);
                v
            };
            let mut ch = lp(b"flac");
            ch.extend(lp(b"fLaC-head"));
            send(&mut s, CODEC_HEADER, ch.clone());
            for part in [&b"-one"[..], b"-two"] {
                let mut w = vec![0u8; 8];
                w.extend(lp(part));
                send(&mut s, WIRE_CHUNK, w);
            }
            send(&mut s, CODEC_HEADER, ch);
            std::thread::sleep(Duration::from_millis(200));
        });
        let abort = Arc::new(AtomicBool::new(false));
        // No stream named: no control API is asked.
        let (mut r, name) = open(&format!("snapcast://127.0.0.1:{port}"), abort).unwrap();
        assert_eq!(name, "snapcast.flac");
        let mut all = Vec::new();
        r.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"fLaC-head-one-two");
        srv.join().unwrap();
    }
}
