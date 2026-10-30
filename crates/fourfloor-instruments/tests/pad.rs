//! Rendered-audio tests for `poly.pad`, under the allocation guard.

mod common;

use common::{cents, fundamental, hz, play, window, RATE};
use fourfloor_core::{Device, EventKind, ParamId, SampleRate};
use fourfloor_instruments::PolyPad;
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, is_silent, notes, null_residual_db, peak, render_instrument,
    rms, spectrum, spectrum_band_energy, stereo_correlation, AllocDisabler, RenderConfig,
    TimedEvent,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn pad(params: &[(ParamId, f32)]) -> PolyPad {
    let mut synth = PolyPad::new();
    for (id, value) in params {
        synth.set_param(*id, *value);
    }
    synth
}

/// A steady, dry voicing: organ-like envelope, no chorus, no modulation,
/// a filter that does not move.
const DRY: [(ParamId, f32); 9] = [
    (PolyPad::CHORUS, 0.0),
    (PolyPad::PWM, 0.0),
    (PolyPad::ENV_AMOUNT, 0.0),
    (PolyPad::KEY_TRACK, 0.0),
    (PolyPad::ATTACK, 0.0),
    (PolyPad::SUSTAIN, 1.0),
    (PolyPad::RELEASE, 0.05),
    (PolyPad::RESONANCE, 0.0),
    (PolyPad::SUB, 0.0),
];

fn dry(extra: &[(ParamId, f32)]) -> PolyPad {
    let mut synth = pad(&DRY);
    for (id, value) in extra {
        synth.set_param(*id, *value);
    }
    synth
}

/// Energy within 4 Hz of `freq`.
fn tone(signal: &[f32], freq: f64) -> f64 {
    spectrum_band_energy(signal, RATE, freq as f32 - 4.0, freq as f32 + 4.0)
}

#[test]
fn plays_the_fundamental_of_each_note_and_the_sub_an_octave_below() {
    for pitch in [36u8, 45, 57, 60, 72] {
        let out = play(&mut dry(&[]), &[(0.0, 1.4, pitch, 100)], 1.5);
        assert_buffer_ok(&out);
        assert!(peak(&out.left) > 0.02, "pitch {pitch}");
        let found = fundamental(&out.left, 0.3, 1.3, hz(pitch));
        assert!(
            cents(found, hz(pitch)).abs() < 3.0,
            "pitch {pitch}: {found} Hz"
        );
        // Nothing below the fundamental without the sub.
        let held = window(&out.left, 0.3, 1.3);
        assert!(tone(held, 0.5 * hz(pitch)) < 1.0e-6 * tone(held, hz(pitch)));
        // Dry output is the same on both sides.
        assert!((stereo_correlation(&out.left, &out.right) - 1.0).abs() < 1e-6);
    }
    // The sub alone is a square one octave down: odd harmonics only.
    let mut sub = dry(&[
        (PolyPad::SAW, 0.0),
        (PolyPad::PULSE, 0.0),
        (PolyPad::SUB, 1.0),
        (PolyPad::CUTOFF, 20_000.0),
    ]);
    let out = play(&mut sub, &[(0.0, 1.4, 57, 100)], 1.5);
    let held = window(&out.left, 0.3, 1.3);
    assert!(cents(fundamental(&out.left, 0.3, 1.3, 110.0), 110.0).abs() < 3.0);
    assert!(tone(held, 220.0) < 1.0e-3 * tone(held, 110.0));
    assert!(tone(held, 330.0) > 0.05 * tone(held, 110.0));
    // With the default mix the sub sits under the note.
    let out = play(
        &mut dry(&[(PolyPad::SUB, 0.5)]),
        &[(0.0, 1.4, 57, 100)],
        1.5,
    );
    let held = window(&out.left, 0.3, 1.3);
    assert!(tone(held, 110.0) > 0.02 * tone(held, 220.0));
    assert!(tone(held, 220.0) > tone(held, 110.0));
}

#[test]
fn oscillator_shapes_have_the_right_harmonics() {
    let render = |saw: f32, pulse: f32, width: f32| {
        let mut synth = dry(&[
            (PolyPad::SAW, saw),
            (PolyPad::PULSE, pulse),
            (PolyPad::PULSE_WIDTH, width),
            (PolyPad::CUTOFF, 20_000.0),
        ]);
        let out = play(&mut synth, &[(0.0, 1.4, 45, 100)], 1.5);
        assert_buffer_ok(&out);
        out.left[14_400..62_400].to_vec()
    };
    let harmonic = |signal: &[f32], n: f64| tone(signal, 110.0 * n);
    // Sawtooth: every harmonic, amplitudes 1/n (power 1/n^2).
    let saw = render(1.0, 0.0, 50.0);
    for n in [2.0, 3.0, 5.0] {
        let ratio = harmonic(&saw, n) / harmonic(&saw, 1.0) * n * n;
        assert!(ratio > 0.8 && ratio < 1.25, "saw harmonic {n}: {ratio}");
    }
    // Square: odd harmonics only. A 25 % pulse has no fourth harmonic.
    let square = render(0.0, 1.0, 50.0);
    assert!(harmonic(&square, 2.0) < 1.0e-4 * harmonic(&square, 1.0));
    assert!(harmonic(&square, 3.0) > 0.08 * harmonic(&square, 1.0));
    let narrow = render(0.0, 1.0, 25.0);
    assert!(harmonic(&narrow, 2.0) > 0.2 * harmonic(&narrow, 1.0));
    assert!(harmonic(&narrow, 4.0) < 1.0e-3 * harmonic(&narrow, 1.0));
    // Band-limited: almost nothing between the harmonics up high, where
    // aliases of a naive oscillator would land.
    let spectrum = spectrum(&saw, RATE);
    let mut between = 0.0;
    let mut k = 60.0;
    while 110.0 * k < 20_000.0 {
        between += spectrum.band_power(110.0 * k + 20.0, 110.0 * k + 90.0);
        k += 1.0;
    }
    // A naive sawtooth puts about 0.14 of the band's power there; the
    // two-sample PolyBLEP leaves about a thousandth.
    let share = between / spectrum.band_power(6_500.0, 20_000.0).max(1.0e-12);
    assert!(share < 5.0e-3, "aliasing share {share}");
}

#[test]
fn a_chord_is_the_sum_of_its_notes() {
    let chord = [45u8, 52, 57, 60, 64];
    let specs: Vec<(f64, f64, u8, u8)> = chord.iter().map(|p| (0.0, 0.8, *p, 100)).collect();
    // Default sound but for the chorus: filter envelope, key tracking and
    // pulse-width modulation all on.
    let make = || pad(&[(PolyPad::CHORUS, 0.0), (PolyPad::LEVEL, 0.4)]);
    let together = play(&mut make(), &specs, 2.0);
    assert_buffer_ok(&together);
    let mut sum = vec![0.0f32; together.frames()];
    for spec in &specs {
        let alone = play(&mut make(), &[*spec], 2.0);
        for (total, x) in sum.iter_mut().zip(&alone.left) {
            *total += x;
        }
    }
    assert_null(&together.left, &sum, -100.0);
    let held = window(&together.left, 0.3, 0.8);
    for pitch in chord {
        assert!(tone(held, hz(pitch)) > 1.0e-5, "pitch {pitch} is missing");
    }
    assert!(rms(held) > 1.5 * rms(window(&play(&mut make(), &specs[..1], 2.0).left, 0.3, 0.8)));
}

#[test]
fn eight_notes_sound_at_once_and_the_ninth_steals_the_oldest() {
    const _: () = assert!(PolyPad::VOICES >= 8);
    let sine_like = || {
        // A low cutoff leaves little but each note's fundamental.
        dry(&[
            (PolyPad::PULSE, 0.0),
            (PolyPad::CUTOFF, 300.0),
            (PolyPad::KEY_TRACK, 1.0),
            (PolyPad::LEVEL, 0.5),
        ])
    };
    let pitches: Vec<u8> = (0..9).map(|k| 48 + 2 * k).collect();
    let eight: Vec<(f64, f64, u8, u8)> = pitches[..8]
        .iter()
        .enumerate()
        .map(|(k, p)| (0.01 * k as f64, 2.0, *p, 100))
        .collect();
    let out = play(&mut sine_like(), &eight, 1.0);
    assert_buffer_ok(&out);
    let held = window(&out.left, 0.3, 1.0);
    let level = tone(held, hz(pitches[0]));
    assert!(level > 1.0e-5);
    for pitch in &pitches[..8] {
        let found = tone(held, hz(*pitch));
        assert!(
            found > 0.3 * level && found < 3.0 * level,
            "pitch {pitch}: {found} vs {level}"
        );
    }

    let mut nine = eight.clone();
    nine.push((0.5, 1.5, pitches[8], 100));
    let out = play(&mut sine_like(), &nine, 1.2);
    assert_buffer_ok(&out);
    let after = window(&out.left, 0.7, 1.2);
    assert!(
        tone(after, hz(pitches[0])) < 1.0e-2 * level,
        "the oldest note should be gone"
    );
    for pitch in &pitches[1..] {
        assert!(
            tone(after, hz(*pitch)) > 0.3 * level,
            "pitch {pitch} is missing"
        );
    }

    // A released voice is taken before a held one.
    let mut released = eight.clone();
    released[4].1 = 0.3;
    released.push((0.5, 1.5, pitches[8], 100));
    let out = play(&mut sine_like(), &released, 1.2);
    let after = window(&out.left, 0.7, 1.2);
    assert!(tone(after, hz(pitches[0])) > 0.3 * level);
    assert!(tone(after, hz(pitches[8])) > 0.3 * level);
    assert!(tone(after, hz(pitches[4])) < 1.0e-2 * level);
}

#[test]
fn the_envelope_attacks_decays_sustains_and_releases_to_silence() {
    let mut synth = dry(&[
        (PolyPad::ATTACK, 0.2),
        (PolyPad::DECAY, 0.3),
        (PolyPad::SUSTAIN, 0.4),
        (PolyPad::RELEASE, 0.3),
        (PolyPad::CUTOFF, 20_000.0),
    ]);
    let out = play(&mut synth, &[(0.0, 1.0, 57, 127)], 2.0);
    assert_buffer_ok(&out);
    let at = |t: f64| f64::from(rms(window(&out.left, t - 0.01, t + 0.01)));
    let top = at(0.2);
    // Linear attack.
    assert!((at(0.05) / top - 0.25).abs() < 0.05, "{}", at(0.05) / top);
    assert!((at(0.1) / top - 0.5).abs() < 0.05, "{}", at(0.1) / top);
    // Decay to the sustain level and stay there.
    assert!((at(0.6) / top - 0.4).abs() < 0.03, "{}", at(0.6) / top);
    assert!((at(0.95) / at(0.6) - 1.0).abs() < 0.03);
    // Release: 60 dB in 0.3 s, so 20 dB down after 0.1 s; then silence.
    let down = 20.0 * (at(1.1) / at(0.95)).log10();
    assert!((down + 20.0).abs() < 2.5, "{down} dB after a third");
    assert!(is_silent(window(&out.left, 1.45, 2.0), -100.0));
    assert!(is_silent(window(&out.right, 1.45, 2.0), -100.0));

    // The default pad with chorus also ends in silence, on both sides.
    let out = play(
        &mut PolyPad::new(),
        &[(0.0, 0.5, 57, 100), (0.0, 0.5, 64, 100)],
        2.5,
    );
    assert_buffer_ok(&out);
    assert!(rms(window(&out.left, 0.3, 0.5)) > 0.01);
    assert!(is_silent(window(&out.left, 1.8, 2.5), -100.0));
    assert!(is_silent(window(&out.right, 1.8, 2.5), -100.0));

    // All-notes-off releases; reset leaves nothing behind.
    let events = [
        TimedEvent::at_seconds(0.0, RATE, EventKind::note_on(57, 100)),
        TimedEvent::at_seconds(0.4, RATE, EventKind::AllNotesOff),
    ];
    let mut synth = dry(&[]);
    let out = render_instrument(&mut synth, &events, 1.0, &RenderConfig::default());
    assert!(rms(window(&out.left, 0.2, 0.4)) > 0.01);
    assert!(is_silent(window(&out.left, 0.5, 1.0), -100.0));
    synth.reset();
    let silent = render_instrument(&mut synth, &[], 0.1, &RenderConfig::default());
    assert!(is_silent(&silent.left, -140.0));
}

#[test]
fn the_lowpass_cuts_above_the_cutoff_and_resonance_peaks_there() {
    let render = |params: &[(ParamId, f32)]| {
        let mut synth = dry(&[(PolyPad::PULSE, 0.0)]);
        for (id, value) in params {
            synth.set_param(*id, *value);
        }
        let out = play(&mut synth, &[(0.0, 1.4, 33, 100)], 1.5);
        assert_buffer_ok(&out);
        out.left[14_400..62_400].to_vec()
    };
    let band = |signal: &[f32], lo: f32, hi: f32| spectrum_band_energy(signal, RATE, lo, hi);
    let open = render(&[(PolyPad::CUTOFF, 20_000.0)]);
    let closed = render(&[(PolyPad::CUTOFF, 500.0)]);
    // Four poles: 24 dB per octave. Two octaves above the cutoff the saw is
    // about 48 dB down, the bass untouched.
    let cut = 10.0 * (band(&closed, 1_800.0, 2_200.0) / band(&open, 1_800.0, 2_200.0)).log10();
    assert!(cut < -40.0 && cut > -56.0, "{cut} dB two octaves up");
    let bass = 10.0 * (band(&closed, 40.0, 120.0) / band(&open, 40.0, 120.0)).log10();
    assert!(bass.abs() < 1.0, "{bass} dB in the bass");
    // Opening the cutoff lets more through, step by step.
    let mid = render(&[(PolyPad::CUTOFF, 2_000.0)]);
    let top = |signal: &[f32]| band(signal, 3_000.0, 12_000.0);
    assert!(top(&closed) < 0.01 * top(&mid) && top(&mid) < 0.5 * top(&open));

    // Resonance lifts the band at the cutoff against the same band without.
    for cutoff in [700.0f32, 1_400.0] {
        let around = |signal: &[f32]| {
            band(signal, 0.85 * cutoff, 1.15 * cutoff) / band(signal, 40.0, 0.4 * cutoff)
        };
        let flat = around(&render(&[(PolyPad::CUTOFF, cutoff)]));
        let half = around(&render(&[
            (PolyPad::CUTOFF, cutoff),
            (PolyPad::RESONANCE, 0.5),
        ]));
        let high = around(&render(&[
            (PolyPad::CUTOFF, cutoff),
            (PolyPad::RESONANCE, 0.9),
        ]));
        assert!(
            half > 1.5 * flat && high > 4.0 * half,
            "{flat} {half} {high}"
        );
        // The peak is at the cutoff, not beside it.
        let resonant = render(&[(PolyPad::CUTOFF, cutoff), (PolyPad::RESONANCE, 0.9)]);
        let beside = band(&resonant, 0.4 * cutoff, 0.6 * cutoff).max(band(
            &resonant,
            1.6 * cutoff,
            2.4 * cutoff,
        ));
        assert!(band(&resonant, 0.85 * cutoff, 1.15 * cutoff) > 3.0 * beside);
    }

    // The envelope opens the filter: brighter at the top of the attack
    // than once it has decayed.
    let mut swept = dry(&[
        (PolyPad::PULSE, 0.0),
        (PolyPad::CUTOFF, 200.0),
        (PolyPad::ENV_AMOUNT, 1.0),
        (PolyPad::DECAY, 0.4),
        (PolyPad::SUSTAIN, 0.0),
    ]);
    let out = play(&mut swept, &[(0.0, 1.4, 33, 100)], 1.5);
    let share = |a: f64, b: f64| {
        let part = window(&out.left, a, b);
        band(part, 1_000.0, 12_000.0) / band(part, 40.0, 12_000.0)
    };
    assert!(share(0.0, 0.05) > 20.0 * share(0.3, 0.4));
    // Key tracking moves the cutoff with the note.
    let tracked = |track: f32| {
        let mut synth = dry(&[
            (PolyPad::PULSE, 0.0),
            (PolyPad::CUTOFF, 400.0),
            (PolyPad::KEY_TRACK, track),
        ]);
        let out = play(&mut synth, &[(0.0, 0.9, 84, 100)], 1.0);
        rms(window(&out.left, 0.2, 0.9))
    };
    assert!(tracked(1.0) > 4.0 * tracked(0.0));
}

#[test]
fn the_chorus_widens_the_sound_and_modulation_moves_it() {
    let render = |mode: f32| {
        let mut synth = dry(&[(PolyPad::CHORUS, mode)]);
        play(&mut synth, &[(0.0, 2.9, 57, 100), (0.0, 2.9, 64, 100)], 3.0)
    };
    let (off, one, two) = (render(0.0), render(1.0), render(2.0));
    for out in [&off, &one, &two] {
        assert_buffer_ok(out);
    }
    let width = |out: &fourfloor_testkit::StereoBuffer| {
        1.0 - stereo_correlation(window(&out.left, 0.5, 2.9), window(&out.right, 0.5, 2.9))
    };
    assert!(width(&off).abs() < 1e-6);
    assert!(width(&one) > 0.05, "chorus I width {}", width(&one));
    assert!(width(&two) > 0.05, "chorus II width {}", width(&two));
    assert!(null_residual_db(&one.left, &two.left) > -40.0);
    // About as loud as the dry sound, on both sides.
    for out in [&one, &two] {
        for side in [&out.left, &out.right] {
            let ratio = rms(window(side, 0.5, 2.9)) / rms(window(&off.left, 0.5, 2.9));
            assert!(ratio > 0.7 && ratio < 1.5, "level ratio {ratio}");
        }
    }
    // The pitch is unchanged on average.
    let mut single = dry(&[(PolyPad::CHORUS, 1.0)]);
    let out = play(&mut single, &[(0.0, 2.9, 57, 100)], 3.0);
    // (Zero crossings are no use here: the sweeping comb notches the
    // fundamental out for a moment each cycle.)
    let found = fourfloor_testkit::dominant_frequency(window(&out.left, 0.5, 2.5), RATE);
    assert!((found - 220.0).abs() < 1.0, "{found}");

    // Vibrato: the pitch swings by the set depth around the note.
    let mut vibrato = dry(&[
        (PolyPad::VIBRATO, 50.0),
        (PolyPad::LFO_RATE, 2.0),
        (PolyPad::PULSE, 0.0),
        (PolyPad::CUTOFF, 300.0),
    ]);
    let out = play(&mut vibrato, &[(0.0, 2.4, 57, 100)], 2.5);
    // The LFO starts at zero and rises: a quarter period in it is at its
    // top, three quarters in at its bottom.
    let up = cents(fundamental(&out.left, 0.10, 0.15, 220.0), 220.0);
    let down = cents(fundamental(&out.left, 0.35, 0.40, 220.0), 220.0);
    assert!(up > 35.0 && up < 55.0, "up {up}");
    assert!(down < -35.0 && down > -55.0, "down {down}");

    // Pulse-width modulation changes the pulse's spectrum over time.
    let pwm = |depth: f32| {
        let mut synth = dry(&[
            (PolyPad::SAW, 0.0),
            (PolyPad::PULSE, 1.0),
            (PolyPad::PWM, depth),
            (PolyPad::LFO_RATE, 1.0),
            (PolyPad::CUTOFF, 20_000.0),
        ]);
        let out = play(&mut synth, &[(0.0, 1.4, 45, 100)], 1.5);
        // Second harmonic: absent from a square, present in a narrow pulse.
        let second = |a: f64, b: f64| {
            let part = window(&out.left, a, b);
            spectrum_band_energy(part, RATE, 200.0, 240.0)
                / spectrum_band_energy(part, RATE, 90.0, 130.0)
        };
        (second(0.2, 0.3), second(0.45, 0.55))
    };
    let (wide, square) = pwm(1.0);
    assert!(wide > 0.3 && square < 0.05, "{wide} {square}");
    let (a, b) = pwm(0.0);
    assert!(a < 1.0e-3 && b < 1.0e-3);
}

#[test]
fn renders_the_same_at_any_block_size_and_rate_and_survives_extreme_settings() {
    let events = notes(
        &[
            (0.0, 0.3, 48, 127),
            (0.1, 0.3, 55, 80),
            (0.25, 0.2, 60, 100),
            (0.5, 0.2, 36, 127),
        ],
        RATE,
    );
    let render = |block: usize| {
        let config = RenderConfig::default().with_block(block);
        render_instrument(&mut PolyPad::new(), &events, 1.5, &config)
    };
    let (a, b) = (render(512), render(37));
    assert!(null_residual_db(&a.left, &b.left) < -140.0);
    assert!(null_residual_db(&a.right, &b.right) < -140.0);

    for rate in [SampleRate::HZ_44100, SampleRate::HZ_96000] {
        let events = notes(&[(0.0, 1.4, 57, 100)], rate);
        let out = render_instrument(&mut dry(&[]), &events, 1.5, &RenderConfig::at_rate(rate));
        assert_buffer_ok(&out);
        let from = (0.3 * rate.as_f64()) as usize;
        let to = (1.3 * rate.as_f64()) as usize;
        let found = f64::from(fourfloor_testkit::dominant_frequency(
            &out.left[from..to],
            rate,
        ));
        assert!((found - 220.0).abs() < 1.0, "{} Hz: {found}", rate.hz());
    }

    let mut wild = PolyPad::new();
    let ids: Vec<ParamId> = wild.params().iter().map(|info| info.id).collect();
    for id in &ids {
        wild.set_param(*id, 1.0e9);
    }
    wild.set_param(PolyPad::ATTACK, 0.0);
    wild.set_param(PolyPad::RELEASE, 0.05);
    wild.set_param(PolyPad::LEVEL, 0.05);
    let many: Vec<(f64, f64, u8, u8)> = (0..24)
        .map(|k| (0.02 * f64::from(k), 0.3, 24 + 4 * k as u8, 127))
        .collect();
    let out = play(&mut wild, &many, 1.0);
    assert!(out.left.iter().all(|x| x.is_finite()));
    assert!(out.right.iter().all(|x| x.is_finite()));
    assert!(rms(&out.left) > 1.0e-4);
    wild.set_param(PolyPad::CUTOFF, f32::NAN);
    wild.set_param(ParamId(999), 1.0);
    let out = play(&mut wild, &many, 0.5);
    assert!(out.left.iter().all(|x| x.is_finite()));
}
