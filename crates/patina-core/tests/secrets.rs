//! The keyring path of [`patina_core::secrets`], against a small Secret
//! Service of our own on a private D-Bus: store, find, replace, unlock
//! through a prompt, and delete. Skipped when `dbus-daemon` isn't installed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use patina_core::secrets::{Secrets, Stored};
use zbus::blocking::Connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

struct Bus {
    child: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn private_bus() -> Option<Bus> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut()?)
        .read_line(&mut line)
        .ok()?;
    Some(Bus {
        child,
        address: line.trim().to_string(),
    })
}

fn connect(bus: &Bus) -> Connection {
    zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap()
}

type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[derive(Default)]
struct Store {
    items: Vec<StoredItem>,
    next: u32,
    /// New items start locked, so reading them needs an unlock prompt.
    lock_new: bool,
    prompts_run: u32,
}

struct StoredItem {
    path: String,
    attrs: HashMap<String, String>,
    secret: Vec<u8>,
    locked: bool,
}

const COLL: &str = "/org/freedesktop/secrets/collection/login";

struct FakeService(Arc<Mutex<Store>>);

#[zbus::interface(name = "org.freedesktop.Secret.Service")]
impl FakeService {
    fn open_session(&self, _algorithm: &str, _input: Value<'_>) -> (OwnedValue, OwnedObjectPath) {
        (
            OwnedValue::try_from(Value::from("")).unwrap(),
            OwnedObjectPath::try_from("/org/freedesktop/secrets/session/1").unwrap(),
        )
    }

    fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        let s = self.0.lock().unwrap();
        let (mut open, mut shut) = (Vec::new(), Vec::new());
        for it in &s.items {
            if attributes.iter().all(|(k, v)| it.attrs.get(k) == Some(v)) {
                let p = OwnedObjectPath::try_from(it.path.as_str()).unwrap();
                if it.locked {
                    shut.push(p)
                } else {
                    open.push(p)
                }
            }
        }
        (open, shut)
    }

    async fn unlock(
        &self,
        objects: Vec<OwnedObjectPath>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        let any_locked = {
            let s = self.0.lock().unwrap();
            objects
                .iter()
                .any(|o| s.items.iter().any(|i| i.path == o.as_str() && i.locked))
        };
        if !any_locked {
            return (objects, OwnedObjectPath::try_from("/").unwrap());
        }
        let path = "/org/freedesktop/secrets/prompt/u1";
        let _ = server
            .at(
                path,
                FakePrompt {
                    store: self.0.clone(),
                    unlock: objects.iter().map(|o| o.to_string()).collect(),
                },
            )
            .await;
        (Vec::new(), OwnedObjectPath::try_from(path).unwrap())
    }

    fn get_secrets(
        &self,
        items: Vec<OwnedObjectPath>,
        session: OwnedObjectPath,
    ) -> HashMap<OwnedObjectPath, Secret> {
        let s = self.0.lock().unwrap();
        items
            .into_iter()
            .filter_map(|p| {
                let it = s.items.iter().find(|i| i.path == p.as_str() && !i.locked)?;
                Some((
                    p,
                    (
                        session.clone(),
                        Vec::new(),
                        it.secret.clone(),
                        "text/plain".into(),
                    ),
                ))
            })
            .collect()
    }

    fn read_alias(&self, name: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(if name == "default" { COLL } else { "/" }).unwrap()
    }
}

struct FakeCollection(Arc<Mutex<Store>>);

#[zbus::interface(name = "org.freedesktop.Secret.Collection")]
impl FakeCollection {
    async fn create_item(
        &self,
        properties: HashMap<String, OwnedValue>,
        secret: Secret,
        replace: bool,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        let attrs: HashMap<String, String> = properties
            .get("org.freedesktop.Secret.Item.Attributes")
            .and_then(|v| HashMap::<String, String>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default();
        let path = {
            let mut s = self.0.lock().unwrap();
            if replace && let Some(it) = s.items.iter_mut().find(|i| i.attrs == attrs) {
                it.secret = secret.2.clone();
                return (
                    OwnedObjectPath::try_from(it.path.as_str()).unwrap(),
                    OwnedObjectPath::try_from("/").unwrap(),
                );
            }
            s.next += 1;
            let path = format!("{COLL}/{}", s.next);
            let locked = s.lock_new;
            s.items.push(StoredItem {
                path: path.clone(),
                attrs,
                secret: secret.2,
                locked,
            });
            path
        };
        let _ = server
            .at(path.as_str(), FakeItem(self.0.clone(), path.clone()))
            .await;
        (
            OwnedObjectPath::try_from(path).unwrap(),
            OwnedObjectPath::try_from("/").unwrap(),
        )
    }
}

struct FakeItem(Arc<Mutex<Store>>, String);

#[zbus::interface(name = "org.freedesktop.Secret.Item")]
impl FakeItem {
    fn delete(&self) -> OwnedObjectPath {
        self.0.lock().unwrap().items.retain(|i| i.path != self.1);
        OwnedObjectPath::try_from("/").unwrap()
    }
}

struct FakePrompt {
    store: Arc<Mutex<Store>>,
    unlock: Vec<String>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Prompt")]
impl FakePrompt {
    async fn prompt(&self, _window_id: &str, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
        {
            let mut s = self.store.lock().unwrap();
            s.prompts_run += 1;
            for it in &mut s.items {
                if self.unlock.contains(&it.path) {
                    it.locked = false;
                }
            }
        }
        let _ = FakePrompt::completed(&e, false, Value::from("")).await;
    }

    #[zbus(signal)]
    async fn completed(
        e: &SignalEmitter<'_>,
        dismissed: bool,
        result: Value<'_>,
    ) -> zbus::Result<()>;
}

fn fake_keyring(bus: &Bus, store: Arc<Mutex<Store>>) -> Connection {
    let c = connect(bus);
    c.object_server()
        .at("/org/freedesktop/secrets", FakeService(store.clone()))
        .unwrap();
    c.object_server().at(COLL, FakeCollection(store)).unwrap();
    c.request_name("org.freedesktop.secrets").unwrap();
    c
}

#[test]
fn passwords_go_in_the_keyring_not_the_file() {
    let Some(bus) = private_bus() else {
        eprintln!("no dbus-daemon, skipping");
        return;
    };
    let store = Arc::new(Mutex::new(Store::default()));
    let _svc = fake_keyring(&bus, store.clone());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secrets.toml");
    let s = Secrets::with(Some(connect(&bus)), file.clone());
    assert!(s.has_keyring());

    assert_eq!(s.get("https://music.example", "zack").unwrap(), None);
    assert_eq!(
        s.set("https://music.example", "zack", "hunter2").unwrap(),
        Stored::Keyring
    );
    assert!(!file.exists(), "nothing written to disk");
    assert_eq!(
        s.get("https://music.example", "zack").unwrap().as_deref(),
        Some("hunter2")
    );
    // Saving again replaces it rather than piling up items.
    s.set("https://music.example", "zack", "correct horse")
        .unwrap();
    assert_eq!(store.lock().unwrap().items.len(), 1);
    assert_eq!(
        s.get("https://music.example", "zack").unwrap().as_deref(),
        Some("correct horse")
    );
    // Other users and servers don't see it.
    assert_eq!(s.get("https://music.example", "someone").unwrap(), None);

    s.delete("https://music.example", "zack").unwrap();
    assert_eq!(s.get("https://music.example", "zack").unwrap(), None);
    assert!(store.lock().unwrap().items.is_empty());
}

#[test]
fn a_locked_keyring_is_unlocked_through_its_prompt() {
    let Some(bus) = private_bus() else {
        return;
    };
    let store = Arc::new(Mutex::new(Store {
        lock_new: true,
        ..Default::default()
    }));
    let _svc = fake_keyring(&bus, store.clone());
    let dir = tempfile::tempdir().unwrap();
    let s = Secrets::with(Some(connect(&bus)), dir.path().join("secrets.toml"));
    s.set("https://a", "me", "pw").unwrap();
    assert!(store.lock().unwrap().items[0].locked);
    assert_eq!(s.get("https://a", "me").unwrap().as_deref(), Some("pw"));
    assert_eq!(store.lock().unwrap().prompts_run, 1);
}

#[test]
fn no_keyring_on_the_bus_means_the_file() {
    let Some(bus) = private_bus() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secrets.toml");
    let s = Secrets::with(Some(connect(&bus)), file.clone());
    assert!(!s.has_keyring());
    assert_eq!(s.set("https://a", "me", "pw").unwrap(), Stored::File(file));
    assert_eq!(s.get("https://a", "me").unwrap().as_deref(), Some("pw"));
}
