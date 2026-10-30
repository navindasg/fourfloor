//! Everything the library and DJ browser want to know about a track, in
//! one call.

use std::path::Path;

use fourfloor_audio::AudioBuffer;

use crate::error::AnalysisError;
use crate::features::{extract_features_with_tempo, FeatureVector};
use crate::keydetect::{detect_key, KeyEstimate, KeyOptions};
use crate::levels::{measure_levels, LevelReport};
use crate::tempo::{estimate_tempo, TempoEstimate, TempoOptions};

/// Settings for [`analyze_track`].
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TrackOptions {
    /// Tempo search settings.
    pub tempo: TempoOptions,
    /// Key detection settings.
    pub key: KeyOptions,
}

/// The analysis of one track or sample.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackAnalysis {
    /// Sample rate of the audio in Hz.
    pub sample_rate: u32,
    /// Number of channels.
    pub channels: usize,
    /// Length in seconds.
    pub duration_seconds: f64,
    /// Tempo and beat grid, or `None` when the audio is too short or has
    /// no regular beat (one-shots, pads, silence): see
    /// [`TempoEstimate::is_reliable`].
    pub tempo: Option<TempoEstimate>,
    /// Key, or `None` when the audio has no pitched content.
    pub key: Option<KeyEstimate>,
    /// Loudness, peaks and stereo image.
    pub levels: LevelReport,
    /// Feature vector for similarity search.
    pub features: FeatureVector,
}

/// Analyzes a decoded track: tempo and beat grid, key, levels and features.
pub fn analyze_track(
    buffer: &AudioBuffer,
    options: &TrackOptions,
) -> Result<TrackAnalysis, AnalysisError> {
    if buffer.frames() == 0 || buffer.channel_count() == 0 {
        return Err(AnalysisError::Empty);
    }
    let mono = buffer.to_mono();
    let rate = buffer.sample_rate;
    let tempo = match estimate_tempo(&mono, rate, &options.tempo) {
        Ok(estimate) => Some(estimate).filter(TempoEstimate::is_reliable),
        Err(AnalysisError::InvalidOption { name, reason }) => {
            return Err(AnalysisError::InvalidOption { name, reason })
        }
        Err(_) => None,
    };
    let key = match detect_key(&mono, rate, &options.key) {
        Ok(estimate) => Some(estimate),
        Err(AnalysisError::InvalidOption { name, reason }) => {
            return Err(AnalysisError::InvalidOption { name, reason })
        }
        Err(_) => None,
    };
    let bpm = tempo.map(|estimate| estimate.bpm());
    Ok(TrackAnalysis {
        sample_rate: rate.hz(),
        channels: buffer.channel_count(),
        duration_seconds: buffer.duration_seconds(),
        tempo,
        key,
        levels: measure_levels(buffer)?,
        features: extract_features_with_tempo(&mono, rate, bpm)?,
    })
}

/// Decodes an audio file (WAV, AIFF, FLAC, MP3 or Ogg Vorbis) and analyzes
/// it with default options.
pub fn analyze_file(path: &Path) -> Result<TrackAnalysis, AnalysisError> {
    let buffer = fourfloor_audio::decode_file(path)?;
    analyze_track(&buffer, &TrackOptions::default())
}
