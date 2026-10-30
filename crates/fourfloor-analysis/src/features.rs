//! Feature vectors for similarity search: a compact description of how a
//! track or sample sounds, and a distance between two of them.
//!
//! The features are the classic ones: tempo, spectral shape (centroid,
//! roll-off, flatness, zero crossings), timbre as mel-frequency cepstral
//! coefficients, and harmony as a chroma vector. bliss-rs (GPL-3) is the
//! reference for the choice of features; this is an independent
//! implementation and its numbers are not comparable with bliss's.
//!
//! The scaling of each feature in [`FeatureVector::embedding`] is a fixed,
//! hand-set heuristic, not something fitted to a library.

use fourfloor_core::SampleRate;

use crate::error::AnalysisError;
use crate::keydetect::{chromagram, KeyOptions};
use crate::stft::{fft_size_for, for_each_frame};
use crate::tempo::{estimate_tempo, TempoEstimate, TempoOptions};

/// Analysis window in seconds (2048 samples at 48 kHz).
const WINDOW_SECONDS: f64 = 0.043;
/// Number of mel bands.
const MEL_BANDS: usize = 40;
/// Mel filterbank range in Hz.
const MEL_RANGE_HZ: (f64, f64) = (30.0, 16_000.0);
/// Number of cepstral coefficients kept (the first, which is only level,
/// is dropped).
pub const MFCC_COUNT: usize = 12;
/// Frames quieter than this, relative to the loudest frame, are left out.
const GATE: f64 = 1e-4;
/// Share of the power below the roll-off frequency.
const ROLLOFF: f64 = 0.85;
/// Length of [`FeatureVector::embedding`].
pub const EMBEDDING_LEN: usize = 2 * MFCC_COUNT + 4 + 12 + 1;

/// A description of how a piece of audio sounds.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeatureVector {
    /// Tempo in BPM, when the audio has a clear beat.
    pub tempo_bpm: Option<f64>,
    /// Length in seconds.
    pub duration_seconds: f64,
    /// Average spectral centroid in Hz: the "brightness".
    pub centroid_hz: f32,
    /// Average frequency below which 85% of the power lies, in Hz.
    pub rolloff_hz: f32,
    /// Average spectral flatness, 0 (pure tone) to 1 (white noise).
    pub flatness: f32,
    /// Zero crossings per second.
    pub zero_crossing_rate: f32,
    /// Mean of mel-frequency cepstral coefficients 1 to 12: the timbre.
    pub mfcc_mean: [f32; MFCC_COUNT],
    /// Standard deviation of the same coefficients: how much the timbre
    /// moves.
    pub mfcc_std: [f32; MFCC_COUNT],
    /// Energy per pitch class from C to B, as a unit-length vector. All
    /// zeros for silence.
    pub chroma: [f32; 12],
}

/// How much each group of features counts in the distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureWeights {
    /// Timbre: the cepstral coefficients. Default 1.
    pub timbre: f32,
    /// Spectral shape: centroid, roll-off, flatness, zero crossings.
    /// Default 1.
    pub brightness: f32,
    /// Harmony: the chroma vector. Default 0.5.
    pub harmony: f32,
    /// Tempo. Default 1.
    pub tempo: f32,
}

impl Default for FeatureWeights {
    fn default() -> Self {
        Self {
            timbre: 1.0,
            brightness: 1.0,
            harmony: 0.5,
            tempo: 1.0,
        }
    }
}

impl FeatureVector {
    /// The features as a fixed-length vector ([`EMBEDDING_LEN`] values)
    /// scaled so that plain Euclidean distance between two embeddings is a
    /// useful similarity measure. This is the form to put in a vector
    /// index. The last value is the tempo and is zero when there is none.
    pub fn embedding(&self) -> Vec<f32> {
        self.weighted_embedding(&FeatureWeights::default())
    }

    /// [`Self::embedding`] with custom group weights.
    pub fn weighted_embedding(&self, weights: &FeatureWeights) -> Vec<f32> {
        let mut out = Vec::with_capacity(EMBEDDING_LEN);
        // Cepstral coefficients of very different sounds differ by tens;
        // dividing by 10 and by the root of their number brings the whole
        // group to order one.
        let timbre = weights.timbre / (10.0 * (MFCC_COUNT as f32).sqrt());
        out.extend(self.mfcc_mean.iter().map(|&c| c * timbre));
        out.extend(self.mfcc_std.iter().map(|&c| c * timbre));
        // Frequencies in octaves: a doubling counts the same anywhere.
        let octaves = |hz: f32| (hz.max(1.0) / 1000.0).log2();
        let shape = weights.brightness * 0.25;
        out.push(octaves(self.centroid_hz) * shape);
        out.push(octaves(self.rolloff_hz) * shape);
        out.push(self.flatness * weights.brightness);
        out.push(octaves(self.zero_crossing_rate) * shape);
        out.extend(self.chroma.iter().map(|&c| c * weights.harmony));
        out.push(
            self.tempo_bpm
                .map_or(0.0, |bpm| tempo_term(bpm) * weights.tempo),
        );
        out
    }

    /// Distance to `other`: zero for identical features, larger for less
    /// similar sounds. Tempo counts only when both have one.
    pub fn distance(&self, other: &FeatureVector) -> f32 {
        self.weighted_distance(other, &FeatureWeights::default())
    }

    /// [`Self::distance`] with custom group weights.
    pub fn weighted_distance(&self, other: &FeatureVector, weights: &FeatureWeights) -> f32 {
        let a = self.weighted_embedding(weights);
        let b = other.weighted_embedding(weights);
        let both_have_tempo = self.tempo_bpm.is_some() && other.tempo_bpm.is_some();
        let used = if both_have_tempo {
            EMBEDDING_LEN
        } else {
            EMBEDDING_LEN - 1
        };
        a.iter()
            .zip(&b)
            .take(used)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt()
    }
}

/// Tempo on a scale where a 6% change (one semitone of pitch fader, about
/// the limit of a comfortable beat-match) is a distance of about 0.25.
fn tempo_term(bpm: f64) -> f32 {
    (3.0 * (bpm.max(1.0) / 120.0).log2()) as f32
}

/// Extracts the features of a mono signal, estimating the tempo as well.
/// The tempo is kept only when the audio has a regular beat
/// ([`TempoEstimate::is_reliable`]).
pub fn extract_features(
    samples: &[f32],
    sample_rate: SampleRate,
) -> Result<FeatureVector, AnalysisError> {
    let tempo = estimate_tempo(samples, sample_rate, &TempoOptions::default())
        .ok()
        .filter(TempoEstimate::is_reliable)
        .map(|estimate| estimate.bpm());
    extract_features_with_tempo(samples, sample_rate, tempo)
}

/// Extracts the features of a mono signal, taking the tempo as given
/// (`None` for one-shots and anything without a beat).
pub fn extract_features_with_tempo(
    samples: &[f32],
    sample_rate: SampleRate,
    tempo_bpm: Option<f64>,
) -> Result<FeatureVector, AnalysisError> {
    if samples.is_empty() {
        return Err(AnalysisError::Empty);
    }
    let rate = sample_rate.as_f64();
    let window = fft_size_for(WINDOW_SECONDS, rate);
    let bin_hz = rate / window as f64;
    let filters = mel_filters(window, rate);
    let dct = dct_table();

    // Per frame: power, centroid, roll-off, flatness, then the MFCCs.
    const WIDTH: usize = 4 + MFCC_COUNT;
    let mut rows: Vec<f64> = Vec::with_capacity((samples.len() / (window / 2) + 1) * WIDTH);
    let mut mel = [0.0f64; MEL_BANDS];
    for_each_frame(samples, window, window / 2, |_, power| {
        let bins = &power[1..];
        let total: f64 = bins.iter().map(|&p| f64::from(p)).sum();
        let mut weighted = 0.0f64;
        let mut log_sum = 0.0f64;
        for (index, &p) in bins.iter().enumerate() {
            weighted += (index + 1) as f64 * f64::from(p);
            log_sum += (f64::from(p) + 1e-20).ln();
        }
        let mut running = 0.0f64;
        let rolloff_bin = bins
            .iter()
            .position(|&p| {
                running += f64::from(p);
                running >= ROLLOFF * total
            })
            .map_or(bins.len(), |index| index + 1);
        let mean = total / bins.len() as f64;
        let (centroid, flatness) = if total > 0.0 {
            (
                weighted / total * bin_hz,
                (log_sum / bins.len() as f64).exp() / mean,
            )
        } else {
            (0.0, 0.0)
        };
        rows.extend([total, centroid, rolloff_bin as f64 * bin_hz, flatness]);
        let scale = if total > 0.0 { total } else { 1.0 };

        for (slot, (first, gains)) in mel.iter_mut().zip(&filters) {
            let energy: f64 = gains
                .iter()
                .zip(&power[*first..])
                .map(|(&gain, &p)| f64::from(gain) * f64::from(p))
                .sum();
            // Relative to the frame's own power, so that the coefficients
            // describe the shape of the spectrum and not its level, with a
            // floor 80 dB down.
            *slot = (energy / scale + 1e-8).ln();
        }
        rows.extend(
            dct.iter()
                .map(|basis| basis.iter().zip(&mel).map(|(&b, &m)| b * m).sum::<f64>()),
        );
    });

    let loudest = rows
        .chunks_exact(WIDTH)
        .map(|row| row[0])
        .fold(0.0, f64::max);
    let mut mean = [0.0f64; WIDTH];
    let mut square = [0.0f64; WIDTH];
    let mut count = 0usize;
    for row in rows.chunks_exact(WIDTH) {
        if loudest <= 0.0 || row[0] < GATE * loudest {
            continue;
        }
        count += 1;
        for ((sum, sq), &value) in mean.iter_mut().zip(&mut square).zip(row) {
            *sum += value;
            *sq += value * value;
        }
    }
    let mut mfcc_mean = [0.0f32; MFCC_COUNT];
    let mut mfcc_std = [0.0f32; MFCC_COUNT];
    if count > 0 {
        for (sum, sq) in mean.iter_mut().zip(&mut square) {
            *sum /= count as f64;
            *sq = (*sq / count as f64 - *sum * *sum).max(0.0).sqrt();
        }
        for index in 0..MFCC_COUNT {
            mfcc_mean[index] = mean[4 + index] as f32;
            mfcc_std[index] = square[4 + index] as f32;
        }
    }

    let crossings = samples
        .windows(2)
        .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
        .count();
    let duration_seconds = samples.len() as f64 / rate;

    let mut chroma = chromagram(samples, sample_rate, &KeyOptions::default())?.total();
    let length = chroma.iter().map(|&c| c * c).sum::<f32>().sqrt();
    if length > 0.0 {
        for value in &mut chroma {
            *value /= length;
        }
    }

    Ok(FeatureVector {
        tempo_bpm,
        duration_seconds,
        centroid_hz: mean[1] as f32,
        rolloff_hz: mean[2] as f32,
        flatness: (mean[3] as f32).clamp(0.0, 1.0),
        zero_crossing_rate: (crossings as f64 / duration_seconds) as f32,
        mfcc_mean,
        mfcc_std,
        chroma,
    })
}

fn hz_to_mel(hz: f64) -> f64 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (10f64.powf(mel / 2595.0) - 1.0)
}

/// Triangular mel filters as `(first bin, gains)`.
fn mel_filters(window: usize, sample_rate: f64) -> Vec<(usize, Vec<f32>)> {
    let bin_hz = sample_rate / window as f64;
    let top = MEL_RANGE_HZ.1.min(sample_rate / 2.0);
    let (low, high) = (hz_to_mel(MEL_RANGE_HZ.0), hz_to_mel(top));
    let point =
        |index: usize| mel_to_hz(low + (high - low) * index as f64 / (MEL_BANDS + 1) as f64);
    (0..MEL_BANDS)
        .map(|band| {
            let (left, centre, right) = (point(band), point(band + 1), point(band + 2));
            let first = ((left / bin_hz).floor() as usize).max(1);
            let last = ((right / bin_hz).ceil() as usize).min(window / 2);
            let gains: Vec<f32> = (first..=last.max(first))
                .map(|bin| {
                    let hz = bin as f64 * bin_hz;
                    let rising = (hz - left) / (centre - left);
                    let falling = (right - hz) / (right - centre);
                    rising.min(falling).clamp(0.0, 1.0) as f32
                })
                .collect();
            // A filter narrower than one bin would otherwise catch nothing.
            if gains.iter().all(|&gain| gain == 0.0) {
                let nearest = ((centre / bin_hz).round() as usize).clamp(1, window / 2);
                return (nearest, vec![1.0]);
            }
            (first, gains)
        })
        .collect()
}

/// Rows 1 to [`MFCC_COUNT`] of the orthonormal DCT-II over the mel bands.
fn dct_table() -> Vec<[f64; MEL_BANDS]> {
    let scale = (2.0 / MEL_BANDS as f64).sqrt();
    (1..=MFCC_COUNT)
        .map(|k| {
            let mut basis = [0.0f64; MEL_BANDS];
            for (m, slot) in basis.iter_mut().enumerate() {
                let angle = std::f64::consts::PI * k as f64 * (m as f64 + 0.5) / MEL_BANDS as f64;
                *slot = scale * angle.cos();
            }
            basis
        })
        .collect()
}
