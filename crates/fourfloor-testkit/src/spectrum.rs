//! Frequency-domain measurements: the dominant frequency of a signal and
//! how much of its power lies in a band.

use fourfloor_core::SampleRate;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// The power spectrum of a signal, from one Hann-windowed FFT over the
/// whole signal.
///
/// Power is scaled so that it adds up to the signal's mean square: the
/// power of a sine of peak `a` sums to `a * a / 2` over the bins around its
/// frequency, the same number as its RMS squared. That makes band powers
/// comparable with [`rms`](crate::rms) results.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    /// Width of one bin in Hz.
    pub bin_hz: f64,
    /// Power per bin, from 0 Hz up to the Nyquist frequency.
    pub power: Vec<f64>,
}

/// Computes the power spectrum of `signal`. An empty signal gives an empty
/// spectrum.
pub fn spectrum(signal: &[f32], sample_rate: SampleRate) -> Spectrum {
    let n = signal.len();
    if n == 0 {
        return Spectrum {
            bin_hz: 0.0,
            power: Vec::new(),
        };
    }
    let mut window_power = 0.0f64;
    let mut buffer: Vec<Complex<f64>> = signal
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            window_power += w * w;
            Complex::new(f64::from(*sample) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buffer);

    let half = n / 2;
    let scale = 1.0 / (n as f64 * window_power.max(f64::MIN_POSITIVE));
    let power = (0..=half)
        .map(|k| {
            // Every bin but 0 Hz and Nyquist also stands for its mirror.
            let sides = if k == 0 || (n.is_multiple_of(2) && k == half) {
                1.0
            } else {
                2.0
            };
            sides * buffer[k].norm_sqr() * scale
        })
        .collect();
    Spectrum {
        bin_hz: sample_rate.as_f64() / n as f64,
        power,
    }
}

impl Spectrum {
    /// Total power: the signal's mean square.
    pub fn total_power(&self) -> f64 {
        self.power.iter().sum()
    }

    /// Power between `low_hz` and `high_hz`, inclusive.
    pub fn band_power(&self, low_hz: f64, high_hz: f64) -> f64 {
        if self.bin_hz <= 0.0 || high_hz < low_hz {
            return 0.0;
        }
        let first = (low_hz.max(0.0) / self.bin_hz).ceil() as usize;
        let last = ((high_hz / self.bin_hz).floor() as usize).min(self.power.len() - 1);
        if first > last {
            return 0.0;
        }
        self.power[first..=last].iter().sum()
    }

    /// Frequency of the strongest component, in Hz, refined between bins by
    /// fitting a parabola to the peak. Zero for silence. The 0 Hz bin is
    /// ignored, but a large DC offset also leaks into the bin above it;
    /// [`dominant_frequency`] removes the offset first.
    pub fn peak_frequency(&self) -> f64 {
        let Some((peak, power)) = self
            .power
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
        else {
            return 0.0;
        };
        if *power <= 0.0 {
            return 0.0;
        }
        let mut offset = 0.0;
        if peak + 1 < self.power.len() {
            let ln = |k: usize| self.power[k].max(f64::MIN_POSITIVE).ln();
            let (a, b, c) = (ln(peak - 1), ln(peak), ln(peak + 1));
            let curve = a - 2.0 * b + c;
            if curve < 0.0 {
                offset = (0.5 * (a - c) / curve).clamp(-0.5, 0.5);
            }
        }
        (peak as f64 + offset) * self.bin_hz
    }

    /// The power-weighted mean frequency in Hz: a measure of brightness.
    /// Zero for silence.
    pub fn centroid_hz(&self) -> f64 {
        let total = self.total_power();
        if total <= 0.0 {
            return 0.0;
        }
        let weighted: f64 = self
            .power
            .iter()
            .enumerate()
            .map(|(k, power)| k as f64 * self.bin_hz * power)
            .sum();
        weighted / total
    }
}

/// Frequency of the strongest component of `signal`, in Hz. Accurate to a
/// small fraction of `sample_rate / signal.len()` for a steady tone. A DC
/// offset is removed first and never counts. Zero for silence.
pub fn dominant_frequency(signal: &[f32], sample_rate: SampleRate) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    let mean = signal.iter().map(|s| f64::from(*s)).sum::<f64>() / signal.len() as f64;
    let centred: Vec<f32> = signal
        .iter()
        .map(|s| (f64::from(*s) - mean) as f32)
        .collect();
    spectrum(&centred, sample_rate).peak_frequency() as f32
}

/// Power of `signal` between `low_hz` and `high_hz`, scaled like a mean
/// square (see [`Spectrum`]). Compare two bands, or one band of two
/// renders, rather than reading the absolute number.
pub fn spectrum_band_energy(
    signal: &[f32],
    sample_rate: SampleRate,
    low_hz: f32,
    high_hz: f32,
) -> f64 {
    spectrum(signal, sample_rate).band_power(f64::from(low_hz), f64::from(high_hz))
}

/// [`spectrum_band_energy`] in decibels (`10 * log10`), so a full-scale
/// sine inside the band reads -3.01.
pub fn spectrum_band_energy_db(
    signal: &[f32],
    sample_rate: SampleRate,
    low_hz: f32,
    high_hz: f32,
) -> f64 {
    10.0 * spectrum_band_energy(signal, sample_rate, low_hz, high_hz).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{dc, mix, sine, white_noise};

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn dominant_frequency_finds_tones_between_bins() {
        // A quarter second: bins are 4 Hz apart.
        for frequency in [55.0, 123.4, 440.0, 1_001.7, 9_876.5] {
            let found = dominant_frequency(&sine(frequency, 0.5, 0.25, RATE), RATE);
            assert!(
                (f64::from(found) - frequency).abs() < 0.3,
                "{frequency} Hz read as {found}"
            );
        }
    }

    #[test]
    fn dominant_frequency_picks_the_loudest_and_ignores_dc() {
        let both = mix(&[
            &sine(200.0, 0.2, 0.5, RATE),
            &sine(3_000.0, 0.6, 0.5, RATE),
            &dc(0.9, 24_000),
        ]);
        let found = dominant_frequency(&both, RATE);
        assert!((found - 3_000.0).abs() < 1.0, "{found}");
        assert_eq!(dominant_frequency(&dc(0.0, 4_800), RATE), 0.0);
        assert_eq!(dominant_frequency(&[], RATE), 0.0);
    }

    #[test]
    fn band_energy_of_a_sine_is_its_mean_square_inside_the_band_only() {
        let tone = sine(1_000.0, 0.5, 0.5, RATE);
        let inside = spectrum_band_energy(&tone, RATE, 900.0, 1_100.0);
        assert!((inside - 0.125).abs() < 1e-3, "{inside}");
        let outside = spectrum_band_energy(&tone, RATE, 2_000.0, 20_000.0);
        assert!(outside < inside * 1e-8, "leakage {outside}");
        let db = spectrum_band_energy_db(&sine(1_000.0, 1.0, 0.5, RATE), RATE, 0.0, 24_000.0);
        assert!((db + 3.01).abs() < 0.05, "{db}");
        assert_eq!(spectrum_band_energy(&tone, RATE, 500.0, 100.0), 0.0);
        assert_eq!(spectrum_band_energy(&[], RATE, 0.0, 100.0), 0.0);
    }

    #[test]
    fn total_power_matches_the_mean_square() {
        let noise = white_noise(3, 0.5, 1.0, RATE);
        let mean_square: f64 =
            noise.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / noise.len() as f64;
        let spectrum = spectrum(&noise, RATE);
        let ratio = spectrum.total_power() / mean_square;
        assert!((ratio - 1.0).abs() < 0.02, "ratio {ratio}");
        assert_eq!(spectrum.power.len(), 24_001);
        assert_eq!(spectrum.bin_hz, 1.0);
        // White noise: each half of the band holds about half the power.
        let low = spectrum.band_power(0.0, 12_000.0) / spectrum.total_power();
        assert!((low - 0.5).abs() < 0.03, "low half {low}");
    }

    #[test]
    fn centroid_tells_bright_from_dark() {
        let dark = spectrum(&sine(100.0, 0.5, 0.25, RATE), RATE).centroid_hz();
        let bright = spectrum(&sine(8_000.0, 0.5, 0.25, RATE), RATE).centroid_hz();
        assert!((dark - 100.0).abs() < 5.0, "{dark}");
        assert!((bright - 8_000.0).abs() < 5.0, "{bright}");
        assert_eq!(spectrum(&dc(0.0, 64), RATE).centroid_hz(), 0.0);
    }

    #[test]
    fn odd_lengths_and_single_samples_do_not_panic() {
        let odd = sine(1_000.0, 0.5, 0.1, RATE);
        let found = dominant_frequency(&odd[..4_799], RATE);
        assert!((found - 1_000.0).abs() < 1.0);
        assert_eq!(dominant_frequency(&[0.5], RATE), 0.0);
        assert_eq!(dominant_frequency(&[0.5, -0.5], RATE), 24_000.0);
    }
}
