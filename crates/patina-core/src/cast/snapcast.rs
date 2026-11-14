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
//!
//! The same control API picks which rooms hear it. Snapcast plays one
//! stream per group of clients, so putting one client on Patina takes it
//! out of its group first if it shares one (`Group.SetClients` with the
//! others: the one left out gets a group of its own), then sets that
//! group's stream (`Group.SetStream`). Taking it off sets it back to the
//! stream it was on before. Volume is per client (`Client.SetVolume`).

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

/// The id of Patina's stream (a TCP source on `port`), which is what
/// `Group.SetStream` takes.
fn tcp_stream_id(status: &Value, port: u16) -> Option<String> {
    let streams = status.pointer("/server/streams")?.as_array()?;
    let name = tcp_source(status, port)?;
    streams
        .iter()
        .find_map(|s| {
            let id = s.get("id")?.as_str()?;
            let q = s.pointer("/uri/query/name").and_then(Value::as_str);
            (id == name || q == Some(name.as_str())).then(|| id.to_string())
        })
        .or(Some(name))
}

/// One snapclient, as the snapserver reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapClient {
    pub id: String,
    /// The name set in Snapcast, else the machine's.
    pub name: String,
    pub connected: bool,
    pub group: String,
    /// The stream its group plays.
    pub stream: String,
    /// It's playing Patina's stream.
    pub on_patina: bool,
    pub volume: u8,
    pub muted: bool,
}

fn groups(status: &Value) -> Vec<Value> {
    status
        .pointer("/server/groups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn clients_of(status: &Value, patina: Option<&str>) -> Vec<SnapClient> {
    let mut out = Vec::new();
    for g in groups(status) {
        let gid = g
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let stream = g
            .get("stream_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        for c in g
            .get("clients")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = c.get("id").and_then(Value::as_str) else {
                continue;
            };
            let name = c
                .pointer("/config/name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .or_else(|| c.pointer("/host/name").and_then(Value::as_str))
                .unwrap_or(id)
                .to_string();
            out.push(SnapClient {
                id: id.to_string(),
                name,
                connected: c.get("connected").and_then(Value::as_bool).unwrap_or(false),
                group: gid.clone(),
                stream: stream.clone(),
                on_patina: patina.is_some_and(|p| p == stream),
                volume: c
                    .pointer("/config/volume/percent")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .min(100) as u8,
                muted: c
                    .pointer("/config/volume/muted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
    }
    out.sort_by_key(|c| (!c.connected, c.name.to_lowercase()));
    out
}

/// Every client the snapserver knows, connected or not.
pub fn clients(t: &Target) -> Result<Vec<SnapClient>, String> {
    clients_at(t, (RPC_TCP, RPC_HTTP))
}

fn clients_at(t: &Target, ports: (u16, u16)) -> Result<Vec<SnapClient>, String> {
    let status = rpc(&t.host(), ports, "Server.GetStatus", json!({}))?;
    let patina = tcp_stream_id(&status, port_of(t));
    Ok(clients_of(&status, patina.as_deref()))
}

/// Put one client on Patina's stream, or take it off. Off sets it back to
/// `back_to` (the stream it was on before), else to the first other
/// stream. Returns the stream it was on, for taking it off later.
pub fn route(t: &Target, client: &str, on: bool, back_to: Option<&str>) -> Result<String, String> {
    route_at(t, (RPC_TCP, RPC_HTTP), client, on, back_to)
}

fn route_at(
    t: &Target,
    ports: (u16, u16),
    client: &str,
    on: bool,
    back_to: Option<&str>,
) -> Result<String, String> {
    let host = t.host();
    if on && let Ready::NoControl(e) = ensure_source_at(t, ports) {
        return Err(format!("the snapserver's control API didn't answer: {e}"));
    }
    let status = rpc(&host, ports, "Server.GetStatus", json!({}))?;
    let patina = tcp_stream_id(&status, port_of(t))
        .ok_or_else(|| "the snapserver has no Patina stream yet".to_string())?;
    let find = |st: &Value| -> Option<(String, String, Vec<String>)> {
        groups(st).into_iter().find_map(|g| {
            let ids: Vec<String> = g
                .get("clients")?
                .as_array()?
                .iter()
                .filter_map(|c| c.get("id")?.as_str().map(str::to_string))
                .collect();
            ids.iter().any(|i| i == client).then(|| {
                (
                    g.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    g.get("stream_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    ids.into_iter().filter(|i| i != client).collect(),
                )
            })
        })
    };
    let (group, current, others) =
        find(&status).ok_or_else(|| format!("the snapserver doesn't know {client}"))?;
    let want = if on {
        patina.clone()
    } else {
        let streams: Vec<String> = status
            .pointer("/server/streams")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("id")?.as_str().map(str::to_string))
            .collect();
        back_to
            .filter(|b| *b != patina && streams.iter().any(|s| s == b))
            .map(str::to_string)
            .or_else(|| streams.into_iter().find(|s| *s != patina))
            .ok_or_else(|| "the snapserver has no other stream to go back to".to_string())?
    };
    if current == want {
        return Ok(current);
    }
    // Sharing a group: the others stay where they are, and this one gets
    // a group of its own.
    let group = if others.is_empty() {
        group
    } else {
        let after = rpc(
            &host,
            ports,
            "Group.SetClients",
            json!({ "id": group, "clients": others }),
        )?;
        let after = if after.pointer("/server/groups").is_some() {
            after
        } else {
            rpc(&host, ports, "Server.GetStatus", json!({}))?
        };
        find(&after)
            .map(|(g, _, _)| g)
            .ok_or_else(|| format!("{client} went missing from the snapserver"))?
    };
    rpc(
        &host,
        ports,
        "Group.SetStream",
        json!({ "id": group, "stream_id": want }),
    )?;
    Ok(current)
}

/// A client's volume, 0 to 100, and whether it's muted.
pub fn set_volume(t: &Target, client: &str, percent: u8, muted: bool) -> Result<(), String> {
    rpc(
        &t.host(),
        (RPC_TCP, RPC_HTTP),
        "Client.SetVolume",
        json!({ "id": client, "volume": { "muted": muted, "percent": percent.min(100) } }),
    )
    .map(|_| ())
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

    /// A stand-in control port that answers each call with `answer`.
    fn fake_rpc(
        calls: usize,
        answer: impl Fn(&str, &Value) -> Value + Send + 'static,
    ) -> (u16, std::thread::JoinHandle<Vec<(String, Value)>>) {
        let rpc = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = rpc.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..calls {
                let (mut s, _) = rpc.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut l = String::new();
                r.read_line(&mut l).unwrap();
                let v: Value = serde_json::from_str(&l).unwrap();
                let m = v["method"].as_str().unwrap().to_string();
                let result = answer(&m, &v["params"]);
                writeln!(s, "{}", json!({"id":1,"jsonrpc":"2.0","result":result})).unwrap();
                seen.push((m, v["params"].clone()));
            }
            seen
        });
        (port, h)
    }

    fn two_rooms(split: bool) -> Value {
        let client = |id: &str, name: &str| json!({"id": id, "connected": true, "config": {"name": name, "volume": {"percent": 60, "muted": false}}, "host": {"name": "box"}});
        let groups = if split {
            json!([
                {"id": "g1", "stream_id": "radio", "clients": [client("kitchen", "Kitchen")]},
                {"id": "g2", "stream_id": "radio", "clients": [client("den", "Den")]}
            ])
        } else {
            json!([{"id": "g1", "stream_id": "radio", "clients": [client("kitchen", "Kitchen"), client("den", "Den")]}])
        };
        json!({"server": {
            "groups": groups,
            "streams": [
                {"id": "radio", "uri": {"scheme": "pipe", "host": "", "query": {"name": "radio"}}},
                {"id": "Patina", "uri": {"scheme": "tcp", "host": "0.0.0.0:4953", "query": {"name": "Patina", "mode": "server"}}}
            ]
        }})
    }

    #[test]
    fn clients_are_listed_with_their_stream() {
        let c = clients_of(&two_rooms(true), Some("Patina"));
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "Den");
        assert_eq!(c[1].volume, 60);
        assert!(!c[0].on_patina);
    }

    #[test]
    fn one_client_of_a_shared_group_is_split_off_and_put_on_patina() {
        // GetStatus (ensure), GetStatus, SetClients, SetStream.
        let (port, h) = fake_rpc(4, |m, _| match m {
            "Group.SetClients" => two_rooms(true),
            "Group.SetStream" => json!({"stream_id": "Patina"}),
            _ => two_rooms(false),
        });
        let t = Target::snapcast("127.0.0.1:4953").unwrap();
        let was = route_at(&t, (port, 1), "den", true, None).unwrap();
        assert_eq!(was, "radio");
        let seen = h.join().unwrap();
        assert_eq!(seen[2].0, "Group.SetClients");
        assert_eq!(seen[2].1, json!({"id": "g1", "clients": ["kitchen"]}));
        assert_eq!(seen[3].0, "Group.SetStream");
        assert_eq!(seen[3].1, json!({"id": "g2", "stream_id": "Patina"}));
    }

    #[test]
    fn taking_a_client_off_puts_it_back() {
        let (port, h) = fake_rpc(2, |m, _| {
            if m == "Server.GetStatus" {
                let mut s = two_rooms(true);
                s["server"]["groups"][1]["stream_id"] = json!("Patina");
                s
            } else {
                json!({"stream_id": "radio"})
            }
        });
        let t = Target::snapcast("127.0.0.1:4953").unwrap();
        route_at(&t, (port, 1), "den", false, Some("radio")).unwrap();
        let seen = h.join().unwrap();
        assert_eq!(seen[1].1, json!({"id": "g2", "stream_id": "radio"}));
    }

    #[test]
    fn no_control_api_says_so() {
        let t = Target::snapcast("127.0.0.1:4953").unwrap();
        assert!(matches!(ensure_source_at(&t, (1, 1)), Ready::NoControl(_)));
    }
}
