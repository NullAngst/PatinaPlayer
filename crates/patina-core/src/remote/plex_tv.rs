//! plex.tv: signing in, keeping the sign-in fresh, and finding servers.
//!
//! This follows "Authenticating with Plex" in Plex's API docs
//! (developer.plex.tv/pms, Option 1). The app makes an Ed25519 key pair
//! and asks plex.tv for a PIN, sending the public half as a JWK. You enter
//! the PIN at plex.tv/link. The app then polls the PIN with a JWT signed by
//! its private key, and gets back a Plex JWT once you've linked it.
//!
//! A Plex JWT lasts 7 days. Before it runs out the app trades a fresh
//! nonce, signed the same way, for a new one. Plex lets that happen at any
//! time, even after it has expired, so a laptop left off for a month still
//! signs back in on its own.
//!
//! A token pasted by hand (the classic X-Plex-Token) works too. It never
//! expires and needs no key.
//!
//! What's kept, in the keyring, is a [`Login`]: the client identifier, the
//! key's seed, and the tokens. Nothing here writes it anywhere itself.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{RemoteError, Result};

/// Where plex.tv's API answers. Tests point this at a stand-in.
pub fn base() -> String {
    std::env::var("PATINA_PLEX_TV_URL").unwrap_or_else(|_| "https://clients.plex.tv/api/v2".into())
}

/// Where you enter a four-character PIN.
pub const LINK: &str = "https://plex.tv/link";

/// Refresh the Plex JWT when it has less than this left.
const REFRESH_BEFORE: i64 = 24 * 3600;

/// The scopes asked for in a new Plex JWT. Only what's needed to show
/// whose account it is.
const SCOPES: &str = "username,friendly_name";

/// What's kept for a Plex server, as JSON in the keyring.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    /// X-Plex-Client-Identifier: this install's id with Plex. Stays the
    /// same for as long as the server is added, since the key is
    /// registered to it.
    pub client: String,
    /// The Ed25519 private key's seed, base64url. Empty for a pasted token.
    #[serde(default)]
    pub seed: String,
    /// The key's id, as registered with plex.tv.
    #[serde(default)]
    pub kid: String,
    /// The account's token: a Plex JWT, or a classic token.
    #[serde(default)]
    pub token: String,
    /// `token` is a JWT that needs refreshing.
    #[serde(default)]
    pub jwt: bool,
    /// The server's own token, from plex.tv's server list. A shared
    /// server needs it; an owned one usually takes the account token too.
    #[serde(default)]
    pub server_token: String,
}

impl Login {
    /// Read what the keyring holds. A bare string is a pasted token.
    pub fn parse(secret: &str) -> Login {
        let s = secret.trim();
        if s.starts_with('{')
            && let Ok(l) = serde_json::from_str::<Login>(s)
        {
            return l;
        }
        Login {
            client: new_client_id(),
            token: s.to_string(),
            ..Default::default()
        }
    }

    pub fn to_secret(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    fn key(&self) -> Result<Ed25519KeyPair> {
        let seed = B64
            .decode(self.seed.as_bytes())
            .map_err(|_| RemoteError::Parse("the saved Plex key is damaged".into()))?;
        Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|_| RemoteError::Parse("the saved Plex key is damaged".into()))
    }

    /// When the JWT runs out, in Unix seconds, from its own claims.
    pub fn expires(&self) -> Option<i64> {
        if !self.jwt {
            return None;
        }
        jwt_claims(&self.token)?.get("exp")?.as_i64()
    }

    /// Whether the JWT is close enough to its end to refresh now.
    pub fn due(&self, now: i64) -> bool {
        self.jwt && self.expires().is_none_or(|e| e - now < REFRESH_BEFORE)
    }
}

/// The claims of a JWT, unverified. Only used to read when ours runs out.
pub fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_, body, _) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = B64.decode(body.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A JWT has three dot-separated parts; a classic token has none.
pub fn looks_like_jwt(token: &str) -> bool {
    token.split('.').count() == 3
}

/// A fresh client identifier: 16 random bytes in hex.
pub fn new_client_id() -> String {
    let mut b = [0u8; 16];
    let _ = SystemRandom::new().fill(&mut b);
    hex::encode(b)
}

/// A new key pair: its seed and its id.
fn new_key() -> Result<(String, String)> {
    let mut seed = [0u8; 32];
    SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| RemoteError::Parse("no randomness for a key".into()))?;
    let pair = Ed25519KeyPair::from_seed_unchecked(&seed)
        .map_err(|_| RemoteError::Parse("couldn't make a key".into()))?;
    // The key's id: a hash of the public key, so it gives nothing away.
    let kid = hex::encode(Sha256::digest(pair.public_key().as_ref()));
    Ok((B64.encode(seed), kid))
}

/// The public key as a JWK, the way plex.tv wants it.
fn jwk(login: &Login) -> Result<Value> {
    let pair = login.key()?;
    Ok(json!({
        "kty": "OKP",
        "crv": "Ed25519",
        "x": B64.encode(pair.public_key().as_ref()),
        "kid": login.kid,
        "alg": "EdDSA",
        "use": "sig",
    }))
}

/// A device JWT: header with the key's id, the claims, signed with EdDSA.
fn sign(login: &Login, mut claims: Value) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    let c = claims
        .as_object_mut()
        .ok_or_else(|| RemoteError::Parse("claims".into()))?;
    c.insert("aud".into(), json!("plex.tv"));
    c.insert("iss".into(), json!(login.client));
    c.insert("iat".into(), json!(now));
    c.insert("exp".into(), json!(now + 300));
    let header = json!({"alg": "EdDSA", "typ": "JWT", "kid": login.kid});
    let input = format!(
        "{}.{}",
        B64.encode(header.to_string()),
        B64.encode(claims.to_string())
    );
    let sig = login.key()?.sign(input.as_bytes());
    Ok(format!("{input}.{}", B64.encode(sig.as_ref())))
}

/// The X-Plex headers every request carries. Plex shows these in its
/// list of devices and on its dashboard.
pub fn headers(client: &str) -> Vec<(&'static str, String)> {
    let platform = if cfg!(target_os = "android") {
        "Android"
    } else {
        "Linux"
    };
    vec![
        ("Accept", "application/json".into()),
        ("X-Plex-Client-Identifier", client.to_string()),
        ("X-Plex-Product", super::CLIENT.into()),
        ("X-Plex-Version", env!("CARGO_PKG_VERSION").into()),
        ("X-Plex-Platform", platform.into()),
        ("X-Plex-Device", platform.into()),
        ("X-Plex-Device-Name", crate::control::machine_name()),
        ("X-Plex-Model", "PatinaPlayer".into()),
    ]
}

fn http() -> Result<reqwest::blocking::Client> {
    crate::net::client_builder()
        .user_agent(concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| RemoteError::Http(e.to_string()))
}

/// A JSON request body.
fn json_body(
    rb: reqwest::blocking::RequestBuilder,
    v: &Value,
) -> reqwest::blocking::RequestBuilder {
    rb.header("Content-Type", "application/json")
        .body(v.to_string())
}

fn send(rb: reqwest::blocking::RequestBuilder, client: &str) -> Result<Value> {
    let mut rb = rb;
    for (k, v) in headers(client) {
        rb = rb.header(k, v);
    }
    let resp = rb.send().map_err(super::http::http_err)?;
    let status = resp.status();
    let body = resp.bytes().map_err(super::http::http_err)?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(RemoteError::Auth);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(RemoteError::Server {
            code: 404,
            message: "not found".into(),
        });
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(RemoteError::Http("plex.tv asks to slow down".into()));
    }
    if !status.is_success() {
        return Err(RemoteError::Http(format!("plex.tv: HTTP {status}")));
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body).map_err(|e| RemoteError::Parse(format!("plex.tv: {e}")))
}

/// A PIN waiting for you to enter it at plex.tv/link.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub id: i64,
    /// The four characters to type.
    pub code: String,
    /// The login it'll become: the key's here already.
    pub login: Login,
    /// Seconds it stays good, as plex.tv said.
    pub expires_in: i64,
}

impl Pin {
    /// plex.tv/link with the code filled in.
    pub fn link(&self) -> String {
        format!("{LINK}/?pin={}", self.code)
    }
}

/// Ask plex.tv for a PIN, with a new key pair's public half.
pub fn start_pin() -> Result<Pin> {
    let (seed, kid) = new_key()?;
    let login = Login {
        client: new_client_id(),
        seed,
        kid,
        ..Default::default()
    };
    let body = json!({ "jwk": jwk(&login)?, "strong": false });
    let v = send(
        json_body(http()?.post(format!("{}/pins", base())), &body),
        &login.client,
    )?;
    let id = v
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| RemoteError::Parse("plex.tv sent a PIN without an id".into()))?;
    let code = v
        .get("code")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| RemoteError::Parse("plex.tv sent a PIN without a code".into()))?
        .to_string();
    Ok(Pin {
        id,
        code,
        login,
        expires_in: v.get("expiresIn").and_then(Value::as_i64).unwrap_or(900),
    })
}

/// Has the PIN been entered? The login, with its token, once it has.
pub fn check_pin(pin: &Pin) -> Result<Option<Login>> {
    let device_jwt = sign(&pin.login, json!({ "scope": SCOPES }))?;
    let v = match send(
        http()?
            .get(format!("{}/pins/{}", base(), pin.id))
            .query(&[("deviceJWT", device_jwt)]),
        &pin.login.client,
    ) {
        Ok(v) => v,
        Err(RemoteError::Server { code: 404, .. }) => {
            return Err(RemoteError::Server {
                code: 404,
                message: "the PIN ran out; start again".into(),
            });
        }
        Err(e) => return Err(e),
    };
    let token = v
        .get("authToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return Ok(None);
    }
    let mut login = pin.login.clone();
    // plex.tv quietly hands out a classic token if it couldn't match the
    // key. That works too; it just never needs refreshing.
    login.jwt = looks_like_jwt(&token);
    login.token = token;
    Ok(Some(login))
}

/// Trade a signed nonce for a new Plex JWT, if this one is near its end.
/// True when the token changed.
pub fn refresh(login: &mut Login, force: bool) -> Result<bool> {
    let now = chrono::Utc::now().timestamp();
    if !login.jwt || login.seed.is_empty() || !(force || login.due(now)) {
        return Ok(false);
    }
    let c = http()?;
    let v = send(c.get(format!("{}/auth/nonce", base())), &login.client)?;
    let nonce = v
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or_else(|| RemoteError::Parse("plex.tv sent no nonce".into()))?
        .to_string();
    let device_jwt = sign(login, json!({ "nonce": nonce, "scope": SCOPES }))?;
    let v = send(
        json_body(
            c.post(format!("{}/auth/token", base())),
            &json!({ "jwt": device_jwt }),
        ),
        &login.client,
    )?;
    let token = v
        .get("auth_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| RemoteError::Parse("plex.tv sent no token".into()))?;
    login.token = token.to_string();
    Ok(true)
}

/// One way to reach a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub uri: String,
    pub local: bool,
    pub relay: bool,
}

/// A server on the account, as plex.tv lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    /// Its machine identifier.
    pub id: String,
    pub token: String,
    pub owned: bool,
    pub connections: Vec<Connection>,
}

impl Resource {
    /// Its addresses, best first: on this network, then direct over the
    /// internet, then through Plex's relay (which is slow and capped).
    pub fn ordered(&self) -> Vec<String> {
        let rank = |c: &Connection| match (c.relay, c.local) {
            (false, true) => 0,
            (false, false) => 1,
            (true, _) => 2,
        };
        let mut c = self.connections.clone();
        c.sort_by_key(rank);
        c.into_iter().map(|c| c.uri).collect()
    }
}

/// The servers this account can use: its own and ones shared with it.
pub fn servers(login: &Login) -> Result<Vec<Resource>> {
    let v = send(
        http()?
            .get(format!("{}/resources", base()))
            .query(&[
                ("includeHttps", "1"),
                ("includeRelay", "1"),
                ("includeIPv6", "0"),
            ])
            .header("X-Plex-Token", &login.token),
        &login.client,
    )?;
    let list = v.as_array().cloned().unwrap_or_default();
    Ok(list
        .into_iter()
        .filter(|d| {
            d.get("provides")
                .and_then(Value::as_str)
                .is_some_and(|p| p.split(',').any(|x| x.trim() == "server"))
        })
        .map(|d| Resource {
            name: d
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Plex")
                .to_string(),
            id: d
                .get("clientIdentifier")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            token: d
                .get("accessToken")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            owned: d.get("owned").and_then(Value::as_bool).unwrap_or(false),
            connections: d
                .get("connections")
                .and_then(Value::as_array)
                .map(|cs| {
                    cs.iter()
                        .filter_map(|c| {
                            Some(Connection {
                                uri: c.get("uri")?.as_str()?.trim_end_matches('/').to_string(),
                                local: c.get("local").and_then(Value::as_bool).unwrap_or(false),
                                relay: c.get("relay").and_then(Value::as_bool).unwrap_or(false),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .filter(|r| !r.id.is_empty())
        .collect())
}

/// Whose account this is: the user name, or its friendly name.
pub fn account(login: &Login) -> Result<String> {
    let v = send(
        http()?
            .get(format!("{}/user", base()))
            .header("X-Plex-Token", &login.token),
        &login.client,
    )?;
    Ok(["username", "title", "friendlyName"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .unwrap_or("Plex")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_jwt_checks_out_with_its_public_key() {
        let (seed, kid) = new_key().unwrap();
        let login = Login {
            client: "abc".into(),
            seed,
            kid: kid.clone(),
            ..Default::default()
        };
        let t = sign(&login, json!({"nonce": "n1", "scope": SCOPES})).unwrap();
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: Value = serde_json::from_slice(&B64.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "EdDSA");
        assert_eq!(header["kid"], kid.as_str());
        let claims = jwt_claims(&t).unwrap();
        assert_eq!(claims["aud"], "plex.tv");
        assert_eq!(claims["iss"], "abc");
        assert_eq!(claims["nonce"], "n1");
        assert!(claims["exp"].as_i64().unwrap() > claims["iat"].as_i64().unwrap());
        // The signature verifies against the JWK's public key.
        let x = jwk(&login).unwrap()["x"].as_str().unwrap().to_string();
        let pubkey = B64.decode(x).unwrap();
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pubkey)
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &B64.decode(parts[2]).unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn logins_round_trip_and_bare_tokens_parse() {
        let l = Login {
            client: "c".into(),
            token: "t".into(),
            jwt: true,
            ..Default::default()
        };
        assert_eq!(Login::parse(&l.to_secret()), l);
        let pasted = Login::parse("  xyzToken ");
        assert_eq!(pasted.token, "xyzToken");
        assert!(!pasted.jwt && pasted.seed.is_empty() && pasted.client.len() == 32);
    }

    #[test]
    fn expiry_comes_from_the_token() {
        let claims = B64.encode(r#"{"exp": 2000000000}"#);
        let l = Login {
            token: format!("h.{claims}.s"),
            jwt: true,
            ..Default::default()
        };
        assert_eq!(l.expires(), Some(2_000_000_000));
        assert!(!l.due(2_000_000_000 - 3 * 86400));
        assert!(l.due(2_000_000_000 - 3600));
        let classic = Login {
            token: "abc".into(),
            ..Default::default()
        };
        assert!(!classic.due(0));
    }

    #[test]
    fn addresses_go_local_first_and_relay_last() {
        let r = Resource {
            name: "s".into(),
            id: "m".into(),
            token: "t".into(),
            owned: true,
            connections: vec![
                Connection {
                    uri: "https://relay".into(),
                    local: false,
                    relay: true,
                },
                Connection {
                    uri: "https://remote".into(),
                    local: false,
                    relay: false,
                },
                Connection {
                    uri: "http://192.168.1.5:32400".into(),
                    local: true,
                    relay: false,
                },
            ],
        };
        assert_eq!(
            r.ordered(),
            vec![
                "http://192.168.1.5:32400",
                "https://remote",
                "https://relay"
            ]
        );
    }
}
