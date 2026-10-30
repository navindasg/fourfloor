//! Rendered-audio tests for the compressor, the glue compressor, the
//! limiter and the pump.

mod common;

use common::*;
use fourfloor_core::{Device, DeviceId, ParamId, PrepareConfig, SampleRate, Tempo};
use fourfloor_effects::{Compressor, Limiter, Pump};
use fourfloor_engine::devices::{Gain, Mixer, SineOsc};
use fourfloor_engine::GraphBuilder;
use fourfloor_testkit::{
    assert_null, from_db, mix, onset_times, peak, peak_db, render_effect,
    render_effect_with_sidechain, render_graph, rms_db, rms_window_seconds, sine,
    spectrum_band_energy_db, times_match, white_noise, AllocDisabler, RenderConfig, StereoBuffer,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn compressor(params: &[(ParamId, f32)]) -> Compressor {
    let mut device = Compressor::new();
    set(&mut device, params);
    device
}

fn limiter(params: &[(ParamId, f32)]) -> Limiter {
    let mut device = Limiter::new();
    set(&mut device, params);
    device
}

fn pump(params: &[(ParamId, f32)]) -> Pump {
    let mut device = Pump::new();
    set(&mut device, params);
    device
}

/// A level that steps from `before` to `after` at `at` seconds: a constant
/// input makes the compressor's gain readable sample by sample.
fn step(before: f32, after: f32, at: f64, seconds: f64) -> Vec<f32> {
    let mut signal = vec![before; frames(seconds)];
    signal[frames(at)..].fill(after);
    signal
}

/// Gain reduction in dB (positive) at `seconds`, from a constant-level
/// render.
fn reduction_at(input: &[f32], output: &[f32], seconds: f64) -> f32 {
    let index = frames(seconds);
    -20.0 * (output[index] / input[index]).log10()
}

// -------------------------------------------------------- compressor

#[test]
fn compressor_leaves_quiet_signals_alone() {
    let input = dual_mono(&sine(1_000.0, from_db(-30.0), 0.5, RATE));
    let output = render_effect(&mut Compressor::new(), &input, &config());
    assert_eq!(output.left, input.left, "below the threshold: untouched");
}

#[test]
fn compressor_reduces_gain_by_the_ratio() {
    // A -6 dBFS sine over a -18 dB threshold is 12 dB over.
    for (ratio, expected) in [(2.0f32, 6.0f32), (4.0, 9.0), (20.0, 11.4)] {
        let mut device = compressor(&[
            (Compressor::THRESHOLD, -18.0),
            (Compressor::RATIO, ratio),
            (Compressor::KNEE, 0.0),
            (Compressor::ATTACK, 1.0),
            (Compressor::RELEASE, 200.0),
        ]);
        let reduction = -tone_gain_db_at_level(&mut device, 1_000.0, from_db(-6.0));
        assert!(
            (reduction - expected).abs() < 0.5,
            "{ratio}:1 gave {reduction} dB of reduction, expected {expected}"
        );
    }
}

#[test]
fn compressor_attack_time_is_the_time_constant_of_the_gain_change() {
    // From -26 dBFS (under the threshold) to -6 dBFS: 9 dB of reduction at
    // 4:1. After one attack time 63 % of it is there, after three 95 %.
    let input = step(0.05, from_db(-6.0), 0.5, 1.0);
    for attack_ms in [5.0f32, 20.0, 80.0] {
        let mut device = compressor(&[
            (Compressor::THRESHOLD, -18.0),
            (Compressor::RATIO, 4.0),
            (Compressor::KNEE, 0.0),
            (Compressor::ATTACK, attack_ms),
            (Compressor::RELEASE, 500.0),
        ]);
        let output = render_mono(&mut device, &input);
        let attack = f64::from(attack_ms) * 0.001;
        assert!(reduction_at(&input, &output, 0.499) < 0.01);
        let one = reduction_at(&input, &output, 0.5 + attack);
        let three = reduction_at(&input, &output, 0.5 + 3.0 * attack);
        assert!(
            (one - 9.0 * 0.632).abs() < 0.3,
            "{attack_ms} ms attack: {one} dB after one time constant"
        );
        assert!(
            (three - 9.0 * 0.950).abs() < 0.3,
            "{attack_ms} ms attack: {three} dB after three"
        );
    }
}

#[test]
fn compressor_release_time_is_the_time_constant_of_the_recovery() {
    let input = step(from_db(-6.0), 0.05, 0.5, 1.5);
    for release_ms in [50.0f32, 200.0] {
        let mut device = compressor(&[
            (Compressor::THRESHOLD, -18.0),
            (Compressor::RATIO, 4.0),
            (Compressor::KNEE, 0.0),
            (Compressor::ATTACK, 0.5),
            (Compressor::RELEASE, release_ms),
        ]);
        let output = render_mono(&mut device, &input);
        let release = f64::from(release_ms) * 0.001;
        let held = reduction_at(&input, &output, 0.499);
        assert!((held - 9.0).abs() < 0.1, "settled at {held} dB");
        let one = reduction_at(&input, &output, 0.5 + release);
        let three = reduction_at(&input, &output, 0.5 + 3.0 * release);
        assert!(
            (one - 9.0 * 0.368).abs() < 0.35,
            "{release_ms} ms release: {one} dB left after one time constant"
        );
        assert!(
            (three - 9.0 * 0.050).abs() < 0.2,
            "{release_ms} ms release: {three} dB left after three"
        );
    }
}

#[test]
fn compressor_soft_knee_starts_early_and_gently() {
    // 2 dB under the threshold: a hard knee does nothing, a 12 dB knee
    // gives slope * (over + knee/2)^2 / (2 * knee) = 0.75 * 16 / 24 dB.
    let level = from_db(-20.0);
    let input = vec![level; frames(0.5)];
    let reduction = |knee: f32| {
        let mut device = compressor(&[
            (Compressor::THRESHOLD, -18.0),
            (Compressor::RATIO, 4.0),
            (Compressor::KNEE, knee),
        ]);
        let output = render_mono(&mut device, &input);
        reduction_at(&input, &output, 0.45)
    };
    assert_eq!(reduction(0.0), 0.0);
    assert!((reduction(12.0) - 0.5).abs() < 0.05, "{}", reduction(12.0));
}

/// A pad to be ducked and a kick-like key that fires every half second.
fn pad_and_kick() -> (StereoBuffer, StereoBuffer, Vec<f64>) {
    let seconds = 2.0;
    let pad = sine(220.0, from_db(-12.0), seconds, RATE);
    let hits = vec![0.0, 0.5, 1.0, 1.5];
    let mut kick = vec![0.0f32; pad.len()];
    let burst = sine(60.0, 0.9, 0.1, RATE);
    for hit in &hits {
        let start = frames(*hit);
        kick[start..start + burst.len()].copy_from_slice(&burst);
    }
    (dual_mono(&pad), dual_mono(&kick), hits)
}

fn ducker() -> Compressor {
    // The pad alone peaks 2 dB under the threshold; only the key is over.
    compressor(&[
        (Compressor::THRESHOLD, -10.0),
        (Compressor::RATIO, 8.0),
        (Compressor::KNEE, 0.0),
        (Compressor::ATTACK, 1.0),
        (Compressor::RELEASE, 50.0),
    ])
}

#[test]
fn compressor_ducks_when_the_sidechain_key_fires() {
    let (pad, kick, hits) = pad_and_kick();
    let output = render_effect_with_sidechain(&mut ducker(), &pad, &kick, &config());
    let level = |buffer: &StereoBuffer, start: f64| {
        20.0 * rms_window_seconds(&buffer.left, RATE, start, 0.06).log10()
    };
    for hit in hits {
        // The key is 9 dB over the threshold: 8:1 takes 7.9 dB off.
        let ducked = level(&output, hit + 0.03) - level(&pad, hit + 0.03);
        let recovered = level(&output, hit + 0.42) - level(&pad, hit + 0.42);
        assert!(
            (-8.5..-6.5).contains(&ducked),
            "hit at {hit}: {ducked} dB while the key sounds"
        );
        assert!(
            recovered.abs() < 0.2,
            "hit at {hit}: {recovered} dB before the next one"
        );
    }
    // The key itself is never heard. (Ducking a 220 Hz pad this fast puts
    // real sidebands of the pad at the key's 60 Hz, so the proof is a render
    // with nothing on the main input: the key alone must give silence.)
    let nothing = StereoBuffer::silent(RATE, pad.frames());
    let keyed = render_effect_with_sidechain(&mut ducker(), &nothing, &kick, &config());
    assert!(keyed.left.iter().chain(&keyed.right).all(|s| *s == 0.0));
    // And far more of the output sits at the pad's pitch than at the key's.
    let at_key = spectrum_band_energy_db(&output.left, RATE, 50.0, 70.0);
    let at_pad = spectrum_band_energy_db(&output.left, RATE, 200.0, 240.0);
    assert!(
        at_pad - at_key > 30.0,
        "pad {at_pad} dB, key band {at_key} dB"
    );
}

#[test]
fn compressor_without_a_sidechain_or_with_the_key_switched_off_keys_from_its_input() {
    let (pad, kick, _) = pad_and_kick();
    // Nothing connected: the pad never reaches the threshold on its own.
    let alone = render_effect(&mut ducker(), &pad, &config());
    assert_eq!(alone.left, pad.left);
    // Connected, but External Key is off.
    let mut ignoring = ducker();
    ignoring.set_param(Compressor::EXTERNAL_KEY, 0.0);
    let output = render_effect_with_sidechain(&mut ignoring, &pad, &kick, &config());
    assert_eq!(output.left, pad.left);
}

#[test]
fn compressor_key_high_pass_keeps_bass_out_of_the_detector() {
    let reduction = |key_high_pass: f32| {
        let mut device = compressor(&[
            (Compressor::THRESHOLD, -18.0),
            (Compressor::KNEE, 0.0),
            (Compressor::KEY_HIGH_PASS, key_high_pass),
        ]);
        -tone_gain_db_at_level(&mut device, 50.0, from_db(-6.0))
    };
    assert!(reduction(20.0) > 8.0, "full band: {}", reduction(20.0));
    assert!(reduction(300.0) < 0.1, "filtered: {}", reduction(300.0));
}

#[test]
fn compressor_makeup_and_mix() {
    let quiet = from_db(-40.0);
    let mut makeup = compressor(&[(Compressor::MAKEUP, 6.0)]);
    let gain = tone_gain_db_at_level(&mut makeup, 1_000.0, quiet);
    assert!((gain - 6.0).abs() < 0.05, "makeup: {gain} dB");

    // Mix 0 is the dry signal, however hard the compressor works.
    let loud = dual_mono(&sine(1_000.0, 0.9, 0.5, RATE));
    let mut dry = compressor(&[(Compressor::THRESHOLD, -40.0), (Compressor::MIX, 0.0)]);
    assert_eq!(render_effect(&mut dry, &loud, &config()).left, loud.left);

    // Half way is parallel compression: less reduction than fully wet.
    let wet = -tone_gain_db_at_level(&mut compressor(&[(Compressor::KNEE, 0.0)]), 1_000.0, 0.9);
    let half = -tone_gain_db_at_level(
        &mut compressor(&[(Compressor::KNEE, 0.0), (Compressor::MIX, 0.5)]),
        1_000.0,
        0.9,
    );
    assert!(half > 1.0 && half < wet - 3.0, "{half} dB against {wet} dB");
}

#[test]
fn compressor_gain_is_linked_across_the_stereo_pair() {
    let left = sine(1_000.0, 0.9, 0.5, RATE);
    let right = sine(300.0, 0.05, 0.5, RATE);
    let input = StereoBuffer::new(RATE, left, right);
    let output = render_effect(&mut Compressor::new(), &input, &config());
    let gain = |out: &[f32], inp: &[f32]| rms_db(&out[12_000..]) - rms_db(&inp[12_000..]);
    let (left_gain, right_gain) = (
        gain(&output.left, &input.left),
        gain(&output.right, &input.right),
    );
    assert!(left_gain < -6.0, "the loud side is compressed: {left_gain}");
    assert!(
        (left_gain - right_gain).abs() < 0.2,
        "the quiet side gets the same gain: {left_gain} against {right_gain}"
    );
}

#[test]
fn compressor_reports_its_gain_reduction() {
    let mut device = compressor(&[(Compressor::KNEE, 0.0)]);
    assert_eq!(device.gain_reduction_db(), 0.0);
    render_mono(&mut device, &sine(1_000.0, from_db(-6.0), 0.5, RATE));
    assert!((device.gain_reduction_db() - 9.0).abs() < 0.5);
    device.reset();
    assert_eq!(device.gain_reduction_db(), 0.0);
}

// -------------------------------------------------------------- glue

#[test]
fn glue_compressor_is_gentle_and_ignores_the_kick() {
    // RMS detection: a -4 dBFS sine measures -7 dB, 9 dB over the -16 dB
    // threshold; 2:1 takes half of that off.
    let mut device = Compressor::glue();
    let mids = -tone_gain_db_at_level(&mut device, 1_000.0, from_db(-4.0));
    assert!((mids - 4.5).abs() < 0.5, "{mids} dB on a 1 kHz tone");
    // The same level at 50 Hz sits under the 90 Hz key high-pass.
    let bass = -tone_gain_db_at_level(&mut Compressor::glue(), 50.0, from_db(-4.0));
    assert!(bass < 1.0, "{bass} dB on a 50 Hz tone");
    // The plain compressor at the same settings would clamp down on it.
    let mut plain = compressor(&[(Compressor::THRESHOLD, -16.0), (Compressor::RATIO, 2.0)]);
    assert!(-tone_gain_db_at_level(&mut plain, 50.0, from_db(-4.0)) > 4.0);
}

#[test]
fn glue_compressor_narrows_the_dynamic_range_of_a_mix() {
    // Loud and quiet halves, 12 dB apart.
    let loud = sine(800.0, from_db(-3.0), 1.0, RATE);
    let quiet = sine(800.0, from_db(-15.0), 1.0, RATE);
    let input: Vec<f32> = loud.iter().chain(quiet.iter()).copied().collect();
    let output = render_mono(&mut Compressor::glue(), &input);
    let range = |signal: &[f32]| {
        20.0 * (rms_window_seconds(signal, RATE, 0.5, 0.4)
            / rms_window_seconds(signal, RATE, 1.5, 0.4))
        .log10()
    };
    assert!((range(&input) - 12.0).abs() < 0.1);
    let squeezed = range(&output);
    assert!(
        (7.0..10.5).contains(&squeezed),
        "12 dB of range became {squeezed} dB"
    );
}

// ----------------------------------------------------------- limiter

/// Peaks far over full scale: noise, a loud tone and single-sample spikes.
fn hot_signal() -> StereoBuffer {
    let seconds = 2.0;
    let mut left = mix(&[
        &white_noise(21, 1.5, seconds, RATE),
        &sine(55.0, 2.0, seconds, RATE),
    ]);
    let mut right = mix(&[
        &white_noise(22, 0.7, seconds, RATE),
        &sine(3_000.0, 1.0, seconds, RATE),
    ]);
    for spike in [4_000usize, 30_000, 30_001, 61_234, 90_000] {
        left[spike] = 6.0;
        right[spike + 7] = -5.0;
    }
    StereoBuffer::new(RATE, left, right)
}

#[test]
fn limiter_never_exceeds_its_ceiling() {
    let input = hot_signal();
    for (ceiling_db, block) in [(-0.3f32, 512usize), (-1.0, 64), (-6.0, 37), (0.0, 1024)] {
        let mut device = limiter(&[(Limiter::CEILING, ceiling_db)]);
        let output = render_effect(&mut device, &input, &config().with_block(block));
        let ceiling = from_db(ceiling_db);
        let loudest = peak(&output.left).max(peak(&output.right));
        assert!(
            loudest <= ceiling,
            "ceiling {ceiling_db} dB ({ceiling}), block {block}: peak {loudest}"
        );
        // It limits rather than turning everything down.
        assert!(
            peak_db(&output.left) > ceiling_db - 0.5,
            "the output still reaches the ceiling: {}",
            peak_db(&output.left)
        );
    }
}

#[test]
fn limiter_turns_the_gain_down_instead_of_clipping() {
    // A sine 12 dB over the ceiling must come out as a sine at the ceiling.
    // A clipper would add strong odd harmonics; a limiter that works by
    // gain does not.
    let input = dual_mono(&sine(1_000.0, from_db(11.0), 1.0, RATE));
    let mut device = limiter(&[(Limiter::CEILING, -1.0)]);
    let output = render_effect(&mut device, &input, &config());
    let settled = &output.left[24_000..];
    assert!(
        (peak_db(settled) + 1.0).abs() < 0.05,
        "{}",
        peak_db(settled)
    );
    let fundamental = spectrum_band_energy_db(settled, RATE, 900.0, 1_100.0);
    let third = spectrum_band_energy_db(settled, RATE, 2_900.0, 3_100.0);
    let fifth = spectrum_band_energy_db(settled, RATE, 4_900.0, 5_100.0);
    assert!(
        third - fundamental < -60.0 && fifth - fundamental < -60.0,
        "harmonics at {} and {} dB relative to the tone",
        third - fundamental,
        fifth - fundamental
    );
}

#[test]
fn limiter_reports_its_lookahead_as_latency() {
    let mut device = Limiter::new();
    device.prepare(&PrepareConfig::stereo(RATE, 512));
    assert_eq!(device.latency_samples(), 240, "5 ms at 48 kHz");
    device.prepare(&PrepareConfig::stereo(SampleRate::HZ_96000, 512));
    assert_eq!(device.latency_samples(), 480, "5 ms at 96 kHz");

    let mut short = limiter(&[(Limiter::LOOKAHEAD, 2.0)]);
    short.prepare(&PrepareConfig::stereo(RATE, 512));
    assert_eq!(short.latency_samples(), 96, "set before prepare: 2 ms");
    // A change after prepare waits for the next prepare, so the latency
    // the engine compensated for stays true.
    short.set_param(Limiter::LOOKAHEAD, 10.0);
    assert_eq!(short.latency_samples(), 96);
    short.prepare(&PrepareConfig::stereo(RATE, 512));
    assert_eq!(short.latency_samples(), 480);
}

#[test]
fn limiter_output_is_the_input_delayed_by_exactly_the_reported_latency() {
    // Under the ceiling nothing is limited, so the output must null
    // against the input shifted by the latency.
    let left = mix(&[
        &sine(220.0, 0.3, 1.0, RATE),
        &white_noise(31, 0.3, 1.0, RATE),
    ]);
    let right = sine(1_234.0, 0.5, 1.0, RATE);
    let input = StereoBuffer::new(RATE, left, right);
    for lookahead_ms in [0.5f32, 5.0, 10.0] {
        let mut device = limiter(&[(Limiter::LOOKAHEAD, lookahead_ms)]);
        let output = render_effect(&mut device, &input, &config().with_block(100));
        let latency = device.latency_samples() as usize;
        assert_eq!(latency, frames(f64::from(lookahead_ms) * 0.001));
        let n = input.frames() - latency;
        assert_eq!(&output.left[latency..], &input.left[..n]);
        assert_eq!(&output.right[latency..], &input.right[..n]);
        assert!(output.left[..latency].iter().all(|s| *s == 0.0));
        // One sample either way does not null: the alignment is exact.
        assert!(!fourfloor_testkit::null_test(
            &output.left[latency + 1..],
            &input.left[..n - 1],
            -20.0
        ));
    }
}

#[test]
fn limiter_latency_is_compensated_by_the_engine() {
    // The delay-compensation null test from the build plan: a tone through
    // the limiter, plus the same tone inverted on a parallel path. With the
    // reported latency compensated they cancel.
    let build = |compensate: bool| {
        let (source, limited, inverted, sum) = (DeviceId(1), DeviceId(2), DeviceId(3), DeviceId(4));
        let mut graph = GraphBuilder::stereo();
        graph.set_delay_compensation(compensate);
        graph
            .add_node(source, Box::new(SineOsc::new(440.0, 0.5)), 0)
            .expect("source");
        graph
            .add_node(limited, Box::new(Limiter::new()), 1)
            .expect("limiter");
        graph
            .add_node(inverted, Box::new(Gain::new(-1.0)), 1)
            .expect("inverter");
        graph
            .add_node(sum, Box::new(Mixer::new(2)), 2)
            .expect("mixer");
        graph.connect(source, limited, 0).expect("edge");
        graph.connect(source, inverted, 0).expect("edge");
        graph.connect(limited, sum, 0).expect("edge");
        graph.connect(inverted, sum, 1).expect("edge");
        graph.set_output(sum).expect("output");
        graph.compile(RATE, 256).expect("compile")
    };
    let graph = build(true);
    assert_eq!(graph.latency_samples(), 240);
    let output = render_graph(graph, Tempo::DEFAULT, 0.5).expect("render");
    // Skip the first 5 ms, where the limiter's own delay line is filling.
    let settled = &output.left[480..];
    assert!(
        peak(settled) < 1.0e-6,
        "compensated paths cancel: residual {}",
        peak_db(settled)
    );
    // Without compensation the two paths are 240 samples apart and do not.
    let output = render_graph(build(false), Tempo::DEFAULT, 0.5).expect("render");
    assert!(peak(&output.left[480..]) > 0.1);
}

#[test]
fn limiter_gain_moves_smoothly_before_a_peak_and_recovers_after() {
    // A constant level with one huge sample: the gain is readable directly.
    let level = 0.5f32;
    let spike_at = 24_000usize;
    let mut signal = vec![level; frames(1.5)];
    signal[spike_at] = 4.0;
    let mut device = limiter(&[(Limiter::CEILING, -0.3), (Limiter::RELEASE, 50.0)]);
    let output = render_mono(&mut device, &signal);
    let latency = device.latency_samples() as usize;
    let ceiling = from_db(-0.3);
    let landed = spike_at + latency;

    // The spike comes out exactly at the ceiling: caught, not over-limited.
    assert!(
        (output[landed] - ceiling).abs() < 1.0e-4,
        "{}",
        output[landed]
    );
    // Before it, the gain glides down over the lookahead: no jump.
    let largest_step = (landed - latency..landed)
        .map(|n| (output[n] - output[n - 1]).abs())
        .fold(0.0f32, f32::max);
    assert!(largest_step < 0.005, "largest step {largest_step}");
    assert!((output[landed - latency - 1] - level).abs() < 1.0e-6);
    assert!(output[landed - 1] < level * 0.3, "{}", output[landed - 1]);
    // After it, the gain comes back within a few release times.
    assert!(output[landed + frames(0.02)] < level * 0.9);
    assert!((output[landed + frames(0.5)] - level).abs() < 0.005);
    assert!(peak(&output) <= ceiling);
}

#[test]
fn limiter_input_gain_makes_things_louder_up_to_the_ceiling() {
    let input = dual_mono(&sine(200.0, 0.1, 1.0, RATE));
    let mut device = limiter(&[(Limiter::INPUT_GAIN, 12.0), (Limiter::CEILING, -1.0)]);
    let output = render_effect(&mut device, &input, &config());
    // 0.1 * 12 dB = 0.398: still under the ceiling, so simply louder.
    let gain = rms_db(&output.left[24_000..]) - rms_db(&input.left[24_000..]);
    assert!((gain - 12.0).abs() < 0.05, "{gain} dB");

    let mut driven = limiter(&[(Limiter::INPUT_GAIN, 36.0), (Limiter::CEILING, -1.0)]);
    let output = render_effect(&mut driven, &input, &config());
    assert!(peak(&output.left) <= from_db(-1.0));
    assert!(peak_db(&output.left[24_000..]) > -1.2);
}

// -------------------------------------------------------------- pump

/// The pump's gain over time: the output for a constant input of 1.
fn pump_gain(device: &mut Pump, seconds: f64, config: &RenderConfig) -> Vec<f32> {
    let input = dual_mono(&vec![1.0f32; frames(seconds)]);
    render_effect(device, &input, config).left
}

/// Where the gain dips: onsets of the amount of ducking.
fn dip_times(gain: &[f32]) -> Vec<f64> {
    let ducking: Vec<f32> = gain.iter().map(|g| 1.0 - g).collect();
    onset_times(&ducking, RATE)
}

#[test]
fn pump_ducks_on_every_beat_by_its_depth() {
    for depth in [0.8f32, 0.4] {
        let mut device = pump(&[(Pump::DEPTH, depth)]);
        let gain = pump_gain(&mut device, 3.0, &config());
        for beat in 0..6 {
            let start = frames(f64::from(beat) * 0.5);
            let cycle = &gain[start..start + frames(0.5)];
            let lowest = cycle.iter().copied().fold(f32::INFINITY, f32::min);
            let floor = 1.0 - depth;
            assert!(
                lowest >= floor - 1.0e-3 && lowest < floor + 0.1 * depth,
                "depth {depth}, beat {beat}: lowest gain {lowest}"
            );
            // Back at unity for the last part of the beat.
            assert!(cycle[frames(0.35)..frames(0.49)].iter().all(|g| *g > 0.999));
        }
    }
}

#[test]
fn pump_dips_land_on_the_beat_at_any_tempo() {
    for bpm in [100.0, 124.0, 128.0, 174.0] {
        let gain = pump_gain(&mut Pump::new(), 3.0, &at_bpm(bpm));
        let beat = 60.0 / bpm;
        let expected: Vec<f64> = (0..(3.0 / beat).ceil() as usize)
            .map(|k| k as f64 * beat)
            .filter(|t| *t < 3.0 - 0.01)
            .collect();
        let found = dip_times(&gain);
        assert!(
            times_match(&found, &expected, 0.004),
            "at {bpm} BPM dips at {found:?}, expected {expected:?}"
        );
    }
}

#[test]
fn pump_rate_is_in_beats() {
    for (rate, period) in [(0.5f32, 0.25f64), (2.0, 1.0), (4.0, 2.0)] {
        let gain = pump_gain(&mut pump(&[(Pump::RATE, rate)]), 4.0, &config());
        let expected: Vec<f64> = (0..(4.0 / period) as usize)
            .map(|k| k as f64 * period)
            .collect();
        let found = dip_times(&gain);
        assert!(
            times_match(&found, &expected, 0.004),
            "rate {rate}: dips at {found:?}"
        );
    }
}

#[test]
fn pump_gain_follows_the_documented_shape() {
    let (depth, hold, release, curve) = (0.7f32, 0.1f32, 0.5f32, 2.0f32);
    let mut device = pump(&[
        (Pump::DEPTH, depth),
        (Pump::HOLD, hold),
        (Pump::RELEASE, release),
        (Pump::CURVE, curve),
        (Pump::SMOOTH, 0.5),
    ]);
    let gain = pump_gain(&mut device, 2.0, &config());
    for beat in 1..4 {
        for phase in [0.02f32, 0.08, 0.15, 0.25, 0.35, 0.5, 0.59, 0.7, 0.95] {
            let time = (f64::from(beat) + f64::from(phase)) * 0.5;
            let expected = Pump::shape(phase, depth, hold, release, curve);
            let actual = gain[frames(time)];
            assert!(
                (actual - expected).abs() < 0.01,
                "beat {beat}, phase {phase}: gain {actual}, shape {expected}"
            );
        }
    }
    // Hold keeps it fully down; with depth 1 that is silence.
    let silent = pump_gain(
        &mut pump(&[(Pump::DEPTH, 1.0), (Pump::HOLD, 0.2)]),
        1.0,
        &config(),
    );
    assert!(silent[frames(0.55)..frames(0.59)]
        .iter()
        .all(|g| *g < 1.0e-3));
}

#[test]
fn pump_phase_moves_the_dip_and_depth_zero_is_transparent() {
    // Half a cycle of phase puts the dip on the off-beat.
    let gain = pump_gain(&mut pump(&[(Pump::PHASE, 0.5)]), 2.0, &config());
    let found = dip_times(&gain);
    assert!(
        times_match(&found, &[0.25, 0.75, 1.25, 1.75], 0.004),
        "{found:?}"
    );

    let input = dual_mono(&white_noise(41, 0.5, 0.5, RATE));
    let output = render_effect(&mut pump(&[(Pump::DEPTH, 0.0)]), &input, &config());
    assert_eq!(output.left, input.left);
}

#[test]
fn pump_ducks_audio_under_a_kick_pattern() {
    // What it is for: a sustained bass that gets out of the way on each
    // beat, measured on audio rather than on a constant.
    let bass = dual_mono(&sine(55.0, 0.5, 2.0, RATE));
    let output = render_effect(&mut Pump::new(), &bass, &at_bpm(125.0));
    let beat = 60.0 / 125.0;
    for k in 0..4 {
        let start = f64::from(k) * beat;
        let on_beat = rms_window_seconds(&output.left, RATE, start + 0.005, 0.04);
        let off_beat = rms_window_seconds(&output.left, RATE, start + 0.7 * beat, 0.08);
        let ducked = 20.0 * (on_beat / off_beat).log10();
        assert!(
            (-14.0..-8.0).contains(&ducked),
            "beat {k}: {ducked} dB under the level between beats"
        );
    }
    assert_null(
        &output.left[frames(0.7 * beat)..frames(0.95 * beat)],
        &bass.left[frames(0.7 * beat)..frames(0.95 * beat)],
        -80.0,
    );
}

#[test]
fn pump_keeps_pumping_while_the_transport_is_stopped() {
    let gain = pump_gain(&mut Pump::new(), 2.0, &RenderConfig::default().stopped());
    let found = dip_times(&gain);
    assert!(
        times_match(&found, &[0.0, 0.5, 1.0, 1.5], 0.004),
        "{found:?}"
    );
}
