//! Varispeed and key lock, measured on rendered audio. With the `stretch`
//! feature these run against Signalsmith Stretch, otherwise against the
//! built-in WSOLA stretcher.

mod common;

use common::{renderer, run, tone_pulses, RATE};
use fourfloor_dj::{Deck, DjCommand};
use fourfloor_testkit::{dominant_frequency, onset_times, rms};

/// Renders a 440 Hz pulse track at `percent` pitch, with or without key
/// lock. Returns the deck output (left) after the first second.
fn play(percent: f64, key_lock: bool) -> Vec<f32> {
    let mut dj = renderer(2, &[]);
    dj.load(0, tone_pulses(440.0, 120.0, 30.0)).unwrap();
    run(&mut dj, DjCommand::SetPitch { deck: 0, percent });
    run(
        &mut dj,
        DjCommand::SetKeyLock {
            deck: 0,
            on: key_lock,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 0 });
    let out = dj.render_seconds(12.0).unwrap();
    out.decks[0][0][RATE.hz() as usize..].to_vec()
}

/// Mean beat gap, skipping the first onset (the audio starts mid-pulse).
fn mean_gap(onsets: &[f64]) -> f64 {
    let onsets = &onsets[1..];
    assert!(onsets.len() > 10, "{} onsets", onsets.len());
    (onsets[onsets.len() - 1] - onsets[0]) / (onsets.len() - 1) as f64
}

#[test]
fn varispeed_raises_pitch_with_tempo() {
    let audio = play(6.0, false);
    let frequency = dominant_frequency(&audio, RATE);
    assert!((frequency - 466.4).abs() < 2.0, "{frequency} Hz");
    let gap = mean_gap(&onset_times(&audio, RATE));
    assert!((gap - 0.5 / 1.06).abs() < 0.002, "beat gap {gap}");
}

#[test]
fn key_lock_keeps_the_pitch_while_tempo_changes() {
    println!("key lock engine: {}", Deck::key_lock_engine());
    for percent in [6.0, -8.0, 16.0] {
        let audio = play(percent, true);
        let frequency = dominant_frequency(&audio, RATE);
        assert!(
            (frequency - 440.0).abs() < 440.0 * 0.01,
            "{percent} %: {frequency} Hz"
        );
        let rate = 1.0 + percent / 100.0;
        let gap = mean_gap(&onset_times(&audio, RATE));
        assert!(
            (gap - 0.5 / rate).abs() < 0.004,
            "{percent} %: beat gap {gap}"
        );
        // The level is kept (no cancellation between grains).
        let level = rms(&audio);
        let reference = rms(&play(0.0, true));
        assert!(
            (20.0 * (level / reference).log10()).abs() < 1.5,
            "{percent} %: level {level} vs {reference}"
        );
    }
}

/// A track at 44.1 kHz on the 48 kHz engine: key lock must play grains at
/// the track's own rate, so the pitch stays put while the tempo changes.
#[test]
fn key_lock_keeps_the_pitch_of_a_track_at_another_sample_rate() {
    let track_rate = fourfloor_core::SampleRate::HZ_44100;
    let seconds = 20.0;
    let frames = (seconds * track_rate.as_f64()) as usize;
    let signal: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f64 / track_rate.as_f64();
            (0.4 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32
        })
        .collect();
    let track = fourfloor_dj::DeckTrack::from_mono(track_rate, &signal)
        .unwrap()
        .with_bpm(120.0, 0.0)
        .unwrap();
    for (percent, key_lock, expected) in [(8.0, true, 440.0), (8.0, false, 475.2)] {
        let mut dj = renderer(2, &[]);
        dj.load(0, track.clone()).unwrap();
        run(&mut dj, DjCommand::SetPitch { deck: 0, percent });
        run(
            &mut dj,
            DjCommand::SetKeyLock {
                deck: 0,
                on: key_lock,
            },
        );
        run(&mut dj, DjCommand::Play { deck: 0 });
        let out = dj.render_seconds(6.0).unwrap();
        let audio = &out.decks[0][0][RATE.hz() as usize..];
        let frequency = dominant_frequency(audio, RATE);
        assert!(
            (frequency - expected).abs() < expected * 0.01,
            "key lock {key_lock}: {frequency} Hz, wanted {expected}"
        );
        // Six seconds at +8 % covers 6.48 s of the track.
        let position = dj.session().state().decks[0].position_seconds;
        assert!((position - 6.0 * 1.08).abs() < 0.01, "position {position}");
    }
}

#[test]
fn key_lock_at_normal_speed_is_transparent() {
    let locked = play(0.0, true);
    let plain = play(0.0, false);
    let frequency = dominant_frequency(&locked, RATE);
    assert!((frequency - 440.0).abs() < 1.0, "{frequency} Hz");
    if Deck::key_lock_engine() == "wsola" {
        // WSOLA at rate 1 places every grain where it was: a null.
        fourfloor_testkit::assert_null(&locked, &plain, -100.0);
    }
}
