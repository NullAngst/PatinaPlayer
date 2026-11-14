//! The live stream a Chromecast or a DLNA renderer plays: one endless WAV
//! file over HTTP, from a server of its own on a free port.
//!
//! Hand-rolled rather than a server library: the whole job is one URL,
//! and renderers are fussy about the answer. Many don't take chunked
//! transfer, so the body is plain bytes until the connection closes, and
//! DLNA ones want their own headers saying it's a stream.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

use super::RATE;

/// Blocks a listener can fall behind by before it misses some: two
/// seconds of 20 ms blocks.
const BACKLOG: usize = 100;

/// A block of the stream, shared by every listener.
type Block = Arc<[u8]>;
/// Who's listening.
type Listeners = Arc<Mutex<Vec<Sender<Block>>>>;

pub(super) struct Broadcast {
    clients: Listeners,
    stop: Arc<AtomicBool>,
}

impl Broadcast {
    /// Listen on a free port on every address. Gives the port.
    pub(super) fn start(stop: Arc<AtomicBool>) -> std::io::Result<(Broadcast, u16)> {
        let listener = TcpListener::bind("0.0.0.0:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let clients: Listeners = Arc::default();
        let (c2, s2) = (clients.clone(), stop.clone());
        std::thread::Builder::new()
            .name("patina-cast-http".into())
            .spawn(move || {
                while !s2.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((sock, peer)) => {
                            log::info!("cast: {peer} connected for the stream");
                            let (c3, s3) = (c2.clone(), s2.clone());
                            let _ = std::thread::Builder::new()
                                .name("patina-cast-client".into())
                                .spawn(move || serve(sock, &c3, &s3));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(e) => {
                            log::warn!("cast: accept failed: {e}");
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
            })?;
        Ok((Broadcast { clients, stop }, port))
    }

    /// Hand a block to everyone listening. A listener too far behind
    /// misses it rather than holding the others up.
    pub(super) fn send(&self, pcm: &[u8]) {
        let block: Block = Arc::from(pcm);
        self.clients.lock().unwrap().retain(|c| {
            !matches!(
                c.try_send(block.clone()),
                Err(TrySendError::Disconnected(_))
            )
        });
    }

    pub(super) fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // Dropping the senders ends each client's loop.
        self.clients.lock().unwrap().clear();
    }
}

/// The WAV header for a stream of unknown length: sizes at their largest,
/// which players take as "until it ends".
pub(super) fn wav_header() -> [u8; 44] {
    let mut h = [0u8; 44];
    let ch: u16 = 2;
    let bits: u16 = 16;
    let block = ch * bits / 8;
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&ch.to_le_bytes());
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * block as u32).to_le_bytes());
    h[32..34].copy_from_slice(&block.to_le_bytes());
    h[34..36].copy_from_slice(&bits.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&(u32::MAX - 36).to_le_bytes());
    h
}

/// The request line and headers, up to the blank line.
fn read_request(sock: &TcpStream) -> Option<(String, String)> {
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new(sock);
    let mut first = String::new();
    r.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    // The rest of the headers: nothing in them changes the answer.
    for _ in 0..100 {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
    }
    Some((method, path))
}

fn serve(mut sock: TcpStream, clients: &Mutex<Vec<Sender<Block>>>, stop: &AtomicBool) {
    let Some((method, path)) = read_request(&sock) else {
        return;
    };
    if !path.starts_with("/stream.wav") || !(method == "GET" || method == "HEAD") {
        let _ = sock
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: audio/wav\r\n\
                Cache-Control: no-cache\r\n\
                Connection: close\r\n\
                Accept-Ranges: none\r\n\
                transferMode.dlna.org: Streaming\r\n\
                contentFeatures.dlna.org: DLNA.ORG_OP=00;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000\r\n\
                Server: PatinaPlayer\r\n\r\n";
    if sock.write_all(head.as_bytes()).is_err() || method == "HEAD" {
        return;
    }
    let _ = sock.set_write_timeout(Some(Duration::from_secs(10)));
    let _ = sock.set_nodelay(true);
    if sock.write_all(&wav_header()).is_err() {
        return;
    }
    let (tx, rx): (Sender<Block>, Receiver<Block>) = bounded(BACKLOG);
    clients.lock().unwrap().push(tx);
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(b) => {
                if sock.write_all(&b).is_err() {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    let _ = sock.shutdown(std::net::Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn the_header_says_48k_stereo_16_bit() {
        let h = wav_header();
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2);
        assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), 48_000);
        assert_eq!(u32::from_le_bytes([h[28], h[29], h[30], h[31]]), 192_000);
        assert_eq!(u16::from_le_bytes([h[34], h[35]]), 16);
    }

    #[test]
    fn a_listener_gets_the_header_then_the_blocks() {
        let stop = Arc::new(AtomicBool::new(false));
        let (b, port) = Broadcast::start(stop.clone()).unwrap();
        let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        sock.write_all(b"GET /stream.wav HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        // Wait for it to sign up, sending as the output would.
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            b.send(&[1, 2, 3, 4]);
            std::thread::sleep(Duration::from_millis(20));
            sock.set_nonblocking(true).unwrap();
            while let Ok(n) = sock.read(&mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            sock.set_nonblocking(false).unwrap();
            if got.windows(4).any(|w| w == [1, 2, 3, 4]) {
                break;
            }
        }
        let text = String::from_utf8_lossy(&got);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("Content-Type: audio/wav"));
        let body = got
            .windows(4)
            .position(|w| w == b"RIFF")
            .expect("a WAV header");
        assert!(got[body + 44..].starts_with(&[1, 2, 3, 4]));
        b.close();
    }

    #[test]
    fn anything_else_is_not_found() {
        let stop = Arc::new(AtomicBool::new(false));
        let (b, port) = Broadcast::start(stop).unwrap();
        let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        sock.write_all(b"GET /etc/passwd HTTP/1.1\r\n\r\n").unwrap();
        let mut s = String::new();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = sock.read_to_string(&mut s);
        assert!(s.starts_with("HTTP/1.1 404"), "{s}");
        b.close();
    }
}
