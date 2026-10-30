//! [`StereoBuffer`]: what every render helper returns.

use std::path::Path;

use fourfloor_audio::{decode_file, write_wav_file, DecodeError, WavError, WavSpec};
use fourfloor_core::SampleRate;

/// A stereo render: two channels of the same length and their sample rate.
#[derive(Clone, Debug, PartialEq)]
pub struct StereoBuffer {
    /// Sample rate of the audio.
    pub sample_rate: SampleRate,
    /// Left channel.
    pub left: Vec<f32>,
    /// Right channel.
    pub right: Vec<f32>,
}

impl StereoBuffer {
    /// A buffer from two channels. The shorter one is padded with silence.
    pub fn new(sample_rate: SampleRate, mut left: Vec<f32>, mut right: Vec<f32>) -> Self {
        let frames = left.len().max(right.len());
        left.resize(frames, 0.0);
        right.resize(frames, 0.0);
        Self {
            sample_rate,
            left,
            right,
        }
    }

    /// `frames` frames of silence.
    pub fn silent(sample_rate: SampleRate, frames: usize) -> Self {
        Self::new(sample_rate, vec![0.0; frames], vec![0.0; frames])
    }

    /// The same signal on both channels.
    pub fn from_mono(sample_rate: SampleRate, signal: &[f32]) -> Self {
        Self::new(sample_rate, signal.to_vec(), signal.to_vec())
    }

    /// A buffer from interleaved stereo samples (`L R L R ...`). A trailing
    /// odd sample is dropped.
    pub fn from_interleaved(sample_rate: SampleRate, samples: &[f32]) -> Self {
        let (left, right) = samples
            .chunks_exact(2)
            .map(|frame| (frame[0], frame[1]))
            .unzip();
        Self {
            sample_rate,
            left,
            right,
        }
    }

    /// Number of frames.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Length in seconds.
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.as_f64()
    }

    /// Both channels as slices: `[left, right]`.
    pub fn channels(&self) -> [&[f32]; 2] {
        [&self.left, &self.right]
    }

    /// The mid signal, `(left + right) / 2`. Most single-signal assertions
    /// (`dominant_frequency`, `onset_times`, ...) are run on this.
    pub fn mono(&self) -> Vec<f32> {
        self.left
            .iter()
            .zip(&self.right)
            .map(|(l, r)| (l + r) * 0.5)
            .collect()
    }

    /// The side signal, `(left - right) / 2`: silent for dual mono.
    pub fn side(&self) -> Vec<f32> {
        self.left
            .iter()
            .zip(&self.right)
            .map(|(l, r)| (l - r) * 0.5)
            .collect()
    }

    /// The samples interleaved, `L R L R ...`.
    pub fn interleaved(&self) -> Vec<f32> {
        self.left
            .iter()
            .zip(&self.right)
            .flat_map(|(l, r)| [*l, *r])
            .collect()
    }

    /// The frames from `start_seconds` to `end_seconds`, clamped to the
    /// buffer.
    pub fn slice_seconds(&self, start_seconds: f64, end_seconds: f64) -> Self {
        let rate = self.sample_rate.as_f64();
        let at = |seconds: f64| ((seconds.max(0.0) * rate).round() as usize).min(self.frames());
        let (start, end) = (at(start_seconds), at(end_seconds));
        let end = end.max(start);
        Self {
            sample_rate: self.sample_rate,
            left: self.left[start..end].to_vec(),
            right: self.right[start..end].to_vec(),
        }
    }

    /// Writes the buffer as a 32-bit float WAV file, which stores the
    /// samples exactly. Useful for listening to what a failing test heard.
    pub fn write_wav(&self, path: &Path) -> Result<(), WavError> {
        write_wav_file(
            path,
            WavSpec::float_stereo(self.sample_rate),
            &self.interleaved(),
        )
    }

    /// Reads an audio file as stereo: mono is copied to both channels and
    /// channels past the second are dropped.
    pub fn read_wav(path: &Path) -> Result<Self, DecodeError> {
        let mut audio = decode_file(path)?.to_stereo();
        let right = audio.channels.pop().unwrap_or_default();
        let left = audio.channels.pop().unwrap_or_default();
        Ok(Self::new(audio.sample_rate, left, right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn constructors_agree_on_layout() {
        let buffer = StereoBuffer::from_interleaved(RATE, &[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(buffer.left, [1.0, 3.0]);
        assert_eq!(buffer.right, [2.0, 4.0]);
        assert_eq!(buffer.frames(), 2);
        assert_eq!(buffer.interleaved(), [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(buffer.channels()[1], [2.0, 4.0]);
        assert_eq!(buffer.mono(), [1.5, 3.5]);
        assert_eq!(buffer.side(), [-0.5, -0.5]);

        let padded = StereoBuffer::new(RATE, vec![1.0], vec![1.0, 2.0]);
        assert_eq!(padded.left, [1.0, 0.0]);
        let dual = StereoBuffer::from_mono(RATE, &[0.5, 0.25]);
        assert_eq!(dual.left, dual.right);
        assert!(dual.side().iter().all(|s| *s == 0.0));
        assert_eq!(StereoBuffer::silent(RATE, 48_000).seconds(), 1.0);
    }

    #[test]
    fn slices_are_taken_in_seconds_and_clamped() {
        let ramp: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        let buffer = StereoBuffer::from_mono(RATE, &ramp);
        let part = buffer.slice_seconds(0.25, 0.5);
        assert_eq!(part.frames(), 12_000);
        assert_eq!(part.left[0], 12_000.0);
        assert_eq!(buffer.slice_seconds(0.9, 5.0).frames(), 4_800);
        assert_eq!(buffer.slice_seconds(2.0, 1.0).frames(), 0);
    }

    #[test]
    fn wav_files_round_trip_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("render.wav");
        let buffer = StereoBuffer::new(RATE, vec![0.1, -0.2, 0.3], vec![0.9, 0.8, -0.7]);
        buffer.write_wav(&path).unwrap();
        assert_eq!(StereoBuffer::read_wav(&path).unwrap(), buffer);
        assert!(StereoBuffer::read_wav(&dir.path().join("missing.wav")).is_err());
    }
}
