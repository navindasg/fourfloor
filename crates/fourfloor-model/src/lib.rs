//! Edit model for fourfloor: project structs, typed commands, undo and the
//! on-disk project format.
//!
//! - [`Project`] and friends are plain serde structs. A project is a value:
//!   edits produce a new project that shares untouched tracks through `Arc`.
//! - [`Command`] is the only way to edit. Each command has a stable name and
//!   a JSON Schema ([`command_schemas`]), so an agent tool layer can map one
//!   tool to one command.
//! - [`CommandBus`] validates and applies commands and keeps undo and redo.
//! - [`storage`] saves and loads the project folder: a versioned
//!   `project.json` plus content-hashed audio and plugin state files.
//! - [`sequencer`] holds swing math and a house pattern writer; [`groove`]
//!   holds groove templates and [`music`] scales, chords and quantizing.
//! - [`modulation`] holds modulators, macros, controller mappings and
//!   launcher scenes.
//!
//! Nothing here runs on the audio thread; this crate allocates freely.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bus;
pub mod checks;
mod cleanup;
pub mod clip;
pub mod command;
pub mod device;
pub mod error;
pub mod groove;
pub mod hash;
pub mod ids;
pub mod modulation;
pub mod music;
pub mod project;
pub mod sequencer;
pub mod storage;
pub mod track;
pub mod validate;

pub use fourfloor_core as core;

pub use bus::{CommandBus, HistoryEntry, Transaction};
pub use clip::{AudioRef, Clip, ClipContent, MidiNote, Warp};
pub use command::{command_schemas, Command, CommandSchema, Outcome};
pub use device::{AutomationLane, AutomationPoint, DeviceInstance, Param, SidechainSource};
pub use error::ModelError;
pub use groove::{GrooveStep, GrooveTemplate, BUILTIN_GROOVES};
pub use hash::ContentHash;
pub use ids::{MacroId, ModulatorId, SceneId};
pub use modulation::{
    ControllerMapping, LfoRate, LfoShape, Macro, MacroTarget, ModSource, ModTarget, Modulator,
    Scene,
};
pub use music::{ChordKind, Scale};
pub use project::{Groove, Project, SwingGrid, DEFAULT_LAUNCH_QUANTIZE};
pub use storage::{load_project, save_project, AudioPool, PluginStateStore, StorageError};
pub use track::{MixerLane, MixerParam, Send, SessionSlot, Track, TrackKind};
