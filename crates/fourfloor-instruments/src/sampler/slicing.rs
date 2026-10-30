//! Finds where the hits in a loop start, by spectral flux.
//!
//! Spectral flux is how much the magnitude spectrum grew since the last
//! analysis frame, summed over all bins. It jumps when a new sound starts,
//! whatever its pitch, which makes it a good onset detector for drum loops
//! and chopped phrases. Runs off the audio thread: it allocates.

use fourfloor_core::SampleRate;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// Settings for [`detect_slices`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliceOptions {
    /// 0 to 1. Higher finds quieter onsets (and more false ones).
    pub sensitivity: f32,
    /// Closest two slices may be, in milliseconds.
    pub min_gap_ms: f32,
    /// Most slices to return.
    pub max_slices: usize,
}

impl Default for SliceOptions {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_gap_ms: 60.0,
            max_slices: 64,
        }
    }
}

/// Returns the frames where sounds start in `mono`, ascending.
///
/// Each onset is first located to within one analysis hop from the peaks
/// of the spectral flux, then moved to the sample where the level actually
/// starts to rise. Silence gives an empty list.
pub fn detect_slices(mono: &[f32], sample_rate: SampleRate, options: &SliceOptions) -> Vec<usize> {
    let peak = mono.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
    if mono.is_empty() || peak.is_nan() || peak < 1.0e-6 || options.max_slices == 0 {
        return Vec::new();
    }
    let rate = sample_rate.as_f64();
    let frame = ((rate * 0.0213) as usize).next_power_of_two().max(64);
    let hop = frame / 8;

    let flux = spectral_flux(mono, frame, hop, peak);
    let gap_frames = ((f64::from(options.min_gap_ms) * 0.001 * rate) as usize).max(1);
    let candidates = pick_peaks(&flux, hop, rate, options.sensitivity);

    let hold = (rate * 0.005) as usize;
    let mut onsets: Vec<usize> = Vec::new();
    for peak_frame in candidates {
        // The analysis window of frame m ends at sample m * hop.
        let end = (peak_frame * hop).min(mono.len());
        let start = end.saturating_sub(frame / 2 + 2 * hop);
        let onset = refine(mono, start, end, hold);
        match onsets.last() {
            Some(last) if onset < last + gap_frames => {}
            _ => onsets.push(onset),
        }
        if onsets.len() >= options.max_slices {
            break;
        }
    }
    onsets
}

/// Spectral flux per analysis frame. Frame `m` looks at the `frame`
/// samples that end at sample `m * hop`, so the very start of the signal
/// is seen entering the window like any other onset.
fn spectral_flux(mono: &[f32], frame: usize, hop: usize, peak: f32) -> Vec<f32> {
    let fft = FftPlanner::<f32>::new().plan_fft_forward(frame);
    let window: Vec<f32> = (0..frame)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / frame as f32).cos())
        .collect();
    let bins = frame / 2;
    // Log compression so a quiet hat counts next to a loud kick.
    let scale = 1_000.0 / (peak * frame as f32 * 0.25);
    // Stop while the window still ends inside the signal: a window that
    // runs off the end sees the cut as a burst of new spectrum.
    let frames = mono.len() / hop + 1;
    let mut previous = vec![0.0f32; bins];
    let mut buffer = vec![Complex::new(0.0f32, 0.0); frame];
    let mut flux = Vec::with_capacity(frames);
    for m in 0..frames {
        let end = m * hop;
        for (i, (slot, w)) in buffer.iter_mut().zip(&window).enumerate() {
            let sample = (end + i)
                .checked_sub(frame)
                .and_then(|index| mono.get(index))
                .copied()
                .unwrap_or(0.0);
            *slot = Complex::new(sample * w, 0.0);
        }
        fft.process(&mut buffer);
        let mut sum = 0.0;
        for (bin, last) in buffer[..bins].iter().zip(previous.iter_mut()) {
            let magnitude = (1.0 + scale * bin.norm()).ln();
            sum += (magnitude - *last).max(0.0);
            *last = magnitude;
        }
        flux.push(sum);
    }
    let max = flux.iter().fold(0.0f32, |max, x| max.max(*x));
    if max > 0.0 {
        for value in &mut flux {
            *value /= max;
        }
    }
    flux
}

/// Frames whose flux is a local maximum and stands clear of the local
/// average.
fn pick_peaks(flux: &[f32], hop: usize, rate: f64, sensitivity: f32) -> Vec<usize> {
    let sensitivity = sensitivity.clamp(0.0, 1.0);
    let floor = 0.02 + 0.3 * (1.0 - sensitivity) * (1.0 - sensitivity);
    let average_span = ((0.1 * rate) as usize / hop).max(1);
    let neighbourhood = 3usize;
    let mut peaks = Vec::new();
    for (m, value) in flux.iter().enumerate() {
        let lo = m.saturating_sub(average_span);
        let hi = (m + average_span + 1).min(flux.len());
        let mean = flux[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
        if *value <= 1.5 * mean + floor {
            continue;
        }
        let lo = m.saturating_sub(neighbourhood);
        let hi = (m + neighbourhood + 1).min(flux.len());
        let is_max = flux[lo..hi]
            .iter()
            .enumerate()
            .all(|(offset, other)| other < value || (other == value && lo + offset >= m));
        if is_max {
            peaks.push(m);
        }
    }
    peaks
}

/// Finds the sample in `start..end` where the level begins its rise to the
/// loudest point of that range.
fn refine(mono: &[f32], start: usize, end: usize, hold: usize) -> usize {
    if start >= end {
        return start.min(mono.len().saturating_sub(1));
    }
    // Level at i: the largest magnitude over the `hold` samples up to i,
    // so the zero crossings of a low note do not read as silence.
    let level = |i: usize| {
        mono[i.saturating_sub(hold)..=i]
            .iter()
            .fold(0.0f32, |max, x| max.max(x.abs()))
    };
    let mut loudest = start;
    for i in start..end {
        if mono[i].abs() > mono[loudest].abs() {
            loudest = i;
        }
    }
    let top = mono[loudest].abs();
    let base = (start..=loudest).map(level).fold(f32::INFINITY, f32::min);
    let threshold = base + 0.2 * (top - base);
    let mut onset = loudest;
    while onset > start && level(onset - 1) >= threshold {
        onset -= 1;
    }
    onset
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    /// A decaying burst of a sine at `freq`, starting at `at`.
    fn add_hit(signal: &mut [f32], at: usize, freq: f32, level: f32) {
        for (n, slot) in signal[at..].iter_mut().enumerate().take(4_000) {
            let t = n as f32 / 48_000.0;
            *slot += level * (-t * 60.0).exp() * (std::f32::consts::TAU * freq * t).sin();
        }
    }

    #[test]
    fn finds_hits_of_different_pitch_and_level_to_within_a_millisecond_or_two() {
        let mut signal = vec![0.0f32; 48_000];
        let hits = [
            (0usize, 80.0, 0.9),
            (12_000, 3_000.0, 0.3),
            (24_000, 200.0, 0.8),
            (36_500, 6_000.0, 0.2),
        ];
        for (at, freq, level) in hits {
            add_hit(&mut signal, at, freq, level);
        }
        let found = detect_slices(&signal, RATE, &SliceOptions::default());
        assert_eq!(found.len(), hits.len(), "found {found:?}");
        for (found, (at, ..)) in found.iter().zip(hits) {
            assert!(found.abs_diff(at) <= 96, "onset {found} for hit at {at}");
        }
    }

    #[test]
    fn silence_and_steady_tones_have_no_extra_slices() {
        assert!(detect_slices(&[0.0; 10_000], RATE, &SliceOptions::default()).is_empty());
        assert!(detect_slices(&[], RATE, &SliceOptions::default()).is_empty());
        let tone: Vec<f32> = (0..48_000)
            .map(|n| 0.5 * (std::f32::consts::TAU * 440.0 * n as f32 / 48_000.0).sin())
            .collect();
        // Only the start of the tone is an onset.
        let found = detect_slices(&tone, RATE, &SliceOptions::default());
        assert_eq!(found.len(), 1, "found {found:?}");
        assert!(found[0] < 96);
    }

    #[test]
    fn honours_the_minimum_gap_and_the_slice_limit() {
        let mut signal = vec![0.0f32; 48_000];
        for at in [0usize, 1_000, 12_000, 24_000, 36_000] {
            add_hit(&mut signal, at, 1_000.0, 0.8);
        }
        let options = SliceOptions {
            min_gap_ms: 60.0,
            ..SliceOptions::default()
        };
        let found = detect_slices(&signal, RATE, &options);
        assert!(
            found.windows(2).all(|pair| pair[1] - pair[0] >= 2_880),
            "found {found:?}"
        );
        let limited = detect_slices(
            &signal,
            RATE,
            &SliceOptions {
                max_slices: 2,
                ..options
            },
        );
        assert_eq!(limited.len(), 2);
    }
}
