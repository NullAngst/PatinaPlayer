//! Device reservation over D-Bus, against a private bus with a stand-in for
//! PipeWire holding the card. Skipped when `dbus-daemon` isn't installed.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use patina_core::audio::output::reserve::reserve_on;
use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

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

/// Plays the part of PipeWire: holds the card, and gives it up or not.
struct Holder {
    gives_up: bool,
}

#[zbus::interface(name = "org.freedesktop.ReserveDevice1")]
impl Holder {
    fn request_release(&self, priority: i32) -> bool {
        self.gives_up && priority > -20
    }
    #[zbus(property)]
    fn application_name(&self) -> String {
        "PipeWire".into()
    }
}

fn hold(bus: &Bus, card: i32, gives_up: bool) -> Connection {
    let c = connect(bus);
    c.object_server()
        .at(
            format!("/org/freedesktop/ReserveDevice1/Audio{card}").as_str(),
            Holder { gives_up },
        )
        .unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&c).unwrap();
    let r = dbus
        .request_name(
            format!("org.freedesktop.ReserveDevice1.Audio{card}")
                .as_str()
                .try_into()
                .unwrap(),
            RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement,
        )
        .unwrap();
    assert_eq!(r, RequestNameReply::PrimaryOwner);
    c
}

fn owner(bus: &Bus, card: i32) -> String {
    let c = connect(bus);
    let dbus = zbus::blocking::fdo::DBusProxy::new(&c).unwrap();
    dbus.get_name_owner(
        format!("org.freedesktop.ReserveDevice1.Audio{card}")
            .as_str()
            .try_into()
            .unwrap(),
    )
    .map(|o| o.to_string())
    .unwrap_or_default()
}

#[test]
fn takes_a_free_card_and_gives_it_back() {
    let Some(bus) = private_bus() else {
        eprintln!("no dbus-daemon, skipping");
        return;
    };
    let me = connect(&bus);
    let my_name = me.unique_name().unwrap().to_string();
    let r = reserve_on(me, 3, "hw:3,0").unwrap();
    assert_eq!(owner(&bus, 3), my_name);
    drop(r);
    assert_eq!(owner(&bus, 3), "", "released on drop");
}

#[test]
fn asks_pipewire_to_let_go() {
    let Some(bus) = private_bus() else {
        return;
    };
    let _pw = hold(&bus, 1, true);
    let me = connect(&bus);
    let my_name = me.unique_name().unwrap().to_string();
    let _r = reserve_on(me, 1, "hw:1,0").unwrap();
    assert_eq!(owner(&bus, 1), my_name);
}

#[test]
fn says_who_wont_let_go() {
    let Some(bus) = private_bus() else {
        return;
    };
    let _pw = hold(&bus, 2, false);
    let err = reserve_on(connect(&bus), 2, "hw:2,0").err().unwrap();
    assert!(err.contains("PipeWire"), "{err}");
}
