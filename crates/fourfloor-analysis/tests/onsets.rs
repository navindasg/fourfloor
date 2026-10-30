//! Onset detection tests: known sounds at known times.

mod common;

use common::{clap, frames, hat, kick, note, place, RATE};
use fourfloor_analysis::{detect_onsets, onset_envelope, OnsetOptions};
use fourfloor_core::SampleRate;
use fourfloor_testkit::{click_train, sine, times_match, white_noise};

fn times(samples: &[f32], rate: SampleRate) -> Vec<f64> {
    detect_onsets(samples, rate, &OnsetOptions::default())
        .iter()
        .map(|onset| onset.time_seconds)
        .collect()
}

#[test]
fn clicks_are_found_to_the_sample() {
    // The last two are 100 ms apart: twice the default minimum gap.
    let expected = [0.1, 0.35, 0.5, 0.9, 1.2, 1.3];
    for rate in [SampleRate::HZ_44100, RATE, SampleRate::HZ_96000] {
        let found = times(&click_train(&expected, 0.8, 1.5, rate), rate);
        assert!(
            times_match(&found, &expected, 1.5 / rate.as_f64()),
            "{} Hz: {found:?}",
            rate.hz()
        );
    }
}

#[test]
fn drum_hits_are_found_within_a_few_milliseconds() {
    let mut audio = vec![0.0f32; frames(3.0, RATE)];
    let hits = [0.2, 0.7, 0.95, 1.4, 1.9, 2.15, 2.6];
    place(&mut audio, &kick(RATE), hits[0], RATE);
    place(&mut audio, &hat(1, 0.4, 0.04, RATE), hits[1], RATE);
    place(&mut audio, &clap(2, RATE), hits[2], RATE);
    place(&mut audio, &kick(RATE), hits[3], RATE);
    // A4: with 47 Hz bins a note much nearer 200 Hz leaks into the low band.
    place(&mut audio, &note(69.0, 0.5, 0.2, RATE), hits[4], RATE);
    place(&mut audio, &hat(3, 0.4, 0.04, RATE), hits[5], RATE);
    place(&mut audio, &kick(RATE), hits[6], RATE);

    let onsets = detect_onsets(&audio, RATE, &OnsetOptions::default());
    let found: Vec<f64> = onsets.iter().map(|onset| onset.time_seconds).collect();
    // The note fades in over 10 ms, so its onset is placed part way up
    // the fade; the drums are placed within a millisecond or two.
    assert!(times_match(&found, &hits, 0.012), "{found:?}");
    for (index, (&time, &hit)) in found.iter().zip(&hits).enumerate() {
        if index != 4 {
            assert!((time - hit).abs() < 0.003, "hit {index} at {time}");
        }
    }

    // Kicks are low, hats are not.
    for (onset, low) in onsets
        .iter()
        .zip([true, false, false, true, false, false, true])
    {
        assert_eq!(onset.low_strength > 0.5, low, "{onset:?}");
        assert!((0.0..=1.0).contains(&onset.strength));
        assert_eq!(
            onset.sample,
            (onset.time_seconds * 48_000.0).round() as usize
        );
    }
}

#[test]
fn onsets_are_found_over_a_steady_background() {
    let mut audio = sine(330.0, 0.2, 2.0, RATE);
    let hits = [0.5, 1.0, 1.5];
    for &time in &hits {
        place(&mut audio, &hat(9, 0.5, 0.04, RATE), time, RATE);
    }
    let found = times(&audio, RATE);
    // The tone starts with a step at zero and is cut off at the end, and
    // both of those are clicks. In between, only the hats.
    let later: Vec<f64> = found
        .into_iter()
        .filter(|&time| time > 0.1 && time < 1.9)
        .collect();
    assert!(times_match(&later, &hits, 0.008), "{later:?}");
}

#[test]
fn steady_sounds_and_silence_have_no_onsets_after_they_start() {
    assert!(times(&[], RATE).is_empty());
    assert!(times(&vec![0.0; 48_000], RATE).is_empty());
    let noise = white_noise(5, 0.3, 2.0, RATE);
    let found = times(&noise, RATE);
    assert!(found.iter().all(|&time| time < 0.02), "{found:?}");
}

#[test]
fn sounds_closer_than_the_minimum_gap_merge() {
    let audio = click_train(&[0.5, 0.51, 1.0], 0.8, 1.5, RATE);
    assert_eq!(times(&audio, RATE).len(), 2);
    let options = OnsetOptions {
        min_gap_seconds: 0.002,
        ..OnsetOptions::default()
    };
    // The two are still one flux peak, so this option alone cannot split
    // them; it only stops merging of separate peaks.
    assert!(detect_onsets(&audio, RATE, &options).len() >= 2);
}

#[test]
fn the_envelope_has_one_frame_per_hop() {
    let audio = click_train(&[0.5], 0.8, 1.0, RATE);
    let envelope = onset_envelope(&audio, RATE);
    assert_eq!((envelope.window, envelope.hop), (1024, 256));
    assert_eq!(envelope.len(), 48_000 / 256 + 1);
    assert_eq!(envelope.low_flux.len(), envelope.len());
    assert!((envelope.frame_rate() - 187.5).abs() < 1e-9);
    let loudest = envelope
        .flux
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(frame, _)| envelope.frame_time(frame as f64))
        .unwrap();
    assert!((loudest - 0.5).abs() < 0.015, "{loudest}");
    assert!(!envelope.is_empty());
    assert_eq!(envelope.combined(2.0).len(), envelope.len());
    assert!(onset_envelope(&[], RATE).is_empty());
}
