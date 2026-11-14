//! Snapcast: making sure the snapserver has somewhere for the sound to go.
//!
//! Patina sends raw PCM to a TCP source on the snapserver, in server mode
//! (the snapserver listens, Patina connects). That source can be written
//! into `snapserver.conf` by hand, but there's no need: before connecting,
//! Patina asks the snapserver over its JSON-RPC control API (TCP port
//! 1705, or HTTP on 1780 when that's closed) whether a TCP source is
//! listening on the port it's about to use, and adds one with
//! `Stream.AddStream` when there isn't. A source added that way lasts
//! until the snapserver restarts; Patina adds it again next time.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{SNAPCAST_PORT, Target};

/// The snapserver's JSON-RPC ports: raw TCP, and HTTP.
const RPC_TCP: u16 = 1705;
const RPC_HTTP: u16 = 1780;
/// What the stream is called in Snapweb and the Snapcast apps.
pub const STREAM_NAME: &str = "Patina";

/// How it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ready {
    /// A TCP source was already there, by this name.
    Had(String),
    /// Patina added one.
    Added,
    /// The control API didn't answer; whether a source is there is
    /// anyone's guess until the connection is tried.
    NoControl(String),
}

fn port_of(t: &Target) -> u16 {
    t.addr
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(SNAPCAST_PORT)
}

fn connect(host: &str, port: u16, wait: Duration) -> std::io::Result<TcpStream> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("can't find {host}")))?;
    TcpStream::connect_timeout(&addr, wait)
}

/// One JSON-RPC call: over raw TCP first, then HTTP.
fn rpc(host: &str, ports: (u16, u16), method: &str, params: Value) -> Result<Value, String> {
    let (rpc_tcp, rpc_http) = ports;
    let req = json!({ "id": 1, "jsonrpc": "2.0", "method": method, "params": params });
    let tcp = || -> Result<Value, String> {
        let mut s = connect(host, rpc_tcp, Duration::from_secs(2)).map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(4)));
        let mut line = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        line.push(b'\n');
        s.write_all(&line).map_err(|e| e.to_string())?;
        let mut r = BufReader::new(s);
        let deadline = Instant::now() + Duration::from_secs(4);
        // Notifications can come first: wait for the answer to id 1.
        while Instant::now() < deadline {
            let mut l = String::new();
            if r.read_line(&mut l).map_err(|e| e.to_string())? == 0 {
                break;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&l)
                && v.get("id") == Some(&json!(1))
            {
                return Ok(v);
            }
        }
        Err("no answer".into())
    };
    let http = || -> Result<Value, String> {
        let c = crate::net::client_builder()
            .timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| e.to_string())?;
        let body = c
            .post(format!("http://{host}:{rpc_http}/jsonrpc"))
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&req).map_err(|e| e.to_string())?)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    };
    let v = tcp().or_else(|e1| http().map_err(|e2| format!("{e1}; {e2}")))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("an error")
            .to_string();
        let data = err.get("data").and_then(Value::as_str).unwrap_or("");
        return Err(if data.is_empty() {
            msg
        } else {
            format!("{msg}: {data}")
        });
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| "an answer with no result".to_string())
}

/// The name of a TCP source in server mode on `port` among the
/// snapserver's streams, if there is one.
fn tcp_source(status: &Value, port: u16) -> Option<String> {
    let streams = status.pointer("/server/streams")?.as_array()?;
    streams.iter().find_map(|s| {
        let uri = s.get("uri")?;
        if uri.get("scheme")?.as_str()? != "tcp" {
            return None;
        }
        let q = uri.get("query");
        let mode = q
            .and_then(|q| q.get("mode"))
            .and_then(Value::as_str)
            .unwrap_or("server");
        if mode != "server" {
            return None;
        }
        // "host" is "0.0.0.0:4953", or no port at all for the default.
        let host = uri.get("host").and_then(Value::as_str).unwrap_or("");
        let p = host
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse::<u16>().ok())
            .unwrap_or(SNAPCAST_PORT);
        (p == port).then(|| {
            q.and_then(|q| q.get("name"))
                .and_then(Value::as_str)
                .or_else(|| s.get("id").and_then(Value::as_str))
                .unwrap_or("")
                .to_string()
        })
    })
}

/// Make sure the snapserver has a TCP source on the port Patina will use.
pub fn ensure_source(t: &Target) -> Ready {
    ensure_source_at(t, (RPC_TCP, RPC_HTTP))
}

fn ensure_source_at(t: &Target, ports: (u16, u16)) -> Ready {
    let host = t.host();
    let port = port_of(t);
    let status = match rpc(&host, ports, "Server.GetStatus", json!({})) {
        Ok(s) => s,
        Err(e) => return Ready::NoControl(e),
    };
    if let Some(name) = tcp_source(&status, port) {
        return Ready::Had(name);
    }
    let uri =
        format!("tcp://0.0.0.0:{port}?name={STREAM_NAME}&mode=server&sampleformat=48000:16:2");
    match rpc(
        &host,
        ports,
        "Stream.AddStream",
        json!({ "streamUri": uri }),
    ) {
        Ok(_) => {
            log::info!("cast: added {uri} to the snapserver at {host}");
            // A moment for it to start listening.
            std::thread::sleep(Duration::from_millis(300));
            Ready::Added
        }
        Err(e) => Ready::NoControl(format!("it wouldn't add a source: {e}")),
    }
}

/// Connect to the source, a few tries apart (a source just added can take
/// a moment to listen).
pub(super) fn open(t: &Target) -> Result<TcpStream, String> {
    let host = t.host();
    let port = port_of(t);
    let mut last = String::new();
    for i in 0..3 {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(400));
        }
        match connect(&host, port, Duration::from_secs(3)) {
            Ok(s) => return Ok(s),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Check a snapserver all the way, for when it's picked: its control API,
/// the TCP source (added if missing), and a connection to it. Says what's
/// ready, or what's wrong in words a person can act on.
pub fn check(t: &Target) -> Result<String, String> {
    let host = t.host();
    let port = port_of(t);
    let ready = ensure_source(t);
    match (open(t), ready) {
        (Ok(s), r) => {
            let _ = s.shutdown(std::net::Shutdown::Both);
            Ok(match r {
                Ready::Added => format!(
                    "{}: added a \"{STREAM_NAME}\" stream. Set your rooms to it in Snapweb or the Snapcast app.",
                    t.name
                ),
                Ready::Had(n) if !n.is_empty() => format!(
                    "{}: ready. Patina plays on its \"{n}\" stream; set your rooms to it in Snapweb or the Snapcast app.",
                    t.name
                ),
                _ => format!(
                    "{}: ready. Set your rooms to Patina's stream in Snapweb or the Snapcast app.",
                    t.name
                ),
            })
        }
        (Err(e), Ready::NoControl(why)) => Err(format!(
            "Can't reach the snapserver at {host}: nothing on port {port} ({e}), and its control API on {RPC_TCP}/{RPC_HTTP} didn't help ({why}). Check that snapserver is running there and that its firewall lets those ports in."
        )),
        (Err(e), _) => Err(format!(
            "The snapserver at {host} has a source on port {port} but won't take a connection to it ({e}). Its firewall may be letting the control port in but not {port}."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(raw: &[(&str, &str, Option<&str>)]) -> Value {
        let streams: Vec<Value> = raw
            .iter()
            .map(|(scheme, host, mode)| {
                let mut q = json!({ "name": format!("{scheme} on {host}") });
                if let Some(m) = mode {
                    q["mode"] = json!(m);
                }
                json!({ "id": "x", "uri": { "scheme": scheme, "host": host, "query": q } })
            })
            .collect();
        json!({ "server": { "streams": streams } })
    }

    #[test]
    fn a_listening_tcp_source_on_the_port_is_found() {
        let s = status(&[("pipe", "", None), ("tcp", "0.0.0.0:4953", Some("server"))]);
        assert_eq!(tcp_source(&s, 4953).as_deref(), Some("tcp on 0.0.0.0:4953"));
        assert_eq!(tcp_source(&s, 5000), None);
    }

    #[test]
    fn no_port_means_the_default_and_client_mode_doesnt_count() {
        let s = status(&[("tcp", "0.0.0.0", None)]);
        assert!(tcp_source(&s, 4953).is_some());
        let s = status(&[("tcp", "192.168.1.9:4953", Some("client"))]);
        assert_eq!(tcp_source(&s, 4953), None);
    }

    #[test]
    fn a_missing_source_is_added_over_the_control_api() {
        // A stand-in snapserver control port: no TCP source at first.
        let rpc = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let rpc_port = rpc.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut s, _) = rpc.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut l = String::new();
                r.read_line(&mut l).unwrap();
                let v: Value = serde_json::from_str(&l).unwrap();
                let m = v["method"].as_str().unwrap().to_string();
                let result = if m == "Server.GetStatus" {
                    json!({ "server": { "streams": [] } })
                } else {
                    json!({ "stream_id": STREAM_NAME })
                };
                // A notification first, as a real one may send.
                writeln!(s, "{}", json!({"jsonrpc":"2.0","method":"Server.OnUpdate"})).unwrap();
                writeln!(s, "{}", json!({"id":1,"jsonrpc":"2.0","result":result})).unwrap();
                seen.push((m, v["params"].clone()));
            }
            seen
        });
        let t = Target::snapcast("127.0.0.1:4953").unwrap();
        // HTTP on a port nothing listens on: the TCP answer has to do.
        assert_eq!(ensure_source_at(&t, (rpc_port, 1)), Ready::Added);
        let seen = h.join().unwrap();
        assert_eq!(seen[0].0, "Server.GetStatus");
        assert_eq!(seen[1].0, "Stream.AddStream");
        let uri = seen[1].1["streamUri"].as_str().unwrap();
        assert!(uri.starts_with("tcp://0.0.0.0:4953?"), "{uri}");
        assert!(uri.contains("mode=server"));
    }

    #[test]
    fn no_control_api_says_so() {
        let t = Target::snapcast("127.0.0.1:4953").unwrap();
        assert!(matches!(ensure_source_at(&t, (1, 1)), Ready::NoControl(_)));
    }
}
