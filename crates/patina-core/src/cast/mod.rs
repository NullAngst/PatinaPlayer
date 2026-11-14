//! Playing to another device: a Chromecast, a DLNA renderer, or a
//! Snapcast server.
//!
//! The sound still comes out of this app's own player, after the EQ, the
//! amp and the leveling, so everything keeps working as it does on the
//! speakers here: the queue, gapless, crossfades. Instead of a sound card
//! the player's output is a [`CastOutput`], which sends 16-bit 48 kHz
//! stereo PCM on:
//!
//! - Chromecast and DLNA renderers fetch it from a small HTTP server here
//!   as one endless WAV stream ([`stream`]). The device is told where to
//!   find it over its own protocol ([`chromecast`], [`dlna`]).
//! - Snapcast takes it over TCP: a snapserver with a TCP source in server
//!   mode (`source = tcp://0.0.0.0:4953?name=Patina&mode=server`), the
//!   format left at its default 48000:16:2.
//!
//! Chromecasts and DLNA renderers buffer a few seconds before they play,
//! so pause and skip take that long to be heard. Snapcast's delay is its
//! own buffer setting, a second by default.
//!
//! The network side, a [`Session`], outlives any one output: the player
//! reopens its output when a song at a different channel count comes up,
//! and the device mustn't have to reconnect every time.

pub mod chromecast;
pub mod dlna;
mod stream;

use std::net::{IpAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::output::{Dither, Output, OutputError};
use crate::audio::render::Renderer;

/// What goes out: the rate every receiver takes.
pub const RATE: u32 = 48_000;
/// Frames per block, 20 ms.
const BLOCK: usize = 960;
/// Where a snapserver's TCP source listens unless told otherwise.
pub const SNAPCAST_PORT: u16 = 4953;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Chromecast,
    Dlna,
    Snapcast,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Chromecast => "Chromecast",
            Kind::Dlna => "DLNA",
            Kind::Snapcast => "Snapcast",
        }
    }
}

/// A device to play on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub kind: Kind,
    pub name: String,
    /// "host:port" for a Chromecast or a snapserver; the device
    /// description URL for a DLNA renderer.
    pub addr: String,
}

impl Target {
    /// A snapserver typed in by hand: "host" or "host:port".
    pub fn snapcast(addr: &str) -> Option<Target> {
        let addr = addr.trim();
        if addr.is_empty() {
            return None;
        }
        let full = if addr
            .rsplit_once(':')
            .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        {
            addr.to_string()
        } else {
            format!("{addr}:{SNAPCAST_PORT}")
        };
        Some(Target {
            kind: Kind::Snapcast,
            name: format!("Snapcast on {}", host_of(&full)),
            addr: full,
        })
    }

    /// One line for a picker.
    pub fn label(&self) -> String {
        format!("{}  ·  {}", self.name, self.kind.label())
    }

    /// The host part, to reach it and to find our own address toward it.
    pub fn host(&self) -> String {
        match self.kind {
            Kind::Dlna => url::Url::parse(&self.addr)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default(),
            _ => host_of(&self.addr),
        }
    }
}

fn host_of(addr: &str) -> String {
    match addr.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => h.trim_matches(['[', ']']).to_string(),
        _ => addr.to_string(),
    }
}

/// Look for Chromecasts, DLNA renderers and snapservers on the local
/// network for `wait`, all three at once.
pub fn discover(wait: Duration) -> Vec<Target> {
    let cc = std::thread::spawn(move || {
        mdns_browse("_googlecast._tcp.local.", wait, |s| {
            let ip = s.get_addresses_v4().into_iter().next()?;
            let name = s
                .get_property_val_str("fn")
                .map(str::to_string)
                .unwrap_or_else(|| s.host.trim_end_matches(".local.").to_string());
            Some(Target {
                kind: Kind::Chromecast,
                name,
                addr: format!("{ip}:{}", s.port),
            })
        })
    });
    let snap = std::thread::spawn(move || {
        mdns_browse("_snapcast._tcp.local.", wait, |s| {
            let ip = s.get_addresses_v4().into_iter().next()?;
            let host = s.host.trim_end_matches('.').trim_end_matches(".local");
            Some(Target {
                kind: Kind::Snapcast,
                name: format!("Snapcast on {host}"),
                addr: format!("{ip}:{SNAPCAST_PORT}"),
            })
        })
    });
    let mut out = dlna::discover(wait);
    for h in [cc, snap] {
        if let Ok(v) = h.join() {
            out.extend(v);
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert((t.kind, t.addr.clone())));
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

fn mdns_browse(
    ty: &str,
    wait: Duration,
    f: impl Fn(&mdns_sd::ResolvedService) -> Option<Target>,
) -> Vec<Target> {
    let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
        return Vec::new();
    };
    let Ok(rx) = daemon.browse(ty) else {
        let _ = daemon.shutdown();
        return Vec::new();
    };
    let deadline = Instant::now() + wait;
    let mut out = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(s)) => {
                if let Some(t) = f(&s) {
                    out.push(t);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(ty);
    let _ = daemon.shutdown();
    out
}

/// This machine's address as seen from `host`: the one the device can
/// fetch the stream from. Nothing is sent; connecting a UDP socket only
/// picks the route.
fn local_ip_toward(host: &str) -> Option<IpAddr> {
    let peer = (host, 9).to_socket_addrs().ok()?.next()?;
    let sock = UdpSocket::bind(if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .ok()?;
    sock.connect(peer).ok()?;
    Some(sock.local_addr().ok()?.ip())
}

// ----- The session -----

/// The connection to one device, kept across output reopens.
pub struct Session {
    target: Target,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    sink: Sink,
    /// When PCM last went out, so silence can fill a gap between outputs.
    last: Mutex<Instant>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

enum Sink {
    Http(stream::Broadcast),
    Tcp(Mutex<Option<TcpStream>>),
}

static SESSION: Mutex<Option<Arc<Session>>> = Mutex::new(None);

impl Session {
    fn start(target: &Target, title: &str) -> Result<Arc<Session>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let host = target.host();
        let mut threads = Vec::new();
        let sink = match target.kind {
            Kind::Snapcast => {
                let addr = target
                    .addr
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut a| a.next())
                    .ok_or_else(|| format!("can't find {}", target.addr))?;
                let s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| {
                    format!(
                        "can't reach the snapserver at {} ({e}). Is its TCP source on, in server mode?",
                        target.addr
                    )
                })?;
                let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                let _ = s.set_nodelay(true);
                Sink::Tcp(Mutex::new(Some(s)))
            }
            Kind::Chromecast | Kind::Dlna => {
                let ip = local_ip_toward(&host)
                    .ok_or_else(|| format!("no route to {host} from here"))?;
                let (bc, port) = stream::Broadcast::start(stop.clone())
                    .map_err(|e| format!("can't serve the stream: {e}"))?;
                let url = match ip {
                    IpAddr::V4(v4) => format!("http://{v4}:{port}/stream.wav"),
                    IpAddr::V6(v6) => format!("http://[{v6}]:{port}/stream.wav"),
                };
                log::info!("cast: serving {url} for {}", target.name);
                let (stop2, err2, t, title) = (
                    stop.clone(),
                    error.clone(),
                    target.clone(),
                    title.to_string(),
                );
                threads.push(
                    std::thread::Builder::new()
                        .name("patina-cast-ctl".into())
                        .spawn(move || {
                            let r = match t.kind {
                                Kind::Chromecast => chromecast::run(&t, &url, &title, &stop2),
                                _ => dlna::run(&t, &url, &title, &stop2),
                            };
                            if let Err(e) = r
                                && !stop2.load(Ordering::Relaxed)
                            {
                                log::warn!("cast: {}: {e}", t.name);
                                *err2.lock().unwrap() = Some(format!("{}: {e}", t.name));
                            }
                        })
                        .map_err(|e| e.to_string())?,
                );
                Sink::Http(bc)
            }
        };
        let s = Arc::new(Session {
            target: target.clone(),
            stop,
            error,
            sink,
            last: Mutex::new(Instant::now()),
            threads: Mutex::new(Vec::new()),
        });
        // Between one output closing and the next opening, the device
        // gets silence rather than nothing, which some take for the end.
        let weak = Arc::downgrade(&s);
        threads.push(
            std::thread::Builder::new()
                .name("patina-cast-fill".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(Duration::from_millis(50));
                        let Some(s) = weak.upgrade() else { return };
                        if s.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let gap = s.last.lock().unwrap().elapsed();
                        if gap > Duration::from_millis(150) {
                            let frames = (RATE as u128 * gap.as_millis().min(200) / 1000) as usize;
                            s.push(&vec![0u8; frames * 4]);
                        }
                    }
                })
                .map_err(|e| e.to_string())?,
        );
        *s.threads.lock().unwrap() = threads;
        Ok(s)
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// 16-bit little-endian stereo frames, as they're due.
    fn push(&self, pcm: &[u8]) {
        *self.last.lock().unwrap() = Instant::now();
        match &self.sink {
            Sink::Http(b) => b.send(pcm),
            Sink::Tcp(s) => {
                use std::io::Write;
                let mut g = s.lock().unwrap();
                if let Some(sock) = g.as_mut()
                    && let Err(e) = sock.write_all(pcm)
                {
                    *g = None;
                    *self.error.lock().unwrap() = Some(format!(
                        "{}: the snapserver went away ({e})",
                        self.target.name
                    ));
                }
            }
        }
    }

    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    fn end(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Sink::Tcp(s) = &self.sink
            && let Some(sock) = s.lock().unwrap().take()
        {
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
        if let Sink::Http(b) = &self.sink {
            b.close();
        }
        // The controller may be waiting on the device for a few seconds:
        // let it finish on its own.
        self.threads.lock().unwrap().clear();
    }
}

/// The session for `target`, started if it isn't running. Ends any
/// session with another device.
fn session_for(target: &Target, title: &str) -> Result<Arc<Session>, String> {
    let mut g = SESSION.lock().unwrap();
    if let Some(s) = g.as_ref() {
        if s.target == *target && !s.stop.load(Ordering::Relaxed) {
            return Ok(s.clone());
        }
        s.end();
        *g = None;
    }
    let s = Session::start(target, title)?;
    *g = Some(s.clone());
    Ok(s)
}

/// Stop playing on the device, if anything is.
pub fn end() {
    if let Some(s) = SESSION.lock().unwrap().take() {
        log::info!("cast: done with {}", s.target.name);
        s.end();
    }
}

// ----- The output -----

/// The player's output while it plays on another device.
pub struct CastOutput {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    session: Arc<Session>,
}

impl CastOutput {
    /// `title` is what a Chromecast shows while it plays, when the session
    /// starts here.
    pub fn start(
        target: &Target,
        title: &str,
        mut renderer: Renderer,
    ) -> Result<Self, OutputError> {
        let fmt = renderer.format();
        if fmt.rate != RATE {
            return Err(OutputError::Rate {
                device: target.name.clone(),
                want: fmt.rate,
                got: RATE,
            });
        }
        let session = session_for(target, title).map_err(OutputError::Cast)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (stop2, sess) = (stop.clone(), session.clone());
        let thread = std::thread::Builder::new()
            .name("patina-cast-out".into())
            .spawn(move || {
                let ch = fmt.channels.max(1) as usize;
                let block_time = Duration::from_secs_f64(BLOCK as f64 / RATE as f64);
                let mut dither = Dither::default();
                let mut bytes = Vec::with_capacity(BLOCK * 4);
                let mut next = Instant::now();
                while !stop2.load(Ordering::Relaxed) {
                    let block = renderer.render(BLOCK);
                    bytes.clear();
                    for f in block.chunks_exact(ch) {
                        // Mono to both sides; past two channels, the
                        // front left and right.
                        let (l, r) = (f[0], if ch > 1 { f[1] } else { f[0] });
                        for s in [l, r] {
                            let v = dither.to_int(s, 16) as i16;
                            bytes.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    sess.push(&bytes);
                    next += block_time;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else if now - next > Duration::from_millis(200) {
                        // Fell well behind (the machine slept): carry on
                        // from now rather than racing to catch up.
                        next = now;
                    }
                }
            })
            .expect("spawn cast output thread");
        Ok(Self {
            stop,
            thread: Some(thread),
            session,
        })
    }
}

impl Output for CastOutput {
    fn describe(&self) -> String {
        format!(
            "{} ({})",
            self.session.target.name,
            self.session.target.kind.label()
        )
    }
    fn error(&self) -> Option<String> {
        self.session.error()
    }
    fn device(&self) -> Option<crate::eq::OutputDevice> {
        let t = &self.session.target;
        Some(crate::eq::OutputDevice {
            key: format!("cast:{}:{}", t.kind.label().to_lowercase(), t.name),
            name: t.name.clone(),
        })
    }
}

impl Drop for CastOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_snapserver_gets_the_default_port() {
        let t = Target::snapcast("music-box").unwrap();
        assert_eq!(t.addr, "music-box:4953");
        assert_eq!(t.host(), "music-box");
        let t = Target::snapcast("10.0.0.5:5000").unwrap();
        assert_eq!(t.addr, "10.0.0.5:5000");
        assert_eq!(t.name, "Snapcast on 10.0.0.5");
        assert!(Target::snapcast("  ").is_none());
    }

    #[test]
    fn a_dlna_renderer_is_reached_at_its_description_host() {
        let t = Target {
            kind: Kind::Dlna,
            name: "Kitchen".into(),
            addr: "http://192.168.1.40:49152/description.xml".into(),
        };
        assert_eq!(t.host(), "192.168.1.40");
    }

    #[test]
    fn our_own_address_toward_this_machine_is_loopback() {
        assert!(local_ip_toward("127.0.0.1").is_some_and(|ip| ip.is_loopback()));
    }
}
