//! Onset detection: where sounds start.

use fourfloor_core::SampleRate;

use crate::level::{from_db, peak};

/// How [`onset_times_with`] decides that a sound started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnsetOptions {
    /// Level a sound must reach to count, in dB relative to the loudest
    /// sample of the signal. Default -12.
    pub threshold_db: f32,
    /// How far below the threshold the level must fall before another
    /// onset can be found, in dB. Default 6.
    pub rearm_db: f32,
    /// Release time of the level follower, in milliseconds. Shorter finds
    /// hits that follow each other closely; longer ignores the ripple of
    /// low notes. Default 5.
    pub release_ms: f32,
    /// Shortest time between two onsets, in milliseconds. Default 30.
    pub min_gap_ms: f32,
}

impl Default for OnsetOptions {
    fn default() -> Self {
        Self {
            threshold_db: -12.0,
            rearm_db: 6.0,
            release_ms: 5.0,
            min_gap_ms: 30.0,
        }
    }
}

/// Sample indices where sounds start, with explicit options.
///
/// The detector is a gate with hysteresis. It follows the signal's level
/// (instant rise, [`OnsetOptions::release_ms`] fall) and reports the first
/// sample that reaches the threshold; it then waits for the level to fall
/// [`OnsetOptions::rearm_db`] below the threshold before it reports again.
///
/// The reported sample is where the threshold is crossed, which is a little
/// after the true start of a sound with a slow attack. Sounds that overlap
/// without the level dropping between them count as one. A silent signal
/// has no onsets.
pub fn onset_samples_with(
    signal: &[f32],
    sample_rate: SampleRate,
    options: &OnsetOptions,
) -> Vec<usize> {
    let loudest = peak(signal);
    if loudest.is_nan() || loudest <= 0.0 {
        return Vec::new();
    }
    let rate = sample_rate.hz() as f32;
    let threshold = loudest * from_db(options.threshold_db.min(0.0));
    let rearm = threshold * from_db(-options.rearm_db.abs());
    let release = if options.release_ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (options.release_ms * 0.001 * rate)).exp()
    };
    let min_gap = (options.min_gap_ms.max(0.0) * 0.001 * rate) as usize;

    let mut onsets = Vec::new();
    let mut level = 0.0f32;
    let mut armed = true;
    for (index, sample) in signal.iter().enumerate() {
        let input = sample.abs();
        level = if input > level {
            input
        } else {
            input + (level - input) * release
        };
        if armed {
            let far_enough = onsets
                .last()
                .is_none_or(|last: &usize| index - last >= min_gap);
            if level >= threshold && far_enough {
                onsets.push(index);
                armed = false;
            }
        } else if level < rearm {
            armed = true;
        }
    }
    onsets
}

/// Times, in seconds, where sounds start, with explicit options. See
/// [`onset_samples_with`].
pub fn onset_times_with(
    signal: &[f32],
    sample_rate: SampleRate,
    options: &OnsetOptions,
) -> Vec<f64> {
    onset_samples_with(signal, sample_rate, options)
        .into_iter()
        .map(|sample| sample as f64 / sample_rate.as_f64())
        .collect()
}

/// Times, in seconds, where sounds start, with the default
/// [`OnsetOptions`]: good for drum hits and other sounds with a clear
/// attack that are at least 30 ms apart.
pub fn onset_times(signal: &[f32], sample_rate: SampleRate) -> Vec<f64> {
    onset_times_with(signal, sample_rate, &OnsetOptions::default())
}

/// Whether `found` and `expected` hold the same number of times and each
/// pair is within `tolerance_seconds`.
pub fn times_match(found: &[f64], expected: &[f64], tolerance_seconds: f64) -> bool {
    found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() <= tolerance_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{click_train, dc, mix, sine};

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn clicks_are_found_where_they_were_put() {
        // Four on the floor at 125 BPM: a hit every 0.48 s.
        let times = [0.0, 0.48, 0.96, 1.44];
        let found = onset_times(&click_train(&times, 0.8, 2.0, RATE), RATE);
        assert!(times_match(&found, &times, 0.000_1), "{found:?}");
    }

    #[test]
    fn quiet_hits_are_ignored_below_the_threshold_and_found_above_it() {
        let loud = click_train(&[0.1, 0.6], 0.8, 1.0, RATE);
        let quiet = click_train(&[0.35], 0.08, 1.0, RATE); // -20 dB
        let signal = mix(&[&loud, &quiet]);
        assert!(times_match(
            &onset_times(&signal, RATE),
            &[0.1, 0.6],
            0.000_1
        ));
        let sensitive = OnsetOptions {
            threshold_db: -30.0,
            ..OnsetOptions::default()
        };
        let found = onset_times_with(&signal, RATE, &sensitive);
        assert!(times_match(&found, &[0.1, 0.35, 0.6], 0.001), "{found:?}");
    }

    #[test]
    fn a_steady_tone_is_one_onset_however_long_it_lasts() {
        let mut signal = dc(0.0, 4_800);
        signal.extend(sine(110.0, 0.5, 1.0, RATE));
        let options = OnsetOptions {
            release_ms: 30.0, // longer than the 9 ms period of 110 Hz
            ..OnsetOptions::default()
        };
        let found = onset_samples_with(&signal, RATE, &options);
        assert_eq!(found.len(), 1, "{found:?}");
        // A sine rises from zero: the crossing is within a quarter cycle.
        assert!((4_800..4_800 + 110).contains(&found[0]), "{found:?}");
    }

    #[test]
    fn hits_closer_than_the_minimum_gap_merge() {
        let signal = click_train(&[0.1, 0.11, 0.5], 0.8, 1.0, RATE);
        let found = onset_times(&signal, RATE);
        assert!(times_match(&found, &[0.1, 0.5], 0.000_1), "{found:?}");
    }

    #[test]
    fn silence_and_broken_signals_have_no_onsets() {
        assert!(onset_times(&dc(0.0, 4_800), RATE).is_empty());
        assert!(onset_times(&[], RATE).is_empty());
        assert!(onset_times(&[0.0, f32::NAN, 1.0], RATE).is_empty());
        assert!(!times_match(&[0.1], &[0.1, 0.2], 0.01));
        assert!(!times_match(&[0.1], &[0.2], 0.01));
    }
}
