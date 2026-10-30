//! A guess at which beat of the grid starts the bar.
//!
//! In house every beat carries the same kick, so loudness says little about
//! where the bar starts. What does change at a bar line is the harmony:
//! bass notes and chords tend to move there. This module averages the
//! spectrum below 1 kHz over each beat, measures how much it differs from
//! the beat before, and picks the position in the bar where that change is
//! largest on average. When no position stands out (a drum loop with no
//! harmony, for instance) it falls back to the first beat that has an
//! onset on it.

use fourfloor_core::SampleRate;

use crate::beatgrid::BeatGrid;
use crate::onset::Onset;
use crate::stft::{fft_size_for, for_each_frame};

/// Window of the harmonic analysis in seconds (8192 samples at 48 kHz).
const WINDOW_SECONDS: f64 = 0.17;
/// Band whose change is measured, in Hz.
const BAND_HZ: (f64, f64) = (40.0, 1000.0);
/// Below this confidence the harmonic guess is not trusted.
const MIN_CONFIDENCE: f32 = 0.15;
/// The largest average change between beats (one minus the cosine
/// similarity of their spectra) must reach this, or there is no harmonic
/// movement to speak of and small differences between positions are noise.
const MIN_CHANGE: f64 = 0.02;
/// An onset this close to a beat is on that beat, in seconds.
const ON_BEAT_SECONDS: f64 = 0.03;

/// The downbeat guess: a position in the bar and how clear it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DownbeatGuess {
    /// Which beat is the first downbeat, in `0..beats_per_bar`, counted
    /// from beat 0 of the grid.
    pub index: u32,
    /// 0 to 1. Zero when the fallback was used.
    pub confidence: f32,
}

/// Guesses the downbeat of `grid` for a mono signal. `onsets` are the
/// onsets of the same signal, used for the fallback.
pub fn guess_downbeat(
    samples: &[f32],
    sample_rate: SampleRate,
    grid: &BeatGrid,
    onsets: &[Onset],
) -> DownbeatGuess {
    let per_bar = grid.beats_per_bar.max(1) as usize;
    let fallback = DownbeatGuess {
        index: first_beat_with_onset(grid, onsets, per_bar),
        confidence: 0.0,
    };
    let period = grid.beat_period_seconds();
    if per_bar < 2 || !(period.is_finite() && period > 0.0) {
        return fallback;
    }
    let rate = sample_rate.as_f64();
    let duration = samples.len() as f64 / rate;
    let beats = ((duration - grid.first_beat_seconds) / period).floor();
    if beats < (2 * per_bar) as f64 {
        return fallback;
    }
    let beats = beats as usize;

    let window = fft_size_for(WINDOW_SECONDS, rate);
    let hop = window / 4;
    let bin_hz = rate / window as f64;
    let low = ((BAND_HZ.0 / bin_hz).ceil() as usize).max(1);
    let high = ((BAND_HZ.1 / bin_hz).floor() as usize).min(window / 2);
    if high <= low {
        return fallback;
    }
    let width = high - low + 1;

    // One averaged magnitude spectrum per beat.
    let mut features = vec![0.0f32; beats * width];
    for_each_frame(samples, window, hop, |frame, power| {
        let time = (frame * hop) as f64 / rate;
        let beat = ((time - grid.first_beat_seconds) / period).floor();
        if beat < 0.0 || beat as usize >= beats {
            return;
        }
        let row = &mut features[beat as usize * width..][..width];
        for (slot, &value) in row.iter_mut().zip(&power[low..=high]) {
            *slot += value.sqrt();
        }
    });

    let norms: Vec<f64> = features
        .chunks_exact(width)
        .map(|row| {
            row.iter()
                .map(|&v| f64::from(v) * f64::from(v))
                .sum::<f64>()
                .sqrt()
        })
        .collect();
    let loudest = norms.iter().copied().fold(0.0, f64::max);
    if loudest <= 0.0 {
        return fallback;
    }

    let mut change = vec![0.0f64; per_bar];
    let mut counts = vec![0usize; per_bar];
    for beat in 1..beats {
        // Beats that are nearly silent say nothing about harmony.
        if norms[beat] < 0.01 * loudest || norms[beat - 1] < 0.01 * loudest {
            continue;
        }
        let here = &features[beat * width..][..width];
        let before = &features[(beat - 1) * width..][..width];
        let dot: f64 = here
            .iter()
            .zip(before)
            .map(|(&a, &b)| f64::from(a) * f64::from(b))
            .sum();
        let similarity = dot / (norms[beat] * norms[beat - 1]);
        change[beat % per_bar] += 1.0 - similarity.clamp(0.0, 1.0);
        counts[beat % per_bar] += 1;
    }
    if counts.contains(&0) {
        return fallback;
    }
    let means: Vec<f64> = change
        .iter()
        .zip(&counts)
        .map(|(&sum, &count)| sum / count as f64)
        .collect();
    let (index, &best) =
        means.iter().enumerate().fold(
            (0, &means[0]),
            |best, item| {
                if item.1 > best.1 {
                    item
                } else {
                    best
                }
            },
        );
    let others = (means.iter().sum::<f64>() - best) / (per_bar - 1) as f64;
    let confidence = if best > 1e-6 {
        ((best - others) / best).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    if confidence < MIN_CONFIDENCE || best < MIN_CHANGE {
        return fallback;
    }
    DownbeatGuess {
        index: index as u32,
        confidence,
    }
}

/// The position in the bar of the first beat that has an onset on it,
/// taking that beat to be a downbeat. Zero when no onset is on a beat.
fn first_beat_with_onset(grid: &BeatGrid, onsets: &[Onset], per_bar: usize) -> u32 {
    onsets
        .iter()
        .map(|onset| grid.nearest_beat(onset.time_seconds))
        .find(|&(_, offset)| offset.abs() <= ON_BEAT_SECONDS)
        .map_or(0, |(beat, _)| beat.rem_euclid(per_bar as i64) as u32)
}
