//! The few things the window asks of the system it runs on, where the
//! desktop and Android differ: picking a folder or a file, and typing text.
//!
//! The desktop needs nothing set here. It uses its own dialogs (through the
//! XDG portal) and Slint's text fields. Android sets [`Hooks`] at start:
//! the system's folder and file pickers (DocumentsUI), and its own text
//! dialogs, since Slint's text field on Android loses track of the
//! keyboard (repeated text, no scrolling, covered by the keyboard).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// What a picker came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Path(PathBuf),
    /// Closed without picking.
    Cancelled,
    /// No picker could open, or what was picked can't be used; says why.
    Unavailable(String),
}

/// One text field in a [`Prompt`].
#[derive(Debug, Clone, Default)]
pub struct PromptField {
    pub label: String,
    pub value: String,
    /// A password: dots on screen, no suggestions.
    pub secret: bool,
}

/// A small form to fill in: the app's text dialogs, and search on Android.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub title: String,
    /// A line under the title, such as why the last try didn't take.
    pub message: String,
    pub fields: Vec<PromptField>,
    /// The confirm button's label.
    pub ok: String,
    /// Report every change as it's typed (search), and sit at the top of
    /// the screen so what it finds shows below.
    pub live: bool,
    /// The app is in a light theme.
    pub light: bool,
}

/// What a prompt reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptEvent {
    /// A field changed (live prompts only).
    Edited { field: usize, text: String },
    /// Closed: the fields' text on confirm, None on cancel.
    Done(Option<Vec<String>>),
}

/// The system's side of the pickers and prompts. Set once, by the
/// platform's own code, before the window starts.
pub trait Hooks: Send + Sync {
    /// Ask for a music folder. Blocks until it's answered, so it's called
    /// on a thread of its own.
    fn pick_folder(&self) -> Picked;
    /// Ask for an AutoEq or Equalizer APO file, as a path the app can
    /// read. Blocks, like [`Hooks::pick_folder`].
    fn pick_eq_file(&self) -> Picked;
    /// Show a prompt. Its answer comes back through [`prompt_event`] under
    /// `id`. False if it couldn't be shown, and the window's own dialog is
    /// used instead.
    fn prompt(&self, id: u64, p: &Prompt) -> bool;
    /// The system's colors for the System theme (Material You), asked
    /// every couple of seconds while it's in use, so a wallpaper or
    /// dark mode change shows. None where there are none.
    fn system_colors(&self) -> Option<crate::system_theme::SystemColors> {
        None
    }
    /// The theme's colors for a home screen widget, sent whenever they
    /// change: see [`WidgetColors`].
    fn widget_colors(&self, _c: &WidgetColors) {}
}

/// The colors a home screen widget takes from the app's theme, as ARGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetColors {
    /// The widget's ground.
    pub surface: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    /// The play button.
    pub accent: u32,
    /// The play icon on it.
    pub on_accent: u32,
}

impl WidgetColors {
    pub fn to_array(self) -> [u32; 6] {
        [
            self.surface,
            self.border,
            self.text,
            self.muted,
            self.accent,
            self.on_accent,
        ]
    }
}

/// Hand the theme's colors to the platform's widget, if it has one.
pub(crate) fn widget_colors(c: &WidgetColors) {
    if let Some(h) = hooks() {
        h.widget_colors(c);
    }
}

static HOOKS: OnceLock<Box<dyn Hooks>> = OnceLock::new();
static EVENTS: Mutex<Vec<(u64, PromptEvent)>> = Mutex::new(Vec::new());
/// Things picked to play outside the window (a car, through Android Auto),
/// by their browse id, waiting for the window's loop to play them.
static MEDIA: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Play something by its car browse id; see `patina_core::browse::auto`.
/// Safe from any thread: the window's loop picks it up on its next tick.
pub fn play_media_id(id: String) {
    MEDIA.lock().unwrap().push(id);
}

pub(crate) fn take_media_ids() -> Vec<String> {
    std::mem::take(&mut *MEDIA.lock().unwrap())
}

/// Set the platform's hooks. Only the first call counts.
pub fn set_hooks(h: Box<dyn Hooks>) {
    let _ = HOOKS.set(h);
}

/// The system's colors, if the platform has any.
pub(crate) fn system_colors() -> Option<crate::system_theme::SystemColors> {
    hooks().and_then(|h| h.system_colors())
}

pub(crate) fn hooks() -> Option<&'static dyn Hooks> {
    HOOKS.get().map(|h| h.as_ref())
}

/// A prompt's answer, from any thread. The window picks it up on its next
/// tick, within a frame or two.
pub fn prompt_event(id: u64, ev: PromptEvent) {
    if let Ok(mut q) = EVENTS.lock() {
        q.push((id, ev));
    }
}

pub(crate) fn take_prompt_events() -> Vec<(u64, PromptEvent)> {
    EVENTS
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}
