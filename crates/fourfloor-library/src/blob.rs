//! Binary forms of the waveform overview, feature vector and embedding
//! columns. All numbers are little-endian; each blob starts with a
//! four-byte tag so a later format can be told apart.

use fourfloor_analysis::{
    AnalysisError, FeatureVector, Peak, WaveformOptions, WaveformOverview, MFCC_COUNT,
};
use fourfloor_core::SampleRate;

use crate::error::LibraryError;

const WAVEFORM_TAG: &[u8; 4] = b"FFW1";
const FEATURES_TAG: &[u8; 4] = b"FFF1";

/// The most peaks a stored waveform holds: enough for a browser row or a
/// DJ overview strip at any sensible width.
pub const WAVEFORM_PEAKS: usize = 2_048;

/// A fixed-width overview of a file's waveform (mono mix), small enough to
/// keep in the database: at most [`WAVEFORM_PEAKS`] columns of minimum,
/// maximum and RMS, each stored in one byte.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveformSummary {
    /// Sample rate of the audio in Hz.
    pub sample_rate: u32,
    /// Length of the audio in frames.
    pub frames: u64,
    /// The columns, evenly spaced over the whole file.
    pub peaks: Vec<Peak>,
}

impl WaveformSummary {
    /// Summarises a mono signal.
    pub fn build(samples: &[f32], sample_rate: SampleRate) -> Result<Self, AnalysisError> {
        let overview = WaveformOverview::build(samples, sample_rate, &WaveformOptions::default())?;
        // Short files get one column per finest block rather than columns
        // that repeat each other.
        let finest = overview.levels.first().map_or(0, |level| level.peaks.len());
        let columns = finest.min(WAVEFORM_PEAKS);
        Ok(Self {
            sample_rate: sample_rate.hz(),
            frames: samples.len() as u64,
            peaks: overview.columns(0, samples.len(), columns),
        })
    }

    /// Frames of audio each column covers.
    pub fn frames_per_peak(&self) -> f64 {
        if self.peaks.is_empty() {
            0.0
        } else {
            self.frames as f64 / self.peaks.len() as f64
        }
    }

    /// Length of the audio in seconds.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / f64::from(self.sample_rate)
        }
    }

    /// The column that holds the audio at `seconds`, if any.
    pub fn peak_at(&self, seconds: f64) -> Option<Peak> {
        let per = self.frames_per_peak();
        if per <= 0.0 || seconds < 0.0 {
            return None;
        }
        let index = (seconds * f64::from(self.sample_rate) / per) as usize;
        self.peaks.get(index).copied()
    }

    /// The blob stored in the `waveform` column. Values are quantised to
    /// one byte each, so levels are exact to about 1 %; anything beyond
    /// full scale is clamped to it.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + self.peaks.len() * 3);
        out.extend_from_slice(WAVEFORM_TAG);
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&self.frames.to_le_bytes());
        out.extend_from_slice(&(self.peaks.len() as u32).to_le_bytes());
        for peak in &self.peaks {
            out.push(quantise_signed(peak.min) as u8);
            out.push(quantise_signed(peak.max) as u8);
            out.push(quantise_unsigned(peak.rms));
        }
        out
    }

    /// Reads a blob written by [`Self::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self, LibraryError> {
        let corrupt = |reason| LibraryError::CorruptBlob {
            what: "waveform",
            reason,
        };
        let mut reader = Reader::new(bytes);
        if reader.take(4) != Some(WAVEFORM_TAG.as_slice()) {
            return Err(corrupt("unknown format tag"));
        }
        let sample_rate = reader.u32().ok_or(corrupt("truncated header"))?;
        let frames = reader.u64().ok_or(corrupt("truncated header"))?;
        let count = reader.u32().ok_or(corrupt("truncated header"))? as usize;
        let body = reader.rest();
        if count.checked_mul(3) != Some(body.len()) {
            return Err(corrupt("peak count does not match the data"));
        }
        let peaks = body
            .chunks_exact(3)
            .map(|chunk| Peak {
                min: f32::from(chunk[0] as i8) / 127.0,
                max: f32::from(chunk[1] as i8) / 127.0,
                rms: f32::from(chunk[2]) / 255.0,
            })
            .collect();
        Ok(Self {
            sample_rate,
            frames,
            peaks,
        })
    }
}

fn quantise_signed(value: f32) -> i8 {
    if value.is_finite() {
        (value.clamp(-1.0, 1.0) * 127.0).round() as i8
    } else {
        0
    }
}

fn quantise_unsigned(value: f32) -> u8 {
    if value.is_finite() {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// The blob stored in the `features` column: every field of the feature
/// vector, exactly.
pub fn encode_features(features: &FeatureVector) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 1 + 16 + 4 * (4 + 2 * MFCC_COUNT + 12));
    out.extend_from_slice(FEATURES_TAG);
    out.push(u8::from(features.tempo_bpm.is_some()));
    out.extend_from_slice(&features.tempo_bpm.unwrap_or(0.0).to_le_bytes());
    out.extend_from_slice(&features.duration_seconds.to_le_bytes());
    let scalars = [
        features.centroid_hz,
        features.rolloff_hz,
        features.flatness,
        features.zero_crossing_rate,
    ];
    let floats = scalars
        .iter()
        .chain(&features.mfcc_mean)
        .chain(&features.mfcc_std)
        .chain(&features.chroma);
    for value in floats {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Reads a blob written by [`encode_features`].
pub fn decode_features(bytes: &[u8]) -> Result<FeatureVector, LibraryError> {
    let corrupt = |reason| LibraryError::CorruptBlob {
        what: "feature vector",
        reason,
    };
    let truncated = || corrupt("truncated");
    let mut reader = Reader::new(bytes);
    if reader.take(4) != Some(FEATURES_TAG.as_slice()) {
        return Err(corrupt("unknown format tag"));
    }
    let has_tempo = reader.take(1).ok_or_else(truncated)?[0] != 0;
    let tempo = reader.f64().ok_or_else(truncated)?;
    let duration_seconds = reader.f64().ok_or_else(truncated)?;
    let centroid_hz = reader.f32().ok_or_else(truncated)?;
    let rolloff_hz = reader.f32().ok_or_else(truncated)?;
    let flatness = reader.f32().ok_or_else(truncated)?;
    let zero_crossing_rate = reader.f32().ok_or_else(truncated)?;
    let mfcc_mean = reader.f32_array::<MFCC_COUNT>().ok_or_else(truncated)?;
    let mfcc_std = reader.f32_array::<MFCC_COUNT>().ok_or_else(truncated)?;
    let chroma = reader.f32_array::<12>().ok_or_else(truncated)?;
    if !reader.rest().is_empty() {
        return Err(corrupt("trailing bytes"));
    }
    Ok(FeatureVector {
        tempo_bpm: has_tempo.then_some(tempo),
        duration_seconds,
        centroid_hz,
        rolloff_hz,
        flatness,
        zero_crossing_rate,
        mfcc_mean,
        mfcc_std,
        chroma,
    })
}

/// A vector of `f32` as bytes, for the `embedding` column.
pub fn encode_f32s(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Reads a blob written by [`encode_f32s`].
pub fn decode_f32s(bytes: &[u8]) -> Result<Vec<f32>, LibraryError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(LibraryError::CorruptBlob {
            what: "embedding",
            reason: "length is not a multiple of four bytes",
        });
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

/// Cosine similarity of two vectors: 1 for the same direction, 0 for
/// unrelated, -1 for opposite. Zero when either is all zeros or the
/// lengths differ.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (&x, &y) in a.iter().zip(b) {
        dot += f64::from(x) * f64::from(y);
        aa += f64::from(x) * f64::from(x);
        bb += f64::from(y) * f64::from(y);
    }
    if aa <= 0.0 || bb <= 0.0 {
        0.0
    } else {
        (dot / (aa.sqrt() * bb.sqrt())) as f32
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.bytes.len() < count {
            return None;
        }
        let (head, tail) = self.bytes.split_at(count);
        self.bytes = tail;
        Some(head)
    }

    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8)?.try_into().ok().map(u64::from_le_bytes)
    }

    fn f32(&mut self) -> Option<f32> {
        self.take(4)?.try_into().ok().map(f32::from_le_bytes)
    }

    fn f64(&mut self) -> Option<f64> {
        self.take(8)?.try_into().ok().map(f64::from_le_bytes)
    }

    fn f32_array<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut out = [0.0f32; N];
        for slot in &mut out {
            *slot = self.f32()?;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features() -> FeatureVector {
        let mut mfcc_mean = [0.0f32; MFCC_COUNT];
        let mut mfcc_std = [0.0f32; MFCC_COUNT];
        let mut chroma = [0.0f32; 12];
        for i in 0..12 {
            mfcc_mean[i] = i as f32 * 1.5 - 4.0;
            mfcc_std[i] = i as f32 * 0.25;
            chroma[i] = 1.0 / (i as f32 + 1.0);
        }
        FeatureVector {
            tempo_bpm: Some(124.0),
            duration_seconds: 7.741_935,
            centroid_hz: 1_234.5,
            rolloff_hz: 6_000.0,
            flatness: 0.31,
            zero_crossing_rate: 900.0,
            mfcc_mean,
            mfcc_std,
            chroma,
        }
    }

    #[test]
    fn features_round_trip_exactly_with_and_without_tempo() {
        let with = features();
        assert_eq!(
            decode_features(&encode_features(&with)).ok(),
            Some(with.clone())
        );
        let without = FeatureVector {
            tempo_bpm: None,
            ..with
        };
        assert_eq!(
            decode_features(&encode_features(&without)).ok(),
            Some(without)
        );
    }

    #[test]
    fn damaged_feature_blobs_are_refused() {
        let blob = encode_features(&features());
        assert!(decode_features(&blob[..blob.len() - 1]).is_err());
        assert!(decode_features(b"nope").is_err());
        let mut longer = blob.clone();
        longer.push(0);
        assert!(decode_features(&longer).is_err());
        assert!(decode_features(&[]).is_err());
    }

    #[test]
    fn waveform_round_trips_to_within_one_quantisation_step() {
        let rate = SampleRate::HZ_48000;
        // A burst in the first half, silence in the second.
        let samples: Vec<f32> = (0..96_000)
            .map(|i| {
                if i < 48_000 {
                    // A period of about 21 samples, well inside one
                    // 47-frame column, so every column sees both peaks.
                    0.8 * (i as f32 * 0.3).sin()
                } else {
                    0.0
                }
            })
            .collect();
        let summary = WaveformSummary::build(&samples, rate).expect("summary");
        assert_eq!(summary.frames, 96_000);
        assert!(summary.peaks.len() <= WAVEFORM_PEAKS && summary.peaks.len() >= 1_000);
        assert!((summary.duration_seconds() - 2.0).abs() < 1e-9);

        let decoded = WaveformSummary::decode(&summary.encode()).expect("decode");
        assert_eq!(decoded.peaks.len(), summary.peaks.len());
        for (a, b) in summary.peaks.iter().zip(&decoded.peaks) {
            assert!((a.min - b.min).abs() <= 0.5 / 127.0 + 1e-6);
            assert!((a.max - b.max).abs() <= 0.5 / 127.0 + 1e-6);
            assert!((a.rms - b.rms).abs() <= 0.5 / 255.0 + 1e-6);
        }
        let loud = decoded.peak_at(0.5).expect("peak");
        let quiet = decoded.peak_at(1.5).expect("peak");
        assert!(loud.max > 0.7 && loud.min < -0.7 && loud.rms > 0.5);
        assert!(quiet.max == 0.0 && quiet.min == 0.0 && quiet.rms == 0.0);
        assert!(decoded.peak_at(2.5).is_none());
    }

    #[test]
    fn short_and_empty_waveforms_are_handled() {
        let rate = SampleRate::HZ_48000;
        let short = WaveformSummary::build(&[0.5; 200], rate).expect("summary");
        assert_eq!(short.peaks.len(), 4); // 200 samples in blocks of 64
        let empty = WaveformSummary::build(&[], rate).expect("summary");
        assert!(empty.peaks.is_empty());
        assert_eq!(empty.frames_per_peak(), 0.0);
        assert_eq!(WaveformSummary::decode(&empty.encode()).ok(), Some(empty));
    }

    #[test]
    fn damaged_waveform_blobs_are_refused() {
        let summary = WaveformSummary::build(&[0.25; 4_096], SampleRate::HZ_48000).expect("ok");
        let blob = summary.encode();
        assert!(WaveformSummary::decode(&blob[..blob.len() - 2]).is_err());
        assert!(WaveformSummary::decode(&blob[..10]).is_err());
        assert!(WaveformSummary::decode(b"XXXX").is_err());
    }

    #[test]
    fn f32_blobs_round_trip_and_cosine_behaves() {
        let values = [1.0f32, -2.5, 0.0, 3.25];
        assert_eq!(
            decode_f32s(&encode_f32s(&values)).ok(),
            Some(values.to_vec())
        );
        assert!(decode_f32s(&[1, 2, 3]).is_err());

        assert!((cosine_similarity(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0);
    }
}
