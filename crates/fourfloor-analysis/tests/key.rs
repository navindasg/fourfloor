//! Key detection tests: chord progressions and scales synthesized in known
//! keys.

mod common;

use common::{add, chord, drum_loop, frames, note, place, Loop, RATE};
use fourfloor_analysis::{
    chromagram, detect_key, key_from_chroma, AnalysisError, Key, KeyOptions, Mode,
};
use fourfloor_core::SampleRate;
use fourfloor_testkit::white_noise;

/// Scale degrees of the triads of a I-IV-V-I (or i-iv-v-i) cadence
/// followed by a turn through the relative chords, in semitones from the
/// tonic.
fn progression(mode: Mode) -> Vec<[f64; 3]> {
    match mode {
        Mode::Major => vec![
            [0.0, 4.0, 7.0],
            [5.0, 9.0, 12.0],
            [7.0, 11.0, 14.0],
            [0.0, 4.0, 7.0],
            [9.0, 12.0, 16.0],
            [5.0, 9.0, 12.0],
            [7.0, 11.0, 14.0],
            [0.0, 4.0, 7.0],
        ],
        Mode::Minor => vec![
            [0.0, 3.0, 7.0],
            [5.0, 8.0, 12.0],
            [7.0, 10.0, 14.0],
            [0.0, 3.0, 7.0],
            [8.0, 12.0, 15.0],
            [5.0, 8.0, 12.0],
            [7.0, 10.0, 14.0],
            [0.0, 3.0, 7.0],
        ],
    }
}

/// The progression in `key`: chords around middle C with the chord root
/// doubled in the bass.
fn song(key: Key, rate: SampleRate) -> Vec<f32> {
    let chords = progression(key.mode());
    let length = 0.75;
    let tonic = 48.0 + f64::from(key.tonic());
    let mut out = vec![0.0f32; frames(length * chords.len() as f64, rate)];
    for (index, degrees) in chords.iter().enumerate() {
        let pitches: Vec<f64> = degrees.iter().map(|d| tonic + 12.0 + d).collect();
        let time = index as f64 * length;
        place(&mut out, &chord(&pitches, 0.15, length, rate), time, rate);
        place(
            &mut out,
            &note(tonic - 12.0 + degrees[0], 0.3, length, rate),
            time,
            rate,
        );
    }
    out
}

fn detect(samples: &[f32], rate: SampleRate) -> Key {
    detect_key(samples, rate, &KeyOptions::default())
        .unwrap()
        .key
}

#[test]
fn chord_progressions_are_detected_in_all_24_keys() {
    for key in Key::all() {
        let found = detect_key(&song(key, RATE), RATE, &KeyOptions::default()).unwrap();
        assert_eq!(found.key, key, "expected {key}, found {}", found.key);
        assert_eq!(found.scores.len(), 24);
        assert_eq!(found.scores[0].key, key);
        assert!(found
            .scores
            .windows(2)
            .all(|pair| pair[0].score >= pair[1].score));
        assert!(found.confidence > 0.0);
        // The tonic is among the strongest pitch classes.
        assert!(
            found.chroma[usize::from(key.tonic())] > 0.6,
            "{key}: {:?}",
            found.chroma
        );
    }
}

#[test]
fn the_key_survives_drums_and_another_sample_rate() {
    for (key, rate) in [
        (Key::minor(9), SampleRate::HZ_44100),
        (Key::minor(5), RATE),
        (Key::major(7), SampleRate::HZ_96000),
    ] {
        let music = song(key, rate);
        let seconds = music.len() as f64 / rate.as_f64();
        let beats = (seconds / (60.0 / 124.0)).floor() as usize;
        let drums = drum_loop(&Loop::new(124.0, beats).rate(rate));
        assert_eq!(
            detect(&add(&music, &drums), rate),
            key,
            "{key} at {} Hz",
            rate.hz()
        );
    }
}

#[test]
fn a_scale_that_rests_on_its_tonic_is_detected() {
    // Up and down the scale, with the tonic held at both ends.
    for (key, steps) in [
        (Key::major(2), [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0]),
        (Key::minor(4), [0.0, 2.0, 3.0, 5.0, 7.0, 8.0, 10.0, 12.0]),
    ] {
        let tonic = 60.0 + f64::from(key.tonic());
        let mut out = vec![0.0f32; frames(8.0, RATE)];
        let mut time = 0.0;
        let up_and_down = steps.iter().chain(steps.iter().rev().skip(1));
        for (index, step) in up_and_down.enumerate() {
            let length = if index == 0 || index == 14 { 1.2 } else { 0.35 };
            place(&mut out, &note(tonic + step, 0.4, length, RATE), time, RATE);
            time += length;
        }
        place(&mut out, &note(tonic - 12.0, 0.2, time, RATE), 0.0, RATE);
        assert_eq!(detect(&out, RATE), key, "{key}");
    }
}

#[test]
fn transposing_the_audio_transposes_the_key() {
    let original = Key::minor(0);
    for semitones in [-5, -1, 2, 7] {
        let moved = original.transposed(semitones);
        assert_eq!(detect(&song(moved, RATE), RATE), moved);
    }
}

#[test]
fn a_different_tuning_reference_shifts_the_reading() {
    // A song in A minor played a semitone sharp is B flat minor at 440,
    // and A minor again when told the band tunes to 466.16.
    let sharp = song(Key::minor(10), RATE);
    assert_eq!(detect(&sharp, RATE), Key::minor(10));
    let options = KeyOptions {
        tuning_hz: 466.16,
        ..KeyOptions::default()
    };
    assert_eq!(
        detect_key(&sharp, RATE, &options).unwrap().key,
        Key::minor(9)
    );
}

#[test]
fn chroma_of_a_single_note_peaks_on_that_note() {
    for pitch_class in 0..12usize {
        let audio = note(60.0 + pitch_class as f64, 0.5, 2.0, RATE);
        let chroma = chromagram(&audio, RATE, &KeyOptions::default()).unwrap();
        assert!((chroma.frame_rate - 48_000.0 / 8192.0).abs() < 1e-9);
        let total = chroma.total();
        let loudest = (0..12)
            .max_by(|&a, &b| total[a].total_cmp(&total[b]))
            .unwrap();
        assert_eq!(loudest, pitch_class);
        assert_eq!(total[pitch_class], 1.0);
    }
}

#[test]
fn a_clear_key_is_more_confident_than_noise() {
    let clear = detect_key(&song(Key::major(0), RATE), RATE, &KeyOptions::default()).unwrap();
    let noise = white_noise(11, 0.3, 6.0, RATE);
    let vague = detect_key(&noise, RATE, &KeyOptions::default()).unwrap();
    assert!(clear.scores[0].score > 0.75, "{}", clear.scores[0].score);
    assert!(vague.scores[0].score < 0.6, "{}", vague.scores[0].score);
    assert!(clear.scores[0].score > vague.scores[0].score + 0.2);
}

#[test]
fn audio_without_pitch_and_bad_options_are_errors() {
    let options = KeyOptions::default();
    assert!(matches!(
        detect_key(&[], RATE, &options),
        Err(AnalysisError::Empty)
    ));
    assert!(matches!(
        detect_key(&vec![0.0; 48_000], RATE, &options),
        Err(AnalysisError::NoTonalContent)
    ));
    assert!(matches!(
        key_from_chroma(&[0.5; 12]),
        Err(AnalysisError::NoTonalContent)
    ));
    assert!(matches!(
        key_from_chroma(&[f32::NAN; 12]),
        Err(AnalysisError::NoTonalContent)
    ));
    for bad in [
        KeyOptions {
            tuning_hz: 0.0,
            ..options
        },
        KeyOptions {
            min_hz: 1.0,
            ..options
        },
        KeyOptions {
            max_hz: 10.0,
            ..options
        },
    ] {
        assert!(matches!(
            detect_key(&[0.1; 4800], RATE, &bad),
            Err(AnalysisError::InvalidOption { .. })
        ));
    }
}

#[test]
fn the_profile_of_a_key_is_recognized_from_its_chroma() {
    // The C major scale with the tonic triad weighted up.
    let chroma = [1.0, 0.0, 0.4, 0.0, 0.7, 0.4, 0.0, 0.8, 0.0, 0.4, 0.0, 0.3];
    let found = key_from_chroma(&chroma).unwrap();
    assert_eq!(found.key, Key::major(0));
    assert_eq!(found.key.camelot(), "8B");
    // The runner-up is a close relative, not something across the wheel.
    assert!(found.key.wheel_distance(found.scores[1].key) <= 1);
}

#[test]
fn a_project_rendered_by_fourfloor_reads_back_in_its_key() {
    use fourfloor_core::{DeviceRegistry, Ticks};
    use fourfloor_model::command::{CreateTrack, WriteMidiClip};
    use fourfloor_model::{CommandBus, MidiNote, Project, TrackKind};

    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();

    // The same cadences, written as a MIDI clip and played by the engine's
    // built-in synth through the project compiler.
    for key in [Key::minor(9), Key::major(5), Key::minor(6)] {
        let beat = Ticks::BEAT.get();
        let chords = progression(key.mode());
        let tonic = 48 + i64::from(key.tonic());
        let mut notes = Vec::new();
        for (index, degrees) in chords.iter().enumerate() {
            let start = Ticks::new(index as i64 * 2 * beat);
            let length = Ticks::new(2 * beat - 60);
            let root = tonic - 12 + degrees[0] as i64;
            let upper = degrees.iter().map(|&degree| tonic + 12 + degree as i64);
            for pitch in upper.chain([root]) {
                notes.push(MidiNote {
                    start,
                    length,
                    pitch: pitch as u8,
                    velocity: 48,
                });
            }
        }
        let mut bus = CommandBus::new(Project::new("Key"));
        let track = bus
            .execute(CreateTrack {
                name: "Chords".to_owned(),
                kind: TrackKind::Midi,
                index: None,
            })
            .unwrap()
            .track()
            .unwrap();
        bus.execute(WriteMidiClip {
            track,
            clip: None,
            name: "Chords".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(chords.len() as i64 * 2 * beat),
            notes,
        })
        .unwrap();
        let rendered =
            fourfloor_testkit::render_project(bus.project(), &registry, RATE, 0.5).unwrap();
        fourfloor_testkit::assert_buffer_ok(&rendered);
        assert!(rendered.seconds() > 7.9, "{}", rendered.seconds());
        assert_eq!(detect(&rendered.mono(), RATE), key, "rendered {key}");
    }
}
