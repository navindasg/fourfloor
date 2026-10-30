//! Offline audio analysis for fourfloor, in pure Rust on `rustfft` and
//! `fourfloor-audio`.
//!
//! | Question | Entry point |
//! | --- | --- |
//! | Everything about a track at once | [`analyze_track`], [`analyze_file`] |
//! | Where do sounds start? | [`detect_onsets`], [`onset_envelope`] |
//! | What tempo, where are the beats and bars? | [`estimate_tempo`] gives a [`BeatGrid`] |
//! | What key, and what mixes with it? | [`detect_key`], [`Key`] (Camelot, Open Key, [`Key::is_compatible_with`]) |
//! | How loud, how wide, how dynamic? | [`measure_levels`], [`CorrelationMeter`] |
//! | Spectrum for a meter | [`SpectrumAnalyzer`], [`spectrum_frames`] |
//! | How does my mix differ from a reference? | [`compare_to_reference`] |
//! | Which sounds are alike? | [`extract_features`], [`FeatureVector::distance`] |
//! | Waveform to draw | [`WaveformOverview`] |
//!
//! Functions that analyze one signal take mono `&[f32]` and a sample rate;
//! use [`fourfloor_audio::AudioBuffer::to_mono`] to get there. Functions
//! that need the stereo image take an `AudioBuffer`.
//!
//! Nothing here is real-time safe: everything allocates. Run it on a
//! worker thread, never on the audio thread.
//!
//! With the `serde` feature the result types derive `Serialize` and
//! `Deserialize`. Levels can be negative infinity (silence), which JSON
//! writes as `null` and cannot read back.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod beatgrid;
pub mod downbeat;
pub mod error;
pub mod features;
pub mod key;
pub mod keydetect;
pub mod levels;
pub mod onset;
pub mod reference;
pub mod spectrum;
mod stft;
pub mod tempo;
pub mod track;
pub mod waveform;

pub use fourfloor_core as core;

pub use beatgrid::BeatGrid;
pub use downbeat::{guess_downbeat, DownbeatGuess};
pub use error::AnalysisError;
pub use features::{
    extract_features, extract_features_with_tempo, FeatureVector, FeatureWeights, EMBEDDING_LEN,
    MFCC_COUNT,
};
pub use key::{HarmonicRelation, Key, Mode, ParseKeyError};
pub use keydetect::{
    chromagram, detect_key, key_from_chroma, Chromagram, KeyEstimate, KeyOptions, KeyScore,
};
pub use levels::{measure_levels, stereo_correlation, stereo_width, CorrelationMeter, LevelReport};
pub use onset::{
    detect_onsets, onset_envelope, onsets_from_envelope, Onset, OnsetEnvelope, OnsetOptions,
};
pub use reference::{
    compare_to_reference, compare_to_reference_with, BandDelta, ReferenceOptions, ReferenceReport,
    Suggestion, SuggestionKind,
};
pub use spectrum::{spectrum_frames, SpectrumAnalyzer, SpectrumFrame, SpectrumOptions};
pub use tempo::{estimate_tempo, TempoEstimate, TempoOptions};
pub use track::{analyze_file, analyze_track, TrackAnalysis, TrackOptions};
pub use waveform::{Peak, WaveformLevel, WaveformOptions, WaveformOverview};
