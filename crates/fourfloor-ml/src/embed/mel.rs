//! Log-mel spectrograms as the CLAP feature extractor computes them.
//!
//! Matches Hugging Face `ClapFeatureExtractor` with `truncation =
//! "rand_trunc"` (what `larger_clap_general` uses): 48 kHz, a centred
//! 1024-point periodic Hann STFT with reflect padding and a 480 hop, power
//! spectrum, 64 Slaney-normalised Slaney-scale mel filters from 50 Hz to
//! 14 kHz, then `10 * log10(max(mel, 1e-10))`. Audio shorter than ten
//! seconds is repeated and zero-padded to ten seconds ("repeatpad").
//! Longer audio is cut to its middle ten seconds; Hugging Face picks a
//! random ten seconds instead, which this deterministic crop replaces.

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

/// Sample rate of the CLAP audio encoder.
pub const CLAP_SAMPLE_RATE: u32 = 48_000;
/// Samples the encoder sees (ten seconds).
pub const CLAP_SAMPLES: usize = 480_000;
/// FFT size.
pub const CLAP_FFT: usize = 1024;
/// Hop size.
pub const CLAP_HOP: usize = 480;
/// Mel bands.
pub const CLAP_MELS: usize = 64;
/// Frames of the encoder input, `CLAP_SAMPLES / CLAP_HOP + 1`.
pub const CLAP_FRAMES: usize = CLAP_SAMPLES / CLAP_HOP + 1;
const MIN_HZ: f64 = 50.0;
const MAX_HZ: f64 = 14_000.0;

/// Repeats short audio and crops long audio to exactly ten seconds.
pub fn fit_to_ten_seconds(signal: &[f32]) -> Vec<f32> {
    if signal.is_empty() {
        return vec![0.0; CLAP_SAMPLES];
    }
    if signal.len() >= CLAP_SAMPLES {
        let start = (signal.len() - CLAP_SAMPLES) / 2;
        return signal[start..start + CLAP_SAMPLES].to_vec();
    }
    let repeats = CLAP_SAMPLES / signal.len();
    let mut out = Vec::with_capacity(CLAP_SAMPLES);
    for _ in 0..repeats {
        out.extend_from_slice(signal);
    }
    out.resize(CLAP_SAMPLES, 0.0);
    out
}

fn hz_to_mel(hz: f64) -> f64 {
    let (min_log_hz, min_log_mel, step) = (1000.0, 15.0, 27.0 / 6.4f64.ln());
    if hz >= min_log_hz {
        min_log_mel + (hz / min_log_hz).ln() * step
    } else {
        3.0 * hz / 200.0
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    let (min_log_hz, min_log_mel, step) = (1000.0, 15.0, 27.0 / 6.4f64.ln());
    if mel >= min_log_mel {
        min_log_hz * ((mel - min_log_mel) / step).exp()
    } else {
        200.0 * mel / 3.0
    }
}

/// The filter bank, `bins * mels` (bin-major), as Hugging Face's
/// `mel_filter_bank(norm="slaney", mel_scale="slaney")` builds it.
pub fn slaney_filter_bank(bins: usize, mels: usize, sample_rate: f64) -> Vec<f32> {
    let (low, high) = (hz_to_mel(MIN_HZ), hz_to_mel(MAX_HZ));
    let edges: Vec<f64> = (0..mels + 2)
        .map(|i| mel_to_hz(low + (high - low) * i as f64 / (mels + 1) as f64))
        .collect();
    let mut bank = vec![0.0f32; bins * mels];
    for bin in 0..bins {
        let hz = sample_rate / 2.0 * bin as f64 / (bins - 1) as f64;
        for mel in 0..mels {
            let down = (hz - edges[mel]) / (edges[mel + 1] - edges[mel]);
            let up = (edges[mel + 2] - hz) / (edges[mel + 2] - edges[mel + 1]);
            let weight = down.min(up).max(0.0) * 2.0 / (edges[mel + 2] - edges[mel]);
            bank[bin * mels + mel] = weight as f32;
        }
    }
    bank
}

/// The `CLAP_FRAMES * CLAP_MELS` log-mel input (frame-major) of ten seconds
/// of 48 kHz mono audio.
pub fn clap_log_mel(ten_seconds: &[f32]) -> Vec<f32> {
    let bins = CLAP_FFT / 2 + 1;
    let bank = slaney_filter_bank(bins, CLAP_MELS, f64::from(CLAP_SAMPLE_RATE));
    let window: Vec<f32> = (0..CLAP_FFT)
        .map(|n| (0.5 - 0.5 * (core::f64::consts::TAU * n as f64 / CLAP_FFT as f64).cos()) as f32)
        .collect();
    // Centred frames with reflect padding of half a window on each side.
    let half = CLAP_FFT / 2;
    let len = ten_seconds.len();
    let at = |index: isize| -> f32 {
        if len == 0 {
            return 0.0;
        }
        let last = len as isize - 1;
        let mut i = index;
        if i < 0 {
            i = -i;
        }
        if i > last {
            i = 2 * last - i;
        }
        ten_seconds[i.clamp(0, last) as usize]
    };
    let frames = 1 + len / CLAP_HOP;
    let fft = FftPlanner::new().plan_fft_forward(CLAP_FFT);
    let mut buffer = vec![Complex32::ZERO; CLAP_FFT];
    let mut power = vec![0.0f32; bins];
    let mut out = Vec::with_capacity(frames * CLAP_MELS);
    for frame in 0..frames {
        let start = (frame * CLAP_HOP) as isize - half as isize;
        for (n, slot) in buffer.iter_mut().enumerate() {
            *slot = Complex32::new(at(start + n as isize) * window[n], 0.0);
        }
        fft.process(&mut buffer);
        for (p, value) in power.iter_mut().zip(&buffer) {
            *p = value.norm_sqr();
        }
        for mel in 0..CLAP_MELS {
            let energy: f32 = (0..bins)
                .map(|bin| power[bin] * bank[bin * CLAP_MELS + mel])
                .sum();
            out.push(10.0 * energy.max(1e-10).log10());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_audio_is_repeated_and_long_audio_cropped() {
        let short = fit_to_ten_seconds(&[1.0; 200_000]);
        assert_eq!(short.len(), CLAP_SAMPLES);
        assert_eq!(short[399_999], 1.0);
        assert_eq!(short[400_000], 0.0);
        let long: Vec<f32> = (0..600_000).map(|i| i as f32).collect();
        let cropped = fit_to_ten_seconds(&long);
        assert_eq!(cropped[0], 60_000.0);
        assert_eq!(fit_to_ten_seconds(&[]).len(), CLAP_SAMPLES);
    }

    #[test]
    fn the_mel_scale_round_trips_and_the_bank_covers_the_range() {
        for hz in [50.0, 700.0, 1000.0, 5000.0, 14_000.0] {
            assert!((mel_to_hz(hz_to_mel(hz)) - hz).abs() < 1e-6);
        }
        let bank = slaney_filter_bank(513, 64, 48_000.0);
        // Every filter has weight, none below 50 Hz or above 14 kHz.
        for mel in 0..64 {
            assert!(
                (0..513).any(|bin| bank[bin * 64 + mel] > 0.0),
                "filter {mel}"
            );
        }
        let bin_hz = 24_000.0 / 512.0;
        for bin in 0..513 {
            let hz = bin as f64 * bin_hz;
            if !(50.0..=14_000.0).contains(&hz) {
                assert!(
                    (0..64).all(|mel| bank[bin * 64 + mel] == 0.0),
                    "bin at {hz} Hz"
                );
            }
        }
    }

    #[test]
    fn a_tone_lights_up_the_band_that_holds_it() {
        // Play a tone on the bin where mel band 20 peaks, well away from
        // the crossing points of neighbouring filters.
        let bank = slaney_filter_bank(513, 64, 48_000.0);
        let band = 20;
        let bin = (0..513)
            .max_by(|&a, &b| bank[a * 64 + band].total_cmp(&bank[b * 64 + band]))
            .expect("bins");
        let hz = bin as f64 * 24_000.0 / 512.0;
        let tone: Vec<f32> = (0..CLAP_SAMPLES)
            .map(|i| (core::f64::consts::TAU * hz * i as f64 / 48_000.0).sin() as f32)
            .collect();
        let mel = clap_log_mel(&tone);
        assert_eq!(mel.len(), CLAP_FRAMES * CLAP_MELS);
        let frame = &mel[500 * CLAP_MELS..501 * CLAP_MELS];
        let loudest = (0..CLAP_MELS)
            .max_by(|&a, &b| frame[a].total_cmp(&frame[b]))
            .expect("bands");
        assert_eq!(loudest, band, "tone at {hz} Hz");
        assert!(clap_log_mel(&vec![0.0; CLAP_SAMPLES])
            .iter()
            .all(|v| (*v + 100.0).abs() < 1e-3));
    }
}
