//! Server passwords and API keys, kept out of config.toml.
//!
//! They go in the desktop keyring through the freedesktop Secret Service
//! API, which KWallet speaks on Plasma 6 and GNOME Keyring speaks
//! everywhere else. With no keyring (a TTY, the TUI over SSH), they go in
//! `~/.config/patinaplayer/secrets.toml`, readable only by you, and the
//! caller is told so it can say where they went.
//!
//! The Secret Service session is the "plain" algorithm: the secret crosses
//! the session bus unencrypted. That bus is private to your login, and it's
//! what most clients do; the keyring itself stores it encrypted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const BUS_NAME: &str = "org.freedesktop.secrets";
const APP: &str = "patinaplayer";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("keyring: {0}")]
    Keyring(String),
    #[error("keyring: unlocking was cancelled")]
    Dismissed,
    #[error("{path}: {msg}")]
    File { path: PathBuf, msg: String },
}

impl From<zbus::Error> for SecretError {
    fn from(e: zbus::Error) -> Self {
        SecretError::Keyring(e.to_string())
    }
}

impl From<zbus::fdo::Error> for SecretError {
    fn from(e: zbus::fdo::Error) -> Self {
        SecretError::Keyring(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// Where a secret ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    Keyring,
    /// No keyring answered, so it's in this file.
    File(PathBuf),
}

/// (session, parameters, value, content type)
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Service",
    default_service = "org.freedesktop.secrets",
    default_path = "/org/freedesktop/secrets"
)]
trait Service {
    fn open_session(
        &self,
        algorithm: &str,
        input: &Value<'_>,
    ) -> zbus::Result<(OwnedValue, OwnedObjectPath)>;
    fn search_items(
        &self,
        attributes: HashMap<&str, &str>,
    ) -> zbus::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;
    fn unlock(
        &self,
        objects: &[OwnedObjectPath],
    ) -> zbus::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)>;
    fn get_secrets(
        &self,
        items: &[OwnedObjectPath],
        session: &OwnedObjectPath,
    ) -> zbus::Result<HashMap<OwnedObjectPath, Secret>>;
    fn read_alias(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn create_collection(
        &self,
        properties: HashMap<&str, Value<'_>>,
        alias: &str,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Collection",
    default_service = "org.freedesktop.secrets"
)]
trait Collection {
    fn create_item(
        &self,
        properties: HashMap<&str, Value<'_>>,
        secret: &Secret,
        replace: bool,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Item",
    default_service = "org.freedesktop.secrets"
)]
trait Item {
    fn delete(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Prompt",
    default_service = "org.freedesktop.secrets"
)]
trait Prompt {
    fn prompt(&self, window_id: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn completed(&self, dismissed: bool, result: Value<'_>) -> zbus::Result<()>;
}

/// The secrets store: the keyring when there is one, the file otherwise.
pub struct Secrets {
    keyring: Option<Connection>,
    file: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct SecretsFile {
    #[serde(default)]
    secret: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct FileEntry {
    server: String,
    user: String,
    secret: String,
}

impl Secrets {
    /// The default place: the session's keyring, and
    /// `~/.config/patinaplayer/secrets.toml` as the fallback.
    pub fn open() -> Self {
        Self::with(
            Connection::session().ok(),
            crate::config::config_dir().join("secrets.toml"),
        )
    }

    /// A given bus (None for no keyring) and fallback file. For tests.
    pub fn with(bus: Option<Connection>, file: PathBuf) -> Self {
        let keyring = bus.filter(has_service);
        Self { keyring, file }
    }

    /// True when a keyring answered.
    pub fn has_keyring(&self) -> bool {
        self.keyring.is_some()
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The secret for this server and user, if one was saved. The keyring
    /// first, then the file, so a password saved while the keyring was
    /// away still works.
    pub fn get(&self, server: &str, user: &str) -> Result<Option<String>> {
        if let Some(c) = &self.keyring
            && let Some(s) = keyring_get(c, server, user)?
        {
            return Ok(Some(s));
        }
        Ok(read_file(&self.file)?
            .secret
            .into_iter()
            .find(|e| e.server == server && e.user == user)
            .map(|e| e.secret))
    }

    /// Save a secret, replacing any older one for the same server and user.
    pub fn set(&self, server: &str, user: &str, secret: &str) -> Result<Stored> {
        if let Some(c) = &self.keyring {
            keyring_set(c, server, user, secret)?;
            // A copy left in the file from a keyring-less day would win
            // nothing and leak something; take it out.
            remove_from_file(&self.file, server, user)?;
            return Ok(Stored::Keyring);
        }
        let mut f = read_file(&self.file)?;
        f.secret.retain(|e| !(e.server == server && e.user == user));
        f.secret.push(FileEntry {
            server: server.into(),
            user: user.into(),
            secret: secret.into(),
        });
        write_file(&self.file, &f)?;
        Ok(Stored::File(self.file.clone()))
    }

    /// Forget a secret, wherever it was.
    pub fn delete(&self, server: &str, user: &str) -> Result<()> {
        if let Some(c) = &self.keyring {
            keyring_delete(c, server, user)?;
        }
        remove_from_file(&self.file, server, user)
    }
}

fn has_service(c: &Connection) -> bool {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(c) else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(BUS_NAME) else {
        return false;
    };
    if dbus.name_has_owner(name.clone()).unwrap_or(false) {
        return true;
    }
    // Not running yet, but the bus can start it (GNOME Keyring, ksecretd).
    dbus.list_activatable_names()
        .map(|names| names.iter().any(|n| n.as_str() == BUS_NAME))
        .unwrap_or(false)
}

fn attrs<'a>(server: &'a str, user: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("application", APP), ("server", server), ("user", user)])
}

/// Run a prompt (an unlock dialog) and wait for the user. True if they
/// went through with it.
fn run_prompt(c: &Connection, path: &OwnedObjectPath) -> Result<bool> {
    if path.as_str() == "/" {
        return Ok(true);
    }
    let prompt = PromptProxyBlocking::builder(c)
        .path(path.clone())?
        .build()?;
    let mut done = prompt.receive_completed()?;
    prompt.prompt("")?;
    // The user may take a while to type a wallet password.
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        if let Some(sig) = done.next() {
            let dismissed = sig.args().map(|a| a.dismissed).unwrap_or(true);
            let _ = tx.send(dismissed);
        }
    });
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(false) => Ok(true),
        Ok(true) => Err(SecretError::Dismissed),
        Err(_) => Err(SecretError::Keyring(
            "no answer from the unlock prompt".into(),
        )),
    }
}

fn unlock(svc: &ServiceProxyBlocking, c: &Connection, paths: &[OwnedObjectPath]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let (_, prompt) = svc.unlock(paths)?;
    if run_prompt(c, &prompt)? {
        Ok(())
    } else {
        Err(SecretError::Dismissed)
    }
}

fn keyring_get(c: &Connection, server: &str, user: &str) -> Result<Option<String>> {
    let svc = ServiceProxyBlocking::new(c)?;
    let (unlocked, locked) = svc.search_items(attrs(server, user))?;
    let mut items = unlocked;
    if items.is_empty() && !locked.is_empty() {
        unlock(&svc, c, &locked)?;
        items = locked;
    }
    let Some(item) = items.into_iter().next() else {
        return Ok(None);
    };
    let (_, session) = svc.open_session("plain", &Value::from(""))?;
    let secrets = svc.get_secrets(std::slice::from_ref(&item), &session)?;
    Ok(secrets
        .get(&item)
        .map(|(_, _, value, _)| String::from_utf8_lossy(value).into_owned()))
}

fn default_collection(svc: &ServiceProxyBlocking, c: &Connection) -> Result<OwnedObjectPath> {
    let path = svc.read_alias("default")?;
    if path.as_str() != "/" {
        return Ok(path);
    }
    // No default collection yet (a fresh keyring): ask for one.
    let props = HashMap::from([(
        "org.freedesktop.Secret.Collection.Label",
        Value::from("Login"),
    )]);
    let (path, prompt) = svc.create_collection(props, "default")?;
    if path.as_str() != "/" {
        return Ok(path);
    }
    run_prompt(c, &prompt)?;
    let path = svc.read_alias("default")?;
    if path.as_str() == "/" {
        return Err(SecretError::Keyring(
            "there is no keyring to save to".into(),
        ));
    }
    Ok(path)
}

fn keyring_set(c: &Connection, server: &str, user: &str, secret: &str) -> Result<()> {
    let svc = ServiceProxyBlocking::new(c)?;
    let coll_path = default_collection(&svc, c)?;
    unlock(&svc, c, std::slice::from_ref(&coll_path))?;
    let (_, session) = svc.open_session("plain", &Value::from(""))?;
    let coll = CollectionProxyBlocking::builder(c)
        .path(coll_path)?
        .build()?;
    let attributes: HashMap<String, String> = attrs(server, user)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let props = HashMap::from([
        (
            "org.freedesktop.Secret.Item.Label",
            Value::from(format!("PatinaPlayer: {user} on {server}")),
        ),
        (
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(attributes),
        ),
    ]);
    let s: Secret = (
        session,
        Vec::new(),
        secret.as_bytes().to_vec(),
        "text/plain".into(),
    );
    let (item, prompt) = coll.create_item(props, &s, true)?;
    if item.as_str() == "/" && !run_prompt(c, &prompt)? {
        return Err(SecretError::Dismissed);
    }
    Ok(())
}

fn keyring_delete(c: &Connection, server: &str, user: &str) -> Result<()> {
    let svc = ServiceProxyBlocking::new(c)?;
    let (unlocked, locked) = svc.search_items(attrs(server, user))?;
    unlock(&svc, c, &locked)?;
    for path in unlocked.into_iter().chain(locked) {
        let item = ItemProxyBlocking::builder(c).path(path)?.build()?;
        let prompt = item.delete()?;
        run_prompt(c, &prompt)?;
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<SecretsFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| SecretError::File {
            path: path.to_path_buf(),
            msg: e.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SecretsFile::default()),
        Err(e) => Err(SecretError::File {
            path: path.to_path_buf(),
            msg: e.to_string(),
        }),
    }
}

fn write_file(path: &Path, f: &SecretsFile) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let err = |e: std::io::Error| SecretError::File {
        path: path.to_path_buf(),
        msg: e.to_string(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let body = toml::to_string(f).map_err(|e| SecretError::File {
        path: path.to_path_buf(),
        msg: e.to_string(),
    })?;
    let text = format!(
        "# Server passwords and API keys PatinaPlayer saved here because no\n\
         # keyring was running. Only you can read this file. Delete an entry\n\
         # and the app asks for that password again.\n\n{body}"
    );
    // Written next to it and renamed over, created 0600 from the start so
    // there's never a moment it's readable by anyone else.
    let tmp = path.with_extension("toml.tmp");
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(err)?;
    out.write_all(text.as_bytes()).map_err(err)?;
    out.sync_all().map_err(err)?;
    drop(out);
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)?;
    Ok(())
}

fn remove_from_file(path: &Path, server: &str, user: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut f = read_file(path)?;
    let before = f.secret.len();
    f.secret.retain(|e| !(e.server == server && e.user == user));
    if f.secret.len() != before {
        write_file(path, &f)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_fallback_is_private_and_replaces() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("secrets.toml");
        let s = Secrets::with(None, path.clone());
        assert!(!s.has_keyring());
        assert_eq!(s.get("https://a", "me").unwrap(), None);
        assert_eq!(
            s.set("https://a", "me", "one").unwrap(),
            Stored::File(path.clone())
        );
        s.set("https://a", "me", "two").unwrap();
        s.set("https://b", "me", "three").unwrap();
        assert_eq!(s.get("https://a", "me").unwrap().as_deref(), Some("two"));
        assert_eq!(s.get("https://b", "me").unwrap().as_deref(), Some("three"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        s.delete("https://a", "me").unwrap();
        assert_eq!(s.get("https://a", "me").unwrap(), None);
        assert!(std::fs::read_to_string(&path).unwrap().contains("Only you"));
    }
}
