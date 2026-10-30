//! Test signals: sines, noise, impulses and click trains, all deterministic.

use fourfloor_core::SampleRate;

/// Number of frames in `seconds` at `sample_rate`, rounded.
pub fn seconds_to_frames(seconds: f64, sample_rate: SampleRate) -> usize {
    (seconds.max(0.0) * sample_rate.as_f64()).round() as usize
}

/// A sine at `frequency` Hz with linear peak `amplitude`, starting at phase
/// zero.
pub fn sine(frequency: f64, amplitude: f32, seconds: f64, sample_rate: SampleRate) -> Vec<f32> {
    let rate = sample_rate.as_f64();
    (0..seconds_to_frames(seconds, sample_rate))
        .map(|i| {
            let phase = std::f64::consts::TAU * frequency * i as f64 / rate;
            (f64::from(amplitude) * phase.sin()) as f32
        })
        .collect()
}

/// White noise, uniform in `-amplitude..amplitude`, from a xorshift
/// generator: the same `seed` always gives the same samples. A seed of zero
/// is replaced, because xorshift cannot leave zero.
pub fn white_noise(seed: u32, amplitude: f32, seconds: f64, sample_rate: SampleRate) -> Vec<f32> {
    let mut state = if seed == 0 { 0x9E37_79B9 } else { seed };
    (0..seconds_to_frames(seconds, sample_rate))
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            ((state >> 8) as f32 / 8_388_608.0 - 1.0) * amplitude
        })
        .collect()
}

/// Silence with a single sample of `amplitude` at frame `at`.
pub fn impulse(at: usize, amplitude: f32, frames: usize) -> Vec<f32> {
    let mut signal = vec![0.0; frames];
    if let Some(sample) = signal.get_mut(at) {
        *sample = amplitude;
    }
    signal
}

/// `frames` frames of silence.
pub fn silence(frames: usize) -> Vec<f32> {
    vec![0.0; frames]
}

/// A constant value.
pub fn dc(value: f32, frames: usize) -> Vec<f32> {
    vec![value; frames]
}

/// Short decaying sine bursts starting at each of `times` (seconds): a
/// stand-in for drum hits when testing onset detection or dynamics. Each
/// burst is a 1 kHz sine with a 10 ms decay, at linear peak `amplitude`.
pub fn click_train(
    times: &[f64],
    amplitude: f32,
    seconds: f64,
    sample_rate: SampleRate,
) -> Vec<f32> {
    let rate = sample_rate.as_f64();
    let mut signal = vec![0.0f32; seconds_to_frames(seconds, sample_rate)];
    let burst = seconds_to_frames(0.08, sample_rate);
    for &time in times {
        let start = seconds_to_frames(time, sample_rate);
        for (offset, sample) in signal.iter_mut().skip(start).take(burst).enumerate() {
            let t = offset as f64 / rate;
            let value = (std::f64::consts::TAU * 1_000.0 * t).cos() * (-t / 0.01).exp();
            *sample += (f64::from(amplitude) * value) as f32;
        }
    }
    signal
}

/// Adds signals sample by sample. The result is as long as the longest.
pub fn mix(signals: &[&[f32]]) -> Vec<f32> {
    let frames = signals.iter().map(|signal| signal.len()).max().unwrap_or(0);
    (0..frames)
        .map(|i| signals.iter().filter_map(|signal| signal.get(i)).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn sine_has_the_requested_length_peak_and_period() {
        let signal = sine(12_000.0, 0.5, 0.001, RATE);
        assert_eq!(signal.len(), 48);
        for (i, expected) in [0.0, 0.5, 0.0, -0.5].iter().enumerate() {
            assert!((signal[i] - expected).abs() < 1e-6);
        }
        assert_eq!(seconds_to_frames(-1.0, RATE), 0);
    }

    #[test]
    fn noise_is_repeatable_bounded_and_roughly_centred() {
        let a = white_noise(7, 0.5, 1.0, RATE);
        assert_eq!(a, white_noise(7, 0.5, 1.0, RATE));
        assert_ne!(a, white_noise(8, 0.5, 1.0, RATE));
        assert!(a.iter().all(|s| s.abs() <= 0.5));
        let mean: f32 = a.iter().sum::<f32>() / a.len() as f32;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!(white_noise(0, 1.0, 0.01, RATE).iter().any(|s| *s != 0.0));
    }

    #[test]
    fn impulse_click_train_and_mix_place_samples_where_asked() {
        assert_eq!(impulse(2, 1.0, 4), [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(impulse(9, 1.0, 2), [0.0, 0.0]);
        assert_eq!(silence(3), dc(0.0, 3));

        let clicks = click_train(&[0.1, 0.5], 0.8, 1.0, RATE);
        assert_eq!(clicks.len(), 48_000);
        assert!(clicks[..4_800].iter().all(|s| *s == 0.0));
        assert!(
            (clicks[4_800] - 0.8).abs() < 1e-6,
            "a click starts at full level"
        );
        assert!((clicks[24_000] - 0.8).abs() < 1e-3);
        assert!(clicks[20_000..24_000].iter().all(|s| s.abs() < 1e-3));

        assert_eq!(mix(&[&[1.0, 2.0], &[10.0]]), [11.0, 2.0]);
        assert!(mix(&[]).is_empty());
    }
}
