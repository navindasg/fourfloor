//! The DSP fallback separator: median-filter harmonic/percussive
//! separation (Fitzgerald, DAFx 2010) plus a band split of the harmonic
//! part. No model and no download, so it always works, but it is a rough
//! stand-in for a trained separator: sustained parts of drums (a kick's
//! tail) can land in the tonal stems and transients of tonal parts in
//! drums.

use serde::{Deserialize, Serialize};

use super::{soft_masks, Magnitudes, MaskEstimator, MaskSeparator, StftLayout};
use crate::error::MlError;

/// Settings of the fallback separator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HpssOptions {
    /// FFT size.
    pub fft_size: usize,
    /// Hop size.
    pub hop: usize,
    /// Length in frames of the median along time (harmonic estimate). Odd.
    pub harmonic_frames: usize,
    /// Length in bins of the median along frequency (percussive estimate).
    /// Odd.
    pub percussive_bins: usize,
    /// Exponent of the soft masks; 2 is a Wiener mask, higher is harder.
    pub mask_power: f32,
    /// Crossover between the `bass` and `other` stems, in hertz.
    pub bass_crossover_hz: f64,
}

impl Default for HpssOptions {
    fn default() -> Self {
        Self {
            fft_size: 2048,
            hop: 512,
            harmonic_frames: 17,
            percussive_bins: 17,
            mask_power: 2.0,
            bass_crossover_hz: 150.0,
        }
    }
}

/// Mask estimator of the fallback separator. Stems: `drums` (percussive),
/// `bass` (harmonic below the crossover), `other` (harmonic above it).
/// The three masks sum to one, so the stems sum back to the mix.
#[derive(Clone, Debug, Default)]
pub struct HpssEstimator {
    options: HpssOptions,
}

/// Name used in stem cache keys.
const NAME: &str = "fallback-hpss-v1";

impl HpssEstimator {
    /// An estimator with `options`.
    pub fn new(options: HpssOptions) -> Result<Self, MlError> {
        let odd = |n: usize| n >= 1 && n % 2 == 1;
        if !odd(options.harmonic_frames) || !odd(options.percussive_bins) {
            return Err(MlError::InvalidInput(
                "median lengths must be odd and positive".to_owned(),
            ));
        }
        let positive = |value: f64| value.is_finite() && value > 0.0;
        if !positive(f64::from(options.mask_power)) || !positive(options.bass_crossover_hz) {
            return Err(MlError::InvalidInput(
                "mask power and crossover must be positive".to_owned(),
            ));
        }
        crate::stft::Stft::new(options.fft_size, options.hop, crate::stft::Window::Hann)?;
        Ok(Self { options })
    }

    /// The fallback separator with default settings.
    pub fn separator() -> MaskSeparator<Self> {
        MaskSeparator::new(Self::default())
    }
}

impl MaskEstimator for HpssEstimator {
    fn name(&self) -> &str {
        NAME
    }

    fn is_fallback(&self) -> bool {
        true
    }

    fn layout(&self) -> StftLayout {
        StftLayout {
            sample_rate: None,
            fft_size: self.options.fft_size,
            hop: self.options.hop,
        }
    }

    fn stem_names(&self) -> Vec<String> {
        vec!["drums".to_owned(), "bass".to_owned(), "other".to_owned()]
    }

    fn masks(&self, input: &Magnitudes) -> Result<Vec<Vec<Vec<f32>>>, MlError> {
        let (frames, bins) = (input.frames, input.bins);
        // One mask for all channels, from their mean magnitude, so the
        // stereo image of each stem matches the mix.
        let mut mean = vec![0.0f32; frames * bins];
        for channel in &input.channels {
            for (sum, value) in mean.iter_mut().zip(channel) {
                *sum += value / input.channels.len() as f32;
            }
        }
        let harmonic = median_along_time(&mean, frames, bins, self.options.harmonic_frames);
        let percussive = median_along_frequency(&mean, frames, bins, self.options.percussive_bins);
        let hp = soft_masks(&[&harmonic, &percussive], self.options.mask_power);
        let crossover = self.options.bass_crossover_hz;
        let low_share: Vec<f32> = (0..bins)
            .map(|bin| {
                let ratio = bin as f64 * input.bin_hz / crossover;
                (1.0 / (1.0 + ratio.powi(8))) as f32
            })
            .collect();
        let mut bass = hp[0].clone();
        let mut other = hp[0].clone();
        for (bass_row, other_row) in bass
            .chunks_exact_mut(bins)
            .zip(other.chunks_exact_mut(bins))
        {
            for ((b, o), share) in bass_row
                .iter_mut()
                .zip(other_row.iter_mut())
                .zip(&low_share)
            {
                *b *= share;
                *o *= 1.0 - share;
            }
        }
        let channels = input.channels.len();
        Ok([hp[1].clone(), bass, other]
            .into_iter()
            .map(|mask| vec![mask; channels])
            .collect())
    }
}

fn median(window: &mut [f32]) -> f32 {
    let mid = window.len() / 2;
    let (_, value, _) = window.select_nth_unstable_by(mid, f32::total_cmp);
    *value
}

fn median_along_time(data: &[f32], frames: usize, bins: usize, length: usize) -> Vec<f32> {
    let half = length / 2;
    let mut out = vec![0.0f32; data.len()];
    let mut window = Vec::with_capacity(length);
    for bin in 0..bins {
        for frame in 0..frames {
            window.clear();
            for offset in 0..length {
                let at = (frame + offset).checked_sub(half).filter(|f| *f < frames);
                window.push(at.map_or(0.0, |f| data[f * bins + bin]));
            }
            out[frame * bins + bin] = median(&mut window);
        }
    }
    out
}

fn median_along_frequency(data: &[f32], frames: usize, bins: usize, length: usize) -> Vec<f32> {
    let half = length / 2;
    let mut out = vec![0.0f32; data.len()];
    let mut window = Vec::with_capacity(length);
    for frame in 0..frames {
        let row = &data[frame * bins..(frame + 1) * bins];
        for bin in 0..bins {
            window.clear();
            for offset in 0..length {
                let at = (bin + offset).checked_sub(half).filter(|b| *b < bins);
                window.push(at.map_or(0.0, |b| row[b]));
            }
            out[frame * bins + bin] = median(&mut window);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medians_pick_the_middle_value() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        // A single spike in time is removed by the time median and kept by
        // the frequency median, and the other way round for a single bin.
        let (frames, bins) = (9, 9);
        let mut data = vec![0.0f32; frames * bins];
        for bin in 0..bins {
            data[4 * bins + bin] = 1.0; // a click: every bin of frame 4
        }
        let along_time = median_along_time(&data, frames, bins, 5);
        let along_freq = median_along_frequency(&data, frames, bins, 5);
        assert_eq!(along_time[4 * bins + 4], 0.0);
        assert_eq!(along_freq[4 * bins + 4], 1.0);
    }

    #[test]
    fn invalid_options_are_refused() {
        let even = HpssOptions {
            harmonic_frames: 16,
            ..HpssOptions::default()
        };
        assert!(HpssEstimator::new(even).is_err());
        let bad_fft = HpssOptions {
            fft_size: 1000,
            hop: 900,
            ..HpssOptions::default()
        };
        assert!(HpssEstimator::new(bad_fft).is_err());
        assert!(HpssEstimator::new(HpssOptions::default()).is_ok());
    }
}
