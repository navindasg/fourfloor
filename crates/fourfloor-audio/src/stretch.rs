//! Offline, pitch-preserving time stretch on the `signalsmith-stretch`
//! 0.1.3 bindings (C++ Signalsmith Stretch 1.3.1, MIT, compiled by `cc` and
//! bound by `bindgen`, so it needs a C++ compiler and libclang). Behind the
//! `stretch` feature. Graduated from the `fourfloor-spikes` stretch spike.
//!
//! This is an offline, allocating helper. It is not meant to be called from
//! the audio thread.

use fourfloor_core::SampleRate;
use signalsmith_stretch::Stretch;

/// Why a stretch request was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StretchError {
    /// The ratio was not a finite number in [`MIN_RATIO`]..=[`MAX_RATIO`].
    InvalidRatio,
    /// The input was empty.
    EmptyInput,
    /// The requested output is shorter than the library can produce in one
    /// shot (it needs at least twice its output latency).
    TooShort,
}

impl std::fmt::Display for StretchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            StretchError::InvalidRatio => "stretch ratio must be finite and within 0.25..=4.0",
            StretchError::EmptyInput => "input buffer is empty",
            StretchError::TooShort => "output would be too short for the stretch window",
        };
        f.write_str(text)
    }
}

impl std::error::Error for StretchError {}

/// Smallest accepted length ratio (4x faster).
pub const MIN_RATIO: f64 = 0.25;
/// Largest accepted length ratio (4x slower).
pub const MAX_RATIO: f64 = 4.0;

/// Number of output frames [`stretch_mono`] produces for `input_len` frames.
pub fn stretched_len(input_len: usize, ratio: f64) -> usize {
    (input_len as f64 * ratio).round() as usize
}

/// Time-stretch a mono buffer so it is `ratio` times as long, keeping pitch.
///
/// `ratio` is output length over input length: 2.0 plays at half speed, 0.5
/// at double speed. The result has exactly [`stretched_len`] frames.
pub fn stretch_mono(
    input: &[f32],
    sample_rate: SampleRate,
    ratio: f64,
) -> Result<Vec<f32>, StretchError> {
    if !ratio.is_finite() || !(MIN_RATIO..=MAX_RATIO).contains(&ratio) {
        return Err(StretchError::InvalidRatio);
    }
    if input.is_empty() {
        return Err(StretchError::EmptyInput);
    }
    let mut output = vec![0.0_f32; stretched_len(input.len(), ratio)];
    let mut stretch = Stretch::preset_default(1, sample_rate.hz());
    // No transpose: pitch stays where it was.
    stretch.set_transpose_factor(1.0, None);
    if stretch.exact(input, &mut output) {
        Ok(output)
    } else {
        Err(StretchError::TooShort)
    }
}

/// Input and output latency, in frames, of the default preset at this rate.
pub fn default_preset_latency(sample_rate: SampleRate) -> (usize, usize) {
    let stretch = Stretch::preset_default(1, sample_rate.hz());
    (stretch.input_latency(), stretch.output_latency())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::{num_complex::Complex, FftPlanner};

    const RATE: SampleRate = SampleRate::HZ_48000;

    fn sine(freq: f64, seconds: f64) -> Vec<f32> {
        let n = (seconds * RATE.as_f64()) as usize;
        (0..n)
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / RATE.as_f64()).sin()) as f32
            })
            .collect()
    }

    /// Dominant frequency of the middle half of `samples`, Hann-windowed.
    fn dominant_hz(samples: &[f32]) -> f64 {
        let quarter = samples.len() / 4;
        let slice = &samples[quarter..samples.len() - quarter];
        let n = slice.len();
        let mut bins: Vec<Complex<f32>> = slice
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
                Complex::new(s * w, 0.0)
            })
            .collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut bins);
        let peak = bins[1..n / 2]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm_sqr().total_cmp(&b.1.norm_sqr()))
            .map(|(i, _)| i + 1)
            .unwrap();
        peak as f64 * RATE.as_f64() / n as f64
    }

    fn rms(samples: &[f32]) -> f64 {
        let quarter = samples.len() / 4;
        let mid = &samples[quarter..samples.len() - quarter];
        (mid.iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>()
            / mid.len() as f64)
            .sqrt()
    }

    #[test]
    fn stretching_changes_length_but_not_pitch() {
        let input = sine(440.0, 2.0);
        assert!((dominant_hz(&input) - 440.0).abs() < 2.0);
        for ratio in [0.5, 0.9, 1.0, 1.03, 1.1, 1.5, 2.0] {
            let out = stretch_mono(&input, RATE, ratio).unwrap();
            assert_eq!(out.len(), stretched_len(input.len(), ratio));
            let hz = dominant_hz(&out);
            assert!(
                (hz - 440.0).abs() < 3.0,
                "ratio {ratio}: dominant frequency moved to {hz} Hz"
            );
            let level = rms(&out) / rms(&input);
            assert!(
                (0.7..1.3).contains(&level),
                "ratio {ratio}: level changed by a factor of {level}"
            );
        }
    }

    #[test]
    fn bad_arguments_are_rejected() {
        let input = sine(440.0, 0.5);
        assert_eq!(
            stretch_mono(&input, RATE, f64::NAN),
            Err(StretchError::InvalidRatio)
        );
        assert_eq!(
            stretch_mono(&input, RATE, 0.0),
            Err(StretchError::InvalidRatio)
        );
        assert_eq!(
            stretch_mono(&input, RATE, 8.0),
            Err(StretchError::InvalidRatio)
        );
        assert_eq!(stretch_mono(&[], RATE, 1.0), Err(StretchError::EmptyInput));
    }

    #[test]
    fn a_buffer_shorter_than_the_window_is_reported_not_garbled() {
        let input = sine(440.0, 0.001);
        assert_eq!(stretch_mono(&input, RATE, 1.0), Err(StretchError::TooShort));
    }

    #[test]
    fn default_preset_reports_nonzero_latency() {
        let (input, output) = default_preset_latency(RATE);
        assert!(input > 0 && output > 0);
    }
}
