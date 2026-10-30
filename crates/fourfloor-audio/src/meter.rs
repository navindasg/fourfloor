//! Loudness metering on the pure-Rust `ebur128` crate: EBU R 128
//! integrated loudness, loudness range, true peak and sample peak.
//!
//! [`measure`] takes a whole buffer. [`LoudnessMeter`] is fed block by
//! block and also reports momentary and short-term loudness; it allocates
//! when built and when fed, so it belongs on a worker, not the audio thread.

use ebur128::{EbuR128, Mode};
use fourfloor_core::SampleRate;

/// Why a measurement could not be taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MeterError {
    /// No channels were supplied.
    #[error("no channels to measure")]
    NoChannels,
    /// The channels do not all have the same number of frames.
    #[error("channels have different lengths")]
    UnevenChannels,
    /// The `ebur128` crate rejected the configuration or the frames.
    #[error("ebur128 error: {0}")]
    Backend(#[from] ebur128::Error),
}

/// Loudness figures for one buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Gated integrated loudness in LUFS. Negative infinity when the buffer
    /// is silent or shorter than one 400 ms gating block.
    pub integrated_lufs: f64,
    /// Loudness range (LRA) in LU. Zero when there is too little audio to
    /// measure one.
    pub loudness_range_lu: f64,
    /// Highest inter-sample (oversampled) peak across channels, in dBTP.
    pub true_peak_dbtp: f64,
    /// Highest sample peak across channels, in dBFS.
    pub sample_peak_dbfs: f64,
}

/// Converts a linear amplitude to decibels (0.0 maps to negative infinity).
pub fn linear_to_db(linear: f64) -> f64 {
    20.0 * linear.log10()
}

/// A loudness meter that is fed block by block.
///
/// Channel 0 is treated as left and channel 1 as right, so a single channel
/// is measured as "left only" and reads 3.01 LU lower than the same signal
/// on both channels of a stereo pair.
pub struct LoudnessMeter {
    meter: EbuR128,
    channels: u32,
}

impl std::fmt::Debug for LoudnessMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoudnessMeter")
            .field("channels", &self.channels)
            .field("rate", &self.meter.rate())
            .finish()
    }
}

impl LoudnessMeter {
    /// A meter for `channels` channels at `sample_rate`.
    pub fn new(channels: usize, sample_rate: SampleRate) -> Result<Self, MeterError> {
        let channels = u32::try_from(channels)
            .ok()
            .filter(|count| *count > 0)
            .ok_or(MeterError::NoChannels)?;
        let mode = Mode::I | Mode::LRA | Mode::S | Mode::M | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK;
        Ok(Self {
            meter: EbuR128::new(channels, sample_rate.hz(), mode)?,
            channels,
        })
    }

    /// Feeds planar audio: one slice per channel, all the same length.
    pub fn add_planar(&mut self, channels: &[&[f32]]) -> Result<(), MeterError> {
        let first = channels.first().ok_or(MeterError::NoChannels)?;
        if channels.len() != self.channels as usize
            || channels.iter().any(|channel| channel.len() != first.len())
        {
            return Err(MeterError::UnevenChannels);
        }
        Ok(self.meter.add_frames_planar_f32(channels)?)
    }

    /// Gated integrated loudness of everything fed so far, in LUFS.
    pub fn integrated_lufs(&self) -> Result<f64, MeterError> {
        Ok(self.meter.loudness_global()?)
    }

    /// Loudness of the last 400 ms, in LUFS.
    pub fn momentary_lufs(&self) -> Result<f64, MeterError> {
        Ok(self.meter.loudness_momentary()?)
    }

    /// Loudness of the last 3 s, in LUFS.
    pub fn short_term_lufs(&self) -> Result<f64, MeterError> {
        Ok(self.meter.loudness_shortterm()?)
    }

    /// Loudness range (LRA) of everything fed so far, in LU.
    pub fn loudness_range_lu(&self) -> Result<f64, MeterError> {
        Ok(self.meter.loudness_range()?)
    }

    /// Highest true peak across channels so far, in dBTP.
    pub fn true_peak_dbtp(&self) -> Result<f64, MeterError> {
        let mut peak = 0.0f64;
        for channel in 0..self.channels {
            peak = peak.max(self.meter.true_peak(channel)?);
        }
        Ok(linear_to_db(peak))
    }

    /// Highest sample peak across channels so far, in dBFS.
    pub fn sample_peak_dbfs(&self) -> Result<f64, MeterError> {
        let mut peak = 0.0f64;
        for channel in 0..self.channels {
            peak = peak.max(self.meter.sample_peak(channel)?);
        }
        Ok(linear_to_db(peak))
    }

    /// Every whole-programme figure at once.
    pub fn loudness(&self) -> Result<Loudness, MeterError> {
        Ok(Loudness {
            integrated_lufs: self.integrated_lufs()?,
            loudness_range_lu: self.loudness_range_lu()?,
            true_peak_dbtp: self.true_peak_dbtp()?,
            sample_peak_dbfs: self.sample_peak_dbfs()?,
        })
    }
}

/// Measures integrated loudness, loudness range and peaks of a planar
/// buffer: one slice per channel, all the same length.
pub fn measure(channels: &[&[f32]], sample_rate: SampleRate) -> Result<Loudness, MeterError> {
    let first = channels.first().ok_or(MeterError::NoChannels)?;
    if channels.iter().any(|c| c.len() != first.len()) {
        return Err(MeterError::UnevenChannels);
    }
    let mut meter = LoudnessMeter::new(channels.len(), sample_rate)?;
    meter.add_planar(channels)?;
    meter.loudness()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    fn sine(freq: f64, peak_dbfs: f64, seconds: f64, phase: f64) -> Vec<f32> {
        let amp = 10.0_f64.powf(peak_dbfs / 20.0);
        let n = (seconds * RATE.as_f64()) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / RATE.as_f64();
                (amp * (2.0 * std::f64::consts::PI * freq * t + phase).sin()) as f32
            })
            .collect()
    }

    /// EBU Tech 3341 test case 1: a 1 kHz stereo sine at -23 dBFS peak reads
    /// -23.0 LUFS, within 0.1 LU.
    #[test]
    fn stereo_1khz_sine_at_minus_23_dbfs_is_minus_23_lufs() {
        let s = sine(1000.0, -23.0, 20.0, 0.0);
        let loudness = measure(&[&s, &s], RATE).unwrap();
        assert!(
            (loudness.integrated_lufs + 23.0).abs() < 0.1,
            "got {} LUFS",
            loudness.integrated_lufs
        );
        assert!((loudness.sample_peak_dbfs + 23.0).abs() < 0.1);
        assert!((loudness.true_peak_dbtp + 23.0).abs() < 0.2);
        assert!(
            loudness.loudness_range_lu < 0.5,
            "a steady tone has no range"
        );
    }

    #[test]
    fn mono_reads_three_lu_below_stereo() {
        let s = sine(1000.0, -20.0, 10.0, 0.0);
        let mono = measure(&[&s], RATE).unwrap();
        assert!(
            (mono.integrated_lufs + 23.01).abs() < 0.1,
            "got {} LUFS",
            mono.integrated_lufs
        );
    }

    /// A sine at a quarter of the sample rate with a 45 degree phase offset
    /// only ever lands on samples 3 dB below its real peak. True peak must
    /// see the real level; sample peak cannot.
    #[test]
    fn true_peak_finds_inter_sample_peaks() {
        let s = sine(12_000.0, -6.0, 2.0, std::f64::consts::FRAC_PI_4);
        let loudness = measure(&[&s], RATE).unwrap();
        assert!(
            (loudness.sample_peak_dbfs + 9.01).abs() < 0.1,
            "sample peak {}",
            loudness.sample_peak_dbfs
        );
        assert!(
            (loudness.true_peak_dbtp + 6.0).abs() < 0.3,
            "true peak {}",
            loudness.true_peak_dbtp
        );
    }

    #[test]
    fn loudness_range_sees_a_quiet_and_a_loud_section() {
        let mut s = sine(1000.0, -30.0, 15.0, 0.0);
        s.extend(sine(1000.0, -15.0, 15.0, 0.0));
        let loudness = measure(&[&s, &s], RATE).unwrap();
        assert!(
            (loudness.loudness_range_lu - 15.0).abs() < 1.5,
            "LRA {}",
            loudness.loudness_range_lu
        );
    }

    #[test]
    fn silence_is_negative_infinity() {
        let s = vec![0.0_f32; 48_000 * 2];
        let loudness = measure(&[&s, &s], RATE).unwrap();
        assert_eq!(loudness.integrated_lufs, f64::NEG_INFINITY);
        assert_eq!(loudness.true_peak_dbtp, f64::NEG_INFINITY);
    }

    #[test]
    fn the_streaming_meter_matches_the_whole_buffer_and_tracks_recent_level() {
        let quiet = sine(1000.0, -40.0, 5.0, 0.0);
        let loud = sine(1000.0, -10.0, 5.0, 0.0);
        let mut meter = LoudnessMeter::new(2, RATE).unwrap();
        for block in quiet.chunks(4_800) {
            meter.add_planar(&[block, block]).unwrap();
        }
        let before = meter.momentary_lufs().unwrap();
        for block in loud.chunks(4_800) {
            meter.add_planar(&[block, block]).unwrap();
        }
        assert!((before + 40.0).abs() < 0.2, "momentary {before}");
        assert!((meter.momentary_lufs().unwrap() + 10.0).abs() < 0.2);
        assert!((meter.short_term_lufs().unwrap() + 10.0).abs() < 0.2);

        let whole: Vec<f32> = quiet.iter().chain(&loud).copied().collect();
        let reference = measure(&[&whole, &whole], RATE).unwrap();
        let streamed = meter.loudness().unwrap();
        assert!((streamed.integrated_lufs - reference.integrated_lufs).abs() < 1e-9);
        assert!(format!("{meter:?}").contains("LoudnessMeter"));
    }

    #[test]
    fn bad_buffers_are_rejected() {
        assert_eq!(measure(&[], RATE), Err(MeterError::NoChannels));
        let a = vec![0.0_f32; 10];
        let b = vec![0.0_f32; 11];
        assert_eq!(measure(&[&a, &b], RATE), Err(MeterError::UnevenChannels));
        assert!(matches!(
            LoudnessMeter::new(0, RATE),
            Err(MeterError::NoChannels)
        ));
        let mut meter = LoudnessMeter::new(2, RATE).unwrap();
        assert_eq!(meter.add_planar(&[&a]), Err(MeterError::UnevenChannels));
        assert_eq!(linear_to_db(1.0), 0.0);
    }
}
