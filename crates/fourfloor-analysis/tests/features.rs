//! Feature vector tests: known spectra give known features, and similar
//! sounds are closer together than dissimilar ones.

mod common;

use common::{chord, clap, drum_loop, frames, hat, kick, note, place, Loop, RATE};
use fourfloor_analysis::{
    extract_features, extract_features_with_tempo, AnalysisError, FeatureVector, FeatureWeights,
    EMBEDDING_LEN,
};
use fourfloor_core::SampleRate;
use fourfloor_testkit::{sine, white_noise};

fn features(samples: &[f32]) -> FeatureVector {
    extract_features_with_tempo(samples, RATE, None).unwrap()
}

/// A one-shot padded with a little silence, as a sample file would be.
fn one_shot(sound: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; sound.len() + frames(0.05, RATE)];
    place(&mut out, sound, 0.01, RATE);
    out
}

/// A kick like [`kick`] but with its own tuning and length.
fn kick_variant(low_hz: f64, decay: f64) -> Vec<f32> {
    let sr = RATE.as_f64();
    let mut phase = 0.0f64;
    (0..frames(0.3, RATE))
        .map(|n| {
            let t = n as f64 / sr;
            let hz = low_hz + 110.0 * (-t / 0.03).exp();
            phase += std::f64::consts::TAU * hz / sr;
            let tail = ((0.3 - t) / 0.02).clamp(0.0, 1.0);
            (0.9 * phase.sin() * (-t / decay).exp() * tail) as f32
        })
        .collect()
}

#[test]
fn a_sine_has_its_frequency_as_centroid_and_no_flatness() {
    for hz in [220.0, 1000.0, 5000.0] {
        let found = features(&sine(hz, 0.5, 2.0, RATE));
        let hz = hz as f32;
        assert!(
            (found.centroid_hz / hz - 1.0).abs() < 0.03,
            "{hz}: {}",
            found.centroid_hz
        );
        assert!(
            (found.rolloff_hz / hz - 1.0).abs() < 0.08,
            "{hz}: {}",
            found.rolloff_hz
        );
        assert!(found.flatness < 0.01, "{hz}: {}", found.flatness);
        // Two zero crossings per cycle.
        assert!((found.zero_crossing_rate / (2.0 * hz) - 1.0).abs() < 0.01);
        assert!((found.duration_seconds - 2.0).abs() < 1e-9);
        assert_eq!(found.tempo_bpm, None);
    }
}

#[test]
fn white_noise_is_flat_bright_and_steady() {
    let found = features(&white_noise(1, 0.3, 2.0, RATE));
    // The centroid of a flat spectrum is the middle of it; 85% of it lies
    // below 85% of the way up.
    assert!(
        (found.centroid_hz - 12_000.0).abs() < 500.0,
        "{}",
        found.centroid_hz
    );
    assert!(
        (found.rolloff_hz - 20_400.0).abs() < 800.0,
        "{}",
        found.rolloff_hz
    );
    assert!(found.flatness > 0.4, "{}", found.flatness);
    assert!(found.flatness > 40.0 * features(&sine(1000.0, 0.3, 2.0, RATE)).flatness);
    // Steady noise: the timbre hardly moves, unlike a drum loop's.
    let loop_features = features(&drum_loop(&Loop::new(124.0, 8)));
    let movement = |f: &FeatureVector| f.mfcc_std.iter().map(|&s| s * s).sum::<f32>().sqrt();
    assert!(movement(&loop_features) > 3.0 * movement(&found));
}

#[test]
fn chroma_follows_the_notes() {
    // An A, alone.
    let found = features(&note(57.0, 0.4, 2.0, RATE));
    let strongest = (0..12)
        .max_by(|&a, &b| found.chroma[a].total_cmp(&found.chroma[b]))
        .unwrap();
    assert_eq!(strongest, 9, "{:?}", found.chroma);
    let length: f32 = found.chroma.iter().map(|&c| c * c).sum::<f32>().sqrt();
    assert!((length - 1.0).abs() < 1e-4);

    // A C major chord: C, E and G are the three strongest pitch classes.
    let found = features(&chord(&[60.0, 64.0, 67.0], 0.2, 2.0, RATE));
    let mut order: Vec<usize> = (0..12).collect();
    order.sort_by(|&a, &b| found.chroma[b].total_cmp(&found.chroma[a]));
    let mut top = order[..3].to_vec();
    top.sort_unstable();
    assert_eq!(top, [0, 4, 7], "{:?}", found.chroma);
}

#[test]
fn level_does_not_change_the_features() {
    let sound = drum_loop(&Loop::new(124.0, 8));
    let quiet: Vec<f32> = sound.iter().map(|&s| s * 0.1).collect();
    let (a, b) = (features(&sound), features(&quiet));
    assert!(a.distance(&b) < 0.02, "{}", a.distance(&b));
}

#[test]
fn similar_one_shots_are_closer_than_dissimilar_ones() {
    let kick_a = features(&one_shot(&kick(RATE)));
    let kick_b = features(&one_shot(&kick_variant(55.0, 0.11)));
    let kick_c = features(&one_shot(&kick_variant(45.0, 0.07)));
    let hat_a = features(&one_shot(&hat(1, 0.4, 0.04, RATE)));
    let hat_b = features(&one_shot(&hat(2, 0.3, 0.05, RATE)));
    let clap_a = features(&one_shot(&clap(3, RATE)));
    let clap_b = features(&one_shot(&clap(4, RATE)));
    let tone = features(&note(60.0, 0.4, 0.5, RATE));

    let groups: [&[&FeatureVector]; 3] = [
        &[&kick_a, &kick_b, &kick_c],
        &[&hat_a, &hat_b],
        &[&clap_a, &clap_b],
    ];
    let mut within = 0.0f32;
    let mut between = f32::INFINITY;
    for (index, group) in groups.iter().enumerate() {
        for a in group.iter() {
            for b in group.iter() {
                within = within.max(a.distance(b));
            }
            for other in groups.iter().skip(index + 1) {
                for b in other.iter() {
                    between = between.min(a.distance(b));
                }
            }
            between = between.min(a.distance(&tone));
        }
    }
    // Every sound is nearer to all of its own kind than to any other kind,
    // with room to spare.
    assert!(
        within * 1.5 < between,
        "largest distance within a kind {within}, smallest between kinds {between}"
    );

    // Nearest-neighbour search from one kick finds the other kicks first.
    let library = [&kick_b, &hat_a, &clap_a, &kick_c, &tone, &hat_b, &clap_b];
    let mut order: Vec<usize> = (0..library.len()).collect();
    order.sort_by(|&a, &b| {
        kick_a
            .distance(library[a])
            .total_cmp(&kick_a.distance(library[b]))
    });
    let mut nearest = order[..2].to_vec();
    nearest.sort_unstable();
    assert_eq!(nearest, [0, 3]);
}

#[test]
fn distance_is_a_metric_on_the_embedding() {
    let a = features(&one_shot(&kick(RATE)));
    let b = features(&one_shot(&hat(1, 0.4, 0.04, RATE)));
    assert_eq!(a.distance(&a), 0.0);
    assert!(a.distance(&b) > 0.0);
    assert_eq!(a.distance(&b), b.distance(&a));

    let (ea, eb) = (a.embedding(), b.embedding());
    assert_eq!(ea.len(), EMBEDDING_LEN);
    assert!(ea.iter().chain(&eb).all(|v| v.is_finite()));
    let euclidean = ea
        .iter()
        .zip(&eb)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt();
    assert!((euclidean - a.distance(&b)).abs() < 1e-5);
    // Neither has a tempo, so the tempo slot is zero.
    assert_eq!(ea[EMBEDDING_LEN - 1], 0.0);
}

#[test]
fn weights_switch_feature_groups_on_and_off() {
    // The same timbre on two different notes: only the harmony (and the
    // pitch-dependent brightness) differs.
    let low = features(&chord(&[48.0, 55.0], 0.3, 1.0, RATE));
    let high = features(&chord(&[50.0, 57.0], 0.3, 1.0, RATE));
    let harmony_only = FeatureWeights {
        timbre: 0.0,
        brightness: 0.0,
        harmony: 1.0,
        tempo: 0.0,
    };
    let none = FeatureWeights {
        harmony: 0.0,
        ..harmony_only
    };
    assert!(low.weighted_distance(&high, &harmony_only) > 0.5);
    assert_eq!(low.weighted_distance(&high, &none), 0.0);
    assert_eq!(low.weighted_embedding(&none), vec![0.0; EMBEDDING_LEN]);
}

#[test]
fn loops_at_a_nearby_tempo_are_closer_than_at_a_distant_one() {
    let at = |bpm: f64| extract_features(&drum_loop(&Loop::new(bpm, 32)), RATE).unwrap();
    let (a, b, c) = (at(124.0), at(126.0), at(150.0));
    for (found, bpm) in [(&a, 124.0), (&b, 126.0), (&c, 150.0)] {
        let tempo = found.tempo_bpm.expect("a tempo");
        assert!((tempo - bpm).abs() < 0.5, "{tempo} for {bpm}");
    }
    assert!(
        a.distance(&b) < a.distance(&c),
        "{} {}",
        a.distance(&b),
        a.distance(&c)
    );

    // Tempo counts only when both sides have one.
    let mut untimed = c.clone();
    untimed.tempo_bpm = None;
    assert!(a.distance(&untimed) < a.distance(&c));
    let tempo_only = FeatureWeights {
        timbre: 0.0,
        brightness: 0.0,
        harmony: 0.0,
        tempo: 1.0,
    };
    assert_eq!(a.weighted_distance(&untimed, &tempo_only), 0.0);
    // A 6% tempo change is a distance of about a quarter.
    let mut faster = a.clone();
    faster.tempo_bpm = a.tempo_bpm.map(|bpm| bpm * 1.06);
    let step = a.weighted_distance(&faster, &tempo_only);
    assert!((step - 0.25).abs() < 0.02, "{step}");
}

#[test]
fn sounds_without_a_beat_get_no_tempo() {
    assert_eq!(
        extract_features(&sine(440.0, 0.4, 4.0, RATE), RATE)
            .unwrap()
            .tempo_bpm,
        None
    );
    assert_eq!(
        extract_features(&one_shot(&kick(RATE)), RATE)
            .unwrap()
            .tempo_bpm,
        None
    );
}

#[test]
fn other_sample_rates_give_nearly_the_same_features() {
    let at = |rate: SampleRate| {
        extract_features_with_tempo(&drum_loop(&Loop::new(124.0, 8).rate(rate)), rate, None)
            .unwrap()
    };
    let (a, b) = (at(RATE), at(SampleRate::HZ_44100));
    // Far closer to itself at another rate than to a different sound.
    let other = features(&one_shot(&hat(1, 0.4, 0.04, RATE)));
    assert!(
        a.distance(&b) * 4.0 < a.distance(&other),
        "{} {}",
        a.distance(&b),
        a.distance(&other)
    );
}

#[test]
fn silence_and_empty_input_are_handled() {
    assert!(matches!(
        extract_features_with_tempo(&[], RATE, None),
        Err(AnalysisError::Empty)
    ));
    let silent = features(&vec![0.0; 48_000]);
    assert_eq!(silent.chroma, [0.0; 12]);
    assert_eq!(silent.mfcc_mean, [0.0; 12]);
    assert_eq!((silent.centroid_hz, silent.flatness), (0.0, 0.0));
    assert!(silent.embedding().iter().all(|v| v.is_finite()));
}
