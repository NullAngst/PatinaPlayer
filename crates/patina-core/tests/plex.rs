//! Plex and Icecast against stand-ins: a plex.tv for signing in (PINs,
//! JWTs signed with the device key, refreshing, the server list) and a
//! Plex Media Server answering the documented calls in JSON. Every request
//! is checked for what the docs ask of it: the X-Plex headers, the token,
//! PUT for ratings, POST for the timeline, both paging headers.

mod common;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use common::{Request, Response, serve};
use patina_core::library::Library;
use patina_core::remote::plex::Plex;
use patina_core::remote::plex_tv::{self, Login};
use patina_core::remote::{
    self, Client, RemoteError, ServerConfig, ServerKind, StreamFormat, sync,
};
use serde_json::{Value, json};

const MACHINE: &str = "MACHINE1";
const SERVER_TOKEN: &str = "SERVERTOKEN";

/// A request as seen: method, path, query.
type Call = (String, String, Vec<(String, String)>);

#[derive(Default)]
struct Seen {
    calls: Vec<Call>,
    /// Checks so far, by PIN id.
    pin_checks: std::collections::HashMap<i64, usize>,
    /// The token the server takes; others get a 401.
    good_tokens: Vec<String>,
    /// Each device key's public half, by client id, from its PIN.
    keys: std::collections::HashMap<String, Vec<u8>>,
    next_pin: i64,
}

fn jwt(exp: i64) -> String {
    format!(
        "{}.{}.sig",
        B64.encode(r#"{"alg":"EdDSA"}"#),
        B64.encode(json!({ "exp": exp }).to_string())
    )
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Check a device JWT's signature with the key from the PIN.
fn verify(seen: &Seen, client: &str, token: &str) -> Value {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "a JWT");
    let key = seen
        .keys
        .get(client)
        .cloned()
        .expect("a key came with the PIN");
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &B64.decode(parts[2]).unwrap(),
        )
        .expect("signed with the device key");
    let claims: Value = serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(claims["aud"], "plex.tv");
    assert_eq!(claims["iss"].as_str(), Some(client), "iss is the client id");
    claims
}

fn track(id: &str, n: u32, detail: bool) -> Value {
    let mut stream = vec![json!({"streamType": 2, "codec": "flac", "channels": 2,
        "samplingRate": 96000, "bitDepth": 24, "gain": -7.5, "peak": 0.98,
        "albumGain": -8.0, "albumPeak": 0.99})];
    if id == "101" {
        stream.push(
            json!({"streamType": 4, "key": "/library/streams/9", "format": "lrc", "timed": true}),
        );
    }
    let mut part = json!({"key": format!("/library/parts/{id}0/1700000000/file.flac"),
        "file": format!("/data/music/Band/Album/{n:02} Song {n}.flac"),
        "container": "flac", "size": 1234});
    if detail {
        part["Stream"] = json!(stream);
    }
    json!({"ratingKey": id, "key": format!("/library/metadata/{id}"), "type": "track",
        "title": format!("Song {n}"), "grandparentTitle": "Band", "parentTitle": "Album",
        "parentRatingKey": "10", "grandparentRatingKey": "5", "index": n, "parentIndex": 1,
        "librarySectionID": 1, "duration": 200000, "userRating": 8, "viewCount": 3,
        "parentThumb": "/library/metadata/10/thumb/1700000000",
        "Media": [{"bitrate": 4000, "audioChannels": 2, "container": "flac", "Part": [part]}]})
}

fn plex(seen: Arc<Mutex<Seen>>) -> String {
    serve(Arc::new(move |r: &Request| {
        let mut s = seen.lock().unwrap_or_else(|e| e.into_inner());
        s.calls
            .push((r.method.clone(), r.path.clone(), r.query.clone()));
        // ----- plex.tv -----
        if let Some(p) = r.path.strip_prefix("/api/v2") {
            let client = r
                .headers
                .get("x-plex-client-identifier")
                .cloned()
                .unwrap_or_else(|| panic!("plex.tv needs the client id on {p}"));
            assert_eq!(
                r.headers.get("x-plex-product").map(String::as_str),
                Some("PatinaPlayer")
            );
            return match (r.method.as_str(), p) {
                ("POST", "/pins") => {
                    let body: Value = serde_json::from_slice(&r.body).unwrap();
                    let jwk = &body["jwk"];
                    assert_eq!(jwk["kty"], "OKP");
                    assert_eq!(jwk["crv"], "Ed25519");
                    assert_eq!(jwk["alg"], "EdDSA");
                    assert!(jwk["kid"].as_str().is_some_and(|k| !k.is_empty()));
                    let x = B64.decode(jwk["x"].as_str().unwrap()).unwrap();
                    s.keys.insert(client, x);
                    s.next_pin += 1;
                    Response::json(json!({"id": 40 + s.next_pin, "code": "ABCD", "expiresIn": 900}))
                }
                ("GET", p) if p.starts_with("/pins/") => {
                    let id: i64 = p["/pins/".len()..].parse().unwrap();
                    let device = r
                        .get("deviceJWT")
                        .expect("the PIN poll carries a device JWT");
                    verify(&s, &client, device);
                    let n = s.pin_checks.entry(id).or_default();
                    *n += 1;
                    let token = (*n > 1).then(|| jwt(now() + 7 * 86400));
                    Response::json(json!({"id": id, "code": "ABCD", "authToken": token}))
                }
                ("GET", "/auth/nonce") => Response::json(json!({"nonce": "n-1"})),
                ("POST", "/auth/token") => {
                    let body: Value = serde_json::from_slice(&r.body).unwrap();
                    let claims = verify(&s, &client, body["jwt"].as_str().unwrap());
                    assert_eq!(claims["nonce"], "n-1");
                    Response::json(json!({"auth_token": jwt(now() + 7 * 86400)}))
                }
                ("GET", "/resources") => {
                    assert_eq!(r.get("includeHttps"), Some("1"));
                    let base = format!("http://{}", r.headers["host"]);
                    Response::json(json!([
                        {"name": "Phone", "clientIdentifier": "PHONE", "provides": "player", "accessToken": "x"},
                        {"name": "Den", "clientIdentifier": MACHINE, "provides": "server",
                         "owned": true, "accessToken": SERVER_TOKEN,
                         "connections": [
                            {"protocol": "http", "address": "10.9.9.9", "port": 1, "uri": "http://10.9.9.9:1", "local": false, "relay": true, "IPv6": false},
                            {"protocol": "http", "address": "127.0.0.1", "port": 0, "uri": base, "local": true, "relay": false, "IPv6": false}
                         ]}
                    ]))
                }
                ("GET", "/user") => Response::json(json!({"username": "zack"})),
                _ => Response::not_found(),
            };
        }
        // ----- The server -----
        if r.path == "/identity" {
            return Response::json(
                json!({"MediaContainer": {"machineIdentifier": MACHINE, "version": "1.43.2.1"}}),
            );
        }
        let token = r
            .headers
            .get("x-plex-token")
            .cloned()
            .or_else(|| r.get("X-Plex-Token").map(str::to_string))
            .unwrap_or_default();
        if !s.good_tokens.contains(&token) {
            return Response {
                status: 401,
                ctype: "text/html".into(),
                body: b"Unauthorized".to_vec(),
            };
        }
        let api = r.headers.get("x-plex-pms-api-version").cloned();
        if !r.path.starts_with("/library/parts") && !r.path.starts_with("/library/streams") {
            assert!(api.is_some(), "{} without X-Plex-Pms-Api-Version", r.path);
        }
        let mc = |v: Value| Response::json(json!({ "MediaContainer": v }));
        match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/") => mc(
                json!({"machineIdentifier": MACHINE, "version": "1.43.2.1-abc", "myPlexSubscription": true}),
            ),
            ("GET", "/library/sections/all") => mc(json!({"Directory": [
                {"key": "1", "type": "artist", "title": "Music", "Location": [{"path": "/data/music"}]},
                {"key": "2", "type": "movie", "title": "Films", "Location": [{"path": "/data/films"}]}
            ]})),
            ("GET", "/library/sections/1/all") => {
                // Both paging headers, as the docs ask.
                assert!(r.headers.contains_key("x-plex-container-start"));
                assert!(r.headers.contains_key("x-plex-container-size"));
                match r.get("type") {
                    Some("8") => mc(json!({"totalSize": 1, "Metadata": [
                        {"ratingKey": "5", "title": "Band", "thumb": "/library/metadata/5/thumb/1"}]})),
                    Some("9") => mc(json!({"totalSize": 1, "Metadata": [
                        {"ratingKey": "10", "title": "Album", "parentTitle": "Band", "year": 1999,
                         "leafCount": 2, "updatedAt": 1700000000, "userRating": 6,
                         "Genre": [{"tag": "Grunge"}], "thumb": "/library/metadata/10/thumb/1"}]})),
                    _ => mc(json!({})),
                }
            }
            ("GET", "/library/metadata/10/children") => mc(
                json!({"totalSize": 2, "Metadata": [track("101", 1, false), track("102", 2, false)]}),
            ),
            ("GET", "/library/metadata/101,102") => {
                mc(json!({"Metadata": [track("101", 1, true), track("102", 2, true)]}))
            }
            ("GET", "/library/metadata/101") => mc(json!({"Metadata": [track("101", 1, true)]})),
            ("GET", "/library/metadata/101/nearest") => {
                mc(json!({"Metadata": [track("102", 2, false), track("101", 1, false)]}))
            }
            ("GET", "/library/metadata/5/similar") => {
                mc(json!({"Metadata": [{"ratingKey": "7", "title": "Other Band"}]}))
            }
            ("GET", "/library/streams/9") => {
                Response::bytes("text/plain", b"[00:01.00]Hello\n[00:03.00]There\n".to_vec())
            }
            ("GET", "/photo/:/transcode") => Response::bytes("image/png", common::png()),
            ("GET", p) if p.starts_with("/library/parts/") => {
                Response::bytes("audio/flac", b"fLaC....".to_vec())
            }
            ("PUT", "/:/rate") => mc(json!({})),
            ("POST", "/:/timeline") => mc(json!({})),
            ("GET", "/playlists") => {
                assert_eq!(r.get("playlistType"), Some("audio"));
                mc(json!({"totalSize": 2, "Metadata": [
                    {"ratingKey": "500", "title": "Road", "smart": false, "leafCount": 1, "updatedAt": 1},
                    {"ratingKey": "501", "title": "Loud", "smart": true, "leafCount": 2}]}))
            }
            ("GET", "/playlists/500/items") => {
                mc(json!({"totalSize": 1, "Metadata": [track("102", 2, false)]}))
            }
            ("GET", "/playlists/501/items") => mc(
                json!({"totalSize": 2, "Metadata": [track("101", 1, false), track("102", 2, false)]}),
            ),
            ("POST", "/playlists") => {
                mc(json!({"Metadata": [{"ratingKey": "600", "title": r.get("title")}]}))
            }
            ("PUT", "/playlists/600/items") | ("DELETE", "/playlists/600/items") => mc(json!({})),
            ("PUT", "/playlists/600") | ("DELETE", "/playlists/600") => mc(json!({})),
            _ => Response::not_found(),
        }
    }))
}

/// One stand-in for the whole file, since plex.tv's address is set once.
fn setup() -> (String, Arc<Mutex<Seen>>) {
    static S: OnceLock<(String, Arc<Mutex<Seen>>)> = OnceLock::new();
    S.get_or_init(|| {
        let seen = Arc::new(Mutex::new(Seen {
            good_tokens: vec![SERVER_TOKEN.into()],
            ..Default::default()
        }));
        let url = plex(seen.clone());
        // SAFETY: set once, before any test thread reads it.
        unsafe { std::env::set_var("PATINA_PLEX_TV_URL", format!("{url}/api/v2")) };
        (url, seen)
    })
    .clone()
}

/// A login as the PIN flow would leave it.
fn signed_in() -> Login {
    setup();
    let pin = plex_tv::start_pin().unwrap();
    assert_eq!(pin.code, "ABCD");
    assert!(pin.link().ends_with("/link/?pin=ABCD"));
    assert_eq!(plex_tv::check_pin(&pin).unwrap(), None, "not entered yet");
    let login = plex_tv::check_pin(&pin).unwrap().expect("entered now");
    assert!(login.jwt);
    login
}

#[test]
fn signing_in_with_a_pin_and_finding_servers() {
    let (url, _) = setup();
    let login = signed_in();
    assert_eq!(plex_tv::account(&login).unwrap(), "zack");
    let servers = plex_tv::servers(&login).unwrap();
    assert_eq!(servers.len(), 1, "only servers, not players");
    assert_eq!(servers[0].id, MACHINE);
    // On this network first, the relay last.
    assert_eq!(servers[0].ordered()[0], url);
}

#[test]
fn connecting_finds_a_working_address_and_reads_the_library() {
    let (url, seen) = setup();
    let login = Login {
        server_token: String::new(),
        ..signed_in()
    };
    // A dead address first: plex.tv's list has a better one.
    let c = Plex::connect(
        "http://127.0.0.1:9",
        MACHINE,
        &login.to_secret(),
        StreamFormat::Raw,
        192,
        true,
    )
    .unwrap();
    assert!(
        c.info().product.contains("Plex Pass"),
        "{}",
        c.info().product
    );
    // The server token from plex.tv worked: it's saved for next time.
    let fresh = Login::parse(&c.fresh_secret().expect("a token to keep"));
    assert_eq!(fresh.server_token, SERVER_TOKEN);

    let albums = c.albums().unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!((albums[0].year, albums[0].rating), (Some(1999), Some(3)));
    let songs = c.album_songs("10").unwrap();
    assert_eq!(songs.len(), 2);
    let s = &songs[0];
    assert_eq!(s.path.as_deref(), Some("Band/Album/01 Song 1.flac"));
    assert_eq!(
        (s.sample_rate, s.bits, s.suffix.as_str()),
        (Some(96000), Some(24), "flac")
    );
    assert_eq!(s.genres, vec!["Grunge".to_string()]);
    assert_eq!((s.rating, s.play_count), (Some(4), 3));
    assert_eq!(
        s.rg_track_gain,
        Some(-7.5),
        "loudness, with Plex Pass features on"
    );
    assert_eq!(
        s.cover.as_deref(),
        Some("/library/metadata/10/thumb/1700000000")
    );

    // The original file, with the token in the URL for FFmpeg.
    let (u, original) = c.stream_url("101").unwrap();
    assert!(original);
    assert!(u.starts_with(&format!("{url}/library/parts/1010/")), "{u}");
    assert!(u.contains("X-Plex-Token=SERVERTOKEN"));
    // A conversion goes through the universal transcoder, as HLS.
    let (u, original) = c.stream_url_as("101", StreamFormat::Mp3, 320).unwrap();
    assert!(!original);
    assert!(
        u.contains("/music/:/transcode/universal/start.m3u8?"),
        "{u}"
    );
    assert!(u.contains("protocol=hls") && u.contains("musicBitrate=320"));
    assert!(u.contains("path=%2Flibrary%2Fmetadata%2F101"));

    // Ratings are out of ten, by PUT.
    c.set_rating(remote::Rated::Song, "101", 4).unwrap();
    // The timeline, by POST, with the session's id.
    c.timeline(
        "101",
        remote::Timeline::Playing,
        Duration::from_secs(12),
        Some(Duration::from_secs(200)),
        false,
    )
    .unwrap();
    c.timeline(
        "101",
        remote::Timeline::Stopped,
        Duration::from_secs(200),
        Some(Duration::from_secs(200)),
        true,
    )
    .unwrap();
    {
        let s = seen.lock().unwrap_or_else(|e| e.into_inner());
        let rate = s.calls.iter().rev().find(|c| c.1 == "/:/rate").unwrap();
        assert_eq!(rate.0, "PUT");
        assert!(rate.2.contains(&("rating".into(), "8".into())));
        assert!(
            rate.2
                .contains(&("identifier".into(), "com.plexapp.plugins.library".into()))
        );
        let tl: Vec<_> = s.calls.iter().filter(|c| c.1 == "/:/timeline").collect();
        assert!(tl.iter().all(|c| c.0 == "POST"));
        let last = &tl.last().unwrap().2;
        assert!(last.contains(&("state".into(), "stopped".into())));
        assert!(last.contains(&("time".into(), "200000".into())));
        assert!(last.contains(&("continuing".into(), "1".into())));
        assert!(last.iter().any(|(k, _)| k == "X-Plex-Session-Identifier"));
    }

    // Plex Pass: sonic neighbours (not the song itself), lyrics.
    assert_eq!(c.similar_songs("101", 10).unwrap(), vec!["102".to_string()]);
    let l = c.lyrics("101", "", "").unwrap().unwrap();
    assert!(l.lines.iter().any(|x| x.1.contains("Hello")));
    assert_eq!(
        c.similar_artists("band", 5).unwrap(),
        vec!["Other Band".to_string()]
    );
    assert!(
        !c.cover("/library/metadata/10/thumb/1", 300)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn plex_pass_features_stay_off_unless_asked() {
    let (url, _) = setup();
    let login = Login {
        server_token: SERVER_TOKEN.into(),
        ..signed_in()
    };
    let c = Plex::connect(
        &url,
        MACHINE,
        &login.to_secret(),
        StreamFormat::Raw,
        192,
        false,
    )
    .unwrap();
    c.albums().unwrap();
    let songs = c.album_songs("10").unwrap();
    assert_eq!(songs[0].rg_track_gain, None);
    assert!(matches!(
        c.similar_songs("101", 5),
        Err(RemoteError::Unsupported(_))
    ));
    assert!(matches!(
        c.lyrics("101", "", ""),
        Err(RemoteError::Unsupported(_))
    ));
}

#[test]
fn a_jwt_near_its_end_is_refreshed_and_kept() {
    let (url, _) = setup();
    let mut login = signed_in();
    login.token = jwt(now() + 3600);
    login.server_token = SERVER_TOKEN.into();
    let c = Plex::connect(
        &url,
        MACHINE,
        &login.to_secret(),
        StreamFormat::Raw,
        192,
        false,
    )
    .unwrap();
    let fresh = Login::parse(&c.fresh_secret().expect("the new token to save"));
    assert!(fresh.expires().unwrap() > now() + 6 * 86400);
    assert_eq!(fresh.seed, login.seed, "the same device key");
}

#[test]
fn a_pasted_token_needs_no_plex_tv() {
    let (url, _) = setup();
    let c = Plex::connect(&url, "", SERVER_TOKEN, StreamFormat::Raw, 192, false).unwrap();
    assert_eq!(c.machine(), MACHINE);
    let bad = Plex::connect(&url, "", "WRONG", StreamFormat::Raw, 192, false);
    assert_eq!(bad.err(), Some(RemoteError::Auth));
    let dead = Plex::connect(
        "http://127.0.0.1:9",
        "",
        SERVER_TOKEN,
        StreamFormat::Raw,
        192,
        false,
    );
    assert!(matches!(dead.err(), Some(RemoteError::Http(_))));
}

#[test]
fn syncing_and_playlists() {
    let (url, seen) = setup();
    let d = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&d.path().join("lib.db")).unwrap();
    let cfg = ServerConfig {
        name: "Den".into(),
        url: url.clone(),
        kind: ServerKind::Plex,
        bitrate: 192,
        udn: MACHINE.into(),
        ..Default::default()
    };
    let c = remote::connect(&cfg, SERVER_TOKEN).unwrap();
    let stats = sync::sync_library(
        &mut lib,
        "Den",
        ServerKind::Plex,
        c.as_ref(),
        false,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(stats.added, 2);
    assert!(sync::sync_playlists(&mut lib, "Den", ServerKind::Plex, c.as_ref()).unwrap());
    let lists = c.playlists().unwrap();
    assert!(
        !lists[0].readonly && lists[1].readonly,
        "smart playlists are Plex's to change"
    );

    let ids: Vec<String> = vec!["101".into(), "102".into()];
    assert_eq!(c.create_playlist("New", &ids).unwrap(), "600");
    c.replace_playlist("600", &ids[..1]).unwrap();
    c.rename_playlist("600", "Renamed").unwrap();
    c.delete_playlist("600").unwrap();
    let s = seen.lock().unwrap_or_else(|e| e.into_inner());
    let create = s
        .calls
        .iter()
        .rev()
        .find(|c| c.0 == "POST" && c.1 == "/playlists")
        .unwrap();
    assert!(create.2.contains(&("type".into(), "audio".into())));
    assert!(create.2.contains(&(
        "uri".into(),
        format!("server://{MACHINE}/com.plexapp.plugins.library/library/metadata/101,102")
    )));
    assert!(
        s.calls
            .iter()
            .any(|c| c.0 == "DELETE" && c.1 == "/playlists/600/items")
    );
    assert!(s.calls.iter().any(|c| c.0 == "PUT"
        && c.1 == "/playlists/600"
        && c.2.contains(&("title".into(), "Renamed".into()))));
}

#[test]
fn icecast_mounts_are_stations() {
    let url = serve(Arc::new(|r: &Request| match r.path.as_str() {
        "/status-json.xsl" => Response::json(json!({"icestats": {"server_id": "Icecast 2.4.4",
        "source": [
            {"listenurl": "http://localhost:8000/live.mp3", "server_name": "Live", "genre": "Rock", "bitrate": 192},
            {"listenurl": "http://localhost:8000/chill.ogg", "server_name": "Unspecified name"}
        ]}})),
        _ => Response::not_found(),
    }));
    let c = remote::connect(
        &ServerConfig {
            name: "Ice".into(),
            url: format!("{url}/status.xsl"),
            kind: ServerKind::Icecast,
            ..Default::default()
        },
        "",
    )
    .unwrap();
    assert_eq!(c.info().product, "Icecast 2.4.4");
    let st = c.radio().unwrap();
    assert_eq!(st.len(), 2);
    assert_eq!(
        st[0].url,
        format!("{url}/live.mp3"),
        "the address typed, not localhost"
    );
    assert_eq!(st[0].name, "Live (Rock, 192 kbps)");
    assert_eq!(st[1].name, "chill.ogg");
    let nothing = serve(Arc::new(|_: &Request| Response::json(json!({"hello": 1}))));
    assert!(matches!(
        remote::icecast::Icecast::connect(&nothing),
        Err(RemoteError::Parse(_))
    ));
}
