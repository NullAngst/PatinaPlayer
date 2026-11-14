//! Both directions across JNI, to and from `dev.patina.player.Native` in
//! `android/java`.
//!
//! Java calls in through the `Java_dev_patina_player_Native_*` functions.
//! Rust calls out through the class reference kept when Java first called
//! `nativeInit`: a thread Rust started can't look the app's classes up by
//! name, since it only sees the system's class loader.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{
    GlobalRef, JBooleanArray, JClass, JIntArray, JObject, JObjectArray, JString, JValue,
};
use jni::sys::{jboolean, jint, jlong};
use patina_core::audio::output::aaudio::{self, AndroidDevice};
use patina_core::browse::auto::CarArt;
use patina_core::{Command, Player};
use patina_gui::platform::{self, Picked, Prompt, PromptEvent};

// Must match Native.java.
const PLAY: jint = 1;
const PAUSE: jint = 2;
const TOGGLE: jint = 3;
const NEXT: jint = 4;
const BACK: jint = 5;
const STOP: jint = 6;
const SEEK: jint = 7;
/// The car's heart button: heart the song playing, or take it off.
const LOVE: jint = 8;
/// The car's shuffle button.
const SHUFFLE: jint = 9;
/// A song picked in the car's queue screen, by its queue id.
const QUEUE_ITEM: jint = 10;

pub const STOPPED: jint = 0;
pub const PLAYING: jint = 1;
pub const PAUSED: jint = 2;

// What a pick is for. Must match Native.java.
const PICK_FOLDER: jint = 1;
const PICK_EQ_FILE: jint = 2;
const PICK_IMAGE: jint = 3;
const PICK_SAVE: jint = 4;

/// How long a picker may stay open before the wait for it is given up.
const PICK_PATIENCE: Duration = Duration::from_secs(15 * 60);

static VM: OnceLock<JavaVM> = OnceLock::new();
static NATIVE: OnceLock<GlobalRef> = OnceLock::new();
static PLAYER: Mutex<Option<Player>> = Mutex::new(None);
/// The pick waiting on Java's answer: what it's for, and where to send it.
static PICK: Mutex<Option<(jint, mpsc::Sender<Picked>)>> = Mutex::new(None);
/// The permission to read music, as Java last reported it.
static CAN_READ: AtomicBool = AtomicBool::new(false);
/// Set when it goes from not granted to granted, until the app looks.
static NEWLY_GRANTED: AtomicBool = AtomicBool::new(false);

pub fn set_player(p: Player) {
    *PLAYER.lock().unwrap() = Some(p);
}

/// True once after the permission to read music is granted, so the app
/// can scan the library again.
pub fn take_newly_granted() -> bool {
    NEWLY_GRANTED.swap(false, Ordering::AcqRel)
}

/// Run `f` with a JNI env on this thread and the Native class. Does nothing
/// before Java has called `nativeInit`.
fn with_java<R>(f: impl FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<R>) -> Option<R> {
    let vm = VM.get()?;
    let class = NATIVE.get()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let class: &JClass = class.as_obj().into();
    match f(&mut env, class) {
        Ok(r) => Some(r),
        Err(e) => {
            // A Java exception left pending would break the next call.
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::warn!("JNI call failed: {e}");
            None
        }
    }
}

// ----- From Java -----

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_nativeInit<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
) {
    if let Ok(vm) = env.get_java_vm() {
        let _ = VM.set(vm);
    }
    if let Ok(g) = env.new_global_ref(&class) {
        let _ = NATIVE.set(g);
    }
}

/// The media service started with no window (a car connected with the
/// app closed): start the player without it. `files` is the app's storage.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_startHeadless<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    files: JString<'local>,
) {
    if let Some(f) = opt_string(&mut env, &files) {
        crate::headless::start(Path::new(&f));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_permissionsDone<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    audio: jboolean,
) {
    let now = audio != 0;
    let before = CAN_READ.swap(now, Ordering::AcqRel);
    if now && !before {
        NEWLY_GRANTED.store(true, Ordering::Release);
    }
}

/// The system's Back, which predictive back no longer sends as a key. The
/// window picks it up on its next tick.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_backPressed<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    edge: jint,
) {
    platform::outside_action(match edge {
        1 => "back-left",
        2 => "back-right",
        _ => "back",
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_command<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    cmd: jint,
    arg: jlong,
) {
    if cmd == LOVE {
        // Hearting goes through the library and Last.fm, which whoever
        // holds the player (the window, or the car's player) does.
        platform::outside_action("love");
        return;
    }
    let Some(p) = PLAYER.lock().unwrap().clone() else {
        return;
    };
    p.send(match cmd {
        SHUFFLE => Command::ToggleShuffle,
        PLAY => Command::Play,
        PAUSE => Command::Pause,
        TOGGLE => Command::TogglePause,
        NEXT => Command::Next,
        BACK => Command::Back,
        STOP => Command::Stop,
        SEEK => Command::Seek(Duration::from_millis(arg.max(0) as u64)),
        QUEUE_ITEM if arg >= 0 => Command::PlayEntry(arg as u64),
        _ => return,
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_devicesChanged<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    ids: JIntArray<'local>,
    names: JObjectArray<'local>,
    usb: JBooleanArray<'local>,
) {
    let read = |env: &mut JNIEnv<'local>| -> jni::errors::Result<Vec<AndroidDevice>> {
        let n = env.get_array_length(&ids)? as usize;
        let mut id = vec![0; n];
        env.get_int_array_region(&ids, 0, &mut id)?;
        let mut u = vec![0u8; n];
        env.get_boolean_array_region(&usb, 0, &mut u)?;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let o = env.get_object_array_element(&names, i as i32)?;
            let s: &JString = (&o).into();
            let name: String = env.get_string(s)?.into();
            out.push(AndroidDevice {
                id: id[i],
                name,
                usb: u[i] != 0,
            });
        }
        Ok(out)
    };
    match read(&mut env) {
        Ok(v) => {
            log::info!("audio devices: {v:?}");
            aaudio::set_devices(v);
        }
        Err(e) => log::warn!("bad device list from Java: {e}"),
    }
}

/// A string Java may have passed as null.
fn opt_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string(s).ok().map(Into::into)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_pickDone<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    what: jint,
    path: JString<'local>,
    error: JString<'local>,
) {
    let path = opt_string(&mut env, &path);
    let error = opt_string(&mut env, &error);
    let picked = match (path, error) {
        (Some(p), _) if !p.is_empty() => Picked::Path(PathBuf::from(p)),
        (_, Some(e)) if !e.is_empty() => Picked::Unavailable(e),
        _ => Picked::Cancelled,
    };
    let waiting = PICK.lock().unwrap().take();
    match waiting {
        Some((w, tx)) if w == what => {
            let _ = tx.send(picked);
        }
        other => *PICK.lock().unwrap() = other,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_promptDone<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    values: JObjectArray<'local>,
) {
    if values.is_null() {
        return platform::prompt_event(id as u64, PromptEvent::Done(None));
    }
    let read = |env: &mut JNIEnv<'local>| -> jni::errors::Result<Vec<String>> {
        let n = env.get_array_length(&values)?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let o = env.get_object_array_element(&values, i)?;
            let s: JString = o.into();
            out.push(opt_string(env, &s).unwrap_or_default());
        }
        Ok(out)
    };
    let ev = match read(&mut env) {
        Ok(v) => PromptEvent::Done(Some(v)),
        Err(e) => {
            log::warn!("bad prompt answer from Java: {e}");
            PromptEvent::Done(None)
        }
    };
    platform::prompt_event(id as u64, ev);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_promptEdited<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    field: jint,
    text: JString<'local>,
) {
    let text = opt_string(&mut env, &text).unwrap_or_default();
    platform::prompt_event(
        id as u64,
        PromptEvent::Edited {
            field: field.max(0) as usize,
            text,
        },
    );
}

// ----- Android Auto -----

/// Its own connection to the library, for the car's browsing: Android asks
/// from its own thread, and SQLite in WAL mode lets this read alongside
/// the app's.
static AUTO_LIB: Mutex<Option<patina_core::library::Library>> = Mutex::new(None);

fn auto_children(parent: &str) -> Vec<patina_core::browse::auto::AutoItem> {
    let mut guard = AUTO_LIB.lock().unwrap();
    if guard.is_none() {
        match patina_core::library::Library::open(&patina_core::config::library_db_path()) {
            Ok(l) => *guard = Some(l),
            Err(e) => {
                log::warn!("car browsing can't open the library: {e}");
                return Vec::new();
            }
        }
    }
    guard
        .as_ref()
        .map(|l| patina_core::browse::auto::children(l, parent))
        .unwrap_or_default()
}

/// The rows under a browse id, five strings each: id, title, subtitle,
/// flags ("b" browsable, "p" playable, "g" its rows as a grid of covers),
/// and the heading it goes under (empty for none).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_browse<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    parent: JString<'local>,
) -> jni::sys::jobjectArray {
    let parent = opt_string(&mut env, &parent).unwrap_or_else(|| "root".into());
    let items = auto_children(&parent);
    let build = |env: &mut JNIEnv<'local>| -> jni::errors::Result<JObjectArray<'local>> {
        let arr = env.new_object_array(
            (items.len() * 6) as i32,
            "java/lang/String",
            JObject::null(),
        )?;
        for (i, it) in items.iter().enumerate() {
            let mut flags = String::new();
            if it.browsable {
                flags.push('b');
            }
            if it.playable {
                flags.push('p');
            }
            if it.grid {
                flags.push('g');
            }
            let art = match &it.art {
                Some(CarArt::Icon(name)) => format!("icon:{name}"),
                Some(a @ (CarArt::Cover(_) | CarArt::Artist(_))) => {
                    format!("art:{}", car_art_key(a))
                }
                None => String::new(),
            };
            for (j, text) in [
                it.id.as_str(),
                it.title.as_str(),
                it.subtitle.as_str(),
                flags.as_str(),
                it.group.as_str(),
                art.as_str(),
            ]
            .into_iter()
            .enumerate()
            {
                let js = env.new_string(text)?;
                env.set_object_array_element(&arr, (i * 6 + j) as i32, &js)?;
                // Two hundred rows are 1,200 strings: let each go
                // rather than fill the local reference table.
                env.delete_local_ref(js)?;
            }
        }
        Ok(arr)
    };
    match build(&mut env) {
        Ok(a) => a.into_raw(),
        Err(e) => {
            log::warn!("couldn't hand the car its list: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Covers handed to the car, by the key in their content URI. Only these
/// are served: the URI names a key, never a path, so nothing else on the
/// phone can ask the app's provider for any file it likes.
static CAR_ART: Mutex<Option<HashMap<String, CarArt>>> = Mutex::new(None);

thread_local! {
    static ART_LIB: std::cell::RefCell<Option<patina_core::library::Library>> =
        const { std::cell::RefCell::new(None) };
}

/// The key a cover or an artist's picture goes by in the car's URIs,
/// remembered so the provider can find it again.
fn car_art_key(art: &CarArt) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    art.hash(&mut h);
    let key = format!("{:016x}", h.finish());
    let mut map = CAR_ART.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    // A long drive through many lists: start over rather than grow
    // forever. The car asks for the lists it shows again.
    if map.len() > 20_000 {
        map.clear();
    }
    map.insert(key.clone(), art.clone());
    key
}

/// A cover's file for the car, by its key: ArtProvider.openFile. Runs on
/// one of the provider's binder threads, so it can take its time loading
/// the cover (the car shows a spinner meanwhile). Null for a key it never
/// gave out.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_carArt<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    key: JString<'local>,
) -> jni::sys::jstring {
    let path = opt_string(&mut env, &key).and_then(|k| {
        let art = CAR_ART.lock().unwrap().as_ref()?.get(&k)?.clone();
        // Only plain keys name files.
        if !k.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        match art {
            CarArt::Cover(track) => patina_core::browse::auto::car_art_file(&track, &k),
            // Its own library connection per binder thread: an artist's
            // picture can wait on a server, and browsing shouldn't wait
            // behind it.
            CarArt::Artist(name) => ART_LIB.with(|l| {
                let mut l = l.borrow_mut();
                if l.is_none() {
                    *l = patina_core::library::Library::open(
                        &patina_core::config::library_db_path(),
                    )
                    .ok();
                }
                patina_core::browse::auto::car_artist_file(l.as_ref()?, &name)
            }),
            CarArt::Icon(_) => None,
        }
    });
    match path.and_then(|p| env.new_string(p.to_string_lossy()).ok()) {
        Some(s) => s.into_raw(),
        None => std::ptr::null_mut(),
    }
}

/// Play what the car picked, by its browse id. The window's loop plays it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_playMediaId<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
) {
    if let Some(id) = opt_string(&mut env, &id) {
        platform::play_media_id(id);
    }
}

// ----- To Java -----

/// Ask Java to open the system's picker, and wait for what comes back.
/// Runs on a thread of its own: the wait is as long as the person takes.
fn pick(what: jint, method: &str) -> Picked {
    let (tx, rx) = mpsc::channel();
    // A pick still open from before (its answer lost) gives way.
    *PICK.lock().unwrap() = Some((what, tx));
    let started = with_java(|env, class| {
        env.call_static_method(class, method, "()V", &[])?;
        Ok(())
    });
    if started.is_none() {
        PICK.lock().unwrap().take();
        return Picked::Unavailable("The system's picker couldn't be opened".into());
    }
    rx.recv_timeout(PICK_PATIENCE).unwrap_or(Picked::Cancelled)
}

/// The system's folder picker (DocumentsUI), as a path the scanner reads.
pub fn pick_folder() -> Picked {
    pick(PICK_FOLDER, "pickFolder")
}

/// The system's file picker, the file copied where the app can read it.
pub fn pick_eq_file() -> Picked {
    pick(PICK_EQ_FILE, "pickFile")
}

/// The system's save dialog, for a copy of `src` offered as `name`.
/// Java copies the file to where it's saved. The answer is that place.
pub fn save_copy(src: &std::path::Path, name: &str) -> Picked {
    let (tx, rx) = mpsc::channel();
    *PICK.lock().unwrap() = Some((PICK_SAVE, tx));
    let src = src.to_string_lossy().into_owned();
    let name = name.to_string();
    let started = with_java(|env, class| {
        let s = env.new_string(&src)?;
        let n = env.new_string(&name)?;
        env.call_static_method(
            class,
            "saveCopy",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&s), JValue::Object(&n)],
        )?;
        Ok(())
    });
    if started.is_none() {
        PICK.lock().unwrap().take();
        return Picked::Unavailable("The system's save dialog couldn't be opened".into());
    }
    rx.recv_timeout(PICK_PATIENCE).unwrap_or(Picked::Cancelled)
}

/// The system's picker for an image, copied where the app can read it:
/// a cover for an album.
pub fn pick_image() -> Picked {
    pick(PICK_IMAGE, "pickImage")
}

/// Show a text form as an Android dialog. False if Java isn't there.
pub fn prompt(id: u64, p: &Prompt) -> bool {
    with_java(|env, class| {
        let n = p.fields.len() as i32;
        let labels = env.new_object_array(n, "java/lang/String", JObject::null())?;
        let values = env.new_object_array(n, "java/lang/String", JObject::null())?;
        let secret = env.new_boolean_array(n)?;
        let mut flags = Vec::with_capacity(n as usize);
        for (i, f) in p.fields.iter().enumerate() {
            let l = env.new_string(&f.label)?;
            env.set_object_array_element(&labels, i as i32, &l)?;
            let v = env.new_string(&f.value)?;
            env.set_object_array_element(&values, i as i32, &v)?;
            flags.push(u8::from(f.secret));
        }
        env.set_boolean_array_region(&secret, 0, &flags)?;
        let title = env.new_string(&p.title)?;
        let message = env.new_string(&p.message)?;
        let ok = env.new_string(&p.ok)?;
        env.call_static_method(
            class,
            "prompt",
            "(JLjava/lang/String;Ljava/lang/String;[Ljava/lang/String;[Ljava/lang/String;[ZLjava/lang/String;ZZ)V",
            &[
                JValue::Long(id as jlong),
                JValue::Object(&title),
                JValue::Object(&message),
                JValue::Object(&labels),
                JValue::Object(&values),
                JValue::Object(&secret),
                JValue::Object(&ok),
                JValue::Bool(u8::from(p.live)),
                JValue::Bool(u8::from(p.light)),
            ],
        )?;
        Ok(())
    })
    .is_some()
}

/// What's playing, for the media session and notification.
#[allow(clippy::too_many_arguments)]
pub fn on_state(
    title: &str,
    artist: &str,
    album: &str,
    art: Option<&Path>,
    duration_ms: i64,
    position_ms: i64,
    state: jint,
    loved: jint,
    shuffle: bool,
) {
    with_java(|env, class| {
        let t = env.new_string(title)?;
        let ar = env.new_string(artist)?;
        let al = env.new_string(album)?;
        let a = env.new_string(art.map(|p| p.to_string_lossy()).unwrap_or_default())?;
        env.call_static_method(
            class,
            "onState",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;JJIIZ)V",
            &[
                JValue::Object(&t),
                JValue::Object(&ar),
                JValue::Object(&al),
                JValue::Object(&a),
                JValue::Long(duration_ms),
                JValue::Long(position_ms),
                JValue::Int(state),
                JValue::Int(loved),
                JValue::Bool(u8::from(shuffle)),
            ],
        )?;
        Ok(())
    });
}

/// One song in the car's queue screen.
pub struct QueueItem {
    pub id: u64,
    pub title: String,
    pub sub: String,
    /// The song's file, for its cover.
    pub art: Option<PathBuf>,
}

/// What's queued, for the car's queue screen: Native.onQueue. Covers go by
/// the same keys as the car's browse lists.
pub fn on_queue(items: &[QueueItem], current: Option<u64>) {
    let ids: Vec<jlong> = items.iter().map(|i| i.id as jlong).collect();
    // Title, artist and cover key for each, one after another.
    let texts: Vec<String> = items
        .iter()
        .flat_map(|i| {
            let art = i
                .art
                .as_ref()
                .map(|p| car_art_key(&CarArt::Cover(p.clone())))
                .unwrap_or_default();
            [i.title.clone(), i.sub.clone(), art]
        })
        .collect();
    let current = current.map_or(-1, |c| c as jlong);
    with_java(|env, class| {
        // A frame of its own: this thread stays attached, so the arrays
        // would otherwise pile up in its reference table.
        env.with_local_frame(8, |env| -> jni::errors::Result<()> {
            let id_arr = env.new_long_array(ids.len() as i32)?;
            env.set_long_array_region(&id_arr, 0, &ids)?;
            let text_arr =
                env.new_object_array(texts.len() as i32, "java/lang/String", JObject::null())?;
            for (k, t) in texts.iter().enumerate() {
                let js = env.new_string(t)?;
                env.set_object_array_element(&text_arr, k as i32, &js)?;
                env.delete_local_ref(js)?;
            }
            env.call_static_method(
                class,
                "onQueue",
                "([J[Ljava/lang/String;J)V",
                &[
                    JValue::Object(&id_arr),
                    JValue::Object(&text_arr),
                    JValue::Long(current),
                ],
            )?;
            Ok(())
        })
    });
}

/// The bit-perfect hook: ask Java to set the USB DAC's mixer. Runs on the
/// player's thread. A rate of 0 clears it (device -1: every device).
pub fn set_bit_perfect(device: i32, rate: u32, bits: u32, channels: u16) -> Result<(), String> {
    with_java(|env, class| {
        let r = env
            .call_static_method(
                class,
                "setBitPerfect",
                "(IIII)Ljava/lang/String;",
                &[
                    JValue::Int(device),
                    JValue::Int(rate as jint),
                    JValue::Int(bits as jint),
                    JValue::Int(channels as jint),
                ],
            )?
            .l()?;
        if r.is_null() {
            return Ok(Ok(()));
        }
        let s: &JString = (&r).into();
        let why: String = env.get_string(s)?.into();
        Ok(Err(why))
    })
    .unwrap_or_else(|| Err("the Android side isn't ready".into()))
}

/// The system's Material You colors and dark mode, as one flat list (see
/// `patina_gui::system_theme::SystemColors::from_flat`). None before
/// Android 12, which has none.
/// The phone is in car mode (Android Auto, or a driving mode that turns
/// it on): Native.isDriving.
pub fn driving() -> bool {
    with_java(|env, class| env.call_static_method(class, "isDriving", "()Z", &[])?.z())
        .unwrap_or(false)
}

pub fn system_colors() -> Option<Vec<i32>> {
    with_java(|env, class| {
        let r = env
            .call_static_method(class, "systemColors", "()[I", &[])?
            .l()?;
        if r.is_null() {
            return Ok(None);
        }
        let arr = JIntArray::from(r);
        let n = env.get_array_length(&arr)? as usize;
        let mut v = vec![0; n];
        env.get_int_array_region(&arr, 0, &mut v)?;
        Ok(Some(v))
    })
    .flatten()
}

/// The theme's colors for the home screen widget, as ARGB (see
/// `patina_gui::platform::WidgetColors`). Java keeps them, so the widget
/// stays in the theme while the app is closed.
pub fn widget_colors(c: &[u32; 6]) {
    with_java(|env, class| {
        let v: Vec<jint> = c.iter().map(|&x| x as jint).collect();
        let arr = env.new_int_array(v.len() as i32)?;
        env.set_int_array_region(&arr, 0, &v)?;
        env.call_static_method(class, "widgetColors", "([I)V", &[JValue::Object(&arr)])?;
        Ok(())
    });
}

/// Back with nowhere left to go: send the app to the background.
pub fn move_to_back() {
    with_java(|env, class| {
        env.call_static_method(class, "moveToBack", "()V", &[])?;
        Ok(())
    });
}

/// The app is closing: take the notification down.
pub fn stopped() {
    with_java(|env, class| {
        env.call_static_method(class, "stopped", "()V", &[])?;
        Ok(())
    });
}
