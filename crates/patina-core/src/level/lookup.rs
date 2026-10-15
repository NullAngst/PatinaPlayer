//! How the decoder thread finds a song's gains: its own connection to the
//! index, set up once by the session. Without one (tests, no library),
//! every song is unknown and plays at the fallback gain.

use std::path::Path;
use std::sync::{Mutex, RwLock};

use super::Gains;
use crate::library::Library;
use crate::track::TrackInfo;

static DB: RwLock<Option<Mutex<Library>>> = RwLock::new(None);

/// Open the decoder's connection to the index, or drop it with None.
pub fn install(db: Option<&Path>) {
    let lib = db.and_then(|p| match Library::open(p) {
        Ok(l) => Some(Mutex::new(l)),
        Err(e) => {
            log::warn!("leveling can't read the library: {e}");
            None
        }
    });
    *DB.write().unwrap() = lib;
}

/// Everything known about a song's loudness. Never fails: a database
/// problem means "nothing known".
pub fn gains(info: &TrackInfo) -> Gains {
    let guard = DB.read().unwrap();
    let Some(lib) = guard.as_ref() else {
        return Gains::default();
    };
    let lib = lib.lock().unwrap();
    lib.gains_for(info).unwrap_or_else(|e| {
        log::warn!("leveling lookup failed: {e}");
        Gains::default()
    })
}
