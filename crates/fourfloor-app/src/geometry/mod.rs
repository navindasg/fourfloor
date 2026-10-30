//! Pure screen geometry: no egui state, no project, fully unit-tested.

pub mod deck;
pub mod grid;
pub mod keys;
pub mod timeline;

pub use grid::CellGrid;
pub use keys::{is_black_key, pitch_name, KeyMap};
pub use timeline::{bar_beat_label, GridKind, Snap, TimeMap, BAR_TICKS, SIXTEENTH_TICKS};
