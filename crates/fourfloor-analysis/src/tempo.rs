//! Tempo and beat grid estimation for constant-tempo music.
//!
//! The method, in four steps:
//!
//! 1. **Coarse tempo.** The autocorrelation of the onset envelope is
//!    sampled at one, two, three and four beat periods for every candidate
//!    tempo. The best candidate, weighted towards the preferred tempo to
//!    settle half/double ambiguities, is good to a fraction of a BPM.
//! 2. **Period and phase.** Around that tempo, each candidate period folds
//!    the onset times onto one beat. The period that piles the most onset
//!    weight onto one phase wins; that phase is where the beats are.
//!    Low-frequency onsets weigh more, so the grid lands on the kicks
//!    rather than the offbeat hats.
//! 3. **Refinement.** A weighted least-squares line through the onsets that
//!    sit on the grid gives the final period and offset. If the onsets half
//!    a beat away carry clearly more low end (a loud open hat on every
//!    offbeat can still win step 2), the grid moves there and is refined
//!    again.
//! 4. **Downbeat.** See [`crate::downbeat`].
//!
//! Mixxx's beat analyzers (GPL) are the reference for the constant-tempo
//! grid idea; this is an independent implementation, not a port.

use fourfloor_core::SampleRate;

use crate::beatgrid::BeatGrid;
use crate::downbeat::guess_downbeat;
use crate::error::AnalysisError;
use crate::onset::{onset_envelope, onsets_from_envelope, Onset, OnsetOptions};
use crate::stft::interpolate;

/// Extra weight of low-band flux in the curve the tempo tracker follows.
const LOW_WEIGHT: f32 = 2.0;
/// Shortest audio a tempo is estimated from, in seconds.
const MIN_SECONDS: f64 = 1.5;
/// Fewest onsets a tempo is estimated from.
const MIN_ONSETS: usize = 4;
/// How many beat periods of autocorrelation each candidate tempo sums.
const HARMONICS: usize = 4;
/// Step of the coarse tempo search in BPM.
const COARSE_STEP_BPM: f64 = 0.05;
/// Width of the tempo prior in octaves.
const PRIOR_OCTAVES: f64 = 0.7;
/// The fine search covers the coarse period plus and minus this fraction.
const FINE_RANGE: f64 = 0.008;
/// Width of one bin of the folded onset histogram, in seconds.
const BIN_SECONDS: f64 = 0.002;
/// Standard deviation of the smoothing of that histogram, in seconds.
const SMOOTH_SECONDS: f64 = 0.006;
/// The fine search steps the period so that the grid moves by at most this
/// much at the end of the audio between two steps.
const FINE_DRIFT_SECONDS: f64 = 0.002;
/// An onset this close to a grid line counts as being on it.
const ON_GRID_SECONDS: f64 = 0.02;
/// Regularity from which an estimate counts as a real beat.
const MIN_REGULARITY: f32 = 0.5;

/// Settings for [`estimate_tempo`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoOptions {
    /// Slowest tempo considered. Default 70.
    pub min_bpm: f64,
    /// Fastest tempo considered. Default 180.
    pub max_bpm: f64,
    /// Tempo preferred when the audio fits two tempos equally well, such as
    /// 62 and 124. Default 124, the middle of house.
    pub preferred_bpm: f64,
    /// Beats in a bar, for the downbeat guess. Default 4.
    pub beats_per_bar: u32,
}

impl Default for TempoOptions {
    fn default() -> Self {
        Self {
            min_bpm: 70.0,
            max_bpm: 180.0,
            preferred_bpm: 124.0,
            beats_per_bar: 4,
        }
    }
}

impl TempoOptions {
    fn validate(&self) -> Result<(), AnalysisError> {
        let invalid = |name, reason| Err(AnalysisError::InvalidOption { name, reason });
        if !(self.min_bpm.is_finite() && self.min_bpm >= 20.0) {
            return invalid("min_bpm", "must be at least 20");
        }
        if !(self.max_bpm.is_finite() && self.max_bpm <= 400.0 && self.max_bpm > self.min_bpm) {
            return invalid("max_bpm", "must be above min_bpm and at most 400");
        }
        if !(self.preferred_bpm.is_finite() && self.preferred_bpm > 0.0) {
            return invalid("preferred_bpm", "must be positive");
        }
        if !(1..=16).contains(&self.beats_per_bar) {
            return invalid("beats_per_bar", "must be 1 to 16");
        }
        Ok(())
    }
}

/// The result of [`estimate_tempo`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TempoEstimate {
    /// The beat grid: tempo, first beat and downbeat guess.
    pub grid: BeatGrid,
    /// Share of the onset weight that sits within 20 ms of a grid line, 0
    /// to 1. A click track scores 1; music with busy offbeats scores less.
    pub confidence: f32,
    /// How regularly the beats are actually sounded, 0 to 1: the share of
    /// grid beats, between the first and the last onset, that have an onset
    /// within 20 ms, less the share that onsets scattered at random with
    /// the same density would hit. A click track or a drum loop scores
    /// near 1; a sustained chord whose only "onsets" are ripples scores
    /// near 0. See [`TempoEstimate::is_reliable`].
    pub regularity: f32,
    /// How clearly one beat of the bar stood out as the downbeat, 0 to 1.
    /// Zero means the guess fell back to the first beat with an onset.
    pub downbeat_confidence: f32,
    /// Number of onsets found in the audio.
    pub onsets: usize,
}

impl TempoEstimate {
    /// Tempo in beats per minute.
    pub fn bpm(&self) -> f64 {
        self.grid.bpm
    }

    /// Whether the audio really has a beat at this tempo: the
    /// [`regularity`](Self::regularity) is at least 0.5. An estimate is
    /// produced whenever there are enough onsets, so check this before
    /// showing a tempo for material that may have none (pads, one-shots).
    pub fn is_reliable(&self) -> bool {
        self.regularity >= MIN_REGULARITY
    }
}

/// Estimates the tempo and beat grid of a mono signal, assuming the tempo
/// is constant.
pub fn estimate_tempo(
    samples: &[f32],
    sample_rate: SampleRate,
    options: &TempoOptions,
) -> Result<TempoEstimate, AnalysisError> {
    options.validate()?;
    if samples.is_empty() {
        return Err(AnalysisError::Empty);
    }
    let duration = samples.len() as f64 / sample_rate.as_f64();
    if duration < MIN_SECONDS {
        return Err(AnalysisError::TooShort {
            needed_seconds: MIN_SECONDS,
            got_seconds: duration,
        });
    }

    let envelope = onset_envelope(samples, sample_rate);
    let curve = envelope.combined(LOW_WEIGHT);
    let onsets = onsets_from_envelope(samples, &envelope, &OnsetOptions::default());
    if onsets.len() < MIN_ONSETS {
        return Err(AnalysisError::NoBeat);
    }

    let coarse_bpm = coarse_tempo(&curve, envelope.frame_rate(), options)?;
    let weighted: Vec<(f64, f64)> = onsets.iter().map(|o| (o.time_seconds, weight(o))).collect();
    let (period, phase) = fold_search(&weighted, 60.0 / coarse_bpm, duration, options);
    let (period, phase) = regress(&weighted, period, phase, options);
    let (period, phase) = match on_the_low_end(&onsets, period, phase) {
        Some(shifted) => regress(&weighted, period, shifted, options),
        None => (period, phase),
    };

    let total: f64 = weighted.iter().map(|&(_, w)| w).sum();
    let on_grid: f64 = weighted
        .iter()
        .filter(|&&(time, _)| distance_to_grid(time, period, phase).abs() <= ON_GRID_SECONDS)
        .map(|&(_, w)| w)
        .sum();

    let mut first = phase.rem_euclid(period);
    // A beat a hair before time zero is the beat at time zero.
    if period - first < 0.003 {
        first = 0.0;
    }
    let mut grid = BeatGrid {
        bpm: 60.0 / period,
        first_beat_seconds: first,
        beats_per_bar: options.beats_per_bar,
        downbeat_index: 0,
    };
    let downbeat = guess_downbeat(samples, sample_rate, &grid, &onsets);
    grid.downbeat_index = downbeat.index;

    Ok(TempoEstimate {
        grid,
        confidence: if total > 0.0 {
            (on_grid / total) as f32
        } else {
            0.0
        },
        regularity: regularity(&weighted, period, phase),
        downbeat_confidence: downbeat.confidence,
        onsets: onsets.len(),
    })
}

/// Chance-corrected share of grid beats that carry an onset; see
/// [`TempoEstimate::regularity`]. `onsets` are in time order.
fn regularity(onsets: &[(f64, f64)], period: f64, phase: f64) -> f32 {
    let (Some(&(start, _)), Some(&(end, _))) = (onsets.first(), onsets.last()) else {
        return 0.0;
    };
    let first = ((start - ON_GRID_SECONDS - phase) / period).ceil() as i64;
    let last = ((end + ON_GRID_SECONDS - phase) / period).floor() as i64;
    let beats = last - first + 1;
    if beats < MIN_ONSETS as i64 {
        return 0.0;
    }
    let mut sounded = 0i64;
    let mut previous = None;
    for &(time, _) in onsets {
        let beat = ((time - phase) / period).round() as i64;
        let on_grid = distance_to_grid(time, period, phase).abs() <= ON_GRID_SECONDS;
        if on_grid && previous != Some(beat) {
            sounded += 1;
            previous = Some(beat);
        }
    }
    let coverage = sounded as f64 / beats as f64;
    // Onsets scattered at random at this density would land on this share
    // of the beats.
    let density = onsets.len() as f64 / (end - start + period);
    let chance = 1.0 - (-density * 2.0 * ON_GRID_SECONDS).exp();
    if chance >= 1.0 {
        return 0.0;
    }
    ((coverage - chance) / (1.0 - chance)).clamp(0.0, 1.0) as f32
}

/// Bass-band onset strength must be this many times larger half a beat
/// away before the grid moves there.
const LOW_END_SWITCH: f64 = 1.5;

/// The phase half a beat from `phase` when the onsets there carry clearly
/// more low end. The fold weighs every onset, so a loud, regular offbeat
/// (an open hat on every "and") can win it; in four-on-the-floor music
/// the beat is where the kick, and so the low end, is.
fn on_the_low_end(onsets: &[Onset], period: f64, phase: f64) -> Option<f64> {
    let low_end = |phase: f64| -> f64 {
        onsets
            .iter()
            .filter(|o| distance_to_grid(o.time_seconds, period, phase).abs() <= ON_GRID_SECONDS)
            .map(|o| f64::from(o.low_strength))
            .sum()
    };
    let shifted = phase + period / 2.0;
    (low_end(shifted) > LOW_END_SWITCH * low_end(phase)).then_some(shifted)
}

fn weight(onset: &Onset) -> f64 {
    f64::from(onset.strength + LOW_WEIGHT * onset.low_strength)
}

/// Signed distance from `time` to the nearest grid line, in seconds.
fn distance_to_grid(time: f64, period: f64, phase: f64) -> f64 {
    let beats = (time - phase) / period;
    (beats - beats.round()) * period
}

/// Step 1: the tempo whose beat period and its multiples line up best with
/// the autocorrelation of the onset curve.
fn coarse_tempo(
    curve: &[f32],
    frame_rate: f64,
    options: &TempoOptions,
) -> Result<f64, AnalysisError> {
    let longest = (HARMONICS as f64 * 60.0 * frame_rate / options.min_bpm).ceil() as usize + 2;
    let lags = longest.min(curve.len().saturating_sub(1));
    let autocorrelation = autocorrelate(curve, lags);
    if autocorrelation.first().copied().unwrap_or(0.0) <= 0.0 {
        return Err(AnalysisError::NoBeat);
    }

    let steps = ((options.max_bpm - options.min_bpm) / COARSE_STEP_BPM).round() as usize;
    let mut best = (f64::NEG_INFINITY, options.preferred_bpm);
    for step in 0..=steps {
        let bpm = options.min_bpm + step as f64 * COARSE_STEP_BPM;
        let period = 60.0 * frame_rate / bpm;
        let mut sum = 0.0f64;
        let mut used = 0usize;
        for harmonic in 1..=HARMONICS {
            let lag = harmonic as f64 * period;
            if lag + 1.0 < autocorrelation.len() as f64 {
                sum += f64::from(interpolate(&autocorrelation, lag));
                used += 1;
            }
        }
        if used == 0 {
            continue;
        }
        let octaves = (bpm / options.preferred_bpm).log2() / PRIOR_OCTAVES;
        let score = sum / used as f64 * (-0.5 * octaves * octaves).exp();
        if score > best.0 {
            best = (score, bpm);
        }
    }
    if best.0 <= 0.0 {
        return Err(AnalysisError::NoBeat);
    }
    Ok(best.1)
}

/// Unbiased autocorrelation of `values` with the mean removed, for lags
/// `0..=lags`.
fn autocorrelate(values: &[f32], lags: usize) -> Vec<f32> {
    let n = values.len();
    if n == 0 {
        return Vec::new();
    }
    let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let centred: Vec<f32> = values
        .iter()
        .map(|&v| (f64::from(v) - mean) as f32)
        .collect();
    (0..=lags.min(n - 1))
        .map(|lag| {
            let sum: f64 = centred[..n - lag]
                .iter()
                .zip(&centred[lag..])
                .map(|(&a, &b)| f64::from(a) * f64::from(b))
                .sum();
            (sum / (n - lag) as f64) as f32
        })
        .collect()
}

/// Step 2: searches periods near `coarse_period` for the one that folds
/// the onsets most tightly onto a single phase. Returns the period and the
/// time of a beat.
fn fold_search(
    onsets: &[(f64, f64)],
    coarse_period: f64,
    duration: f64,
    options: &TempoOptions,
) -> (f64, f64) {
    let shortest = (60.0 / options.max_bpm).max(coarse_period * (1.0 - FINE_RANGE));
    let longest = (60.0 / options.min_bpm).min(coarse_period * (1.0 + FINE_RANGE));
    let beats = (duration / coarse_period).max(1.0);
    let step = FINE_DRIFT_SECONDS / beats;
    let steps = (((longest - shortest) / step).ceil() as usize).clamp(1, 200_000);

    let reach = (3.0 * SMOOTH_SECONDS / BIN_SECONDS).ceil() as isize;
    let kernel: Vec<f64> = (-reach..=reach)
        .map(|offset| {
            let x = offset as f64 * BIN_SECONDS / SMOOTH_SECONDS;
            (-0.5 * x * x).exp()
        })
        .collect();

    let mut histogram: Vec<f64> = Vec::new();
    let mut best = (f64::NEG_INFINITY, coarse_period, 0.0);
    for index in 0..=steps {
        let period = shortest + (longest - shortest) * index as f64 / steps as f64;
        let bins = ((period / BIN_SECONDS).round() as usize).max(8);
        histogram.clear();
        histogram.resize(bins, 0.0);
        for &(time, weight) in onsets {
            let position = (time / period).fract() * bins as f64;
            histogram[(position as usize).min(bins - 1)] += weight;
        }
        for bin in 0..bins {
            // Skip the smoothing where it cannot beat the best so far.
            if histogram[bin] <= 0.0 {
                continue;
            }
            let mut sum = 0.0f64;
            for (tap, &gain) in kernel.iter().enumerate() {
                let other = (bin as isize + tap as isize - reach).rem_euclid(bins as isize);
                sum += gain * histogram[other as usize];
            }
            if sum > best.0 {
                best = (sum, period, (bin as f64 + 0.5) / bins as f64 * period);
            }
        }
    }
    (best.1, best.2)
}

/// Step 3: weighted least squares of onset time against beat number, over
/// the onsets near the grid, repeated with a tightening tolerance.
fn regress(onsets: &[(f64, f64)], period: f64, phase: f64, options: &TempoOptions) -> (f64, f64) {
    let (mut period, mut phase) = (period, phase);
    for tolerance in [0.035, 0.025, ON_GRID_SECONDS] {
        let tolerance = f64::min(tolerance, period * 0.2);
        let (mut sw, mut sk, mut st, mut skk, mut skt) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let (mut lowest, mut highest) = (f64::INFINITY, f64::NEG_INFINITY);
        for &(time, weight) in onsets {
            let beat = ((time - phase) / period).round();
            if (time - (phase + beat * period)).abs() > tolerance {
                continue;
            }
            sw += weight;
            sk += weight * beat;
            st += weight * time;
            skk += weight * beat * beat;
            skt += weight * beat * time;
            lowest = lowest.min(beat);
            highest = highest.max(beat);
        }
        let determinant = sw * skk - sk * sk;
        // A line needs onsets on at least three different beats.
        if highest - lowest < 2.0 || determinant <= f64::EPSILON * sw * skk {
            break;
        }
        let slope = (sw * skt - sk * st) / determinant;
        let intercept = (st - slope * sk) / sw;
        if !(slope.is_finite() && intercept.is_finite()) {
            break;
        }
        let in_range = (60.0 / options.max_bpm..=60.0 / options.min_bpm).contains(&slope);
        if !in_range {
            break;
        }
        period = slope;
        phase = intercept;
    }
    (period, phase)
}
