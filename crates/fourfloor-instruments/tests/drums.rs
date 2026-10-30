//! Rendered-audio tests for the drum voices and the kit. The allocation
//! guard is installed, so any allocation while processing aborts.

mod common;

use common::{crossing_frequency, play, rising_crossings, window, RATE};
use fourfloor_core::{Device, ParamId};
use fourfloor_instruments::drums::{
    ClapVoice, Cowbell808Voice, HatVoice, Kick808Voice, Kick909Voice, RimVoice, Snare909Voice,
    TomVoice,
};
use fourfloor_instruments::{
    Clap909, Cowbell808, DrumKit, Hat909, Kick808, Kick909, KitSlot, Rim909, Snare909, Tom909,
};
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, is_silent, notes, onset_times, peak, render_instrument, rms,
    rms_db, spectrum_band_energy, stereo_correlation, times_match, to_db, AllocDisabler,
    RenderConfig,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const HIT: [(f64, f64, u8, u8); 1] = [(0.0, 0.05, 36, 127)];

fn with(mut device: impl Device, params: &[(ParamId, f32)]) -> impl Device {
    for (id, value) in params {
        device.set_param(*id, *value);
    }
    device
}

#[test]
fn kick909_settles_on_its_tuned_fundamental() {
    for tune in [48.0f32, 60.0] {
        let mut kick = with(
            Kick909::new(),
            &[(Kick909Voice::TUNE, tune), (Kick909Voice::CLICK, 0.0)],
        );
        let out = play(&mut kick, &HIT, 1.0);
        assert_buffer_ok(&out);
        let found = crossing_frequency(window(&out.left, 0.18, 0.4));
        assert!(
            (found - f64::from(tune)).abs() < 1.5,
            "tune {tune}: tail at {found} Hz"
        );
    }
}

#[test]
fn kick909_pitch_sweeps_down_from_tune_plus_sweep() {
    let mut kick = with(
        Kick909::new(),
        &[
            (Kick909Voice::TUNE, 50.0),
            (Kick909Voice::SWEEP, 250.0),
            (Kick909Voice::SWEEP_TIME, 30.0),
            (Kick909Voice::CLICK, 0.0),
            (Kick909Voice::DRIVE, 0.0),
        ],
    );
    let out = play(&mut kick, &HIT, 1.0);
    let times = rising_crossings(&out.left);
    let periods: Vec<f64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(periods.len() > 10);
    // Every cycle is at least as long as the one before: the pitch only falls.
    for pair in periods.windows(2).take(15) {
        assert!(pair[1] >= pair[0] - 2.0e-5, "periods {:?}", &periods[..16]);
    }
    // Each cycle's frequency is the swept pitch at its midpoint:
    // tune + sweep * exp(-t / sweep time).
    for (k, period) in periods.iter().enumerate().take(14) {
        let mid = 0.5 * (times[k] + times[k + 1]);
        let expected = 50.0 + 250.0 * (-mid / 0.030).exp();
        let found = 1.0 / period;
        assert!(
            (found / expected - 1.0).abs() < 0.03,
            "cycle {k}: {found} Hz, expected {expected}"
        );
    }
    let first = 1.0 / periods[0];
    assert!(first > 200.0 && first < 300.0, "first cycle at {first} Hz");
    // No sweep: the first cycle is already at the tuning.
    let mut flat = with(
        Kick909::new(),
        &[
            (Kick909Voice::TUNE, 50.0),
            (Kick909Voice::SWEEP, 0.0),
            (Kick909Voice::CLICK, 0.0),
        ],
    );
    let out = play(&mut flat, &HIT, 0.5);
    let found = crossing_frequency(window(&out.left, 0.0, 0.2));
    assert!((found - 50.0).abs() < 0.5, "{found}");
}

#[test]
fn kick909_click_adds_attack_brightness_only() {
    let hit = [(0.05, 0.05, 36, 127)];
    let render = |click: f32| {
        let mut kick = with(Kick909::new(), &[(Kick909Voice::CLICK, click)]);
        play(&mut kick, &hit, 0.6)
    };
    let (dry, clicky) = (render(0.0), render(1.0));
    // What the click adds: a short burst of noise, not more bass.
    let added: Vec<f32> = clicky
        .left
        .iter()
        .zip(&dry.left)
        .map(|(a, b)| a - b)
        .collect();
    let burst = window(&added, 0.03, 0.09);
    assert!(peak(burst) > 0.05, "click peak {}", peak(burst));
    let bright = spectrum_band_energy(burst, RATE, 500.0, 8_000.0);
    let bass = spectrum_band_energy(burst, RATE, 20.0, 200.0);
    assert!(bright > 10.0 * bass, "{bright} vs {bass}");
    assert_null(
        window(&dry.left, 0.2, 0.6),
        window(&clicky.left, 0.2, 0.6),
        -80.0,
    );
}

#[test]
fn kicks_decay_to_silence_and_the_808_rings_longer() {
    let mut k909 = Kick909::new();
    let mut k808 = Kick808::new();
    let a = play(&mut k909, &HIT, 2.0);
    let b = play(&mut k808, &HIT, 2.0);
    assert_buffer_ok(&a);
    assert_buffer_ok(&b);
    assert!(peak(&a.left) > 0.4 && peak(&b.left) > 0.4);
    // Defaults: 0.45 s and 1.0 s to fall 60 dB.
    assert!(is_silent(window(&a.left, 0.8, 2.0), -90.0));
    assert!(is_silent(window(&b.left, 1.7, 2.0), -90.0));
    let at = |signal: &[f32]| rms_db(window(signal, 0.5, 0.6));
    assert!(
        at(&b.left) > at(&a.left) + 20.0,
        "{} vs {}",
        at(&b.left),
        at(&a.left)
    );
    let found = crossing_frequency(window(&b.left, 0.1, 0.7));
    assert!((found - 50.0).abs() < 0.5, "808 at {found} Hz");
    // The decay parameter is the time to fall 60 dB.
    let mut short = with(
        Kick808::new(),
        &[(Kick808Voice::DECAY, 0.3), (Kick808Voice::TONE, 0.0)],
    );
    let c = play(&mut short, &HIT, 1.0);
    let early = rms_db(window(&c.left, 0.03, 0.05));
    let later = rms_db(window(&c.left, 0.18, 0.20));
    assert!(
        (early - later - 30.0).abs() < 2.0,
        "fell {} dB in 150 ms",
        early - later
    );
}

#[test]
fn hats_are_bright_and_the_open_hat_rings_longer() {
    let mut hat = Hat909::new();
    let closed = play(&mut hat, &[(0.0, 0.05, 42, 127)], 1.5);
    let open = play(&mut hat, &[(0.0, 0.05, 46, 127)], 1.5);
    for out in [&closed, &open] {
        assert_buffer_ok(out);
        assert!(peak(&out.left) > 0.1, "peak {}", peak(&out.left));
        let high = spectrum_band_energy(&out.left, RATE, 5_000.0, 20_000.0);
        let low = spectrum_band_energy(&out.left, RATE, 20.0, 2_000.0);
        assert!(high > 50.0 * low, "high {high} low {low}");
    }
    assert!(is_silent(window(&closed.left, 0.15, 1.5), -70.0));
    assert!(rms_db(window(&open.left, 0.15, 0.25)) > -45.0);
    assert!(is_silent(window(&open.left, 1.0, 1.5), -90.0));
}

#[test]
fn a_closed_hat_chokes_an_open_hat() {
    let mut hat = Hat909::new();
    let open = play(&mut hat, &[(0.0, 0.05, 46, 127)], 1.0);
    let choked = play(&mut hat, &[(0.0, 0.05, 46, 127), (0.1, 0.05, 42, 100)], 1.0);
    // Identical until the closed hit.
    assert_null(
        window(&open.left, 0.0, 0.1),
        window(&choked.left, 0.0, 0.1),
        -120.0,
    );
    let ringing = rms_db(window(&open.left, 0.3, 0.4));
    let after = rms_db(window(&choked.left, 0.3, 0.4));
    assert!(ringing > -50.0, "open hat at {ringing} dB");
    assert!(
        after < ringing - 40.0,
        "choked {after} dB, open {ringing} dB"
    );
    // The other way round: an open hit after a closed one rings.
    let reopened = play(&mut hat, &[(0.0, 0.05, 42, 127), (0.1, 0.05, 46, 127)], 1.0);
    assert!(rms_db(window(&reopened.left, 0.3, 0.4)) > -50.0);
}

#[test]
fn snare_has_a_tuned_body_and_a_noise_top() {
    let mut snare = Snare909::new();
    let out = play(&mut snare, &HIT, 1.0);
    assert_buffer_ok(&out);
    assert!(peak(&out.left) > 0.2);
    let body = spectrum_band_energy(&out.left, RATE, 120.0, 400.0);
    let noise = spectrum_band_energy(&out.left, RATE, 2_000.0, 12_000.0);
    let sub = spectrum_band_energy(&out.left, RATE, 10.0, 80.0);
    assert!(body > 20.0 * sub, "body {body} sub {sub}");
    assert!(noise > 20.0 * sub, "noise {noise} sub {sub}");
    assert!(is_silent(window(&out.left, 0.6, 1.0), -80.0));
    // Snappy sets the noise; without it the body is all that is left.
    let mut dry = with(Snare909::new(), &[(Snare909Voice::SNAPPY, 0.0)]);
    let dry = play(&mut dry, &HIT, 1.0);
    let dry_noise = spectrum_band_energy(&dry.left, RATE, 2_000.0, 12_000.0);
    assert!(noise > 30.0 * dry_noise, "{noise} vs {dry_noise}");
    // Tune moves the body.
    let peak_hz = |tune: f32| {
        let mut snare = with(
            Snare909::new(),
            &[(Snare909Voice::TUNE, tune), (Snare909Voice::SNAPPY, 0.0)],
        );
        let out = play(&mut snare, &HIT, 0.5);
        fourfloor_testkit::spectrum(window(&out.left, 0.03, 0.2), RATE).peak_frequency()
    };
    let (low, high) = (peak_hz(150.0), peak_hz(260.0));
    assert!(
        (low - 150.0).abs() < 25.0 && (high - 260.0).abs() < 25.0,
        "{low} {high}"
    );
}

#[test]
fn clap_is_mid_band_noise_in_several_bursts() {
    let mut clap = with(Clap909::new(), &[(ClapVoice::SPREAD, 12.0)]);
    let out = play(&mut clap, &HIT, 1.0);
    assert_buffer_ok(&out);
    assert!(peak(&out.left) > 0.1);
    let mid = spectrum_band_energy(&out.left, RATE, 600.0, 4_000.0);
    let low = spectrum_band_energy(&out.left, RATE, 10.0, 250.0);
    assert!(mid > 100.0 * low, "mid {mid} low {low}");
    // Three bursts 12 ms apart, then the tail: the level right after each
    // burst starts is higher than just before it.
    for burst in 1..3 {
        let at = 0.012 * f64::from(burst);
        let before = rms(window(&out.left, at - 0.003, at - 0.0005));
        let after = rms(window(&out.left, at + 0.0005, at + 0.003));
        assert!(after > 1.3 * before, "burst {burst}: {before} then {after}");
    }
    assert!(is_silent(window(&out.left, 0.7, 1.0), -80.0));
}

#[test]
fn rim_is_short_and_cowbell_rings_at_its_two_pitches() {
    let mut rim = Rim909::new();
    let out = play(&mut rim, &HIT, 0.5);
    assert_buffer_ok(&out);
    assert!(peak(&out.left) > 0.1);
    assert!(is_silent(window(&out.left, 0.15, 0.5), -80.0));
    let _ = RimVoice::TUNE;

    let mut bell = Cowbell808::new();
    let out = play(&mut bell, &HIT, 1.5);
    assert_buffer_ok(&out);
    assert!(peak(&out.left) > 0.1);
    let body = window(&out.left, 0.02, 0.3);
    let [f1, f2] = Cowbell808Voice::FREQUENCIES;
    let around = |f: f32| spectrum_band_energy(body, RATE, f - 25.0, f + 25.0);
    let between = spectrum_band_energy(body, RATE, 620.0, 720.0);
    assert!(around(f1) > 10.0 * between, "{} vs {between}", around(f1));
    assert!(around(f2) > 10.0 * between, "{} vs {between}", around(f2));
    assert!(is_silent(window(&out.left, 1.0, 1.5), -80.0));
    // Tune scales both partials.
    let mut high = with(Cowbell808::new(), &[(Cowbell808Voice::TUNE, 1.5)]);
    let out = play(&mut high, &HIT, 0.5);
    let body = window(&out.left, 0.02, 0.3);
    let moved = spectrum_band_energy(body, RATE, f1 * 1.5 - 25.0, f1 * 1.5 + 25.0);
    let old = spectrum_band_energy(body, RATE, f1 - 25.0, f1 + 25.0);
    assert!(moved > 10.0 * old, "{moved} vs {old}");
}

#[test]
fn tom_plays_its_tuning_and_gm_pitches_ascend() {
    let mut tom = with(Tom909::new(), &[(TomVoice::TUNE, 140.0)]);
    let out = play(&mut tom, &[(0.0, 0.05, 60, 127)], 1.0);
    assert_buffer_ok(&out);
    let found = fourfloor_testkit::spectrum(window(&out.left, 0.08, 0.3), RATE).peak_frequency();
    assert!((found - 140.0).abs() < 8.0, "tom at {found} Hz");
    assert!(is_silent(window(&out.left, 0.8, 1.0), -80.0));

    let mut last = 0.0;
    for pitch in [41u8, 43, 45, 47, 48, 50] {
        let mut tom = Tom909::new();
        let out = play(&mut tom, &[(0.0, 0.05, pitch, 127)], 0.5);
        let found =
            fourfloor_testkit::spectrum(window(&out.left, 0.08, 0.3), RATE).peak_frequency();
        assert!(
            found > last + 4.0,
            "pitch {pitch} at {found} Hz after {last}"
        );
        last = found;
    }
}

#[test]
fn velocity_scales_level_and_zero_velocity_is_silent() {
    let mut kick = Kick909::new();
    let loud = play(&mut kick, &[(0.0, 0.05, 36, 127)], 0.5);
    let soft = play(&mut kick, &[(0.0, 0.05, 36, 64)], 0.5);
    let ratio = rms(&soft.left) / rms(&loud.left);
    assert!((ratio - 64.0 / 127.0).abs() < 0.01, "ratio {ratio}");
    let none = play(&mut kick, &[(0.0, 0.05, 36, 0)], 0.5);
    assert!(is_silent(&none.left, -140.0));
    let mut quiet = with(Kick909::new(), &[(Kick909Voice::LEVEL, 0.4)]);
    let half = play(&mut quiet, &[(0.0, 0.05, 36, 127)], 0.5);
    assert!((to_db(rms(&half.left) / rms(&loud.left)) + 6.02).abs() < 0.1);
}

#[test]
fn drums_render_the_same_at_any_block_size() {
    let events = notes(
        &[
            (0.0, 0.05, 36, 127),
            (0.1234, 0.05, 46, 90),
            (0.3, 0.05, 42, 110),
            (0.31, 0.05, 38, 100),
        ],
        RATE,
    );
    let render = |block: usize| {
        let mut kit = DrumKit::new();
        render_instrument(
            &mut kit,
            &events,
            1.0,
            &RenderConfig::default().with_block(block),
        )
    };
    let (a, b) = (render(512), render(37));
    assert_null(&a.left, &b.left, -140.0);
    assert_null(&a.right, &b.right, -140.0);
}

#[test]
fn kit_maps_gm_pitches_to_voices() {
    // Each kit pitch sounds exactly like the stand-alone voice at the
    // kit's level (0.8), since idle voices add nothing.
    let single = |device: &mut dyn Device, pitch: u8| play(device, &[(0.0, 0.05, pitch, 127)], 0.6);
    let kit_hit = |pitch: u8| {
        let mut kit = DrumKit::new();
        single(&mut kit, pitch)
    };
    let scaled = |signal: &[f32]| signal.iter().map(|x| 0.8 * x).collect::<Vec<f32>>();
    let cases: [(u8, Box<dyn Device>); 8] = [
        (36, Box::new(Kick909::new())),
        (35, Box::new(Kick808::new())),
        (38, Box::new(Snare909::new())),
        (40, Box::new(Snare909::new())),
        (39, Box::new(Clap909::new())),
        (37, Box::new(Rim909::new())),
        (56, Box::new(Cowbell808::new())),
        (46, Box::new(Hat909::new())),
    ];
    for (pitch, mut alone) in cases {
        let kit = kit_hit(pitch);
        assert_buffer_ok(&kit);
        assert!(peak(&kit.left) > 0.05, "pitch {pitch} is silent");
        let alone = single(alone.as_mut(), pitch);
        assert_null(&kit.left, &scaled(&alone.left), -100.0);
        assert!((stereo_correlation(&kit.left, &kit.right) - 1.0).abs() < 1e-6);
    }
    for pitch in [42u8, 44] {
        let mut hat = Hat909::new();
        assert_null(
            &kit_hit(pitch).left,
            &scaled(&single(&mut hat, 42).left),
            -100.0,
        );
    }
    // Pitches the kit has no voice for make no sound.
    for pitch in [0u8, 34, 51, 60, 127] {
        assert!(is_silent(&kit_hit(pitch).left, -140.0), "pitch {pitch}");
        assert_eq!(KitSlot::for_pitch(pitch), None);
    }
    // Toms ascend across the six GM pitches.
    let mut last = 0.0;
    for pitch in [41u8, 43, 45, 47, 48, 50] {
        let out = kit_hit(pitch);
        let found =
            fourfloor_testkit::spectrum(window(&out.left, 0.08, 0.3), RATE).peak_frequency();
        assert!(
            found > last + 4.0,
            "pitch {pitch} at {found} Hz after {last}"
        );
        last = found;
    }
}

#[test]
fn kit_plays_a_house_beat_on_time_with_choke_and_per_voice_params() {
    // Four on the floor at 120 BPM with off-beat open hats choked by
    // closed hats, and claps on two and four.
    let mut specs = Vec::new();
    for beat in 0..4 {
        let t = 0.5 * f64::from(beat);
        specs.push((t, 0.05, 36u8, 127u8));
        specs.push((t + 0.25, 0.05, 46, 100));
        specs.push((t + 0.375, 0.05, 42, 100));
        if beat % 2 == 1 {
            specs.push((t, 0.05, 39, 110));
        }
    }
    let mut kit = DrumKit::new();
    let out = play(&mut kit, &specs, 2.5);
    assert_buffer_ok(&out);
    // Low-passed view: only the kicks.
    let low: Vec<f32> = {
        let mut state = 0.0f32;
        out.left
            .iter()
            .map(|x| {
                state += 0.01 * (x - state);
                state
            })
            .collect()
    };
    let found = onset_times(&low, RATE);
    assert!(
        times_match(&found, &[0.0, 0.5, 1.0, 1.5], 0.01),
        "{found:?}"
    );
    // Hat band: loud after the open hat, gone soon after the closed one.
    let hat_band =
        |a: f64, b: f64| spectrum_band_energy(window(&out.left, a, b), RATE, 7_000.0, 16_000.0);
    assert!(
        hat_band(0.30, 0.37) > 100.0 * hat_band(0.47, 0.50),
        "{} {}",
        hat_band(0.30, 0.37),
        hat_band(0.47, 0.50)
    );

    // A kit parameter reaches the voice it names.
    let id = DrumKit::param(KitSlot::Kick909, Kick909Voice::TUNE);
    let info = kit.params().iter().find(|info| info.id == id).unwrap();
    assert_eq!(info.name, "Kick 909 Tune");
    kit.set_param(id, 70.0);
    kit.set_param(DrumKit::param(KitSlot::Kick909, Kick909Voice::CLICK), 0.0);
    let out = play(&mut kit, &HIT, 0.6);
    let found = crossing_frequency(window(&out.left, 0.18, 0.4));
    assert!((found - 70.0).abs() < 1.5, "{found}");
    // Master level.
    let before = rms(&out.left);
    kit.set_param(DrumKit::LEVEL, 0.4);
    let out = play(&mut kit, &HIT, 0.6);
    assert!((rms(&out.left) / before - 0.5).abs() < 0.01);
    let _ = HatVoice::OPEN_PITCH;
}
