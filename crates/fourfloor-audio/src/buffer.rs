//! Decoded audio held in memory.

use fourfloor_core::SampleRate;

/// Audio as planar `f32`: one `Vec` per channel, all the same length.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    /// Sample rate of the audio.
    pub sample_rate: SampleRate,
    /// One `Vec` per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    /// A buffer from planar channels. Shorter channels are padded with
    /// silence so that all have the length of the longest.
    pub fn new(sample_rate: SampleRate, mut channels: Vec<Vec<f32>>) -> Self {
        let frames = channels.iter().map(Vec::len).max().unwrap_or(0);
        for channel in &mut channels {
            channel.resize(frames, 0.0);
        }
        Self {
            sample_rate,
            channels,
        }
    }

    /// A buffer from interleaved samples. Trailing samples that do not make
    /// a whole frame are dropped; zero channels gives an empty buffer.
    pub fn from_interleaved(sample_rate: SampleRate, channels: usize, samples: &[f32]) -> Self {
        let planes = (0..channels)
            .map(|channel| {
                samples
                    .chunks_exact(channels)
                    .map(|frame| frame[channel])
                    .collect()
            })
            .collect();
        Self {
            sample_rate,
            channels: planes,
        }
    }

    /// Number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Number of channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Length in seconds.
    pub fn duration_seconds(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.as_f64()
    }

    /// The channels as slices, the shape the meters take.
    pub fn planes(&self) -> Vec<&[f32]> {
        self.channels.iter().map(Vec::as_slice).collect()
    }

    /// The samples interleaved frame by frame.
    pub fn interleaved(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.frames() * self.channel_count());
        for frame in 0..self.frames() {
            out.extend(self.channels.iter().map(|channel| channel[frame]));
        }
        out
    }

    /// A stereo version: mono is copied to both sides, stereo is kept, and
    /// anything wider keeps its first two channels. An empty buffer gives
    /// two empty channels.
    pub fn to_stereo(&self) -> Self {
        let left = self.channels.first().cloned().unwrap_or_default();
        let right = self
            .channels
            .get(1)
            .cloned()
            .unwrap_or_else(|| left.clone());
        Self {
            sample_rate: self.sample_rate,
            channels: vec![left, right],
        }
    }

    /// The average of all channels.
    pub fn to_mono(&self) -> Vec<f32> {
        let count = self.channel_count().max(1) as f32;
        (0..self.frames())
            .map(|frame| self.channels.iter().map(|c| c[frame]).sum::<f32>() / count)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn interleaving_round_trips() {
        let buffer = AudioBuffer::from_interleaved(RATE, 2, &[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(buffer.channels, [vec![1.0, 3.0], vec![2.0, 4.0]]);
        assert_eq!((buffer.frames(), buffer.channel_count()), (2, 2));
        assert_eq!(buffer.interleaved(), [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(buffer.planes()[1], [2.0, 4.0]);
        assert_eq!(buffer.duration_seconds(), 2.0 / 48_000.0);
        assert_eq!(AudioBuffer::from_interleaved(RATE, 0, &[1.0]).frames(), 0);
    }

    #[test]
    fn new_pads_uneven_channels() {
        let buffer = AudioBuffer::new(RATE, vec![vec![1.0, 2.0], vec![3.0]]);
        assert_eq!(buffer.channels[1], [3.0, 0.0]);
    }

    #[test]
    fn stereo_and_mono_conversions() {
        let mono = AudioBuffer::new(RATE, vec![vec![0.5, -0.5]]);
        assert_eq!(
            mono.to_stereo().channels,
            [vec![0.5, -0.5], vec![0.5, -0.5]]
        );
        let wide = AudioBuffer::new(RATE, vec![vec![1.0], vec![2.0], vec![3.0]]);
        assert_eq!(wide.to_stereo().channels, [vec![1.0], vec![2.0]]);
        assert_eq!(wide.to_mono(), [2.0]);
        let empty = AudioBuffer::new(RATE, Vec::new());
        assert_eq!(empty.to_stereo().channel_count(), 2);
        assert!(empty.to_mono().is_empty());
    }
}
