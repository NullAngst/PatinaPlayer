//! PatinaPlayer's window. The desktop app (`main.rs`) and the Android app
//! (`crates/patina-android`) both run it: the Slint UI in `ui/app.slint`,
//! and the bridge that feeds it from the core and sends what you do back.
//!
//! The window steps down in size tiers on its own, from a wide desktop
//! window to a phone screen, so both get the same file.

slint::include_modules!();

mod art;
pub mod bridge;
pub mod desktop;
pub mod platform;
pub mod system_theme;
