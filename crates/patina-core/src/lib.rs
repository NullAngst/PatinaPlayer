//! PatinaPlayer core. Owns all state (queue, playback, output) so the TUI
//! and GUI are thin frontends that send commands and draw snapshots.

pub mod audio;
pub mod player;
pub mod queue;
pub mod track;

pub use audio::output::{Backend, OutputConfig};
pub use player::{Command, Event, PlayState, Player, PlayerConfig, Snapshot};
pub use queue::Repeat;
pub use track::TrackInfo;
