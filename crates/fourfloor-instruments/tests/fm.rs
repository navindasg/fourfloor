//! Rendered-audio tests for `fm.organ` and `fm.stab`, under the
//! allocation guard.

mod common;

use common::{cents, fundamental, hz, play, window, RATE};
use fourfloor_core::{Device, EventKind, ParamId, SampleRate};
use fourfloor_instruments::FmSynth;
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, dominant_frequency, is_silent, notes, null_residual_db, peak,
    render_instrument, rms, spectrum, spectrum_band_energy, stereo_correlation, AllocDisabler,
    RenderConfig, TimedEvent,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn with(mut synth: FmSynth, params: &[(ParamId, f32)]) -> FmSynth {
    for (id, value) in params {
        synth.set_param(*id, *value);
    }
    synth
}

/// One sine per voice: only operator 1 is heard, nothing modulates.
fn sines() -> FmSynth {
    with(
        FmSynth::organ(),
        &[
            (FmSynth::ALGORITHM, 3.0),
            (FmSynth::op_level(2), 0.0),
            (FmSynth::op_level(3), 0.0),
            (FmSynth::op_level(4), 0.0),
            (FmSynth::VELOCITY, 0.0),
        ],
    )
}

/// Energy within 4 Hz of `freq`.
fn tone(signal: &[f32], freq: f64) -> f64 {
    spectrum_band_energy(signal, RATE, freq as f32 - 4.0, freq as f32 + 4.0)
}

/// Share of the energy that sits within 4 Hz of a multiple of `f0`.
fn harmonic_share(signal: &[f32], f0: f64) -> f64 {
    let spectrum = spectrum(signal, RATE);
    let mut on = 0.0;
    let mut k = 1.0;
    while k * f0 < 20_000.0 {
        on += spectrum.band_power(k * f0 - 4.0, k * f0 + 4.0);
        k += 1.0;
    }
    on / spectrum.total_power()
}

#[test]
fn organ_plays_the_fundamental_of_each_note() {
    for pitch in [28u8, 36, 43, 48, 60] {
        let out = play(&mut FmSynth::organ(), &[(0.0, 1.4, pitch, 100)], 1.5);
        assert_buffer_ok(&out);
        assert!(peak(&out.left) > 0.05, "pitch {pitch}");
        let found = fundamental(&out.left, 0.3, 1.3, hz(pitch));
        assert!(
            cents(found, hz(pitch)).abs() < 3.0,
            "pitch {pitch}: {found} Hz"
        );
        let held = window(&out.left, 0.4, 1.4);
        assert!(
            (f64::from(dominant_frequency(held, RATE)) - hz(pitch)).abs() < 1.5,
            "pitch {pitch}"
        );
        // Drawbars at the octave and the twelfth, and nothing off the
        // harmonic series.
        assert!(tone(held, 2.0 * hz(pitch)) > 0.05 * tone(held, hz(pitch)));
        assert!(tone(held, 3.0 * hz(pitch)) > 0.002 * tone(held, hz(pitch)));
        assert!(harmonic_share(held, hz(pitch)) > 0.999, "pitch {pitch}");
        assert!((stereo_correlation(&out.left, &out.right) - 1.0).abs() < 1e-6);
    }
}

#[test]
fn stab_is_harmonic_on_the_note_and_gets_darker_as_it_rings() {
    for pitch in [48u8, 55, 60, 67] {
        let out = play(&mut FmSynth::stab(), &[(0.0, 1.4, pitch, 100)], 1.5);
        assert_buffer_ok(&out);
        let f0 = hz(pitch);
        let held = window(&out.left, 0.4, 1.4);
        assert!(harmonic_share(held, f0) > 0.995, "pitch {pitch}");
        assert!(tone(held, f0) > 1.0e-4, "pitch {pitch} has no fundamental");
        let found = fundamental(&out.left, 0.6, 1.3, f0 * 0.5);
        assert!(
            cents(found, f0).abs() < 5.0,
            "pitch {pitch}: {found} Hz, expected {f0}"
        );
    }
    // The modulators decay, so the overtones fall away faster than the
    // fundamental.
    let out = play(&mut FmSynth::stab(), &[(0.0, 1.4, 48, 100)], 1.5);
    let upper = |a: f64, b: f64| {
        let part = window(&out.left, a, b);
        spectrum_band_energy(part, RATE, 500.0, 12_000.0)
            / spectrum_band_energy(part, RATE, 60.0, 12_000.0)
    };
    let (early, late) = (upper(0.0, 0.1), upper(1.0, 1.4));
    assert!(early > 3.0 * late, "overtone share {early} then {late}");
}

#[test]
fn pitch_does_not_depend_on_the_sample_rate() {
    for rate in [SampleRate::HZ_44100, SampleRate::HZ_96000] {
        let events = notes(&[(0.0, 1.4, 57, 100)], rate);
        let out = render_instrument(&mut sines(), &events, 1.5, &RenderConfig::at_rate(rate));
        assert_buffer_ok(&out);
        let found = f64::from(dominant_frequency(&out.left, rate));
        assert!((found - 220.0).abs() < 1.0, "{} Hz: {found}", rate.hz());
    }
}

#[test]
fn a_chord_is_the_sum_of_its_notes() {
    for make in [FmSynth::organ, FmSynth::stab] {
        let chord = [48u8, 52, 55, 59];
        let specs: Vec<(f64, f64, u8, u8)> = chord.iter().map(|p| (0.0, 0.8, *p, 100)).collect();
        let together = play(&mut make(), &specs, 1.2);
        assert_buffer_ok(&together);
        let mut sum = vec![0.0f32; together.frames()];
        for spec in &specs {
            let alone = play(&mut make(), &[*spec], 1.2);
            for (total, x) in sum.iter_mut().zip(&alone.left) {
                *total += x;
            }
        }
        assert_null(&together.left, &sum, -100.0);
        // Every note of the chord is there.
        let held = window(&together.left, 0.3, 0.8);
        for pitch in chord {
            assert!(tone(held, hz(pitch)) > 1.0e-4, "pitch {pitch} is missing");
        }
    }
}

#[test]
fn eight_notes_sound_at_once_and_the_ninth_takes_the_oldest_voice() {
    // Whole tones from C3: far enough apart to tell in the spectrum.
    // Eight full-level sines that start in phase add up to more than
    // full scale at the default level, so turn it down.
    let sines = || with(sines(), &[(FmSynth::LEVEL, 0.35)]);
    let pitches: Vec<u8> = (0..9).map(|k| 48 + 2 * k).collect();
    let eight: Vec<(f64, f64, u8, u8)> = pitches[..8]
        .iter()
        .enumerate()
        .map(|(k, p)| (0.01 * k as f64, 2.0, *p, 100))
        .collect();
    let out = play(&mut sines(), &eight, 1.0);
    assert_buffer_ok(&out);
    let held = window(&out.left, 0.3, 1.0);
    let level = tone(held, hz(pitches[0]));
    for pitch in &pitches[..8] {
        let found = tone(held, hz(*pitch));
        assert!(
            found > 0.5 * level && found < 2.0 * level,
            "pitch {pitch}: {found}"
        );
    }

    let mut nine = eight.clone();
    nine.push((0.5, 1.5, pitches[8], 100));
    let out = play(&mut sines(), &nine, 1.2);
    assert_buffer_ok(&out);
    let after = window(&out.left, 0.6, 1.2);
    assert!(
        tone(after, hz(pitches[0])) < 1.0e-3 * level,
        "the oldest note should be gone"
    );
    for pitch in &pitches[1..] {
        assert!(
            tone(after, hz(*pitch)) > 0.5 * level,
            "pitch {pitch} is missing"
        );
    }

    // A released voice is taken before a held one.
    let mut released = eight.clone();
    released[4].1 = 0.3; // the fifth note ends early
    released.push((0.5, 1.5, pitches[8], 100));
    let out = play(&mut sines(), &released, 1.2);
    let after = window(&out.left, 0.7, 1.2);
    assert!(tone(after, hz(pitches[0])) > 0.5 * level);
    assert!(tone(after, hz(pitches[8])) > 0.5 * level);
    assert!(tone(after, hz(pitches[4])) < 1.0e-3 * level);
}

#[test]
fn notes_release_to_silence() {
    for (make, release) in [
        (FmSynth::organ as fn() -> FmSynth, 0.06),
        (FmSynth::stab, 0.2),
    ] {
        let out = play(
            &mut make(),
            &[(0.0, 0.3, 48, 100), (0.0, 0.3, 55, 100)],
            1.0,
        );
        assert!(rms(window(&out.left, 0.1, 0.3)) > 0.02);
        // The release time is the time to fall 60 dB; the voice is cut at
        // -80 dB, so a third more than that is plenty.
        assert!(
            is_silent(window(&out.left, 0.3 + 1.4 * release, 1.0), -100.0),
            "release {release}"
        );
        // The release is a fade, not a cut.
        let tail = rms(window(&out.left, 0.3 + 0.3 * release, 0.3 + 0.4 * release));
        assert!(tail > 1.0e-3 && tail < 0.5 * rms(window(&out.left, 0.2, 0.3)));
    }
    // A longer release rings longer.
    let mut slow = with(FmSynth::organ(), &[(FmSynth::RELEASE, 1.0)]);
    let out = play(&mut slow, &[(0.0, 0.3, 48, 100)], 2.0);
    assert!(rms(window(&out.left, 0.7, 0.8)) > 1.0e-3);
    assert!(is_silent(window(&out.left, 1.75, 2.0), -100.0));

    // All-notes-off releases held notes; reset leaves nothing behind.
    let events = [
        TimedEvent::at_seconds(0.0, RATE, EventKind::note_on(48, 100)),
        TimedEvent::at_seconds(0.0, RATE, EventKind::note_on(60, 100)),
        TimedEvent::at_seconds(0.4, RATE, EventKind::AllNotesOff),
    ];
    let mut synth = FmSynth::organ();
    let out = render_instrument(&mut synth, &events, 1.0, &RenderConfig::default());
    assert!(rms(window(&out.left, 0.2, 0.4)) > 0.02);
    assert!(is_silent(window(&out.left, 0.5, 1.0), -100.0));
    synth.reset();
    let silent = render_instrument(&mut synth, &[], 0.1, &RenderConfig::default());
    assert!(is_silent(&silent.left, -140.0));
}

#[test]
fn attack_fades_the_note_in() {
    let mut synth = with(sines(), &[(FmSynth::ATTACK, 0.2)]);
    let out = play(&mut synth, &[(0.0, 0.9, 57, 100)], 1.0);
    let full = rms(window(&out.left, 0.4, 0.8));
    let at = |t: f64| f64::from(rms(window(&out.left, t - 0.01, t + 0.01)) / full);
    // Linear: a quarter of the way up at 50 ms, three quarters at 150 ms.
    assert!((at(0.05) - 0.25).abs() < 0.04, "{}", at(0.05));
    assert!((at(0.15) - 0.75).abs() < 0.04, "{}", at(0.15));
    assert!((at(0.3) - 1.0).abs() < 0.02);
}

#[test]
fn modulation_adds_overtones_and_velocity_sets_the_level() {
    // Stack algorithm with only operator 2 modulating operator 1.
    let render = |index: f32| {
        let mut synth = with(
            FmSynth::organ(),
            &[
                (FmSynth::ALGORITHM, 0.0),
                (FmSynth::op_ratio(2), 1.0),
                (FmSynth::op_level(2), index),
                (FmSynth::op_sustain(2), 1.0),
                (FmSynth::op_level(3), 0.0),
                (FmSynth::op_level(4), 0.0),
            ],
        );
        play(&mut synth, &[(0.0, 0.9, 45, 100)], 1.0)
    };
    let overtones = |index: f32| {
        let out = render(index);
        assert_buffer_ok(&out);
        let held = window(&out.left, 0.2, 0.9);
        assert!(harmonic_share(held, 110.0) > 0.999);
        spectrum_band_energy(held, RATE, 300.0, 12_000.0)
            / spectrum_band_energy(held, RATE, 50.0, 12_000.0)
    };
    let (none, some, lots) = (overtones(0.0), overtones(0.2), overtones(0.8));
    assert!(none < 1.0e-6, "a bare carrier is a sine: {none}");
    assert!(some > 0.01 && lots > 2.0 * some, "{none} {some} {lots}");

    // Velocity: full sensitivity scales the level with it, none ignores it.
    let level = |velocity: u8, sense: f32| {
        let mut synth = with(sines(), &[(FmSynth::VELOCITY, sense)]);
        rms(&play(&mut synth, &[(0.0, 0.4, 57, velocity)], 0.5).left)
    };
    let ratio = level(64, 1.0) / level(127, 1.0);
    assert!((ratio - 64.0 / 127.0).abs() < 0.01, "ratio {ratio}");
    assert!((level(20, 0.0) / level(127, 0.0) - 1.0).abs() < 1.0e-4);
    // The level parameter is a plain gain.
    let mut quiet = with(sines(), &[(FmSynth::LEVEL, 0.3)]);
    let quiet = rms(&play(&mut quiet, &[(0.0, 0.4, 57, 127)], 0.5).left);
    assert!((quiet / level(127, 0.0) - 0.5).abs() < 0.01);
}

#[test]
fn renders_the_same_at_any_block_size_and_survives_extreme_settings() {
    let events = notes(
        &[
            (0.0, 0.3, 48, 127),
            (0.1, 0.3, 55, 80),
            (0.25, 0.2, 60, 100),
            (0.5, 0.2, 36, 127),
        ],
        RATE,
    );
    for make in [FmSynth::organ, FmSynth::stab] {
        let render = |block: usize| {
            let config = RenderConfig::default().with_block(block);
            render_instrument(&mut make(), &events, 1.0, &config)
        };
        let (a, b) = (render(512), render(37));
        assert!(null_residual_db(&a.left, &b.left) < -140.0);
    }

    let mut wild = FmSynth::stab();
    let ids: Vec<ParamId> = wild.params().iter().map(|info| info.id).collect();
    for id in &ids {
        wild.set_param(*id, 1.0e9);
    }
    wild.set_param(FmSynth::ALGORITHM, 0.0);
    wild.set_param(FmSynth::RELEASE, 0.05);
    wild.set_param(FmSynth::LEVEL, 0.3);
    let many: Vec<(f64, f64, u8, u8)> = (0..24)
        .map(|k| (0.02 * f64::from(k), 0.3, 24 + 4 * k as u8, 127))
        .collect();
    let out = play(&mut wild, &many, 1.0);
    assert!(out.left.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(rms(&out.left) > 0.001);
    wild.set_param(FmSynth::op_ratio(1), f32::NAN);
    wild.set_param(ParamId(999), 1.0);
    let out = play(&mut wild, &many, 0.5);
    assert!(out.left.iter().all(|x| x.is_finite()));
}
