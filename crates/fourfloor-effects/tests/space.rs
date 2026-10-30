//! Rendered-audio tests for the delay, the reverb and the chorus.

mod common;

use common::*;
use fourfloor_core::ParamId;
use fourfloor_effects::{Chorus, Delay, Reverb};
use fourfloor_testkit::{
    impulse, is_silent, peak, render_effect, rms_db, rms_window, sine, spectrum_band_energy_db,
    stereo_correlation, white_noise, AllocDisabler, StereoBuffer,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn delay(params: &[(ParamId, f32)]) -> Delay {
    let mut device = Delay::new();
    set(&mut device, params);
    device
}

fn reverb(params: &[(ParamId, f32)]) -> Reverb {
    let mut device = Reverb::new();
    set(&mut device, params);
    device
}

fn chorus(params: &[(ParamId, f32)]) -> Chorus {
    let mut device = Chorus::new();
    set(&mut device, params);
    device
}

/// `signal` followed by silence, `seconds` long in total.
fn padded(signal: &[f32], seconds: f64) -> Vec<f32> {
    let mut out = signal.to_vec();
    out.resize(frames(seconds).max(signal.len()), 0.0);
    out
}

/// The first index whose sample is louder than `threshold`.
fn first_above(signal: &[f32], threshold: f32) -> Option<usize> {
    signal.iter().position(|sample| sample.abs() > threshold)
}

// ------------------------------------------------------------- delay

/// A delay that only plays its repeats, with the feedback filter opened as
/// far as it goes.
const WET_OPEN: [(ParamId, f32); 3] = [
    (Delay::MIX, 1.0),
    (Delay::LOW_CUT, 20.0),
    (Delay::HIGH_CUT, 20_000.0),
];

#[test]
fn delay_echoes_land_at_the_tempo_synced_time() {
    // (tempo, beats, expected delay in samples at 48 kHz)
    let cases = [
        (120.0, 0.75, 18_000usize),
        (128.0, 0.75, 16_875),
        (100.0, 0.5, 14_400),
        (140.0, 1.0, 20_571),
        (120.0, 0.25, 6_000),
    ];
    let click = 1_000usize;
    for (bpm, beats, expected) in cases {
        let mut device = delay(&WET_OPEN);
        set(
            &mut device,
            &[
                (Delay::BEATS, beats),
                (Delay::PING_PONG, 0.0),
                (Delay::FEEDBACK, 0.6),
            ],
        );
        let input = dual_mono(&impulse(click, 0.8, frames(2.0)));
        let output = render_effect(&mut device, &input, &at_bpm(bpm));
        assert!(
            is_silent(&output.left[..click + expected - 2], -120.0),
            "{bpm} BPM: nothing before the first repeat"
        );
        for repeat in 1..=3usize {
            let centre = click + repeat * expected;
            let found = peak_index(&output.left, centre - 400, centre + 400);
            // A repeat of a repeat can land one sample late when the
            // delay is a fraction of a sample long.
            assert!(
                found.abs_diff(centre) <= repeat,
                "{bpm} BPM, {beats} beats: repeat {repeat} at {found}, expected {centre}"
            );
        }
    }
}

#[test]
fn delay_time_is_in_milliseconds_when_not_synced() {
    let click = 500usize;
    let input = dual_mono(&impulse(click, 0.8, frames(1.0)));
    let mut positions = Vec::new();
    for bpm in [90.0, 150.0] {
        let mut device = delay(&WET_OPEN);
        set(
            &mut device,
            &[
                (Delay::SYNC, 0.0),
                (Delay::MILLISECONDS, 250.0),
                (Delay::PING_PONG, 0.0),
            ],
        );
        let output = render_effect(&mut device, &input, &at_bpm(bpm));
        positions.push(peak_index(&output.left, 0, frames(0.4)));
    }
    assert_eq!(positions, vec![click + 12_000, click + 12_000]);
}

#[test]
fn delay_feedback_sets_the_level_of_each_repeat() {
    // A 1 kHz burst sits in the flat part of the feedback filter, so each
    // repeat is the one before times the feedback.
    let burst = sine(1_000.0, 0.5, 0.05, RATE);
    let input = dual_mono(&padded(&burst, 2.5));
    let step = frames(0.375);
    let level = |output: &StereoBuffer, repeat: usize| {
        20.0 * rms_window(&output.left, repeat * step, burst.len()).log10()
    };
    for (feedback, expected_db) in [(0.5f32, -6.02f32), (0.25, -12.04), (0.8, -1.94)] {
        let mut device = delay(&WET_OPEN);
        set(
            &mut device,
            &[(Delay::PING_PONG, 0.0), (Delay::FEEDBACK, feedback)],
        );
        let output = render_effect(&mut device, &input, &config());
        let source = 20.0 * rms_window(&input.left, 0, burst.len()).log10();
        assert!(
            (level(&output, 1) - source).abs() < 0.3,
            "the first repeat is as loud as the source"
        );
        for repeat in 1..4 {
            let drop = level(&output, repeat + 1) - level(&output, repeat);
            assert!(
                (drop - expected_db).abs() < 0.4,
                "feedback {feedback}: repeat {} is {drop} dB under the one before",
                repeat + 1
            );
        }
    }

    let mut once = delay(&WET_OPEN);
    set(
        &mut once,
        &[(Delay::PING_PONG, 0.0), (Delay::FEEDBACK, 0.0)],
    );
    let output = render_effect(&mut once, &input, &config());
    assert!(level(&output, 1) > -12.0, "one repeat");
    assert!(
        is_silent(&output.left[2 * step - 100..], -120.0),
        "and with no feedback, only one"
    );
}

#[test]
fn delay_ping_pong_alternates_sides() {
    let burst = sine(800.0, 0.5, 0.04, RATE);
    let input = dual_mono(&padded(&burst, 2.0));
    let mut device = delay(&WET_OPEN);
    set(
        &mut device,
        &[(Delay::PING_PONG, 1.0), (Delay::FEEDBACK, 0.7)],
    );
    let output = render_effect(&mut device, &input, &config());
    let step = frames(0.375);
    for repeat in 1..=4usize {
        let left = rms_window(&output.left, repeat * step, burst.len());
        let right = rms_window(&output.right, repeat * step, burst.len());
        let (loud, quiet, side) = if repeat % 2 == 1 {
            (left, right, "left")
        } else {
            (right, left, "right")
        };
        assert!(loud > 0.05, "repeat {repeat} sounds on the {side}: {loud}");
        assert!(
            quiet < loud * 1.0e-4,
            "repeat {repeat} is only on the {side}: other side {quiet}"
        );
    }
}

#[test]
fn delay_stereo_mode_keeps_each_side_on_its_own() {
    let burst = sine(800.0, 0.5, 0.04, RATE);
    let left_only = StereoBuffer::new(RATE, padded(&burst, 2.0), vec![0.0; frames(2.0)]);
    let mut device = delay(&WET_OPEN);
    set(
        &mut device,
        &[(Delay::PING_PONG, 0.0), (Delay::FEEDBACK, 0.7)],
    );
    let output = render_effect(&mut device, &left_only, &config());
    assert!(rms_window(&output.left, frames(0.375), burst.len()) > 0.2);
    assert!(rms_window(&output.left, frames(0.75), burst.len()) > 0.1);
    assert!(is_silent(&output.right, -120.0), "nothing crosses over");
}

/// Level of the band `low..high` in repeat number `repeat` of a noise
/// burst, in dB.
fn repeat_band_db(output: &StereoBuffer, repeat: usize, length: usize, low: f32, high: f32) -> f64 {
    let start = repeat * frames(0.375);
    spectrum_band_energy_db(&output.left[start..start + length], RATE, low, high)
}

#[test]
fn delay_feedback_filter_makes_each_repeat_darker_and_thinner() {
    let burst = white_noise(11, 0.5, 0.1, RATE);
    let input = dual_mono(&padded(&burst, 2.0));
    let render = |low_cut: f32, high_cut: f32| {
        let mut device = delay(&[
            (Delay::MIX, 1.0),
            (Delay::PING_PONG, 0.0),
            (Delay::FEEDBACK, 0.9),
            (Delay::LOW_CUT, low_cut),
            (Delay::HIGH_CUT, high_cut),
        ]);
        render_effect(&mut device, &input, &config())
    };
    let n = burst.len();

    // High cut at 1 kHz: the top falls faster than the middle.
    let dark = render(20.0, 1_000.0);
    let tilt = |repeat: usize| {
        repeat_band_db(&dark, repeat, n, 6_000.0, 12_000.0)
            - repeat_band_db(&dark, repeat, n, 200.0, 600.0)
    };
    let tilts: Vec<f64> = (1..=4).map(tilt).collect();
    for pair in tilts.windows(2) {
        assert!(
            pair[1] < pair[0] - 6.0,
            "each repeat loses more top than the last: {tilts:?}"
        );
    }

    // Low cut at 1 kHz: the bottom falls faster than the top.
    let thin = render(1_000.0, 20_000.0);
    let tilt = |repeat: usize| {
        repeat_band_db(&thin, repeat, n, 100.0, 300.0)
            - repeat_band_db(&thin, repeat, n, 4_000.0, 8_000.0)
    };
    let tilts: Vec<f64> = (1..=4).map(tilt).collect();
    for pair in tilts.windows(2) {
        assert!(
            pair[1] < pair[0] - 6.0,
            "each repeat loses more bottom than the last: {tilts:?}"
        );
    }

    // Wide open, the spectrum barely changes from repeat to repeat.
    let open = render(20.0, 20_000.0);
    let tilt = |repeat: usize| {
        repeat_band_db(&open, repeat, n, 4_000.0, 8_000.0)
            - repeat_band_db(&open, repeat, n, 200.0, 600.0)
    };
    assert!((tilt(3) - tilt(1)).abs() < 2.0);
}

#[test]
fn delay_mix_blends_dry_and_repeats() {
    let burst = sine(1_000.0, 0.5, 0.05, RATE);
    let input = dual_mono(&padded(&burst, 1.0));

    let mut dry = delay(&[(Delay::MIX, 0.0)]);
    let output = render_effect(&mut dry, &input, &config());
    assert_eq!(output.left, input.left, "mix 0 is the dry signal");
    assert_eq!(output.right, input.right);

    let mut wet = delay(&WET_OPEN);
    let output = render_effect(&mut wet, &input, &config());
    assert!(
        is_silent(&output.left[..burst.len()], -120.0),
        "mix 1 has no dry signal"
    );

    let mut half = delay(&[(Delay::MIX, 0.5), (Delay::PING_PONG, 0.0)]);
    let output = render_effect(&mut half, &input, &config());
    let dry_db = 20.0 * (rms_window(&output.left, 0, burst.len()) / 0.5f32).log10();
    assert!(
        (dry_db - (-9.03)).abs() < 0.2,
        "half the dry level: {dry_db}"
    );
}

#[test]
fn delay_is_stable_at_full_feedback_and_dies_away() {
    let burst = white_noise(3, 0.9, 0.2, RATE);
    let input = dual_mono(&padded(&burst, 12.0));
    let mut device = delay(&WET_OPEN);
    set(
        &mut device,
        &[(Delay::FEEDBACK, 0.95), (Delay::BEATS, 0.25)],
    );
    let output = render_effect(&mut device, &input, &config());
    assert!(peak(&output.left) <= 1.0 && peak(&output.right) <= 1.0);
    let early = rms_window(&output.mono(), frames(1.0), frames(1.0));
    let late = rms_window(&output.mono(), frames(10.0), frames(1.0));
    assert!(late < early * 0.2, "the repeats fade: {early} then {late}");
}

// ------------------------------------------------------------ reverb

/// A reverb that plays only its tail, with the damping and low cut out of
/// the way so `Decay` alone sets how fast it falls.
fn bare_reverb(mode: f32, decay: f32) -> Reverb {
    reverb(&[
        (Reverb::MODE, mode),
        (Reverb::DECAY, decay),
        (Reverb::MIX, 1.0),
        (Reverb::PRE_DELAY, 0.0),
        (Reverb::DAMPING, 20_000.0),
        (Reverb::LOW_CUT, 20.0),
    ])
}

/// Level of the 200 Hz to 1 kHz band of both channels over 200 ms from
/// `seconds`, in dB.
fn tail_db(output: &StereoBuffer, seconds: f64) -> f64 {
    let window = output.slice_seconds(seconds, seconds + 0.2);
    spectrum_band_energy_db(&window.mono(), RATE, 200.0, 1_000.0).max(spectrum_band_energy_db(
        &window.side(),
        RATE,
        200.0,
        1_000.0,
    ))
}

fn noise_burst(seconds: f64) -> StereoBuffer {
    dual_mono(&padded(&white_noise(5, 0.5, 0.05, RATE), seconds))
}

#[test]
fn reverb_tail_decays_at_the_set_rate() {
    for mode in [Reverb::PLATE, Reverb::HALL] {
        // 60 dB in `decay` seconds, so over one second: 60 / decay.
        for (decay, expected_drop) in [(1.5f32, 40.0f64), (3.0, 20.0), (6.0, 10.0)] {
            let output = render_effect(&mut bare_reverb(mode, decay), &noise_burst(2.5), &config());
            let drop = tail_db(&output, 0.6) - tail_db(&output, 1.6);
            assert!(
                (drop - expected_drop).abs() < 0.2 * expected_drop + 1.5,
                "mode {mode}, decay {decay} s: fell {drop} dB in a second, expected {expected_drop}"
            );
        }
    }
}

#[test]
fn reverb_tail_keeps_falling_until_it_is_gone() {
    let input = noise_burst(4.0);
    let output = render_effect(&mut bare_reverb(Reverb::PLATE, 0.5), &input, &config());
    let levels: Vec<f32> = (0..6)
        .map(|step| {
            let start = 0.1 + 0.15 * f64::from(step);
            rms_db(&output.slice_seconds(start, start + 0.15).mono())
        })
        .collect();
    for pair in levels.windows(2) {
        assert!(pair[1] < pair[0] - 6.0, "always falling: {levels:?}");
    }
    assert!(
        is_silent(&output.slice_seconds(3.0, 4.0).left, -120.0),
        "a half-second reverb is silent after three seconds"
    );
}

#[test]
fn reverb_tail_is_decorrelated_and_width_narrows_it() {
    for mode in [Reverb::PLATE, Reverb::HALL] {
        let output = render_effect(&mut bare_reverb(mode, 2.0), &noise_burst(2.0), &config());
        let tail = output.slice_seconds(0.2, 2.0);
        let correlation = stereo_correlation(&tail.left, &tail.right);
        assert!(
            correlation.abs() < 0.25,
            "mode {mode}: a mono source gives a wide tail, correlation {correlation}"
        );
        let balance = rms_db(&tail.left) - rms_db(&tail.right);
        assert!(balance.abs() < 1.0, "both sides equally loud: {balance} dB");

        let mut narrow = bare_reverb(mode, 2.0);
        set(&mut narrow, &[(Reverb::WIDTH, 0.0)]);
        let output = render_effect(&mut narrow, &noise_burst(2.0), &config());
        assert_eq!(output.left, output.right, "width 0 is mono");

        let mut half = bare_reverb(mode, 2.0);
        set(&mut half, &[(Reverb::WIDTH, 0.5)]);
        let output = render_effect(&mut half, &noise_burst(2.0), &config());
        let tail = output.slice_seconds(0.2, 2.0);
        let between = stereo_correlation(&tail.left, &tail.right);
        assert!(
            between > correlation + 0.3 && between < 0.95,
            "width 0.5 sits in between: {between}"
        );
    }
}

#[test]
fn reverb_damping_makes_the_highs_die_sooner() {
    let high_minus_low = |damping: f32, seconds: f64| {
        let mut device = bare_reverb(Reverb::PLATE, 3.0);
        set(&mut device, &[(Reverb::DAMPING, damping)]);
        let output = render_effect(&mut device, &noise_burst(2.0), &config());
        let window = output.slice_seconds(seconds, seconds + 0.3).left;
        spectrum_band_energy_db(&window, RATE, 5_000.0, 10_000.0)
            - spectrum_band_energy_db(&window, RATE, 200.0, 800.0)
    };
    let (dark_early, dark_late) = (high_minus_low(2_000.0, 0.1), high_minus_low(2_000.0, 1.2));
    let (bright_early, bright_late) =
        (high_minus_low(20_000.0, 0.1), high_minus_low(20_000.0, 1.2));
    assert!(
        dark_late < dark_early - 20.0,
        "damped: the top falls away, {dark_early} then {dark_late}"
    );
    assert!(
        dark_late < bright_late - 20.0,
        "and much more than undamped: {dark_late} against {bright_late}"
    );
    assert!(
        bright_late > bright_early - 15.0,
        "undamped the tail keeps most of its top: {bright_early} then {bright_late}"
    );
}

#[test]
fn reverb_low_cut_keeps_bass_out_of_the_tail() {
    let low_minus_mid = |low_cut: f32| {
        let mut device = bare_reverb(Reverb::PLATE, 2.0);
        set(&mut device, &[(Reverb::LOW_CUT, low_cut)]);
        let output = render_effect(&mut device, &noise_burst(1.5), &config());
        let window = output.slice_seconds(0.1, 1.5).left;
        spectrum_band_energy_db(&window, RATE, 40.0, 120.0)
            - spectrum_band_energy_db(&window, RATE, 3_000.0, 6_000.0)
    };
    let (open, cut) = (low_minus_mid(20.0), low_minus_mid(1_000.0));
    assert!(cut < open - 12.0, "bass removed: {open} then {cut}");
}

#[test]
fn reverb_pre_delay_and_size_move_the_first_reflection() {
    let onset = |mode: f32, pre_delay_ms: f32, size: f32| {
        let mut device = bare_reverb(mode, 1.5);
        set(
            &mut device,
            &[(Reverb::PRE_DELAY, pre_delay_ms), (Reverb::SIZE, size)],
        );
        let input = dual_mono(&impulse(0, 0.9, frames(1.0)));
        let output = render_effect(&mut device, &input, &config());
        first_above(&output.mono(), 1.0e-5)
            .or_else(|| first_above(&output.side(), 1.0e-5))
            .expect("the reverb sounds")
    };
    let base = onset(Reverb::PLATE, 0.0, 1.0);
    // Zero pre-delay is one sample long, so 100 ms moves it by 4799.
    let shifted = onset(Reverb::PLATE, 100.0, 1.0);
    assert!(
        (shifted - base).abs_diff(4_800) <= 2,
        "100 ms of pre-delay: {base} then {shifted}"
    );
    assert!(
        base >= frames(0.020) && base <= frames(0.035),
        "plate starts with its shortest line, about 23 ms: {base}"
    );
    let big = onset(Reverb::PLATE, 0.0, 2.0);
    assert!(
        big > base + frames(0.020),
        "a bigger space answers later: {base} then {big}"
    );
    let hall = onset(Reverb::HALL, 0.0, 1.0);
    assert!(
        hall > base + frames(0.025),
        "the hall is larger than the plate: {base} then {hall}"
    );
}

#[test]
fn reverb_plate_is_denser_than_hall_at_the_start() {
    // Share of the first half second's energy that arrives in the first
    // 80 ms.
    let early_share = |mode: f32| {
        let input = dual_mono(&impulse(0, 0.9, frames(1.0)));
        let output = render_effect(&mut bare_reverb(mode, 2.0), &input, &config());
        let energy = |from: f64, to: f64| {
            let window = output.slice_seconds(from, to);
            let sum: f64 = window
                .left
                .iter()
                .chain(window.right.iter())
                .map(|s| f64::from(*s).powi(2))
                .sum();
            sum
        };
        energy(0.0, 0.08) / energy(0.0, 0.5)
    };
    let (plate, hall) = (early_share(Reverb::PLATE), early_share(Reverb::HALL));
    assert!(
        plate > hall * 1.5,
        "the plate is immediate, the hall builds: {plate} against {hall}"
    );
}

#[test]
fn reverb_mix_blends_dry_and_wet() {
    let input = noise_burst(1.0);
    let mut dry = reverb(&[(Reverb::MIX, 0.0)]);
    let output = render_effect(&mut dry, &input, &config());
    assert_eq!(output.left, input.left, "mix 0 is the dry signal");

    let mut wet = reverb(&[(Reverb::MIX, 1.0), (Reverb::PRE_DELAY, 20.0)]);
    let output = render_effect(&mut wet, &input, &config());
    assert!(
        is_silent(&output.left[..frames(0.02)], -120.0),
        "mix 1 has no dry signal"
    );
    assert!(rms_db(&output.slice_seconds(0.1, 0.5).left) > -50.0);
}

#[test]
fn reverb_stays_in_range_on_loud_sustained_input() {
    for mode in [Reverb::PLATE, Reverb::HALL] {
        let mut device = reverb(&[
            (Reverb::MODE, mode),
            (Reverb::DECAY, 20.0),
            (Reverb::SIZE, 2.0),
            (Reverb::MIX, 1.0),
            (Reverb::DAMPING, 20_000.0),
            (Reverb::LOW_CUT, 20.0),
        ]);
        let input = dual_mono(&white_noise(8, 0.5, 6.0, RATE));
        let output = render_effect(&mut device, &input, &config());
        let level = peak(&output.left).max(peak(&output.right));
        assert!(
            level.is_finite() && level < 4.0,
            "mode {mode}: peak {level}"
        );
        // It has settled rather than still growing.
        let (third, last) = (
            rms_db(&output.slice_seconds(3.0, 4.0).left),
            rms_db(&output.slice_seconds(5.0, 6.0).left),
        );
        assert!(last < third + 3.0, "mode {mode}: {third} then {last}");
    }
}

// ------------------------------------------------------------ chorus

/// A bright mono source: a stack of harmonics of 110 Hz.
fn harmonic_tone(seconds: f64) -> Vec<f32> {
    let partials: Vec<Vec<f32>> = (1..=12u32)
        .map(|n| sine(110.0 * f64::from(n), 0.4 / n as f32, seconds, RATE))
        .collect();
    let views: Vec<&[f32]> = partials.iter().map(Vec::as_slice).collect();
    fourfloor_testkit::mix(&views)
}

#[test]
fn chorus_widens_a_mono_source() {
    let sources = [
        ("harmonic tone", harmonic_tone(4.0)),
        ("noise", white_noise(21, 0.3, 4.0, RATE)),
    ];
    for (name, source) in &sources {
        let input = dual_mono(source);
        assert!(stereo_correlation(&input.left, &input.right) > 0.9999);
        for mode in [0.0, 1.0, 2.0] {
            let output = render_effect(&mut chorus(&[(Chorus::MODE, mode)]), &input, &config());
            let correlation = stereo_correlation(&output.left, &output.right);
            // Mode I+II sweeps only 0.4 ms, so it widens least.
            let limit = if mode == 2.0 { 0.995 } else { 0.9 };
            assert!(
                correlation < limit && correlation > 0.0,
                "{name}, mode {mode}: correlation {correlation}"
            );
            let balance = rms_db(&output.left) - rms_db(&output.right);
            assert!(balance.abs() < 1.0, "{name}, mode {mode}: balanced");
        }
    }
}

#[test]
fn chorus_width_comes_from_the_sweep_and_the_mix() {
    let input = dual_mono(&white_noise(21, 0.3, 3.0, RATE));
    let correlation = |params: &[(ParamId, f32)]| {
        let output = render_effect(&mut chorus(params), &input, &config());
        stereo_correlation(&output.left, &output.right)
    };

    let mut still = chorus(&[(Chorus::DEPTH, 0.0)]);
    let output = render_effect(&mut still, &input, &config());
    assert_eq!(output.left, output.right, "no sweep: both sides the same");

    let mut dry = chorus(&[(Chorus::MIX, 0.0)]);
    let output = render_effect(&mut dry, &input, &config());
    assert_eq!(output.left, input.left, "mix 0 is the dry signal");

    let blend = correlation(&[(Chorus::MIX, 0.5)]);
    let wet = correlation(&[(Chorus::MIX, 1.0)]);
    assert!(
        wet < blend - 0.3,
        "all wet is widest: {wet} against {blend}"
    );

    // Noise loses its correlation at the smallest delay difference; a tone
    // with a few harmonics shows the depth of the sweep.
    let tone = dual_mono(&harmonic_tone(4.0));
    let depth = |amount: f32| {
        let output = render_effect(&mut chorus(&[(Chorus::DEPTH, amount)]), &tone, &config());
        stereo_correlation(&output.left, &output.right)
    };
    let (shallow, half, full) = (depth(0.1), depth(0.5), depth(1.0));
    assert!(
        shallow > half + 0.02 && half > full + 0.02 && shallow > 0.9,
        "more depth is wider: {shallow}, {half}, {full}"
    );
}

#[test]
fn chorus_sweep_rates_match_the_three_modes() {
    // Dry plus a swept delay is a comb filter; for a low tone its level
    // rises and falls once per sweep.
    let rate_of = |mode: f32, tone_hz: f64, seconds: f64| {
        let input = sine(tone_hz, 0.4, seconds, RATE);
        let output = render_mono(&mut chorus(&[(Chorus::MODE, mode)]), &input);
        modulation_rate_hz(&output, 30.0)
    };
    let one = rate_of(0.0, 80.0, 20.0);
    assert!(
        (one - 0.513).abs() < 0.03,
        "mode I sweeps at 0.513 Hz: {one}"
    );
    let two = rate_of(1.0, 80.0, 20.0);
    assert!(
        (two - 0.863).abs() < 0.03,
        "mode II sweeps at 0.863 Hz: {two}"
    );
    let both = rate_of(2.0, 650.0, 4.0);
    assert!(
        (both - 9.75).abs() < 0.15,
        "mode I+II wobbles at 9.75 Hz: {both}"
    );
}

#[test]
fn chorus_delays_stay_inside_the_documented_range() {
    // All wet, a click comes back once per side, somewhere in the sweep.
    for (mode, low_ms, high_ms) in [(0.0, 1.66, 5.35), (1.0, 1.66, 5.35), (2.0, 3.3, 3.7)] {
        let mut device = chorus(&[
            (Chorus::MODE, mode),
            (Chorus::MIX, 1.0),
            (Chorus::TONE, 20_000.0),
        ]);
        let clicks: Vec<usize> = (0..40).map(|n| 1_000 + n * 4_800).collect();
        let mut signal = vec![0.0f32; frames(4.2)];
        for &at in &clicks {
            signal[at] = 0.8;
        }
        let output = render_effect(&mut device, &dual_mono(&signal), &config());
        let (mut shortest, mut longest) = (f64::MAX, 0.0f64);
        for &at in &clicks {
            for side in [&output.left, &output.right] {
                let found = peak_index(side, at, at + 480);
                assert!(side[found].abs() > 0.2, "mode {mode}: the click comes back");
                let ms = (found - at) as f64 / 48.0;
                shortest = shortest.min(ms);
                longest = longest.max(ms);
            }
        }
        assert!(
            shortest >= low_ms - 0.05 && longest <= high_ms + 0.05,
            "mode {mode}: delays {shortest}..{longest} ms"
        );
        assert!(
            longest - shortest > 0.6 * (high_ms - low_ms),
            "mode {mode}: and the sweep covers most of it: {shortest}..{longest} ms"
        );
    }
}

#[test]
fn chorus_sides_sweep_in_opposite_directions() {
    let mut device = chorus(&[(Chorus::MIX, 1.0), (Chorus::TONE, 20_000.0)]);
    let clicks: Vec<usize> = (0..20).map(|n| 1_000 + n * 4_800).collect();
    let mut signal = vec![0.0f32; frames(2.2)];
    for &at in &clicks {
        signal[at] = 0.8;
    }
    let output = render_effect(&mut device, &dual_mono(&signal), &config());
    let centre = 0.5 * (1.66 + 5.35) * 48.0;
    for &at in &clicks {
        let left = (peak_index(&output.left, at, at + 480) - at) as f64 - centre;
        let right = (peak_index(&output.right, at, at + 480) - at) as f64 - centre;
        assert!(
            (left + right).abs() <= 2.0,
            "mirror images around the centre: {left} and {right}"
        );
    }
}

#[test]
fn chorus_tone_darkens_the_delayed_signal() {
    let level = |tone: f32, hz: f64| {
        let mut device = chorus(&[
            (Chorus::MIX, 1.0),
            (Chorus::DEPTH, 0.0),
            (Chorus::TONE, tone),
        ]);
        tone_gain_db(&mut device, hz)
    };
    assert!(level(2_000.0, 200.0) > -0.5, "lows pass");
    let (dark, bright) = (level(2_000.0, 10_000.0), level(20_000.0, 10_000.0));
    assert!(dark < -12.0, "tone at 2 kHz takes 10 kHz down: {dark}");
    assert!(bright > -3.0, "tone wide open leaves it: {bright}");
}
