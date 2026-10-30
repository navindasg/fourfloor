//! Sample data for the sampler and the hand-over that gets it to the
//! audio thread without allocating or freeing there.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use fourfloor_audio::{decode_bytes, decode_file, AudioBuffer, DecodeError};
use fourfloor_core::SampleRate;

use super::slicing::{detect_slices, SliceOptions};

/// Why sample data could not be built.
#[derive(Debug)]
pub enum SampleError {
    /// The audio has no frames.
    Empty,
    /// Left and right have different lengths.
    ChannelMismatch {
        /// Frames in the left channel.
        left: usize,
        /// Frames in the right channel.
        right: usize,
    },
    /// The file could not be read or decoded.
    Decode(DecodeError),
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the sample has no audio"),
            Self::ChannelMismatch { left, right } => write!(
                f,
                "the sample's channels differ in length ({left} and {right} frames)"
            ),
            Self::Decode(source) => write!(f, "cannot decode the sample: {source}"),
        }
    }
}

impl std::error::Error for SampleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(source) => Some(source),
            _ => None,
        }
    }
}

impl From<DecodeError> for SampleError {
    fn from(source: DecodeError) -> Self {
        Self::Decode(source)
    }
}

/// A sample ready to play: stereo audio at its own sample rate, plus the
/// frames where its slices start.
///
/// Build it off the audio thread (decoding and slicing allocate), wrap it
/// in an [`Arc`] and give it to a [`Sampler`](super::Sampler).
#[derive(Clone, Debug, PartialEq)]
pub struct SampleData {
    sample_rate: SampleRate,
    left: Vec<f32>,
    right: Vec<f32>,
    slices: Vec<usize>,
}

impl SampleData {
    /// Most slices a sample keeps: one per MIDI pitch.
    pub const MAX_SLICES: usize = 128;

    /// Stereo sample data. The two channels must be the same length and
    /// not empty. Starts with a single slice covering the whole sample.
    pub fn new(
        sample_rate: SampleRate,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Result<Self, SampleError> {
        if left.len() != right.len() {
            return Err(SampleError::ChannelMismatch {
                left: left.len(),
                right: right.len(),
            });
        }
        if left.is_empty() {
            return Err(SampleError::Empty);
        }
        Ok(Self {
            sample_rate,
            left,
            right,
            slices: vec![0],
        })
    }

    /// Mono sample data, played on both sides.
    pub fn from_mono(sample_rate: SampleRate, samples: Vec<f32>) -> Result<Self, SampleError> {
        let right = samples.clone();
        Self::new(sample_rate, samples, right)
    }

    /// Sample data from a decoded buffer. Mono is doubled; channels past
    /// the second are ignored.
    pub fn from_buffer(buffer: &AudioBuffer) -> Result<Self, SampleError> {
        let left = buffer.channels.first().ok_or(SampleError::Empty)?;
        let right = buffer.channels.get(1).unwrap_or(left);
        Self::new(buffer.sample_rate, left.clone(), right.clone())
    }

    /// Decodes an audio file held in memory (WAV, FLAC, AIFF and the other
    /// formats `fourfloor-audio` reads). `extension` is an optional hint.
    pub fn decode(bytes: Vec<u8>, extension: Option<&str>) -> Result<Self, SampleError> {
        Self::from_buffer(&decode_bytes(bytes, extension)?)
    }

    /// Decodes an audio file from disk.
    pub fn load(path: &Path) -> Result<Self, SampleError> {
        Self::from_buffer(&decode_file(path)?)
    }

    /// Replaces the slices with the given start frames. They are sorted,
    /// de-duplicated, limited to frames inside the sample and to
    /// [`MAX_SLICES`](Self::MAX_SLICES). An empty list gives one slice
    /// covering the whole sample.
    pub fn with_slices(mut self, mut starts: Vec<usize>) -> Self {
        let frames = self.frames();
        starts.retain(|start| *start < frames);
        starts.sort_unstable();
        starts.dedup();
        starts.truncate(Self::MAX_SLICES);
        if starts.is_empty() {
            starts.push(0);
        }
        self.slices = starts;
        self
    }

    /// Replaces the slices with the onsets found by spectral-flux
    /// detection (see [`detect_slices`]).
    pub fn with_detected_slices(self, options: &SliceOptions) -> Self {
        let mono: Vec<f32> = self
            .left
            .iter()
            .zip(&self.right)
            .map(|(l, r)| 0.5 * (l + r))
            .collect();
        let starts = detect_slices(&mono, self.sample_rate, options);
        self.with_slices(starts)
    }

    /// Frames in the sample.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// The rate the sample was recorded at.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Left channel.
    pub fn left(&self) -> &[f32] {
        &self.left
    }

    /// Right channel.
    pub fn right(&self) -> &[f32] {
        &self.right
    }

    /// Frames where slices start, ascending. Never empty.
    pub fn slices(&self) -> &[usize] {
        &self.slices
    }

    /// The frame range of slice `index`: from its start to the next
    /// slice's start, or to the end of the sample for the last one.
    pub fn slice_range(&self, index: usize) -> Option<(usize, usize)> {
        let start = *self.slices.get(index)?;
        let end = self
            .slices
            .get(index + 1)
            .copied()
            .unwrap_or_else(|| self.frames());
        Some((start, end))
    }
}

/// The control side of a [`Sampler`](super::Sampler) made with
/// [`Sampler::with_loader`](super::Sampler::with_loader): sends newly
/// loaded samples to the audio thread and takes the replaced ones back so
/// they are freed here, not there.
///
/// Keep it on the thread that loads samples.
pub struct SampleLoader {
    pub(super) to_audio: rtrb::Producer<Arc<SampleData>>,
    pub(super) from_audio: rtrb::Consumer<Arc<SampleData>>,
}

impl SampleLoader {
    /// Samples that can wait for the audio thread at once.
    pub const CAPACITY: usize = 4;

    /// Hands `sample` to the sampler, which starts using it at its next
    /// block (voices playing the old sample are cut). Also frees any
    /// samples the sampler has finished with.
    ///
    /// Returns the sample back if the queue is full because the audio
    /// thread has not run; try again later.
    pub fn load(&mut self, sample: Arc<SampleData>) -> Result<(), Arc<SampleData>> {
        self.collect();
        self.to_audio.push(sample).map_err(|error| match error {
            rtrb::PushError::Full(sample) => sample,
        })
    }

    /// Frees the samples the sampler has replaced. Returns how many.
    pub fn collect(&mut self) -> usize {
        let mut freed = 0;
        while self.from_audio.pop().is_ok() {
            freed += 1;
        }
        freed
    }
}

impl fmt::Debug for SampleLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SampleLoader").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_mismatched_audio() {
        let rate = SampleRate::HZ_48000;
        assert!(matches!(
            SampleData::new(rate, vec![], vec![]),
            Err(SampleError::Empty)
        ));
        assert!(matches!(
            SampleData::new(rate, vec![0.0; 3], vec![0.0; 2]),
            Err(SampleError::ChannelMismatch { left: 3, right: 2 })
        ));
        assert!(SampleData::decode(vec![1, 2, 3], None).is_err());
    }

    #[test]
    fn slices_are_sorted_bounded_and_never_empty() {
        let data = SampleData::from_mono(SampleRate::HZ_48000, vec![0.0; 100]).unwrap();
        assert_eq!(data.slices(), [0]);
        assert_eq!(data.slice_range(0), Some((0, 100)));
        let data = data.with_slices(vec![50, 10, 10, 100, 2_000]);
        assert_eq!(data.slices(), [10, 50]);
        assert_eq!(data.slice_range(0), Some((10, 50)));
        assert_eq!(data.slice_range(1), Some((50, 100)));
        assert_eq!(data.slice_range(2), None);
        assert_eq!(data.with_slices(vec![]).slices(), [0]);
    }
}
