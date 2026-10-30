//! Rendered-audio tests for `synth.acid303`, under the allocation guard.

mod common;

use common::{cents, fundamental, hz, lowpass, play, rising_crossings, window, RATE};
use fourfloor_core::{Device, EventKind, ParamId, SampleRate};
use fourfloor_instruments::Acid303;
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, is_silent, notes, peak, render_instrument, rms, spectrum,
    spectrum_band_energy, stereo_correlation, AllocDisabler, RenderConfig, TimedEvent,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn acid(params: &[(ParamId, f32)]) -> Acid303 {
    let mut synth = Acid303::new();
    for (id, value) in params {
        synth.set_param(*id, *value);
    }
    synth
}

/// A dark, steady voicing whose zero crossings follow the fundamental.
fn plain() -> Acid303 {
    acid(&[
        (Acid303::RESONANCE, 0.0),
        (Acid303::ENV_MOD, 0.0),
        (Acid303::CUTOFF, 300.0),
    ])
}

#[test]
fn plays_the_right_pitch_for_each_note() {
    for pitch in [33u8, 45, 52, 57] {
        let out = play(&mut plain(), &[(0.0, 1.4, pitch, 90)], 1.5);
        assert_buffer_ok(&out);
        assert!(
            peak(&out.left) > 0.02,
            "pitch {pitch} peak {}",
            peak(&out.left)
        );
        let found = fundamental(&out.left, 0.3, 1.3, hz(pitch));
        assert!(
            cents(found, hz(pitch)).abs() < 3.0,
            "pitch {pitch}: {found} Hz"
        );
        assert!((stereo_correlation(&out.left, &out.right) - 1.0).abs() < 1e-6);
    }
    // Tune transposes; pitch does not depend on the sample rate.
    let mut up = plain();
    up.set_param(Acid303::TUNE, 12.0);
    let out = play(&mut up, &[(0.0, 1.4, 45, 90)], 1.5);
    assert!(cents(fundamental(&out.left, 0.3, 1.3, 220.0), 220.0).abs() < 3.0);
    for rate in [SampleRate::HZ_44100, SampleRate::HZ_96000] {
        let events = notes(&[(0.0, 1.4, 45, 90)], rate);
        let out = render_instrument(&mut plain(), &events, 1.5, &RenderConfig::at_rate(rate));
        assert_buffer_ok(&out);
        let smooth = {
            let k = 1.0 - (-std::f64::consts::TAU * 110.0 / rate.as_f64()).exp();
            let (mut a, mut b) = (0.0f64, 0.0f64);
            out.left
                .iter()
                .map(|x| {
                    a += k * (f64::from(*x) - a);
                    b += k * (a - b);
                    b as f32
                })
                .collect::<Vec<f32>>()
        };
        let from = (0.3 * rate.as_f64()) as usize;
        let to = (1.3 * rate.as_f64()) as usize;
        let times = rising_crossings(&smooth[from..to]);
        // `rising_crossings` assumes 48 kHz; rescale.
        let span = (times[times.len() - 1] - times[0]) * 48_000.0 / rate.as_f64();
        let found = (times.len() - 1) as f64 / span;
        assert!(cents(found, 110.0).abs() < 3.0, "{} Hz: {found}", rate.hz());
    }
}

/// Per-cycle (midpoint time, frequency) of the fundamental.
fn pitch_track(signal: &[f32]) -> Vec<(f64, f64)> {
    let smooth = lowpass(signal, 200.0);
    let times = rising_crossings(&smooth);
    times
        .windows(2)
        .map(|pair| (0.5 * (pair[0] + pair[1]), 1.0 / (pair[1] - pair[0])))
        .collect()
}

fn frequency_at(track: &[(f64, f64)], from: f64, to: f64) -> f64 {
    let inside: Vec<f64> = track
        .iter()
        .filter(|(t, _)| *t >= from && *t < to)
        .map(|(_, f)| *f)
        .collect();
    assert!(!inside.is_empty(), "no cycles between {from} and {to}");
    inside.iter().sum::<f64>() / inside.len() as f64
}

#[test]
fn overlapping_notes_slide_and_separate_notes_jump() {
    // Slide 300 ms: Open303 uses a fifth of it as the glide's time constant.
    let tau = 0.060;
    let mut synth = plain();
    synth.set_param(Acid303::SLIDE, 300.0);
    let slid = play(&mut synth, &[(0.0, 0.5, 45, 90), (0.4, 0.6, 57, 90)], 1.1);
    let jumped = play(&mut synth, &[(0.0, 0.39, 45, 90), (0.4, 0.6, 57, 90)], 1.1);
    assert_buffer_ok(&slid);
    assert_buffer_ok(&jumped);

    let track = pitch_track(&slid.left);
    assert!((frequency_at(&track, 0.2, 0.38) - 110.0).abs() < 0.5);
    assert!((frequency_at(&track, 0.9, 0.98) - 220.0).abs() < 1.0);
    // The glide follows 220 - 110 * exp(-t / tau), and only ever rises.
    // The synth's filters and the lowpass used to track the pitch delay
    // what is measured by a few milliseconds, so the curve is bracketed
    // between the ideal one and the same curve 8 ms late.
    let ideal = |t: f64| 220.0 - 110.0 * (-(t - 0.4).max(0.0) / tau).exp();
    let glide: Vec<(f64, f64)> = track
        .iter()
        .copied()
        .filter(|(t, _)| *t > 0.41 && *t < 0.6)
        .collect();
    assert!(glide.len() > 15);
    for (t, found) in &glide {
        let (late, prompt) = (ideal(t - 0.008), ideal(*t));
        assert!(
            *found > 0.98 * late && *found < 1.02 * prompt,
            "at {t}: {found} Hz, expected {late} to {prompt}"
        );
    }
    for pair in glide.windows(2) {
        assert!(pair[1].1 > pair[0].1 - 0.5, "{pair:?}");
    }
    let midway = frequency_at(&track, 0.43, 0.45);
    assert!(midway > 130.0 && midway < 200.0, "midway {midway}");

    // Without the overlap the new note is at pitch straight away, and its
    // envelopes restart, so it is louder than the slid one.
    let track = pitch_track(&jumped.left);
    let after = frequency_at(&track, 0.43, 0.46);
    assert!((after - 220.0).abs() < 4.0, "after a jump: {after}");
    let slid_level = rms(window(&slid.left, 0.45, 0.6));
    let jumped_level = rms(window(&jumped.left, 0.45, 0.6));
    assert!(
        jumped_level > 1.1 * slid_level,
        "{jumped_level} vs {slid_level}"
    );

    // A faster slide gets there sooner.
    synth.set_param(Acid303::SLIDE, 30.0);
    let quick = play(&mut synth, &[(0.0, 0.5, 45, 90), (0.4, 0.6, 57, 90)], 1.1);
    let quick_midway = frequency_at(&pitch_track(&quick.left), 0.43, 0.45);
    assert!(quick_midway > 215.0, "quick slide midway {quick_midway}");
}

#[test]
fn releasing_the_newer_key_slides_back_to_the_held_one() {
    let mut synth = plain();
    let out = play(&mut synth, &[(0.0, 1.0, 45, 90), (0.3, 0.2, 57, 90)], 1.1);
    let track = pitch_track(&out.left);
    assert!((frequency_at(&track, 0.42, 0.49) - 220.0).abs() < 2.0);
    assert!((frequency_at(&track, 0.7, 0.95) - 110.0).abs() < 1.0);
    assert!(rms(window(&out.left, 0.8, 0.95)) > 0.005);
}

#[test]
fn accented_notes_are_louder_and_brighter() {
    let render = |velocity: u8, accent: f32| {
        let mut synth = acid(&[(Acid303::ACCENT, accent)]);
        play(&mut synth, &[(0.0, 0.3, 45, velocity)], 0.6)
    };
    let normal = render(90, 60.0);
    let accented = render(Acid303::ACCENT_VELOCITY, 60.0);
    assert_buffer_ok(&normal);
    assert_buffer_ok(&accented);
    let (quiet, loud) = (
        rms(window(&normal.left, 0.0, 0.3)),
        rms(window(&accented.left, 0.0, 0.3)),
    );
    assert!(loud > 1.5 * quiet, "accent {loud} vs {quiet}");
    // The accent opens the filter further while its envelope is high:
    // more of the energy sits above 3 kHz at the start of the note.
    let top = |signal: &[f32]| {
        let start = window(signal, 0.005, 0.06);
        spectrum_band_energy(start, RATE, 3_000.0, 12_000.0)
            / spectrum_band_energy(start, RATE, 30.0, 12_000.0)
    };
    assert!(
        top(&accented.left) > 1.3 * top(&normal.left),
        "{} vs {}",
        top(&accented.left),
        top(&normal.left)
    );
    // Velocities on the same side of the threshold sound the same.
    assert_null(&render(1, 60.0).left, &normal.left, -120.0);
    assert_null(&render(127, 60.0).left, &accented.left, -120.0);
    // More accent, more level.
    let stronger = render(127, 100.0);
    assert!(rms(&stronger.left) > 1.2 * rms(&accented.left));
    assert!(
        top(&stronger.left) > 1.2 * top(&accented.left),
        "{} vs {}",
        top(&stronger.left),
        top(&accented.left)
    );
}

/// Share of the energy that lies above 1 kHz.
fn brightness(signal: &[f32]) -> f64 {
    spectrum_band_energy(signal, RATE, 1_000.0, 12_000.0)
        / spectrum_band_energy(signal, RATE, 30.0, 12_000.0)
}

/// Relative level of the band around `centre` against the bass.
fn band_ratio(signal: &[f32], centre: f32) -> f64 {
    spectrum_band_energy(signal, RATE, 0.75 * centre, 1.25 * centre)
        / spectrum_band_energy(signal, RATE, 40.0, 200.0)
}

/// Strongest frequency above 350 Hz.
fn upper_peak(signal: &[f32]) -> f64 {
    let spectrum = spectrum(signal, RATE);
    let mut best = (0usize, 0.0f64);
    for (bin, power) in spectrum.power.iter().enumerate() {
        if bin as f64 * spectrum.bin_hz > 350.0 && *power > best.1 {
            best = (bin, *power);
        }
    }
    best.0 as f64 * spectrum.bin_hz
}

#[test]
fn resonance_raises_the_band_at_the_cutoff() {
    let render = |cutoff: f32, resonance: f32| {
        let mut synth = acid(&[
            (Acid303::CUTOFF, cutoff),
            (Acid303::RESONANCE, resonance),
            (Acid303::ENV_MOD, 0.0),
            (Acid303::DECAY, 200.0),
        ]);
        let out = play(&mut synth, &[(0.0, 2.4, 33, 90)], 2.5);
        assert_buffer_ok(&out);
        out.left[57_600..105_600].to_vec() // 1.2 s to 2.2 s: the envelope has settled
    };
    for cutoff in [800.0f32, 1_600.0] {
        let resonant = render(cutoff, 95.0);
        // Open303's envelope offset leaves the settled filter a little
        // under the knob value.
        let found = upper_peak(&resonant);
        let ratio = found / f64::from(cutoff);
        assert!(
            ratio > 0.65 && ratio < 1.1,
            "cutoff {cutoff}: resonant peak at {found} Hz"
        );
        let flat = render(cutoff, 0.0);
        let centre = found as f32;
        let (with, without) = (band_ratio(&resonant, centre), band_ratio(&flat, centre));
        assert!(with > 8.0 * without, "cutoff {cutoff}: {with} vs {without}");
        let half = band_ratio(&render(cutoff, 50.0), centre);
        assert!(half > without && half < with, "{without} < {half} < {with}");
    }
}

#[test]
fn the_envelope_sweeps_the_filter_down() {
    let render = |env_mod: f32| {
        let mut synth = acid(&[
            (Acid303::ENV_MOD, env_mod),
            (Acid303::CUTOFF, 400.0),
            (Acid303::RESONANCE, 30.0),
        ]);
        play(&mut synth, &[(0.0, 1.9, 33, 90)], 2.0)
    };
    let bright = |signal: &[f32], a: f64, b: f64| brightness(window(signal, a, b));
    let deep = render(100.0);
    let shallow = render(0.0);
    let (early, late) = (bright(&deep.left, 0.0, 0.1), bright(&deep.left, 1.6, 1.9));
    assert!(early > 20.0 * late, "deep sweep {early} to {late}");
    let flat = bright(&shallow.left, 0.0, 0.1);
    assert!(early > 5.0 * flat, "deep {early} vs shallow {flat}");
}

#[test]
fn square_wave_has_odd_harmonics_only() {
    let render = |waveform: f32| {
        let mut synth = acid(&[
            (Acid303::WAVEFORM, waveform),
            (Acid303::CUTOFF, 5_000.0),
            (Acid303::RESONANCE, 0.0),
        ]);
        let out = play(&mut synth, &[(0.0, 1.4, 45, 90)], 1.5);
        out.left[14_400..62_400].to_vec()
    };
    let harmonic = |signal: &[f32], n: f32| {
        spectrum_band_energy(signal, RATE, 110.0 * n - 10.0, 110.0 * n + 10.0)
    };
    let saw = render(0.0);
    let square = render(1.0);
    assert!(harmonic(&saw, 2.0) > 0.1 * harmonic(&saw, 1.0));
    assert!(harmonic(&square, 2.0) < 0.001 * harmonic(&square, 1.0));
    assert!(harmonic(&square, 4.0) < 0.01 * harmonic(&square, 3.0));
    assert!(harmonic(&square, 3.0) > 0.03 * harmonic(&square, 1.0));
}

#[test]
fn notes_release_to_silence_and_all_notes_off_stops_held_notes() {
    // An accented note releases with Open303's 50 ms time constant.
    let out = play(&mut Acid303::new(), &[(0.0, 0.3, 45, 127)], 1.3);
    assert!(rms(window(&out.left, 0.1, 0.3)) > 0.01);
    assert!(
        is_silent(window(&out.left, 1.0, 1.3), -100.0),
        "accented release"
    );
    let out = play(&mut Acid303::new(), &[(0.0, 0.3, 45, 90)], 1.0);
    assert!(
        is_silent(window(&out.left, 0.35, 1.0), -100.0),
        "normal release"
    );

    let events = [
        TimedEvent::at_seconds(0.0, RATE, EventKind::note_on(45, 90)),
        TimedEvent::at_seconds(0.1, RATE, EventKind::note_on(57, 90)),
        TimedEvent::at_seconds(0.5, RATE, EventKind::AllNotesOff),
    ];
    let mut synth = Acid303::new();
    let out = render_instrument(&mut synth, &events, 1.0, &RenderConfig::default());
    assert!(rms(window(&out.left, 0.3, 0.5)) > 0.01);
    assert!(is_silent(window(&out.left, 0.6, 1.0), -100.0));
    // Nothing is left over for the next render after a reset.
    synth.reset();
    let silent = render_instrument(&mut synth, &[], 0.2, &RenderConfig::default());
    assert!(is_silent(&silent.left, -140.0));
}

#[test]
fn renders_the_same_at_any_block_size_and_survives_extreme_settings() {
    let events = notes(
        &[
            (0.0, 0.2, 45, 127),
            (0.15, 0.2, 57, 80),
            (0.4, 0.1, 40, 110),
            (0.6, 0.3, 33, 127),
        ],
        RATE,
    );
    let render = |block: usize| {
        render_instrument(
            &mut Acid303::new(),
            &events,
            1.0,
            &RenderConfig::default().with_block(block),
        )
    };
    assert_null(&render(512).left, &render(61).left, -140.0);

    let mut wild = acid(&[
        (Acid303::RESONANCE, 100.0),
        (Acid303::CUTOFF, 5_000.0),
        (Acid303::ENV_MOD, 100.0),
        (Acid303::ACCENT, 100.0),
        (Acid303::LEVEL, -24.0),
    ]);
    let out = render_instrument(
        &mut wild,
        &notes(&[(0.0, 0.5, 96, 127), (0.5, 0.5, 24, 127)], RATE),
        1.2,
        &RenderConfig::default(),
    );
    assert_buffer_ok(&out);
    assert!(rms(&out.left) > 0.001);
    // Out-of-range and non-finite values are clamped or ignored.
    wild.set_param(Acid303::CUTOFF, f32::NAN);
    wild.set_param(Acid303::CUTOFF, 1.0e9);
    wild.set_param(ParamId(999), 1.0);
    assert_buffer_ok(&render_instrument(
        &mut wild,
        &events,
        0.5,
        &RenderConfig::default(),
    ));
}
