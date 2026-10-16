//! Both directions across JNI, to and from `dev.patina.player.Native` in
//! `android/java`.
//!
//! Java calls in through the `Java_dev_patina_player_Native_*` functions.
//! Rust calls out through the class reference kept when Java first called
//! `nativeInit`: a thread Rust started can't look the app's classes up by
//! name, since it only sees the system's class loader.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JBooleanArray, JClass, JIntArray, JObjectArray, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use patina_core::audio::output::aaudio::{self, AndroidDevice};
use patina_core::{Command, Player};

// Must match Native.java.
const PLAY: jint = 1;
const PAUSE: jint = 2;
const TOGGLE: jint = 3;
const NEXT: jint = 4;
const BACK: jint = 5;
const STOP: jint = 6;
const SEEK: jint = 7;

pub const STOPPED: jint = 0;
pub const PLAYING: jint = 1;
pub const PAUSED: jint = 2;

static VM: OnceLock<JavaVM> = OnceLock::new();
static NATIVE: OnceLock<GlobalRef> = OnceLock::new();
static PLAYER: Mutex<Option<Player>> = Mutex::new(None);
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_patina_player_Native_command<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    cmd: jint,
    arg: jlong,
) {
    let Some(p) = PLAYER.lock().unwrap().clone() else {
        return;
    };
    p.send(match cmd {
        PLAY => Command::Play,
        PAUSE => Command::Pause,
        TOGGLE => Command::TogglePause,
        NEXT => Command::Next,
        BACK => Command::Back,
        STOP => Command::Stop,
        SEEK => Command::Seek(Duration::from_millis(arg.max(0) as u64)),
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

// ----- To Java -----

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
) {
    with_java(|env, class| {
        let t = env.new_string(title)?;
        let ar = env.new_string(artist)?;
        let al = env.new_string(album)?;
        let a = env.new_string(art.map(|p| p.to_string_lossy()).unwrap_or_default())?;
        env.call_static_method(
            class,
            "onState",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;JJI)V",
            &[
                JValue::Object(&t),
                JValue::Object(&ar),
                JValue::Object(&al),
                JValue::Object(&a),
                JValue::Long(duration_ms),
                JValue::Long(position_ms),
                JValue::Int(state),
            ],
        )?;
        Ok(())
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
