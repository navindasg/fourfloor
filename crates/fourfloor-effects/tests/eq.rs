//! Rendered-audio tests for the equaliser, the isolator and the filter.

mod common;

use common::*;
use fourfloor_core::{Device, Tempo};
use fourfloor_effects::{Filter, Isolator, ParametricEq};
use fourfloor_testkit::{
    assert_buffer_ok, render_effect, rms_window_seconds, white_noise, AllocDisabler, RenderConfig,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn eq(params: &[(fourfloor_core::ParamId, f32)]) -> ParametricEq {
    let mut device = ParametricEq::new();
    set(&mut device, params);
    device
}

fn isolator(params: &[(fourfloor_core::ParamId, f32)]) -> Isolator {
    let mut device = Isolator::new();
    set(&mut device, params);
    device
}

fn filter(params: &[(fourfloor_core::ParamId, f32)]) -> Filter {
    let mut device = Filter::new();
    set(&mut device, params);
    device
}

#[track_caller]
fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} dB, expected {expected} +/- {tolerance}"
    );
}

// ---------------------------------------------------------------- EQ

#[test]
fn eq_at_its_defaults_passes_audio_bit_for_bit() {
    let input = dual_mono(&white_noise(1, 0.5, 0.5, RATE));
    let output = render_effect(&mut ParametricEq::new(), &input, &config());
    assert_eq!(output.left, input.left);
    assert_eq!(output.right, input.right);
}

#[test]
fn eq_band_changes_its_own_frequency_and_leaves_the_rest_alone() {
    for gain in [12.0f32, -12.0, 6.0] {
        let params = [
            (ParametricEq::band_freq(1), 1_000.0),
            (ParametricEq::band_gain(1), gain),
            (ParametricEq::band_q(1), 2.0),
        ];
        assert_close(
            tone_gain_db(&mut eq(&params), 1_000.0),
            gain,
            0.2,
            "at the band",
        );
        assert_close(
            tone_gain_db(&mut eq(&params), 100.0),
            0.0,
            0.3,
            "a decade below",
        );
        assert_close(
            tone_gain_db(&mut eq(&params), 10_000.0),
            0.0,
            0.3,
            "a decade above",
        );
    }
}

#[test]
fn eq_bands_are_independent_and_add_up() {
    let params = [
        (ParametricEq::band_freq(0), 80.0),
        (ParametricEq::band_gain(0), 6.0),
        (ParametricEq::band_q(0), 3.0),
        (ParametricEq::band_freq(1), 600.0),
        (ParametricEq::band_gain(1), -9.0),
        (ParametricEq::band_q(1), 3.0),
        (ParametricEq::band_freq(2), 3_000.0),
        (ParametricEq::band_gain(2), 4.0),
        (ParametricEq::band_q(2), 3.0),
        (ParametricEq::band_freq(3), 12_000.0),
        (ParametricEq::band_gain(3), -5.0),
        (ParametricEq::band_q(3), 3.0),
    ];
    for (hz, expected) in [(80.0, 6.0), (600.0, -9.0), (3_000.0, 4.0), (12_000.0, -5.0)] {
        assert_close(tone_gain_db(&mut eq(&params), hz), expected, 0.4, "band");
    }
}

#[test]
fn eq_q_sets_the_width_of_a_band() {
    let band = |q: f32| {
        eq(&[
            (ParametricEq::band_freq(1), 1_000.0),
            (ParametricEq::band_gain(1), 12.0),
            (ParametricEq::band_q(1), q),
        ])
    };
    let wide = tone_gain_db(&mut band(0.5), 2_000.0);
    let narrow = tone_gain_db(&mut band(8.0), 2_000.0);
    assert!(wide > 6.0, "a wide bell still boosts an octave up: {wide}");
    assert!(narrow < 1.0, "a narrow bell does not: {narrow}");
}

#[test]
fn eq_outer_bands_can_be_shelves() {
    let low = [
        (ParametricEq::BAND1_SHELF, 1.0),
        (ParametricEq::band_freq(0), 200.0),
        (ParametricEq::band_gain(0), 6.0),
    ];
    assert_close(
        tone_gain_db(&mut eq(&low), 30.0),
        6.0,
        0.4,
        "below the shelf",
    );
    assert_close(
        tone_gain_db(&mut eq(&low), 5_000.0),
        0.0,
        0.3,
        "above the shelf",
    );

    let high = [
        (ParametricEq::BAND4_SHELF, 1.0),
        (ParametricEq::band_freq(3), 4_000.0),
        (ParametricEq::band_gain(3), -6.0),
    ];
    assert_close(
        tone_gain_db(&mut eq(&high), 16_000.0),
        -6.0,
        0.4,
        "above the shelf",
    );
    assert_close(
        tone_gain_db(&mut eq(&high), 200.0),
        0.0,
        0.3,
        "below the shelf",
    );
}

#[test]
fn eq_high_pass_and_low_pass_fall_at_12_db_per_octave() {
    let high_pass = [(ParametricEq::HP_ON, 1.0), (ParametricEq::HP_FREQ, 400.0)];
    assert_close(
        tone_gain_db(&mut eq(&high_pass), 400.0),
        -3.0,
        0.3,
        "corner",
    );
    let (at_100, at_50) = (
        tone_gain_db(&mut eq(&high_pass), 100.0),
        tone_gain_db(&mut eq(&high_pass), 50.0),
    );
    assert_close(at_100, -24.0, 1.0, "two octaves down");
    assert_close(at_100 - at_50, 12.0, 1.0, "slope per octave");
    assert_close(
        tone_gain_db(&mut eq(&high_pass), 5_000.0),
        0.0,
        0.2,
        "passband",
    );

    let low_pass = [(ParametricEq::LP_ON, 1.0), (ParametricEq::LP_FREQ, 1_000.0)];
    assert_close(
        tone_gain_db(&mut eq(&low_pass), 1_000.0),
        -3.0,
        0.3,
        "corner",
    );
    let (at_2k, at_4k) = (
        tone_gain_db(&mut eq(&low_pass), 2_000.0),
        tone_gain_db(&mut eq(&low_pass), 4_000.0),
    );
    assert_close(at_2k - at_4k, 12.0, 1.0, "slope per octave");
    assert_close(
        tone_gain_db(&mut eq(&low_pass), 100.0),
        0.0,
        0.2,
        "passband",
    );
}

#[test]
fn eq_output_gain_is_in_decibels() {
    let params = [(ParametricEq::OUTPUT, -6.0)];
    assert_close(
        tone_gain_db(&mut eq(&params), 1_000.0),
        -6.0,
        0.05,
        "output",
    );
}

// ---------------------------------------------------------- isolator

#[test]
fn isolator_is_flat_with_all_bands_at_unity() {
    for hz in [
        40.0, 120.0, 250.0, 600.0, 1_500.0, 2_500.0, 6_000.0, 15_000.0,
    ] {
        assert_close(tone_gain_db(&mut Isolator::new(), hz), 0.0, 0.1, "flat sum");
    }
}

#[test]
fn isolator_kills_one_band_and_keeps_the_others() {
    let kill = Isolator::KILL_DB;
    let no_low = [(Isolator::LOW, kill)];
    assert!(tone_gain_db(&mut isolator(&no_low), 60.0) < -40.0);
    assert_close(
        tone_gain_db(&mut isolator(&no_low), 1_000.0),
        0.0,
        0.5,
        "mid",
    );
    assert_close(
        tone_gain_db(&mut isolator(&no_low), 9_000.0),
        0.0,
        0.5,
        "high",
    );

    let no_mid = [(Isolator::MID, kill)];
    assert!(tone_gain_db(&mut isolator(&no_mid), 800.0) < -30.0);
    assert_close(tone_gain_db(&mut isolator(&no_mid), 50.0), 0.0, 0.5, "low");
    assert_close(
        tone_gain_db(&mut isolator(&no_mid), 12_000.0),
        0.0,
        0.5,
        "high",
    );

    let no_high = [(Isolator::HIGH, kill)];
    assert!(tone_gain_db(&mut isolator(&no_high), 12_000.0) < -40.0);
    assert_close(tone_gain_db(&mut isolator(&no_high), 60.0), 0.0, 0.5, "low");
}

#[test]
fn isolator_with_every_band_killed_is_silent() {
    let kill = Isolator::KILL_DB;
    let mut device = isolator(&[
        (Isolator::LOW, kill),
        (Isolator::MID, kill),
        (Isolator::HIGH, kill),
    ]);
    let output = render_mono(&mut device, &white_noise(3, 0.8, 0.5, RATE));
    assert!(output.iter().all(|sample| *sample == 0.0));
}

#[test]
fn isolator_crossovers_fall_at_24_db_per_octave() {
    let kill = Isolator::KILL_DB;
    let only_low = [(Isolator::MID, kill), (Isolator::HIGH, kill)];
    assert_close(
        tone_gain_db(&mut isolator(&only_low), 250.0),
        -6.0,
        0.3,
        "crossover point",
    );
    let (at_1k, at_2k) = (
        tone_gain_db(&mut isolator(&only_low), 1_000.0),
        tone_gain_db(&mut isolator(&only_low), 2_000.0),
    );
    assert_close(at_1k - at_2k, 24.0, 1.5, "low band slope");

    let only_high = [(Isolator::LOW, kill), (Isolator::MID, kill)];
    // Measured well above the low crossover, which also shapes this band.
    let (at_1200, at_600) = (
        tone_gain_db(&mut isolator(&only_high), 1_200.0),
        tone_gain_db(&mut isolator(&only_high), 600.0),
    );
    assert_close(at_1200 - at_600, 24.0, 1.5, "high band slope");
}

#[test]
fn isolator_bands_boost_and_crossovers_move() {
    let boost = [(Isolator::HIGH, 6.0)];
    assert_close(
        tone_gain_db(&mut isolator(&boost), 10_000.0),
        6.0,
        0.3,
        "boost",
    );
    assert_close(tone_gain_db(&mut isolator(&boost), 100.0), 0.0, 0.3, "low");

    // With the low crossover at 800 Hz, 400 Hz belongs to the low band.
    let moved = [
        (Isolator::LOW, Isolator::KILL_DB),
        (Isolator::LOW_CROSSOVER, 800.0),
    ];
    assert!(tone_gain_db(&mut isolator(&moved), 400.0) < -20.0);
    assert!(tone_gain_db(&mut isolator(&[(Isolator::LOW, Isolator::KILL_DB)]), 400.0) > -3.0);
}

// ------------------------------------------------------------ filter

#[test]
fn filter_low_pass_slope_is_12_or_24_db_per_octave() {
    let gentle = [(Filter::CUTOFF, 500.0), (Filter::RESONANCE, 0.0)];
    let (at_2k, at_4k) = (
        tone_gain_db(&mut filter(&gentle), 2_000.0),
        tone_gain_db(&mut filter(&gentle), 4_000.0),
    );
    assert_close(at_2k - at_4k, 12.0, 1.0, "12 dB slope");
    assert_close(
        tone_gain_db(&mut filter(&gentle), 50.0),
        0.0,
        0.2,
        "passband",
    );

    let steep = [
        (Filter::CUTOFF, 500.0),
        (Filter::RESONANCE, 0.0),
        (Filter::STEEP, 1.0),
    ];
    let (at_2k, at_4k) = (
        tone_gain_db(&mut filter(&steep), 2_000.0),
        tone_gain_db(&mut filter(&steep), 4_000.0),
    );
    assert_close(at_2k - at_4k, 24.0, 2.0, "24 dB slope");
}

#[test]
fn filter_high_pass_and_band_pass_modes() {
    let high_pass = [
        (Filter::MODE, Filter::HIGH_PASS as f32),
        (Filter::CUTOFF, 2_000.0),
        (Filter::RESONANCE, 0.0),
    ];
    let (at_500, at_250) = (
        tone_gain_db(&mut filter(&high_pass), 500.0),
        tone_gain_db(&mut filter(&high_pass), 250.0),
    );
    assert_close(at_500 - at_250, 12.0, 1.0, "high-pass slope");
    assert_close(
        tone_gain_db(&mut filter(&high_pass), 15_000.0),
        0.0,
        0.5,
        "passband",
    );

    let band_pass = [
        (Filter::MODE, Filter::BAND_PASS as f32),
        (Filter::CUTOFF, 1_000.0),
        (Filter::RESONANCE, 0.8),
    ];
    assert_close(
        tone_gain_db(&mut filter(&band_pass), 1_000.0),
        0.0,
        0.5,
        "centre",
    );
    assert!(tone_gain_db(&mut filter(&band_pass), 100.0) < -20.0);
    assert!(tone_gain_db(&mut filter(&band_pass), 10_000.0) < -20.0);
}

#[test]
fn filter_resonance_raises_a_peak_at_the_cutoff() {
    let at_cutoff = |resonance: f32| {
        tone_gain_db(
            &mut filter(&[(Filter::CUTOFF, 1_000.0), (Filter::RESONANCE, resonance)]),
            1_000.0,
        )
    };
    assert_close(at_cutoff(0.0), -6.0, 0.4, "no resonance");
    let (some, lots) = (at_cutoff(0.5), at_cutoff(0.9));
    assert!(some > -1.0 && lots > some + 6.0, "{some} then {lots}");
    assert!(lots > 10.0, "a strong peak: {lots}");
}

/// Level of filtered noise around the moments the LFO is at its top and
/// its bottom, for the first few cycles.
fn lfo_levels(output: &[f32], period_seconds: f64, cycles: usize) -> Vec<(f32, f32)> {
    (0..cycles)
        .map(|cycle| {
            let start = cycle as f64 * period_seconds;
            let window = period_seconds * 0.1;
            let top = start + 0.25 * period_seconds - window / 2.0;
            let bottom = start + 0.75 * period_seconds - window / 2.0;
            (
                rms_window_seconds(output, RATE, top, window),
                rms_window_seconds(output, RATE, bottom, window),
            )
        })
        .collect()
}

#[test]
fn filter_lfo_follows_the_tempo() {
    let noise = dual_mono(&white_noise(7, 0.5, 4.0, RATE));
    for bpm in [120.0, 140.0] {
        let mut device = filter(&[
            (Filter::CUTOFF, 1_000.0),
            (Filter::RESONANCE, 0.0),
            (Filter::LFO_DEPTH, 2.0),
            (Filter::LFO_BEATS, 1.0),
        ]);
        let output = render_effect(&mut device, &noise, &at_bpm(bpm));
        assert_buffer_ok(&output);
        let beat = 60.0 / bpm;
        // A sine LFO peaks a quarter of the way through each beat (cutoff
        // 4 kHz) and bottoms out at three quarters (250 Hz).
        for (top, bottom) in lfo_levels(&output.left, beat, 6) {
            assert!(
                top > bottom * 2.5,
                "at {bpm} BPM: open {top}, closed {bottom}"
            );
        }
        let rate = modulation_rate_hz(&output.left, 30.0);
        assert!(
            (f64::from(rate) - bpm / 60.0).abs() < 0.06,
            "at {bpm} BPM the level moves {rate} times a second"
        );
    }
}

#[test]
fn filter_lfo_period_is_in_beats_and_phase_shifts_it() {
    let noise = dual_mono(&white_noise(8, 0.5, 4.0, RATE));
    let render = |period: f32, phase: f32| {
        let mut device = filter(&[
            (Filter::CUTOFF, 1_000.0),
            (Filter::LFO_DEPTH, 2.0),
            (Filter::LFO_BEATS, period),
            (Filter::LFO_PHASE, phase),
        ]);
        render_effect(&mut device, &noise, &config()).left
    };
    // Two beats at 120 BPM: one cycle a second.
    let slow = render(2.0, 0.0);
    for (top, bottom) in lfo_levels(&slow, 1.0, 3) {
        assert!(top > bottom * 2.5);
    }
    // Half a cycle of phase swaps the top and the bottom.
    let shifted = render(2.0, 0.5);
    for (top, bottom) in lfo_levels(&shifted, 1.0, 3) {
        assert!(bottom > top * 2.5);
    }
}

#[test]
fn filter_lfo_runs_in_hertz_when_not_synced() {
    let noise = dual_mono(&white_noise(9, 0.5, 4.0, RATE));
    for bpm in [90.0, 150.0] {
        let mut device = filter(&[
            (Filter::CUTOFF, 1_000.0),
            (Filter::LFO_DEPTH, 2.0),
            (Filter::LFO_SYNC, 0.0),
            (Filter::LFO_HZ, 3.0),
        ]);
        let output = render_effect(&mut device, &noise, &at_bpm(bpm));
        let rate = modulation_rate_hz(&output.left, 30.0);
        assert!(
            (rate - 3.0).abs() < 0.06,
            "free LFO at {bpm} BPM: {rate} Hz"
        );
    }
}

#[test]
fn filter_lfo_keeps_running_while_the_transport_is_stopped() {
    let noise = dual_mono(&white_noise(10, 0.5, 4.0, RATE));
    let mut device = filter(&[(Filter::CUTOFF, 1_000.0), (Filter::LFO_DEPTH, 2.0)]);
    let stopped = RenderConfig::default().stopped();
    let output = render_effect(&mut device, &noise, &stopped);
    let rate = modulation_rate_hz(&output.left, 30.0);
    assert!((rate - 2.0).abs() < 0.06, "one cycle a beat: {rate} Hz");
}

#[test]
fn filter_lfo_shapes_differ() {
    let noise = dual_mono(&white_noise(11, 0.5, 2.0, RATE));
    let render = |shape: f32| {
        let mut device = filter(&[
            (Filter::CUTOFF, 1_000.0),
            (Filter::LFO_DEPTH, 2.0),
            (Filter::LFO_SHAPE, shape),
        ]);
        render_effect(&mut device, &noise, &config()).left
    };
    // Square: open for the first half of each beat, closed for the second.
    let square = render(3.0);
    for beat in 0..3 {
        let start = f64::from(beat) * 0.5;
        let open = rms_window_seconds(&square, RATE, start + 0.05, 0.15);
        let closed = rms_window_seconds(&square, RATE, start + 0.30, 0.15);
        assert!(open > closed * 2.5, "square: {open} then {closed}");
    }
    // Falling saw: opens on the beat and closes steadily.
    let saw = render(2.0);
    for beat in 0..3 {
        let start = f64::from(beat) * 0.5;
        let early = rms_window_seconds(&saw, RATE, start + 0.02, 0.08);
        let late = rms_window_seconds(&saw, RATE, start + 0.40, 0.08);
        assert!(early > late * 2.5, "saw: {early} then {late}");
    }
    // Triangle and sine both peak a quarter of the way in.
    for (top, bottom) in lfo_levels(&render(1.0), 0.5, 3) {
        assert!(top > bottom * 2.5);
    }
}

#[test]
fn filter_follows_a_tempo_change_between_renders() {
    // The same device, re-rendered at another tempo, moves at the new rate.
    let noise = dual_mono(&white_noise(12, 0.5, 4.0, RATE));
    let mut device = filter(&[(Filter::CUTOFF, 1_000.0), (Filter::LFO_DEPTH, 2.0)]);
    let slow = render_effect(
        &mut device,
        &noise,
        &config().with_tempo(Tempo::new(60.0).expect("tempo")),
    );
    assert!((modulation_rate_hz(&slow.left, 30.0) - 1.0).abs() < 0.06);
    let fast = render_effect(&mut device, &noise, &at_bpm(180.0));
    assert!((modulation_rate_hz(&fast.left, 30.0) - 3.0).abs() < 0.06);
    assert_eq!(device.latency_samples(), 0);
}

#[test]
fn eq_band_changes_glide_in_and_a_band_back_at_zero_is_dropped() {
    let block = 256;
    let input = fourfloor_testkit::sine(1_000.0, 0.25, 3.0, RATE);
    let mut eq = ParametricEq::new();
    set(&mut eq, &[(ParametricEq::band_freq(1), 1_000.0)]);
    prepare(&mut eq, block);
    let ctx = stopped();
    let mut output: Vec<f32> = Vec::with_capacity(input.len());
    let change_at = 20 * block;
    let back_at = 220 * block;
    for (index, chunk) in input.chunks_exact(block).enumerate() {
        if index * block == change_at {
            eq.set_param(ParametricEq::band_gain(1), 12.0);
        }
        if index * block == back_at {
            eq.set_param(ParametricEq::band_gain(1), 0.0);
        }
        let out = run_block(&mut eq, &ctx, &[chunk, chunk], 2, block);
        output.extend_from_slice(&out[0]);
    }
    let level_db = |start: usize, len: usize| -> f32 {
        20.0 * (fourfloor_testkit::rms_window(&output, start, len)
            / fourfloor_testkit::rms_window(&input, start, len))
        .log10()
    };

    // One period (48 samples) after the change the band has barely moved;
    // a stepped change would already read +12 dB.
    let early = level_db(change_at, 48);
    assert!(early < 3.0, "gain right after the change: {early} dB");
    let arrived = level_db(back_at - 4_800, 4_800);
    assert!((arrived - 12.0).abs() < 0.05, "settled at {arrived} dB");

    // No click: the output never moves faster than the boosted tone does.
    let tone_step = 0.25 * 3.981 * std::f32::consts::TAU * 1_000.0 / 48_000.0;
    let steepest = output[change_at - 1..back_at]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        steepest < tone_step * 1.02,
        "steepest step {steepest}, the tone alone moves {tone_step}"
    );

    // Back at 0 dB the band glides out and is then skipped: bit for bit.
    let late = level_db(back_at, 48);
    assert!(late > 9.0, "gain right after turning it back: {late} dB");
    let tail = output.len() - 4_800;
    assert!(output[tail..] == input[tail..output.len()], "not bypassed");
}
