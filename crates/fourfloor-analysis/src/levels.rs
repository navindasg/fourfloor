//! Level measurements: loudness, peaks, crest factor and stereo image.

use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;

use crate::error::AnalysisError;

/// Level and stereo figures for one piece of audio.
///
/// Mono audio is measured as dual mono (the same signal on both sides),
/// which is how it plays back. That reads 3 LU higher than
/// `fourfloor_audio::measure` gives for a single channel, which treats it
/// as left only.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LevelReport {
    /// EBU R 128 integrated loudness in LUFS. Negative infinity for
    /// silence or audio shorter than 400 ms.
    pub integrated_lufs: f64,
    /// Loudness range in LU.
    pub loudness_range_lu: f64,
    /// Highest inter-sample peak in dBTP.
    pub true_peak_dbtp: f64,
    /// Highest sample peak in dBFS.
    pub sample_peak_dbfs: f64,
    /// RMS level over both channels in dBFS. A full-scale sine is -3.01.
    pub rms_dbfs: f64,
    /// Sample peak minus RMS, in dB. A sine is 3.01; heavily limited
    /// masters sit around 6 to 9, open mixes at 12 and above.
    pub crest_factor_db: f64,
    /// Correlation of left and right: 1 mono, 0 unrelated, -1 out of phase.
    pub correlation: f32,
    /// Share of the energy in the side (left minus right) signal: 0 mono,
    /// 0.5 unrelated left and right, 1 fully out of phase.
    pub width: f32,
    /// Mean sample value over both channels. Should be near zero.
    pub dc_offset: f32,
}

/// Measures the levels of a buffer. Only the first two channels are used.
pub fn measure_levels(buffer: &AudioBuffer) -> Result<LevelReport, AnalysisError> {
    if buffer.frames() == 0 || buffer.channel_count() == 0 {
        return Err(AnalysisError::Empty);
    }
    let stereo = buffer.to_stereo();
    let (left, right) = (&stereo.channels[0], &stereo.channels[1]);
    let loudness = fourfloor_audio::measure(&stereo.planes(), buffer.sample_rate)?;

    let samples = (left.len() + right.len()) as f64;
    let square_sum: f64 = left
        .iter()
        .chain(right)
        .map(|&s| f64::from(s) * f64::from(s))
        .sum();
    let sum: f64 = left.iter().chain(right).map(|&s| f64::from(s)).sum();
    let rms_dbfs = 10.0 * (square_sum / samples).log10();
    let crest_factor_db = if rms_dbfs.is_finite() {
        loudness.sample_peak_dbfs - rms_dbfs
    } else {
        0.0
    };
    Ok(LevelReport {
        integrated_lufs: loudness.integrated_lufs,
        loudness_range_lu: loudness.loudness_range_lu,
        true_peak_dbtp: loudness.true_peak_dbtp,
        sample_peak_dbfs: loudness.sample_peak_dbfs,
        rms_dbfs,
        crest_factor_db,
        correlation: stereo_correlation(left, right),
        width: stereo_width(left, right),
        dc_offset: (sum / samples) as f32,
    })
}

/// Correlation of two channels over their common length: 1 for identical
/// signals, -1 for inverted, 0 for unrelated or when either is silent.
pub fn stereo_correlation(left: &[f32], right: &[f32]) -> f32 {
    let (mut cross, mut left_power, mut right_power) = (0.0f64, 0.0f64, 0.0f64);
    for (&l, &r) in left.iter().zip(right) {
        let (l, r) = (f64::from(l), f64::from(r));
        cross += l * r;
        left_power += l * l;
        right_power += r * r;
    }
    correlation_of(cross, left_power, right_power)
}

fn correlation_of(cross: f64, left_power: f64, right_power: f64) -> f32 {
    let scale = (left_power * right_power).sqrt();
    if scale > 1e-20 {
        (cross / scale).clamp(-1.0, 1.0) as f32
    } else {
        0.0
    }
}

/// Share of the energy that is in the side signal, 0 to 1: 0 for mono, 0.5
/// for unrelated channels, 1 for fully out of phase. Zero for silence.
pub fn stereo_width(left: &[f32], right: &[f32]) -> f32 {
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    for (&l, &r) in left.iter().zip(right) {
        let (l, r) = (f64::from(l), f64::from(r));
        mid += (l + r) * (l + r);
        side += (l - r) * (l - r);
    }
    if mid + side > 1e-20 {
        (side / (mid + side)) as f32
    } else {
        0.0
    }
}

/// A running correlation meter for a UI: feed it blocks, read the value.
///
/// It averages over a sliding exponential window, so the reading follows
/// the music rather than jumping block by block.
#[derive(Clone, Debug)]
pub struct CorrelationMeter {
    keep: f64,
    cross: f64,
    left_power: f64,
    right_power: f64,
}

impl CorrelationMeter {
    /// A meter averaging over about `window_seconds` (300 ms is usual).
    pub fn new(sample_rate: SampleRate, window_seconds: f64) -> Self {
        let samples = (window_seconds.max(0.0) * sample_rate.as_f64()).max(1.0);
        Self {
            keep: (-1.0 / samples).exp(),
            cross: 0.0,
            left_power: 0.0,
            right_power: 0.0,
        }
    }

    /// Feeds a block and returns the correlation after it, -1 to 1.
    pub fn process(&mut self, left: &[f32], right: &[f32]) -> f32 {
        let pass = 1.0 - self.keep;
        for (&l, &r) in left.iter().zip(right) {
            let (l, r) = (f64::from(l), f64::from(r));
            self.cross = self.keep * self.cross + pass * l * r;
            self.left_power = self.keep * self.left_power + pass * l * l;
            self.right_power = self.keep * self.right_power + pass * r * r;
        }
        self.value()
    }

    /// The current correlation, -1 to 1. Zero during silence.
    pub fn value(&self) -> f32 {
        correlation_of(self.cross, self.left_power, self.right_power)
    }

    /// Forgets everything heard so far.
    pub fn reset(&mut self) {
        self.cross = 0.0;
        self.left_power = 0.0;
        self.right_power = 0.0;
    }
}
