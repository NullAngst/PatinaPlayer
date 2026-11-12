//! The few libgpod calls the iPod sync needs, loaded when an iPod is used
//! rather than linked in, so the app runs (and builds) without libgpod and
//! the iPod rows just say to install it.
//!
//! libgpod's last release is 0.8.3 (2013) and its structs haven't changed
//! since, so the handful of fields read and written here are at fixed
//! offsets, measured from its header on a 64-bit build. maintenance.md
//! has a short C program that prints them from the header, to compare
//! against the `track` and `db` tables below. The iPod test in
//! tests/picking.rs also writes tags through these offsets and reads them
//! back from a freshly parsed database, so a wrong one fails it.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::Path;
use std::ptr;

use libloading::{Library, Symbol};

/// Handles to libgpod's own objects. They're only good while the database
/// they came from is open: every one comes from [`Gpod::parse`] or a call
/// on what it returned, and none outlives the [`Gpod::free`] that ends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Db(*mut c_void);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Track(*mut c_void);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Playlist(*mut c_void);

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

#[repr(C)]
struct GList {
    data: *mut c_void,
    next: *mut GList,
    _prev: *mut GList,
}

/// Offsets into `Itdb_Track` (libgpod 0.8.x, 64-bit).
pub mod track {
    pub const TITLE: usize = 8;
    pub const IPOD_PATH: usize = 16;
    pub const ALBUM: usize = 24;
    pub const ARTIST: usize = 32;
    pub const GENRE: usize = 40;
    pub const FILETYPE: usize = 48;
    pub const COMPOSER: usize = 72;
    pub const ALBUMARTIST: usize = 152;
    pub const SIZE: usize = 220;
    pub const TRACKLEN: usize = 224;
    pub const CD_NR: usize = 228;
    pub const CDS: usize = 232;
    pub const TRACK_NR: usize = 236;
    pub const TRACKS: usize = 240;
    pub const BITRATE: usize = 244;
    pub const SAMPLERATE: usize = 248;
    pub const YEAR: usize = 252;
    pub const TIME_ADDED: usize = 264;
    pub const MEDIATYPE: usize = 460;
}

/// Offsets into `Itdb_iTunesDB` and `Itdb_Playlist`.
pub mod db {
    pub const TRACKS: usize = 0;
    pub const PLAYLISTS: usize = 8;
    pub const PL_NAME: usize = 8;
    pub const PL_MEMBERS: usize = 24;
}

/// ITDB_MEDIATYPE_AUDIO.
pub const MEDIATYPE_AUDIO: u32 = 1;

type FnParse = unsafe extern "C" fn(*const c_char, *mut *mut GError) -> *mut c_void;
type FnWrite = unsafe extern "C" fn(*mut c_void, *mut *mut GError) -> c_int;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnTrackNew = unsafe extern "C" fn() -> *mut c_void;
type FnTrackAdd = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);
type FnTrackRemove = unsafe extern "C" fn(*mut c_void);
type FnMpl = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type FnPlNew = unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void;
type FnPlAdd = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);
type FnPlRemove = unsafe extern "C" fn(*mut c_void);
type FnPlTrack = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);
type FnPlRemoveTrack = unsafe extern "C" fn(*mut c_void, *mut c_void);
type FnPlIs = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnCp = unsafe extern "C" fn(*mut c_void, *const c_char, *mut *mut GError) -> c_int;
type FnFilename = unsafe extern "C" fn(*mut c_void) -> *mut c_char;
type FnThumbs = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;
type FnInit =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *mut *mut GError) -> c_int;
type FnStrdup = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type FnGFree = unsafe extern "C" fn(*mut c_void);
type FnErrFree = unsafe extern "C" fn(*mut GError);

pub struct Gpod {
    // Kept alive for the function pointers below.
    _lib: Library,
    parse: FnParse,
    write: FnWrite,
    free: FnFree,
    track_new: FnTrackNew,
    track_add: FnTrackAdd,
    track_remove: FnTrackRemove,
    mpl: FnMpl,
    pl_new: FnPlNew,
    pl_add: FnPlAdd,
    pl_remove: FnPlRemove,
    pl_add_track: FnPlTrack,
    pl_remove_track: FnPlRemoveTrack,
    pl_is_mpl: FnPlIs,
    pl_is_podcasts: FnPlIs,
    cp: FnCp,
    filename: FnFilename,
    thumbs: Option<FnThumbs>,
    init: FnInit,
    strdup: FnStrdup,
    gfree: FnGFree,
    err_free: FnErrFree,
}

/// What a failed libgpod call said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpodError(pub String);

impl std::fmt::Display for GpodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why libgpod couldn't be loaded, for the iPod rows.
pub const MISSING: &str = "the iPod needs libgpod, which isn't installed. It's libgpod4 on Debian and Ubuntu, libgpod on Fedora, Arch and openSUSE.";

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

fn path_cstr(p: &Path) -> CString {
    use std::os::unix::ffi::OsStrExt;
    CString::new(p.as_os_str().as_bytes().to_vec()).unwrap_or_default()
}

impl Gpod {
    /// Load libgpod, if it's installed.
    pub fn load() -> Result<Self, String> {
        let lib = ["libgpod.so.4", "libgpod.so"]
            .iter()
            .find_map(|n| unsafe { Library::new(n) }.ok())
            .ok_or_else(|| MISSING.to_string())?;
        unsafe {
            macro_rules! sym {
                ($name:literal) => {{
                    let s: Symbol<_> = lib
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| format!("libgpod is missing {}: {e}", $name))?;
                    *s
                }};
            }
            let thumbs = lib
                .get::<FnThumbs>(b"itdb_track_set_thumbnails_from_data\0")
                .ok()
                .map(|s| *s);
            let g = Self {
                parse: sym!("itdb_parse"),
                write: sym!("itdb_write"),
                free: sym!("itdb_free"),
                track_new: sym!("itdb_track_new"),
                track_add: sym!("itdb_track_add"),
                track_remove: sym!("itdb_track_remove"),
                mpl: sym!("itdb_playlist_mpl"),
                pl_new: sym!("itdb_playlist_new"),
                pl_add: sym!("itdb_playlist_add"),
                pl_remove: sym!("itdb_playlist_remove"),
                pl_add_track: sym!("itdb_playlist_add_track"),
                pl_remove_track: sym!("itdb_playlist_remove_track"),
                pl_is_mpl: sym!("itdb_playlist_is_mpl"),
                pl_is_podcasts: sym!("itdb_playlist_is_podcasts"),
                cp: sym!("itdb_cp_track_to_ipod"),
                filename: sym!("itdb_filename_on_ipod"),
                thumbs,
                init: sym!("itdb_init_ipod"),
                // glib is libgpod's own dependency, so it's there to find.
                strdup: sym!("g_strdup"),
                gfree: sym!("g_free"),
                err_free: sym!("g_error_free"),
                _lib: lib,
            };
            Ok(g)
        }
    }

    unsafe fn take_err(&self, e: *mut GError, what: &str) -> GpodError {
        if e.is_null() {
            return GpodError(format!("{what} failed"));
        }
        let msg = unsafe {
            let m = if (*e).message.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*e).message).to_string_lossy().into_owned()
            };
            (self.err_free)(e);
            m
        };
        GpodError(if msg.is_empty() {
            format!("{what} failed")
        } else {
            format!("{what}: {msg}")
        })
    }

    pub fn parse(&self, mount: &Path) -> Result<Db, GpodError> {
        let m = path_cstr(mount);
        let mut e = ptr::null_mut();
        let db = unsafe { (self.parse)(m.as_ptr(), &mut e) };
        if db.is_null() {
            return Err(unsafe { self.take_err(e, "reading the iPod's database") });
        }
        if !e.is_null() {
            unsafe { (self.err_free)(e) };
        }
        Ok(Db(db))
    }

    pub fn write(&self, db: Db) -> Result<(), GpodError> {
        let mut e = ptr::null_mut();
        let ok = unsafe { (self.write)(db.0, &mut e) };
        if ok == 0 {
            return Err(unsafe { self.take_err(e, "writing the iPod's database") });
        }
        Ok(())
    }

    /// Close the database. Every handle that came from it is dead after.
    pub fn free(&self, db: Db) {
        unsafe { (self.free)(db.0) }
    }

    /// Make an empty iPod layout in a folder. For tests.
    pub fn init(&self, mount: &Path, model: &str, name: &str) -> Result<(), GpodError> {
        let (m, mo, n) = (path_cstr(mount), cstr(model), cstr(name));
        let mut e = ptr::null_mut();
        let ok = unsafe { (self.init)(m.as_ptr(), mo.as_ptr(), n.as_ptr(), &mut e) };
        if ok == 0 {
            return Err(unsafe { self.take_err(e, "setting up the iPod") });
        }
        Ok(())
    }

    pub fn tracks(&self, db: Db) -> Vec<Track> {
        unsafe { list(field::<*mut GList>(db.0, db::TRACKS)) }
            .into_iter()
            .map(Track)
            .collect()
    }

    pub fn playlists(&self, db: Db) -> Vec<Playlist> {
        unsafe { list(field::<*mut GList>(db.0, db::PLAYLISTS)) }
            .into_iter()
            .map(Playlist)
            .collect()
    }

    pub fn members(&self, pl: Playlist) -> Vec<Track> {
        unsafe { list(field::<*mut GList>(pl.0, db::PL_MEMBERS)) }
            .into_iter()
            .map(Track)
            .collect()
    }

    pub fn playlist_name(&self, pl: Playlist) -> String {
        unsafe { read_str(pl.0, db::PL_NAME) }
    }

    pub fn is_special(&self, pl: Playlist) -> bool {
        unsafe { (self.pl_is_mpl)(pl.0) != 0 || (self.pl_is_podcasts)(pl.0) != 0 }
    }

    pub fn mpl(&self, db: Db) -> Playlist {
        Playlist(unsafe { (self.mpl)(db.0) })
    }

    pub fn new_playlist(&self, db: Db, name: &str) -> Playlist {
        let n = cstr(name);
        unsafe {
            let pl = (self.pl_new)(n.as_ptr(), 0);
            (self.pl_add)(db.0, pl, -1);
            Playlist(pl)
        }
    }

    pub fn remove_playlist(&self, pl: Playlist) {
        unsafe { (self.pl_remove)(pl.0) }
    }

    pub fn add_to_playlist(&self, pl: Playlist, t: Track) {
        unsafe { (self.pl_add_track)(pl.0, t.0, -1) }
    }

    pub fn remove_from_playlist(&self, pl: Playlist, t: Track) {
        unsafe { (self.pl_remove_track)(pl.0, t.0) }
    }

    /// A new track, not in the database until [`Gpod::add_track`].
    pub fn new_track(&self) -> Track {
        Track(unsafe { (self.track_new)() })
    }

    pub fn add_track(&self, db: Db, t: Track) {
        unsafe { (self.track_add)(db.0, t.0, -1) }
    }

    /// Take a track out of the database (out of every playlist first).
    pub fn remove_track(&self, db: Db, t: Track) {
        for pl in self.playlists(db) {
            if self.members(pl).contains(&t) {
                self.remove_from_playlist(pl, t);
            }
        }
        unsafe { (self.track_remove)(t.0) }
    }

    pub fn copy_to_ipod(&self, t: Track, file: &Path) -> Result<(), GpodError> {
        let f = path_cstr(file);
        let mut e = ptr::null_mut();
        let ok = unsafe { (self.cp)(t.0, f.as_ptr(), &mut e) };
        if ok == 0 {
            return Err(unsafe { self.take_err(e, "copying the song") });
        }
        Ok(())
    }

    /// Where a track's file is on the mounted iPod.
    pub fn file_on_ipod(&self, t: Track) -> Option<std::path::PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        unsafe {
            let p = (self.filename)(t.0);
            if p.is_null() {
                return None;
            }
            let v = CStr::from_ptr(p).to_bytes().to_vec();
            (self.gfree)(p.cast());
            Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(v)))
        }
    }

    /// Cover art, from the image file's bytes. False when libgpod was
    /// built without artwork support or couldn't read the image.
    pub fn set_thumbnail(&self, t: Track, data: &[u8]) -> bool {
        match self.thumbs {
            Some(f) => unsafe { f(t.0, data.as_ptr(), data.len()) != 0 },
            None => false,
        }
    }

    pub fn set_str(&self, t: Track, off: usize, v: &str) {
        let c = cstr(v);
        unsafe {
            let slot = (t.0 as *mut u8).add(off).cast::<*mut c_char>();
            if !(*slot).is_null() {
                (self.gfree)((*slot).cast());
            }
            *slot = (self.strdup)(c.as_ptr());
        }
    }

    pub fn get_str(&self, t: Track, off: usize) -> String {
        unsafe { read_str(t.0, off) }
    }

    pub fn set_i32(&self, t: Track, off: usize, v: i32) {
        unsafe { set_field(t.0, off, v) }
    }

    pub fn get_i32(&self, t: Track, off: usize) -> i32 {
        unsafe { field(t.0, off) }
    }

    pub fn set_u32(&self, t: Track, off: usize, v: c_uint) {
        unsafe { set_field(t.0, off, v) }
    }

    pub fn get_u32(&self, t: Track, off: usize) -> u32 {
        unsafe { field(t.0, off) }
    }

    pub fn set_u16(&self, t: Track, off: usize, v: u16) {
        unsafe { set_field(t.0, off, v) }
    }

    pub fn set_i64(&self, t: Track, off: usize, v: i64) {
        unsafe { set_field(t.0, off, v) }
    }
}

/// The value at a byte offset into one of libgpod's structs.
unsafe fn field<T: Copy>(p: *mut c_void, off: usize) -> T {
    unsafe { (p as *mut u8).add(off).cast::<T>().read_unaligned() }
}

unsafe fn set_field<T>(p: *mut c_void, off: usize, v: T) {
    unsafe { (p as *mut u8).add(off).cast::<T>().write_unaligned(v) }
}

unsafe fn list(mut l: *mut GList) -> Vec<*mut c_void> {
    let mut v = Vec::new();
    while !l.is_null() {
        unsafe {
            v.push((*l).data);
            l = (*l).next;
        }
    }
    v
}

unsafe fn read_str(p: *mut c_void, off: usize) -> String {
    unsafe {
        let s = field::<*const c_char>(p, off);
        if s.is_null() {
            String::new()
        } else {
            CStr::from_ptr(s).to_string_lossy().into_owned()
        }
    }
}
