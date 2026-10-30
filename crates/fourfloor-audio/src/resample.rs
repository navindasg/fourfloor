//! Sample-rate conversion: an offline windowed-sinc resampler.
//!
//! The filter is a Kaiser-windowed sinc with 32 zero crossings on each
//! side, read from a table with linear interpolation between entries. It
//! is zero-phase: the output is not delayed against the input. When
//! converting down, the cutoff moves to the new Nyquist frequency so
//! content above it is removed rather than aliased.

use fourfloor_core::SampleRate;

use crate::buffer::AudioBuffer;

/// Zero crossings of the sinc on each side of its centre.
const HALF_WIDTH: usize = 32;
/// Table entries per zero crossing.
const TABLE_RESOLUTION: usize = 512;
/// Kaiser window shape; about 90 dB of stop-band attenuation.
const KAISER_BETA: f64 = 9.0;
/// Fraction of the Nyquist band that is passed.
const PASSBAND: f64 = 0.94;

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= (half / k as f64).powi(2);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// One side of the windowed sinc: `table[i]` is the filter at
/// `i / TABLE_RESOLUTION` zero crossings from the centre.
fn filter_table() -> Vec<f64> {
    let len = HALF_WIDTH * TABLE_RESOLUTION;
    let norm = bessel_i0(KAISER_BETA);
    (0..=len + 1)
        .map(|i| {
            let x = i as f64 / TABLE_RESOLUTION as f64;
            if i >= len {
                return 0.0;
            }
            let sinc = if i == 0 {
                1.0
            } else {
                let px = std::f64::consts::PI * x * PASSBAND;
                px.sin() / px
            };
            let w = x / HALF_WIDTH as f64;
            let window = bessel_i0(KAISER_BETA * (1.0 - w * w).max(0.0).sqrt()) / norm;
            PASSBAND * sinc * window
        })
        .collect()
}

/// Converts one channel from `from` to `to`. The result has
/// `ceil(input.len() * to / from)` samples and is aligned with the input:
/// output sample `n` is the signal at time `n / to`.
pub fn resample(input: &[f32], from: SampleRate, to: SampleRate) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let table = filter_table();
    let (from_hz, to_hz) = (from.as_f64(), to.as_f64());
    let ratio = to_hz / from_hz;
    // Converting down stretches the filter so its cutoff is the new Nyquist.
    let scale = ratio.min(1.0);
    let reach = HALF_WIDTH as f64 / scale;
    // Whole-number arithmetic: a float product can land a hair above an
    // exact length and round up to one sample too many.
    let (from_int, to_int) = (u128::from(from.hz()), u128::from(to.hz()));
    let out_len = (input.len() as u128 * to_int).div_ceil(from_int) as usize;

    (0..out_len)
        .map(|n| {
            let centre = n as f64 / ratio;
            let first = (centre - reach).ceil().max(0.0) as usize;
            let last = ((centre + reach).floor() as usize).min(input.len() - 1);
            let mut sum = 0.0f64;
            for (k, sample) in input.iter().enumerate().take(last + 1).skip(first) {
                let distance = (centre - k as f64).abs() * scale * TABLE_RESOLUTION as f64;
                let index = distance as usize;
                let frac = distance - index as f64;
                let tap = table[index] + (table[index + 1] - table[index]) * frac;
                sum += f64::from(*sample) * tap;
            }
            (sum * scale) as f32
        })
        .collect()
}

/// Converts every channel of `buffer` to `to`.
pub fn resample_buffer(buffer: &AudioBuffer, to: SampleRate) -> AudioBuffer {
    AudioBuffer {
        sample_rate: to,
        channels: buffer
            .channels
            .iter()
            .map(|channel| resample(channel, buffer.sample_rate, to))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ_44100: SampleRate = SampleRate::HZ_44100;
    const HZ_48000: SampleRate = SampleRate::HZ_48000;

    fn sine(freq: f64, rate: SampleRate, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (0.5 * (std::f64::consts::TAU * freq * i as f64 / rate.as_f64()).sin()) as f32)
            .collect()
    }

    fn rms(signal: &[f32]) -> f64 {
        (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
    }

    /// RMS of the difference from an ideal sine, ignoring the edges where
    /// the filter runs off the ends of the input.
    fn error_db(output: &[f32], freq: f64, rate: SampleRate) -> f64 {
        let ideal = sine(freq, rate, output.len());
        let edge = 2_000;
        let diff: Vec<f32> = output[edge..output.len() - edge]
            .iter()
            .zip(&ideal[edge..])
            .map(|(a, b)| a - b)
            .collect();
        20.0 * (rms(&diff) / rms(&ideal)).log10()
    }

    #[test]
    fn the_same_rate_is_a_copy_and_empty_stays_empty() {
        let input = sine(440.0, HZ_48000, 100);
        assert_eq!(resample(&input, HZ_48000, HZ_48000), input);
        assert!(resample(&[], HZ_44100, HZ_48000).is_empty());
    }

    #[test]
    fn upsampling_keeps_length_pitch_and_phase() {
        let input = sine(1_000.0, HZ_44100, 44_100);
        let output = resample(&input, HZ_44100, HZ_48000);
        assert_eq!(output.len(), 48_000, "one second stays one second");
        let error = error_db(&output, 1_000.0, HZ_48000);
        assert!(error < -70.0, "error {error:.1} dB");
    }

    #[test]
    fn downsampling_keeps_length_pitch_and_phase() {
        let input = sine(5_000.0, HZ_48000, 48_000);
        let output = resample(&input, HZ_48000, HZ_44100);
        assert_eq!(output.len(), 44_100);
        let error = error_db(&output, 5_000.0, HZ_44100);
        assert!(error < -70.0, "error {error:.1} dB");
    }

    #[test]
    fn downsampling_removes_what_would_alias() {
        // 15 kHz cannot exist at 22.05 kHz; it must be filtered out, not
        // folded down to 7.05 kHz.
        let half = SampleRate::new(22_050).unwrap();
        let input = sine(15_000.0, HZ_48000, 48_000);
        let output = resample(&input, HZ_48000, half);
        assert_eq!(output.len(), 22_050);
        let level = 20.0 * (rms(&output[2_000..20_000]) / rms(&input)).log10();
        assert!(level < -60.0, "alias at {level:.1} dB");

        // 5 kHz is inside the band and passes at full level.
        let kept = resample(&sine(5_000.0, HZ_48000, 48_000), HZ_48000, half);
        let level = 20.0 * (rms(&kept[2_000..20_000]) / rms(&input)).log10();
        assert!(level.abs() < 0.1, "passband at {level:.2} dB");
    }

    #[test]
    fn a_round_trip_returns_the_signal() {
        let input = sine(2_000.0, HZ_48000, 24_000);
        let back = resample(&resample(&input, HZ_48000, HZ_44100), HZ_44100, HZ_48000);
        assert_eq!(back.len(), input.len());
        let diff: Vec<f32> = back[2_000..22_000]
            .iter()
            .zip(&input[2_000..])
            .map(|(a, b)| a - b)
            .collect();
        let error = 20.0 * (rms(&diff) / rms(&input)).log10();
        assert!(error < -65.0, "round-trip error {error:.1} dB");
    }

    #[test]
    fn buffers_are_converted_channel_by_channel() {
        let buffer = AudioBuffer::new(
            HZ_44100,
            vec![sine(440.0, HZ_44100, 4_410), vec![0.0; 4_410]],
        );
        let converted = resample_buffer(&buffer, HZ_48000);
        assert_eq!(converted.sample_rate, HZ_48000);
        assert_eq!(converted.frames(), 4_800);
        assert!(converted.channels[1].iter().all(|s| *s == 0.0));
        assert!(rms(&converted.channels[0]) > 0.3);
    }
}
