//! Key detection from a chromagram.
//!
//! A long-window spectrum is folded onto the twelve pitch classes, giving a
//! chroma vector: how much energy each note name carries across the track.
//! That vector is correlated with a major and a minor key profile rotated
//! to each of the twelve tonics, and the best of the 24 wins.
//!
//! The profiles are the Krumhansl-Kessler probe-tone ratings. libKeyFinder
//! (GPL-3) is the reference for the overall design (chromagram, profile
//! correlation); this is an independent implementation, not a port, and it
//! does not use libKeyFinder's own profiles or its segmentation.

use fourfloor_core::SampleRate;

use crate::error::AnalysisError;
use crate::key::{Key, Mode};
use crate::stft::{fft_size_for, for_each_frame};

/// Krumhansl-Kessler major profile, starting at the tonic.
const MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
/// Krumhansl-Kessler minor profile, starting at the tonic.
const MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Analysis window in seconds (32768 samples at 48 kHz). Long, because
/// telling apart bass notes a semitone apart needs fine frequency
/// resolution and key detection needs no time resolution.
const WINDOW_SECONDS: f64 = 0.68;

/// Settings for [`detect_key`] and [`chromagram`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyOptions {
    /// Frequency of the A above middle C. Default 440. The tuning is not
    /// estimated from the audio.
    pub tuning_hz: f64,
    /// Lowest frequency folded into the chroma. Default 55 (A1).
    pub min_hz: f64,
    /// Highest frequency folded into the chroma. Default 2000.
    pub max_hz: f64,
}

impl Default for KeyOptions {
    fn default() -> Self {
        Self {
            tuning_hz: 440.0,
            min_hz: 55.0,
            max_hz: 2000.0,
        }
    }
}

impl KeyOptions {
    fn validate(&self) -> Result<(), AnalysisError> {
        let invalid = |name, reason| Err(AnalysisError::InvalidOption { name, reason });
        if !(self.tuning_hz.is_finite() && (300.0..=600.0).contains(&self.tuning_hz)) {
            return invalid("tuning_hz", "must be between 300 and 600");
        }
        if !(self.min_hz.is_finite() && self.min_hz >= 20.0) {
            return invalid("min_hz", "must be at least 20");
        }
        if !(self.max_hz.is_finite() && self.max_hz > self.min_hz) {
            return invalid("max_hz", "must be above min_hz");
        }
        Ok(())
    }
}

/// Energy per pitch class over time. Index 0 is C, 11 is B.
#[derive(Clone, Debug, PartialEq)]
pub struct Chromagram {
    /// Frames per second.
    pub frame_rate: f64,
    /// One chroma vector per frame.
    pub frames: Vec<[f32; 12]>,
}

impl Chromagram {
    /// The sum of all frames, scaled so the largest pitch class is 1. All
    /// zeros for silence.
    pub fn total(&self) -> [f32; 12] {
        let mut total = [0.0f32; 12];
        for frame in &self.frames {
            for (sum, &value) in total.iter_mut().zip(frame) {
                *sum += value;
            }
        }
        normalize_peak(total)
    }
}

fn normalize_peak(mut chroma: [f32; 12]) -> [f32; 12] {
    let peak = chroma.iter().copied().fold(0.0, f32::max);
    if peak > 0.0 {
        for value in &mut chroma {
            *value /= peak;
        }
    }
    chroma
}

/// Computes the chromagram of a mono signal.
pub fn chromagram(
    samples: &[f32],
    sample_rate: SampleRate,
    options: &KeyOptions,
) -> Result<Chromagram, AnalysisError> {
    options.validate()?;
    let rate = sample_rate.as_f64();
    let window = fft_size_for(WINDOW_SECONDS, rate);
    let hop = window / 4;
    let bin_hz = rate / window as f64;

    // For every bin in range: its pitch class and a weight that is 1 at the
    // centre of a semitone and 0 a quarter tone away, so that energy
    // between two notes is not credited in full to either.
    let first = ((options.min_hz / bin_hz).ceil() as usize).max(1);
    let last = ((options.max_hz.min(rate / 2.0) / bin_hz).floor() as usize).min(window / 2);
    let map: Vec<(usize, f32)> = (first..=last.max(first))
        .map(|bin| {
            let pitch = 69.0 + 12.0 * (bin as f64 * bin_hz / options.tuning_hz).log2();
            let nearest = pitch.round();
            let weight = (std::f64::consts::PI * (pitch - nearest)).cos().powi(2);
            ((nearest as i64).rem_euclid(12) as usize, weight as f32)
        })
        .collect();

    let mut frames = Vec::with_capacity(samples.len() / hop + 1);
    for_each_frame(samples, window, hop, |_, power| {
        let mut chroma = [0.0f32; 12];
        for (&(class, weight), &value) in map.iter().zip(power.iter().skip(first)) {
            chroma[class] += weight * value.sqrt();
        }
        frames.push(chroma);
    });
    Ok(Chromagram {
        frame_rate: rate / hop as f64,
        frames,
    })
}

/// One candidate key and how well it fits.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyScore {
    /// The key.
    pub key: Key,
    /// Correlation of the chroma with this key's profile, -1 to 1.
    pub score: f32,
}

/// The result of [`detect_key`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyEstimate {
    /// The best-fitting key.
    pub key: Key,
    /// How far the best key's correlation is ahead of the runner-up's, 0
    /// to 1. Small values mean the runner-up (often the relative key) is
    /// nearly as good.
    pub confidence: f32,
    /// Energy per pitch class from C to B, largest scaled to 1.
    pub chroma: [f32; 12],
    /// All 24 keys, best first.
    pub scores: Vec<KeyScore>,
}

/// Detects the key of a mono signal.
pub fn detect_key(
    samples: &[f32],
    sample_rate: SampleRate,
    options: &KeyOptions,
) -> Result<KeyEstimate, AnalysisError> {
    if samples.is_empty() {
        return Err(AnalysisError::Empty);
    }
    let chroma = chromagram(samples, sample_rate, options)?.total();
    key_from_chroma(&chroma)
}

/// Picks the key that best fits a chroma vector (C first).
pub fn key_from_chroma(chroma: &[f32; 12]) -> Result<KeyEstimate, AnalysisError> {
    let all_finite = chroma.iter().all(|value| value.is_finite());
    let spread = chroma.iter().copied().fold(0.0, f32::max)
        - chroma.iter().copied().fold(f32::INFINITY, f32::min);
    // A flat chroma (noise, silence, drums alone) fits every key equally.
    if !all_finite || spread <= 1e-6 {
        return Err(AnalysisError::NoTonalContent);
    }
    let mut scores: Vec<KeyScore> = Key::all()
        .map(|key| KeyScore {
            key,
            score: correlate(chroma, key),
        })
        .collect();
    scores.sort_by(|a, b| b.score.total_cmp(&a.score));
    let confidence = (scores[0].score - scores[1].score).clamp(0.0, 1.0);
    Ok(KeyEstimate {
        key: scores[0].key,
        confidence,
        chroma: normalize_peak(*chroma),
        scores,
    })
}

/// Pearson correlation of `chroma` with the profile of `key`.
fn correlate(chroma: &[f32; 12], key: Key) -> f32 {
    let profile = match key.mode() {
        Mode::Major => &MAJOR_PROFILE,
        Mode::Minor => &MINOR_PROFILE,
    };
    let tonic = usize::from(key.tonic());
    let chroma_mean = chroma.iter().sum::<f32>() / 12.0;
    let profile_mean = profile.iter().sum::<f32>() / 12.0;
    let (mut dot, mut chroma_power, mut profile_power) = (0.0f32, 0.0f32, 0.0f32);
    for (class, &value) in chroma.iter().enumerate() {
        let a = value - chroma_mean;
        let b = profile[(class + 12 - tonic) % 12] - profile_mean;
        dot += a * b;
        chroma_power += a * a;
        profile_power += b * b;
    }
    let scale = (chroma_power * profile_power).sqrt();
    if scale > 0.0 {
        dot / scale
    } else {
        0.0
    }
}
