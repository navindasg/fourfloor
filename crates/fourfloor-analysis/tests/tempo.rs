//! Tempo, beat grid and downbeat tests on synthesized click tracks and
//! four-on-the-floor loops, and on audio rendered by fourfloor itself.

mod common;

use common::{add, bass_line, circular_difference, drum_loop, frames, Loop, RATE};
use fourfloor_analysis::{estimate_tempo, AnalysisError, TempoEstimate, TempoOptions};
use fourfloor_core::{DeviceRegistry, SampleRate};
use fourfloor_model::command::{AddDevice, SetTempo};
use fourfloor_model::sequencer::{write_house_pattern, HousePattern};
use fourfloor_model::{CommandBus, Project};
use fourfloor_testkit::{click_train, render_project, sine, white_noise};

const BPM_TOLERANCE: f64 = 0.5;
const OFFSET_TOLERANCE: f64 = 0.010;

fn estimate(samples: &[f32], rate: SampleRate) -> TempoEstimate {
    estimate_tempo(samples, rate, &TempoOptions::default()).expect("a tempo")
}

/// Asserts the tempo and that the grid passes through `offset`.
fn assert_grid(found: &TempoEstimate, bpm: f64, offset: f64, what: &str) {
    let grid = found.grid;
    assert!(
        (grid.bpm - bpm).abs() < BPM_TOLERANCE,
        "{what}: found {:.3} BPM, expected {bpm}",
        grid.bpm
    );
    let period = 60.0 / bpm;
    let error = circular_difference(grid.first_beat_seconds, offset, period);
    assert!(
        error < OFFSET_TOLERANCE,
        "{what}: first beat at {:.4} s, expected {offset:.4} s modulo a beat ({:.1} ms out)",
        grid.first_beat_seconds,
        error * 1000.0
    );
    assert!(
        (0.0..grid.beat_period_seconds()).contains(&grid.first_beat_seconds),
        "{what}: first beat {} is outside the first period",
        grid.first_beat_seconds
    );
}

fn clicks(bpm: f64, offset: f64, seconds: f64, rate: SampleRate) -> Vec<f32> {
    let period = 60.0 / bpm;
    let times: Vec<f64> = (0..)
        .map(|beat| offset + f64::from(beat) * period)
        .take_while(|&time| time < seconds - 0.01)
        .collect();
    click_train(&times, 0.9, seconds, rate)
}

#[test]
fn click_tracks_give_their_tempo_and_offset() {
    for (bpm, offset) in [
        (118.0, 0.0),
        (124.0, 0.137),
        (128.0, 0.25),
        (100.0, 0.05),
        (140.0, 0.3),
        (174.0, 0.1),
        (125.5, 0.2),
        (90.0, 0.4),
    ] {
        let found = estimate(&clicks(bpm, offset, 20.0, RATE), RATE);
        assert_grid(&found, bpm, offset, &format!("clicks at {bpm}"));
        // Clicks are exact, so the estimate should be far inside the
        // tolerance and every onset should be on the grid.
        assert!((found.bpm() - bpm).abs() < 0.05, "{bpm}: {}", found.bpm());
        assert!(found.confidence > 0.99, "{bpm}: {}", found.confidence);
    }
}

#[test]
fn four_on_the_floor_loops_give_their_tempo_and_offset() {
    for rate in [SampleRate::HZ_44100, RATE] {
        for (bpm, offset) in [(118.0, 0.0), (124.0, 0.21), (128.0, 0.333)] {
            let spec = Loop::new(bpm, 48).offset(offset).rate(rate);
            let found = estimate(&drum_loop(&spec), rate);
            assert_grid(
                &found,
                bpm,
                offset,
                &format!("loop at {bpm}, {} Hz", rate.hz()),
            );
            assert!((found.bpm() - bpm).abs() < 0.1, "{bpm}: {}", found.bpm());
        }
    }
}

#[test]
fn the_grid_follows_the_kicks_not_the_offbeat_hats() {
    // Offbeat hats are as regular as the kicks. The grid must sit on the
    // kicks, half a beat away from the hats.
    let spec = Loop::new(126.0, 64).offset(0.1);
    let found = estimate(&drum_loop(&spec), RATE);
    assert_grid(&found, 126.0, 0.1, "kicks and hats");
    let on_hats = circular_difference(
        found.grid.first_beat_seconds,
        0.1 + spec.period() / 2.0,
        spec.period(),
    );
    assert!(on_hats > 0.2, "the grid is on the hats");
}

#[test]
fn a_full_mix_with_bass_and_noise_keeps_its_tempo() {
    let spec = Loop::new(122.0, 64).offset(0.05);
    let drums = drum_loop(&spec);
    let bass = bass_line(&spec, &[33.0, 36.0, 31.0, 38.0]);
    let pad = sine(220.0, 0.1, spec.seconds(), RATE);
    let hiss = white_noise(3, 0.02, spec.seconds(), RATE);
    let mix = add(&add(&drums, &bass), &add(&pad, &hiss));
    let found = estimate(&mix, RATE);
    assert_grid(&found, 122.0, 0.05, "full mix");
}

#[test]
fn a_long_track_does_not_drift() {
    // Five minutes: a tempo error of 0.01 BPM would put the last beat
    // 25 ms off. Check the last beat directly.
    let (bpm, offset, seconds) = (127.0, 0.2, 300.0);
    let found = estimate(&clicks(bpm, offset, seconds, RATE), RATE);
    assert_grid(&found, bpm, offset, "five minutes");
    let beats = ((seconds - 1.0 - offset) / (60.0 / bpm)).floor();
    let expected = offset + beats * 60.0 / bpm;
    let (_, error) = found.grid.nearest_beat(expected);
    assert!(
        error.abs() < 0.005,
        "last beat is {:.1} ms off",
        error * 1000.0
    );
}

#[test]
fn half_and_double_tempo_resolve_towards_the_preferred_tempo() {
    let slow = clicks(62.0, 0.0, 30.0, RATE);
    // 62 is below the default range, so the answer is its double.
    assert!((estimate(&slow, RATE).bpm() - 124.0).abs() < BPM_TOLERANCE);
    // With the range and preference moved, the same audio reads as 62.
    let options = TempoOptions {
        min_bpm: 50.0,
        max_bpm: 100.0,
        preferred_bpm: 60.0,
        ..TempoOptions::default()
    };
    let found = estimate_tempo(&slow, RATE, &options).unwrap();
    assert!(
        (found.bpm() - 62.0).abs() < BPM_TOLERANCE,
        "{}",
        found.bpm()
    );
}

#[test]
fn the_downbeat_is_where_the_harmony_changes() {
    // The audio starts part way through a bar, so the first downbeat is
    // not the first beat. The bass changes note at each bar line.
    for starts_on in 0..4 {
        let spec = Loop::new(124.0, 64).offset(0.05).starts_on(starts_on);
        let mix = add(
            &drum_loop(&spec),
            &bass_line(&spec, &[33.0, 38.0, 29.0, 36.0]),
        );
        let found = estimate(&mix, RATE);
        assert_grid(&found, 124.0, 0.05, "downbeat loop");
        let expected = (4 - starts_on) % 4;
        assert_eq!(
            found.grid.downbeat_index as usize, expected,
            "loop starting on beat {starts_on}"
        );
        assert!(
            found.downbeat_confidence > 0.3,
            "{}",
            found.downbeat_confidence
        );
        let first_downbeat = 0.05 + expected as f64 * spec.period();
        assert!((found.grid.first_downbeat_seconds() - first_downbeat).abs() < OFFSET_TOLERANCE);
        let downbeats = found.grid.downbeats(spec.seconds());
        assert_eq!(downbeats.len(), 16);
    }
}

#[test]
fn without_harmony_the_downbeat_falls_back_to_the_first_audible_beat() {
    // Kicks and hats only, from the start: every beat sounds the same, so
    // nothing marks the bar and the first kick is taken as the downbeat.
    let mut spec = Loop::new(120.0, 32).offset(0.05);
    spec.claps = false;
    let found = estimate(&drum_loop(&spec), RATE);
    assert_grid(&found, 120.0, 0.05, "drums only");
    assert_eq!(found.downbeat_confidence, 0.0);
    assert_eq!(found.grid.downbeat_index, 0);
}

#[test]
fn music_that_enters_after_silence_starts_on_a_downbeat() {
    // A bar and a half of silence at the tempo, then drums. 3.0 s at
    // 120 BPM is beat 6 of the grid, position 2 of the bar.
    let spec = Loop::new(120.0, 32).offset(3.0);
    let found = estimate(&drum_loop(&spec), RATE);
    assert_grid(&found, 120.0, 3.0, "late start");
    assert_eq!(found.grid.downbeat_index, 2);
    assert!((found.grid.first_downbeat_seconds() - 1.0).abs() < OFFSET_TOLERANCE);
}

#[test]
fn audio_without_a_beat_is_reported_as_such() {
    let options = TempoOptions::default();
    assert!(matches!(
        estimate_tempo(&[], RATE, &options),
        Err(AnalysisError::Empty)
    ));
    assert!(matches!(
        estimate_tempo(&vec![0.0; frames(1.0, RATE)], RATE, &options),
        Err(AnalysisError::TooShort { .. })
    ));
    assert!(matches!(
        estimate_tempo(&vec![0.0; frames(10.0, RATE)], RATE, &options),
        Err(AnalysisError::NoBeat)
    ));
    // A steady tone has one onset, at its start.
    let tone = sine(440.0, 0.5, 10.0, RATE);
    assert!(matches!(
        estimate_tempo(&tone, RATE, &options),
        Err(AnalysisError::NoBeat)
    ));
}

#[test]
fn invalid_options_are_rejected() {
    let audio = clicks(120.0, 0.0, 5.0, RATE);
    for options in [
        TempoOptions {
            min_bpm: 0.0,
            ..TempoOptions::default()
        },
        TempoOptions {
            max_bpm: 60.0,
            ..TempoOptions::default()
        },
        TempoOptions {
            preferred_bpm: f64::NAN,
            ..TempoOptions::default()
        },
        TempoOptions {
            beats_per_bar: 0,
            ..TempoOptions::default()
        },
    ] {
        assert!(matches!(
            estimate_tempo(&audio, RATE, &options),
            Err(AnalysisError::InvalidOption { .. })
        ));
    }
}

fn registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    registry
}

#[test]
fn a_project_rendered_by_fourfloor_reads_back_at_its_tempo() {
    // The same project `fourfloor new --bpm ...` writes, rendered through
    // the same compiler and engine as `fourfloor render`.
    for bpm in [118.0, 124.0, 128.0] {
        let mut bus = CommandBus::new(Project::new("Analysis"));
        bus.execute(SetTempo { bpm }).unwrap();
        // Every track plays the engine's placeholder synth, which sounds
        // the General MIDI drum pitches as bass notes within a few
        // semitones of each other. Move the hats up to where hats are, so
        // that the kick is the only thing in the low end.
        let pattern = HousePattern {
            bars: 8,
            open_hat_pitch: 94,
            closed_hat_pitch: Some(90),
            ..HousePattern::default()
        };
        write_house_pattern(&mut bus, &pattern).unwrap();
        let rendered = render_project(bus.project(), &registry(), RATE, 0.25).unwrap();
        assert!((rendered.seconds() - (32.0 * 60.0 / bpm + 0.25)).abs() < 0.01);
        let found = estimate(&rendered.mono(), RATE);
        assert_grid(&found, bpm, 0.0, &format!("rendered project at {bpm}"));
    }
}

#[test]
fn a_909_pattern_with_loud_open_hats_on_every_offbeat_is_gridded_on_the_kicks() {
    // The open hat is the loudest, brightest hit of each beat, so the fold
    // alone puts the grid on it; the low end has to move it to the kick.
    let mut registry = registry();
    fourfloor_instruments::register(&mut registry).unwrap();
    for bpm in [124.0, 128.0] {
        let mut bus = CommandBus::new(Project::new("909"));
        bus.execute(SetTempo { bpm }).unwrap();
        let pattern = HousePattern {
            bars: 8,
            ..HousePattern::default()
        };
        let written = write_house_pattern(&mut bus, &pattern).unwrap();
        for (track, kind) in [
            (written.kick_track, "drum.kick909"),
            (written.hat_track, "drum.hat909"),
        ] {
            bus.execute(AddDevice {
                track,
                kind: kind.to_owned(),
                name: None,
                index: None,
                params: Vec::new(),
            })
            .unwrap();
        }
        let rendered = render_project(bus.project(), &registry, RATE, 0.25).unwrap();
        let found = estimate(&rendered.mono(), RATE);
        assert_grid(&found, bpm, 0.0, &format!("909 pattern at {bpm}"));
    }
}
