//! Level measurements in the time domain: peak, RMS, envelope, silence,
//! stereo correlation and decibel conversion.

use fourfloor_core::SampleRate;

/// Converts a linear amplitude to decibels. Zero maps to negative infinity.
pub fn to_db(linear: f32) -> f32 {
    20.0 * linear.abs().log10()
}

/// Converts decibels to a linear amplitude.
pub fn from_db(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

/// Highest absolute sample value. Zero for an empty signal. A NaN sample
/// makes the result NaN, so a broken render cannot pass as quiet.
pub fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0f32, |peak, sample| {
        if sample.is_nan() || peak.is_nan() {
            f32::NAN
        } else {
            peak.max(sample.abs())
        }
    })
}

/// [`peak`] in dBFS.
pub fn peak_db(signal: &[f32]) -> f32 {
    to_db(peak(signal))
}

/// Root mean square of the whole signal. Zero for an empty signal.
pub fn rms(signal: &[f32]) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    let sum: f64 = signal.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / signal.len() as f64).sqrt() as f32
}

/// [`rms`] in dBFS. A full-scale sine reads -3.01.
pub fn rms_db(signal: &[f32]) -> f32 {
    to_db(rms(signal))
}

/// RMS of `len` samples starting at `start`, clamped to the signal. Zero
/// when the window is empty or starts past the end.
pub fn rms_window(signal: &[f32], start: usize, len: usize) -> f32 {
    let start = start.min(signal.len());
    let end = start.saturating_add(len).min(signal.len());
    rms(&signal[start..end])
}

/// RMS of the window from `start_seconds` lasting `length_seconds`.
pub fn rms_window_seconds(
    signal: &[f32],
    sample_rate: SampleRate,
    start_seconds: f64,
    length_seconds: f64,
) -> f32 {
    let rate = sample_rate.as_f64();
    rms_window(
        signal,
        (start_seconds.max(0.0) * rate).round() as usize,
        (length_seconds.max(0.0) * rate).round() as usize,
    )
}

/// Whether no sample is louder than `threshold_db` dBFS. An empty signal is
/// silent; a signal with a NaN in it is not.
pub fn is_silent(signal: &[f32], threshold_db: f32) -> bool {
    peak(signal) <= from_db(threshold_db)
}

/// An envelope follower: tracks the absolute value of the signal, rising
/// with time constant `attack_ms` and falling with `release_ms`. The result
/// has one value per input sample. An attack of zero follows peaks
/// instantly.
pub fn envelope(
    signal: &[f32],
    sample_rate: SampleRate,
    attack_ms: f32,
    release_ms: f32,
) -> Vec<f32> {
    let rate = sample_rate.hz() as f32;
    let coefficient = |ms: f32| {
        if ms <= 0.0 {
            0.0
        } else {
            (-1.0 / (ms * 0.001 * rate)).exp()
        }
    };
    let (attack, release) = (coefficient(attack_ms), coefficient(release_ms));
    let mut level = 0.0f32;
    signal
        .iter()
        .map(|sample| {
            let input = sample.abs();
            let keep = if input > level { attack } else { release };
            level = input + (level - input) * keep;
            level
        })
        .collect()
}

/// Correlation between two channels, -1 to 1: 1 for identical signals
/// (dual mono), 0 for unrelated ones, -1 for one being the other inverted.
/// This is what a phase correlation meter shows. Zero when either channel
/// is silent. Only the overlapping part is compared.
pub fn stereo_correlation(left: &[f32], right: &[f32]) -> f32 {
    let (mut cross, mut left_power, mut right_power) = (0.0f64, 0.0f64, 0.0f64);
    for (l, r) in left.iter().zip(right) {
        let (l, r) = (f64::from(*l), f64::from(*r));
        cross += l * r;
        left_power += l * l;
        right_power += r * r;
    }
    let scale = (left_power * right_power).sqrt();
    if scale == 0.0 {
        0.0
    } else {
        (cross / scale) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{dc, sine, white_noise};

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn decibel_conversions_hit_known_points() {
        assert_eq!(to_db(1.0), 0.0);
        assert!((to_db(0.5) + 6.0206).abs() < 1e-3);
        assert!((to_db(-0.5) + 6.0206).abs() < 1e-3, "sign is ignored");
        assert_eq!(to_db(0.0), f32::NEG_INFINITY);
        assert!((from_db(-20.0) - 0.1).abs() < 1e-6);
        assert!((from_db(to_db(0.3)) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn peak_and_rms_of_a_sine_match_theory() {
        let tone = sine(1_000.0, 0.5, 1.0, RATE);
        assert!((peak(&tone) - 0.5).abs() < 1e-4);
        assert!((peak_db(&tone) + 6.02).abs() < 0.01);
        assert!((rms(&tone) - 0.5 / 2.0f32.sqrt()).abs() < 1e-4);
        assert!((rms_db(&sine(1_000.0, 1.0, 1.0, RATE)) + 3.01).abs() < 0.01);
        assert_eq!((peak(&[]), rms(&[])), (0.0, 0.0));
        assert!(peak(&[0.1, f32::NAN, 0.2]).is_nan());
    }

    #[test]
    fn rms_window_measures_only_its_window() {
        let mut signal = dc(0.0, 1_000);
        signal.extend(dc(0.5, 1_000));
        assert_eq!(rms_window(&signal, 0, 1_000), 0.0);
        assert_eq!(rms_window(&signal, 1_000, 1_000), 0.5);
        assert!((rms_window(&signal, 500, 1_000) - 0.5 / 2.0f32.sqrt()).abs() < 1e-6);
        assert_eq!(rms_window(&signal, 1_900, 5_000), 0.5, "clamped to the end");
        assert_eq!(rms_window(&signal, 9_000, 10), 0.0);
        let by_time = rms_window_seconds(&signal, RATE, 1_000.0 / 48_000.0, 0.01);
        assert_eq!(by_time, 0.5);
    }

    #[test]
    fn silence_is_judged_by_peak() {
        assert!(is_silent(&[], -120.0));
        assert!(is_silent(&dc(0.0, 10), -120.0));
        assert!(is_silent(&dc(0.000_5, 10), -60.0));
        assert!(!is_silent(&dc(0.002, 10), -60.0));
        assert!(!is_silent(&[0.0, f32::NAN], -60.0));
    }

    #[test]
    fn envelope_rises_and_falls_with_its_time_constants() {
        // One second of full level, then silence.
        let mut signal = sine(1_000.0, 1.0, 0.5, RATE);
        signal.extend(dc(0.0, 24_000));
        let follower = envelope(&signal, RATE, 0.0, 100.0);
        assert_eq!(follower.len(), signal.len());
        assert!(follower[23_000] > 0.95, "holds the peak of the sine");
        // 100 ms after the sound stops the envelope is down to about 1/e.
        let after = follower[24_000 + 4_800];
        assert!((after - (-1.0f32).exp()).abs() < 0.03, "{after}");
        assert!(follower[47_999] < 0.01);

        let slow = envelope(&dc(1.0, 4_800), RATE, 50.0, 50.0);
        assert!((slow[2_400] - (1.0 - (-1.0f32).exp())).abs() < 0.01);
        assert!(slow.windows(2).all(|w| w[1] >= w[0]), "monotonic rise");
    }

    #[test]
    fn correlation_reads_mono_inverted_and_unrelated_signals() {
        let tone = sine(440.0, 0.5, 0.5, RATE);
        let inverted: Vec<f32> = tone.iter().map(|s| -s).collect();
        assert!((stereo_correlation(&tone, &tone) - 1.0).abs() < 1e-6);
        assert!((stereo_correlation(&tone, &inverted) + 1.0).abs() < 1e-6);
        let (a, b) = (
            white_noise(1, 1.0, 1.0, RATE),
            white_noise(99, 1.0, 1.0, RATE),
        );
        assert!(stereo_correlation(&a, &b).abs() < 0.05);
        // A quarter cycle apart: uncorrelated.
        let quarter: Vec<f32> = sine(440.0, 0.5, 0.5, RATE)
            .iter()
            .enumerate()
            .map(|(i, _)| {
                (0.5 * (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).cos()) as f32
            })
            .collect();
        assert!(stereo_correlation(&tone, &quarter).abs() < 0.01);
        assert_eq!(stereo_correlation(&tone, &dc(0.0, tone.len())), 0.0);
    }
}
