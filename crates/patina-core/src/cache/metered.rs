//! Whether the connection is metered, the way Plasma and GNOME decide it:
//! NetworkManager's `Metered` property. A phone hotspot is the usual case.

/// NetworkManager's NMMetered values.
const YES: u32 = 1;
const GUESS_YES: u32 = 3;

/// True when NetworkManager says the connection is (or is probably)
/// metered. False when it says otherwise, or when there's no
/// NetworkManager to ask.
pub fn is_metered() -> bool {
    match query() {
        Ok(v) => v == YES || v == GUESS_YES,
        Err(e) => {
            log::debug!("can't ask NetworkManager about metering: {e}");
            false
        }
    }
}

fn query() -> zbus::Result<u32> {
    let conn = zbus::blocking::Connection::system()?;
    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )?;
    proxy.get_property::<u32>("Metered")
}
