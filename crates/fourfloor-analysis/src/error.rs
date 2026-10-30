//! The error type shared by every analyzer.

use fourfloor_audio::{DecodeError, MeterError};

/// Why an analysis could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// The audio has no frames or no channels.
    #[error("there is no audio to analyze")]
    Empty,
    /// The audio is shorter than the analyzer needs.
    #[error("audio is too short: {got_seconds:.2} s, need at least {needed_seconds:.2} s")]
    TooShort {
        /// Shortest length the analyzer accepts, in seconds.
        needed_seconds: f64,
        /// Length that was supplied, in seconds.
        got_seconds: f64,
    },
    /// The audio is silent, or steady enough that it has no transients, so
    /// there is nothing to hang a tempo on.
    #[error("no rhythmic content was found")]
    NoBeat,
    /// The audio has no pitched content to take a key from.
    #[error("no tonal content was found")]
    NoTonalContent,
    /// An option is outside its valid range.
    #[error("invalid option {name}: {reason}")]
    InvalidOption {
        /// Name of the option.
        name: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The two buffers of a comparison have different sample rates.
    #[error("sample rates differ: {left} Hz and {right} Hz")]
    SampleRateMismatch {
        /// Rate of the first buffer.
        left: u32,
        /// Rate of the second buffer.
        right: u32,
    },
    /// The loudness meter failed.
    #[error("loudness measurement failed: {0}")]
    Meter(#[from] MeterError),
    /// The file could not be decoded.
    #[error("could not decode audio: {0}")]
    Decode(#[from] DecodeError),
}
