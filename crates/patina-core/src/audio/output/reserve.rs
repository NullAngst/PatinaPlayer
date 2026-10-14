//! Asking whoever holds a sound card to let go of it, over D-Bus.
//!
//! This is the freedesktop device reservation protocol
//! (`org.freedesktop.ReserveDevice1`). PipeWire, PulseAudio and JACK all
//! speak it: each one owns the bus name `org.freedesktop.ReserveDevice1.AudioN`
//! for card N while it uses the card, and gives the card up when an app with
//! a higher priority asks. Without this, opening `hw:` would fail with
//! "device busy" whenever PipeWire had the card open, which on a desktop is
//! most of the time.
//!
//! The name is held for as long as the [`Reservation`] lives. Dropping it
//! releases the name, and PipeWire takes the card back on its own.

use zbus::blocking::Connection;
use zbus::blocking::fdo::DBusProxy;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::WellKnownName;

/// Ours. PipeWire reserves cards at priority -20 by default
/// (`api.alsa.reserve.priority`), so this wins against it. An app holding
/// the card at a higher priority keeps it.
const PRIORITY: i32 = 5;

struct Reserver {
    app: String,
    device: String,
}

#[zbus::interface(name = "org.freedesktop.ReserveDevice1")]
impl Reserver {
    /// Someone else wants the card. We keep it: bit-perfect was asked for,
    /// and giving the card away mid-song would be worse than saying no.
    fn request_release(&self, _priority: i32) -> bool {
        false
    }

    #[zbus(property)]
    fn priority(&self) -> i32 {
        PRIORITY
    }

    #[zbus(property)]
    fn application_name(&self) -> String {
        self.app.clone()
    }

    #[zbus(property)]
    fn application_device_name(&self) -> String {
        self.device.clone()
    }
}

/// Holds card N for us until dropped.
pub struct Reservation {
    conn: Connection,
    name: WellKnownName<'static>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(dbus) = DBusProxy::new(&self.conn) {
            let _ = dbus.release_name(self.name.as_ref());
        }
    }
}

/// Take card `card`, asking the current owner to release it if there is
/// one. Returns `Ok(None)` when there's no session bus to ask on (a TTY,
/// a minimal system), in which case opening the device is all there is.
pub fn reserve(card: i32, device: &str) -> Result<Option<Reservation>, String> {
    let Ok(conn) = Connection::session() else {
        return Ok(None);
    };
    reserve_on(conn, card, device).map(Some)
}

/// [`reserve`] on a given bus connection. Split out for tests.
pub fn reserve_on(conn: Connection, card: i32, device: &str) -> Result<Reservation, String> {
    let name: WellKnownName<'static> =
        WellKnownName::try_from(format!("org.freedesktop.ReserveDevice1.Audio{card}"))
            .map_err(|e| e.to_string())?;
    let path = format!("/org/freedesktop/ReserveDevice1/Audio{card}");
    conn.object_server()
        .at(
            path.as_str(),
            Reserver {
                app: "PatinaPlayer".into(),
                device: device.into(),
            },
        )
        .map_err(|e| e.to_string())?;
    let dbus = DBusProxy::new(&conn).map_err(|e| e.to_string())?;

    let first = dbus
        .request_name(name.as_ref(), RequestNameFlags::DoNotQueue.into())
        .map_err(|e| e.to_string())?;
    match first {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
            return Ok(Reservation { conn, name });
        }
        RequestNameReply::Exists | RequestNameReply::InQueue => {}
    }

    // Someone has it. Ask them, by priority, to let go.
    let owner = dbus
        .get_name_owner(name.as_ref().into())
        .map_err(|e| e.to_string())?;
    let holder = zbus::blocking::Proxy::new(
        &conn,
        owner.to_owned(),
        path.as_str(),
        "org.freedesktop.ReserveDevice1",
    )
    .map_err(|e| e.to_string())?;
    let who: String = holder
        .get_property("ApplicationName")
        .unwrap_or_else(|_| "another app".into());
    let released: bool = holder
        .call("RequestRelease", &(PRIORITY,))
        .map_err(|e| format!("{who} didn't answer: {e}"))?;
    if !released {
        return Err(format!("{who} is using this card and won't let go of it"));
    }
    let second = dbus
        .request_name(
            name.as_ref(),
            RequestNameFlags::DoNotQueue | RequestNameFlags::ReplaceExisting,
        )
        .map_err(|e| e.to_string())?;
    match second {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
            Ok(Reservation { conn, name })
        }
        _ => Err(format!("{who} let go of the card but kept its claim on it")),
    }
}
