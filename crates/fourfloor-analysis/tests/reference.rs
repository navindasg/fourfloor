//! Reference A/B tests: mixes that differ from a reference in a known way.

mod common;

use common::{add, dual_mono, gain, high_pass, low_pass, stereo, RATE};
use fourfloor_analysis::{
    compare_to_reference, compare_to_reference_with, AnalysisError, ReferenceOptions,
    SuggestionKind,
};
use fourfloor_audio::resample;
use fourfloor_core::SampleRate;
use fourfloor_testkit::white_noise;

const SECONDS: f64 = 6.0;

/// White noise: the same level in every hertz.
fn flat(seed: u32) -> Vec<f32> {
    white_noise(seed, 0.2, SECONDS, RATE)
}

/// The flat signal with a second, louder layer of noise below about
/// 120 Hz. The layer is cut off steeply (24 dB per octave) so that it adds
/// nothing to the mids.
fn bass_heavy(seed: u32) -> Vec<f32> {
    let mut bass = white_noise(seed + 100, 0.2, SECONDS, RATE);
    for _ in 0..4 {
        bass = low_pass(&bass, 120.0, RATE);
    }
    add(&flat(seed), &gain(&bass, 6.0))
}

#[test]
fn a_bass_heavy_mix_is_told_to_cut_the_low_end() {
    // The reference is the mix without its extra bass.
    let report =
        compare_to_reference(&dual_mono(&bass_heavy(1), RATE), &dual_mono(&flat(1), RATE)).unwrap();
    assert!(report.low.delta_db > 6.0, "low {}", report.low.delta_db);
    // The bass makes the mix louder, so at matched loudness the rest is
    // lower than in the reference. Not by much: loudness weighting counts
    // little below 100 Hz.
    assert!(
        report.loudness_delta_lu > 0.0,
        "{}",
        report.loudness_delta_lu
    );
    for band in [report.mid, report.high] {
        assert!(
            band.delta_db < 0.0 && band.delta_db > -1.0,
            "{}",
            band.delta_db
        );
        assert!((band.delta_db + report.loudness_delta_lu).abs() < 0.05);
    }
    assert!(report.suggestion(SuggestionKind::MidBand).is_none());
    assert!(report.suggestion(SuggestionKind::HighBand).is_none());

    let low = report
        .suggestion(SuggestionKind::LowBand)
        .expect("a low band suggestion");
    assert!((low.change + report.low.delta_db).abs() < 1e-9);
    assert!(low.change < -6.0);
    assert!(
        low.message.contains("more") && low.message.contains("Cut"),
        "{}",
        low.message
    );

    // The octave bands say where: far more at 63 Hz than at 1 kHz.
    assert_eq!(report.octaves.len(), 10);
    let octave = |hz: f64| {
        report
            .octaves
            .iter()
            .find(|band| band.low_hz < hz && hz < band.high_hz)
            .unwrap()
            .delta_db
    };
    assert!(
        octave(63.0) > octave(1000.0) + 8.0,
        "{} {}",
        octave(63.0),
        octave(1000.0)
    );
    // The correction curve is the mirror image.
    let eq = report.eq_correction();
    assert_eq!(eq.len(), 10);
    assert!((eq[1].0 - 63.0).abs() < 0.5);
    assert!((eq[1].1 + octave(63.0)).abs() < 1e-9);
    assert!(eq[1].1 < -6.0);
}

#[test]
fn the_direction_reverses_when_the_roles_are_swapped() {
    let report =
        compare_to_reference(&dual_mono(&flat(2), RATE), &dual_mono(&bass_heavy(1), RATE)).unwrap();
    assert!(report.low.delta_db < -6.0, "low {}", report.low.delta_db);
    let low = report.suggestion(SuggestionKind::LowBand).unwrap();
    assert!(low.change > 6.0);
    assert!(low.message.contains("less") && low.message.contains("Boost"));
    assert!(report.high.delta_db > 0.0);
}

#[test]
fn a_dull_mix_is_told_to_boost_the_highs() {
    // 12 dB per octave above 3 kHz.
    let dull = low_pass(&low_pass(&flat(3), 3000.0, RATE), 3000.0, RATE);
    let report = compare_to_reference(&dual_mono(&dull, RATE), &dual_mono(&flat(4), RATE)).unwrap();
    assert!(report.high.delta_db < -3.0, "{}", report.high.delta_db);
    assert!(report.suggestion(SuggestionKind::HighBand).unwrap().change > 3.0);
    // White noise is mostly highs, so matching its loudness brings the
    // rest of the dull mix up.
    assert!(report.low.delta_db > 3.0, "{}", report.low.delta_db);

    let bright = high_pass(&flat(3), 3000.0, RATE);
    let report =
        compare_to_reference(&dual_mono(&bright, RATE), &dual_mono(&flat(4), RATE)).unwrap();
    // Only a little more highs than white noise, which is nearly all
    // highs already, but far less of everything below.
    assert!(report.high.delta_db > 0.1, "{}", report.high.delta_db);
    assert!(report.low.delta_db < -6.0, "{}", report.low.delta_db);
    assert!(report.suggestion(SuggestionKind::LowBand).unwrap().change > 6.0);
    assert!(report.suggestion(SuggestionKind::HighBand).is_none());
}

#[test]
fn the_comparison_is_loudness_matched() {
    let reference = dual_mono(&flat(2), RATE);
    let mix = bass_heavy(1);
    let loud = compare_to_reference(&dual_mono(&mix, RATE), &reference).unwrap();
    let quiet = compare_to_reference(&dual_mono(&gain(&mix, 0.25), RATE), &reference).unwrap();

    // 12 dB quieter, and told so.
    assert!((quiet.loudness_delta_lu - (loud.loudness_delta_lu - 12.04)).abs() < 0.05);
    assert!((quiet.gain_to_match_db + quiet.loudness_delta_lu).abs() < 1e-12);
    let loudness = quiet.suggestion(SuggestionKind::Loudness).unwrap();
    assert!((loudness.change - quiet.gain_to_match_db).abs() < 1e-12);
    assert!(loudness.change > 0.0 && loudness.message.contains("quieter"));

    // The tonal balance does not depend on the level of the mix.
    for (a, b) in [
        (loud.low, quiet.low),
        (loud.mid, quiet.mid),
        (loud.high, quiet.high),
    ] {
        assert!(
            (a.delta_db - b.delta_db).abs() < 0.05,
            "{} {}",
            a.delta_db,
            b.delta_db
        );
    }
    assert!(
        (quiet.true_peak_after_match_dbtp - (quiet.mix.true_peak_dbtp + quiet.gain_to_match_db))
            .abs()
            < 1e-12
    );
}

#[test]
fn a_mix_like_its_reference_gets_no_suggestions() {
    let signal = flat(5);
    let same = compare_to_reference(&dual_mono(&signal, RATE), &dual_mono(&signal, RATE)).unwrap();
    assert!(same.suggestions.is_empty(), "{:?}", same.suggestions);
    assert_eq!(same.loudness_delta_lu, 0.0);
    assert_eq!(
        (same.low.delta_db, same.mid.delta_db, same.high.delta_db),
        (0.0, 0.0, 0.0)
    );
    assert_eq!((same.width_delta, same.crest_delta_db), (0.0, 0.0));

    // A different take of the same kind of sound is within the thresholds.
    let other =
        compare_to_reference(&dual_mono(&flat(6), RATE), &dual_mono(&signal, RATE)).unwrap();
    assert!(other.suggestions.is_empty(), "{:?}", other.suggestions);
    for band in [other.low, other.mid, other.high] {
        assert!(band.delta_db.abs() < 0.5, "{}", band.delta_db);
    }
}

#[test]
fn stereo_width_is_compared() {
    let (a, b) = (flat(7), flat(8));
    let wide = stereo(a.clone(), b, RATE);
    let mono = dual_mono(&a, RATE);

    let report = compare_to_reference(&wide, &mono).unwrap();
    assert!(
        (report.width_delta - 0.5).abs() < 0.05,
        "{}",
        report.width_delta
    );
    assert!(report.correlation_delta < -0.9);
    let width = report.suggestion(SuggestionKind::StereoWidth).unwrap();
    assert!(
        width.change < 0.0 && width.message.contains("Narrow"),
        "{}",
        width.message
    );

    let report = compare_to_reference(&mono, &wide).unwrap();
    let width = report.suggestion(SuggestionKind::StereoWidth).unwrap();
    assert!(width.change > 0.0 && width.message.contains("Widen"));
}

#[test]
fn crest_factor_and_true_peak_are_compared() {
    // Sparse spikes over quiet noise: a very high crest factor.
    let mut open = white_noise(9, 0.02, SECONDS, RATE);
    for sample in open.iter_mut().step_by(12_000) {
        *sample = 0.9;
    }
    // The same with the spikes clipped off.
    let squashed: Vec<f32> = open.iter().map(|&s| s.clamp(-0.02, 0.02)).collect();

    let report =
        compare_to_reference(&dual_mono(&squashed, RATE), &dual_mono(&open, RATE)).unwrap();
    assert!(report.crest_delta_db < -10.0, "{}", report.crest_delta_db);
    let dynamics = report.suggestion(SuggestionKind::Dynamics).unwrap();
    assert!(dynamics.change > 10.0 && dynamics.message.contains("less crest"));
    assert!(report.suggestion(SuggestionKind::TruePeak).is_none());

    // The other way round, matching the loudness of a dense reference
    // would push the spikes over the ceiling.
    let dense = gain(&flat(10), 2.0);
    let report = compare_to_reference(&dual_mono(&open, RATE), &dual_mono(&dense, RATE)).unwrap();
    assert!(report.crest_delta_db > 10.0);
    assert!(report.suggestion(SuggestionKind::Dynamics).unwrap().change < -10.0);
    assert!(report.true_peak_after_match_dbtp > -1.0);
    let peak = report.suggestion(SuggestionKind::TruePeak).unwrap();
    assert!((peak.change + (report.true_peak_after_match_dbtp + 1.0)).abs() < 1e-9);
}

#[test]
fn thresholds_decide_what_is_worth_a_suggestion() {
    let mix = dual_mono(&gain(&flat(5), 0.8), RATE);
    let reference = dual_mono(&flat(5), RATE);
    // 1.9 dB quieter: over the default 1 LU, under a threshold of 3.
    assert!(compare_to_reference(&mix, &reference)
        .unwrap()
        .suggestion(SuggestionKind::Loudness)
        .is_some());
    let relaxed = ReferenceOptions {
        loudness_lu: 3.0,
        ..ReferenceOptions::default()
    };
    assert!(compare_to_reference_with(&mix, &reference, &relaxed)
        .unwrap()
        .suggestions
        .is_empty());
}

#[test]
fn length_channel_count_and_sample_rate_may_differ() {
    let mix = bass_heavy(1);
    let reference = flat(2);
    let same_rate =
        compare_to_reference(&dual_mono(&mix, RATE), &dual_mono(&reference, RATE)).unwrap();

    let rate = SampleRate::HZ_44100;
    let resampled = resample(&reference[..reference.len() / 2], RATE, rate);
    let other = fourfloor_audio::AudioBuffer::new(rate, vec![resampled]);
    let mixed = compare_to_reference(&dual_mono(&mix, RATE), &other).unwrap();
    assert!((mixed.low.delta_db - same_rate.low.delta_db).abs() < 1.0);
    assert!((mixed.mid.delta_db - same_rate.mid.delta_db).abs() < 1.0);
    // The 44.1 kHz reference has lost the top of its white noise to the
    // resampler, which makes it a little quieter and so the mix a little
    // louder by comparison.
    let shift = mixed.loudness_delta_lu - same_rate.loudness_delta_lu;
    assert!((0.0..1.0).contains(&shift), "{shift}");
}

#[test]
fn silent_or_empty_audio_cannot_be_compared() {
    let good = dual_mono(&flat(1), RATE);
    let silent = dual_mono(&vec![0.0; 96_000], RATE);
    let short = dual_mono(&flat(1)[..4800], RATE);
    let empty = fourfloor_audio::AudioBuffer::new(RATE, vec![Vec::new()]);
    for bad in [&silent, &short] {
        assert!(matches!(
            compare_to_reference(bad, &good),
            Err(AnalysisError::TooShort { .. })
        ));
        assert!(matches!(
            compare_to_reference(&good, bad),
            Err(AnalysisError::TooShort { .. })
        ));
    }
    assert!(matches!(
        compare_to_reference(&empty, &good),
        Err(AnalysisError::Empty)
    ));
}
