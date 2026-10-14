//! PatinaPlayer core. Owns all state (queue, playback, output) so the TUI
//! and GUI are thin frontends that send commands and draw snapshots.

pub mod art;
pub mod audio;
pub mod browse;
pub mod config;
pub mod library;
pub mod player;
pub mod podcast;
pub mod queue;
pub mod radio;
pub mod remote;
pub mod secrets;
pub mod session;
pub mod track;

pub use audio::output::{Backend, OutputConfig};
pub use player::{Command, Event, PlayState, Player, PlayerConfig, Snapshot};
pub use queue::Repeat;
pub use track::TrackInfo;
