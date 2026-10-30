//! Errors of the DJ crate.

use std::path::PathBuf;

use fourfloor_analysis::AnalysisError;
use fourfloor_audio::{DecodeError, WavError};
use fourfloor_core::RegistryError;
use fourfloor_engine::GraphError;

/// Why a track could not be built or loaded.
#[derive(Debug, thiserror::Error)]
pub enum TrackError {
    /// The audio has no frames.
    #[error("the track is empty")]
    Empty,
    /// Left and right (or a stem and the mix) differ in length.
    #[error("channel lengths differ: {left} and {right} frames")]
    ChannelMismatch {
        /// Frames in the first channel.
        left: usize,
        /// Frames in the other channel.
        right: usize,
    },
    /// A stem is at another sample rate or length than the track.
    #[error("stem \"{name}\" does not match the track: {reason}")]
    StemMismatch {
        /// The stem's name.
        name: String,
        /// What differs.
        reason: &'static str,
    },
    /// More stems than a deck can play.
    #[error("{count} stems given, at most {max} are supported")]
    TooManyStems {
        /// Stems given.
        count: usize,
        /// The limit ([`MAX_STEMS`](crate::MAX_STEMS)).
        max: usize,
    },
    /// The beat grid is not usable.
    #[error("invalid beat grid: {0}")]
    InvalidGrid(&'static str),
    /// The file could not be decoded.
    #[error("could not decode the track: {0}")]
    Decode(#[from] DecodeError),
}

/// Anything that can go wrong driving a DJ session.
#[derive(Debug, thiserror::Error)]
pub enum DjError {
    /// The command names a deck that does not exist.
    #[error("there is no deck {0}")]
    NoSuchDeck(usize),
    /// The command needs a loaded track.
    #[error("deck {0} has no track loaded")]
    NoTrack(usize),
    /// The command needs a beat grid and the deck's track has none.
    #[error("deck {0} has no beat grid (tempo unknown)")]
    NoGrid(usize),
    /// The deck's track has no stem of that name.
    #[error("the track on deck {deck} has no stem \"{stem}\"")]
    NoSuchStem {
        /// The deck.
        deck: usize,
        /// The stem name given.
        stem: String,
    },
    /// A parameter change named a node the graph does not have.
    #[error("the graph has no node {0}")]
    NoSuchNode(u64),
    /// A slot, stem or band index is out of range.
    #[error("{what} {index} is out of range (limit {limit})")]
    OutOfRange {
        /// What kind of index.
        what: &'static str,
        /// The index given.
        index: usize,
        /// One past the largest valid index.
        limit: usize,
    },
    /// A value is not finite or outside what the control accepts.
    #[error("invalid value for {what}: {value}")]
    InvalidValue {
        /// The control.
        what: &'static str,
        /// The value given.
        value: f64,
    },
    /// The session was built with an unsupported number of decks.
    #[error("a session has 2 to 4 decks, not {0}")]
    DeckCount(usize),
    /// The audio thread is not taking commands (its queue is full).
    #[error("the command queue of {0} is full")]
    QueueFull(&'static str),
    /// A track failed to load.
    #[error(transparent)]
    Track(#[from] TrackError),
    /// Analysis failed.
    #[error("analysis failed: {0}")]
    Analysis(#[from] AnalysisError),
    /// The effect kind is not in the registry, or failed to build.
    #[error(transparent)]
    Effect(#[from] RegistryError),
    /// The effect is not an audio effect.
    #[error("\"{0}\" is not an audio effect")]
    NotAnEffect(String),
    /// The effect slot has no such parameter.
    #[error("the effect on deck {deck} has no parameter \"{param}\"")]
    NoSuchParam {
        /// The deck.
        deck: usize,
        /// The parameter name given.
        param: String,
    },
    /// A recording is already running, or none is.
    #[error("recording: {0}")]
    Recording(&'static str),
    /// The recording could not be written.
    #[error("could not write the recording: {0}")]
    Wav(#[from] WavError),
    /// A cache or session file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What failed.
        source: std::io::Error,
    },
    /// A cache or session file is not valid JSON of the expected shape.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The engine graph could not be built.
    #[error("could not build the DJ graph: {0}")]
    Graph(#[from] GraphError),
}
