//! Pattern generators for house music: drums, basslines and chord parts.
//!
//! Every generator mixes fixed rules (what makes the part sound like
//! house) with Markov chains (what makes two patterns differ). They take
//! the random source as an argument, so a seed fully decides the result,
//! and they return plain data on the straight sixteenth grid: nothing here
//! touches a project. The tool layer writes the result through the command
//! bus, which is where swing is applied.

mod bass;
mod chords;
mod drums;
mod key;

pub use bass::{bassline, BassOptions, BassStyle};
pub use chords::{chord_part, progression, ChordOptions, ChordPart, ChordStyle};
pub use drums::{house_drums, DrumLane, DrumOptions, DrumPattern, DrumStyle, DrumVoice};
pub use key::{parse_pitch_class, scale_name, Key, KeyParseError, PITCH_CLASS_NAMES};

/// Longest pattern a generator makes, in bars. Matches the model's limit
/// for step patterns.
pub const MAX_BARS: u32 = fourfloor_model::sequencer::MAX_PATTERN_BARS;

/// Why a generator refused its options.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GenerateError {
    /// The length is not 1 to [`MAX_BARS`] bars.
    #[error("{0} bars is outside 1 to 64")]
    Bars(u32),
    /// A numeric option is out of range.
    #[error("invalid {field}: {reason}")]
    Option {
        /// The option.
        field: &'static str,
        /// Why.
        reason: String,
    },
}

/// Clamps a 0-to-1 option; non-finite values become 0.5.
pub(crate) fn clamp_unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}
