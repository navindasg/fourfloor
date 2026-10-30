//! Typed errors for the edit model.

use fourfloor_core::{ClipId, DeviceId, ParamId, TrackId};

use crate::track::TrackKind;

/// Why a command was rejected or a project failed validation.
///
/// A rejected command never changes the project.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ModelError {
    /// No track has this id.
    #[error("track {0} not found")]
    TrackNotFound(TrackId),
    /// No clip has this id.
    #[error("clip {0} not found")]
    ClipNotFound(ClipId),
    /// The clip exists but lives on a different track than the one named.
    #[error("clip {clip} is not on track {track}")]
    ClipNotOnTrack {
        /// The clip that was named.
        clip: ClipId,
        /// The track it was expected on.
        track: TrackId,
    },
    /// No device has this id.
    #[error("device {0} not found")]
    DeviceNotFound(DeviceId),
    /// The device has no parameter with this id.
    #[error("device {device} has no parameter {param}")]
    ParamNotFound {
        /// The device that was named.
        device: DeviceId,
        /// The missing parameter.
        param: ParamId,
    },
    /// The operation needs a different kind of track.
    #[error("track {track} is a {actual} track; this needs a {expected} track")]
    WrongTrackKind {
        /// The track that was named.
        track: TrackId,
        /// The kind the operation needs.
        expected: TrackKind,
        /// The kind the track has.
        actual: TrackKind,
    },
    /// A name is empty, too long or contains control characters.
    #[error("invalid {field}: {reason}")]
    InvalidName {
        /// Which name was rejected.
        field: &'static str,
        /// Why.
        reason: String,
    },
    /// Tempo is not finite or outside 20 to 999 BPM.
    #[error("tempo {bpm} BPM is outside 20 to 999")]
    InvalidTempo {
        /// The rejected value.
        bpm: f64,
    },
    /// Swing is not finite or outside 50 to 75 percent.
    #[error("swing {percent}% is outside 50 to 75")]
    InvalidSwing {
        /// The rejected value.
        percent: f64,
    },
    /// A parameter or automation value is outside the parameter's range.
    #[error("value {value} for parameter {param} of device {device} is outside {min} to {max}")]
    ParamOutOfRange {
        /// The device that owns the parameter.
        device: DeviceId,
        /// The parameter.
        param: ParamId,
        /// The rejected value.
        value: f32,
        /// Lowest allowed value.
        min: f32,
        /// Highest allowed value.
        max: f32,
    },
    /// A MIDI note is malformed.
    #[error("note {index} is invalid: {reason}")]
    InvalidNote {
        /// Position of the note in the submitted list.
        index: usize,
        /// Why.
        reason: String,
    },
    /// Some other field failed validation.
    #[error("invalid {field}: {reason}")]
    InvalidValue {
        /// Which field was rejected.
        field: &'static str,
        /// Why.
        reason: String,
    },
    /// An insert position is past the end of the list.
    #[error("index {index} is out of range (length {len})")]
    IndexOutOfRange {
        /// The rejected index.
        index: usize,
        /// Length of the list.
        len: usize,
    },
    /// A track output would not form a valid route.
    #[error("track {track} cannot use that output: {reason}")]
    InvalidRouting {
        /// The track whose output was being set.
        track: TrackId,
        /// Why.
        reason: String,
    },
    /// No scene has this id.
    #[error("scene {0} not found")]
    SceneNotFound(crate::ids::SceneId),
    /// No modulator has this id.
    #[error("modulator {0} not found")]
    ModulatorNotFound(crate::ids::ModulatorId),
    /// No macro has this id.
    #[error("macro {0} not found")]
    MacroNotFound(crate::ids::MacroId),
    /// Something a command asked to remove is not there.
    #[error("{what} not found")]
    NotFound {
        /// What was looked for, for example `send to track 3`.
        what: String,
    },
    /// Two objects in a loaded project share an id, or an id is not below
    /// the project's id counter.
    #[error("id {0} is duplicated or not below the project's id counter")]
    InvalidId(u64),
    /// The project ran out of ids.
    #[error("the project has run out of ids")]
    IdExhausted,
    /// Undo was requested with an empty undo stack.
    #[error("nothing to undo")]
    NothingToUndo,
    /// Redo was requested with an empty redo stack.
    #[error("nothing to redo")]
    NothingToRedo,
    /// A command name or its arguments could not be decoded.
    #[error("cannot decode command `{name}`: {reason}")]
    Decode {
        /// The command name that was supplied.
        name: String,
        /// The decoder's message.
        reason: String,
    },
}
