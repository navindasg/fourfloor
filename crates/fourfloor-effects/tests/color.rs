//! Rendered-audio tests for the saturator and the utility.

mod common;

use common::*;
use fourfloor_core::{Device, ParamId};
use fourfloor_effects::{Saturator, Utility};
use fourfloor_testkit::{
    assert_null, is_silent, peak, render_effect, rms_db, sine, spectrum, stereo_correlation,
    white_noise, AllocDisabler, StereoBuffer,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const MODES: [(usize, &str); 4] = [
    (Saturator::TAPE, "tape"),
    (Saturator::SPIRAL, "spiral"),
    (Saturator::CONSOLE, "console"),
    (Saturator::HARD, "hard"),
];

fn saturator(mode: usize, params: &[(ParamId, f32)]) -> Saturator {
    let mut device = Saturator::new();
    set(&mut device, &[(Saturator::MODE, mode as f32)]);
    set(&mut device, params);
    device
}

fn utility(params: &[(ParamId, f32)]) -> Utility {
    let mut device = Utility::new();
    set(&mut device, params);
    device
}

/// Level of harmonic `number` of a 1 kHz tone relative to the fundamental,
/// in dB, measured over the second half of the render.
fn harmonic_db(output: &[f32], number: u32) -> f64 {
    let spectrum = spectrum(&output[output.len() / 2..], RATE);
    let band = |n: u32| {
        let centre = 1_000.0 * f64::from(n);
        spectrum.band_power(centre - 60.0, centre + 60.0)
    };
    10.0 * (band(number) / band(1)).max(1.0e-30).log10()
}

/// A 1 kHz tone of peak `amplitude` through the device.
fn drive_tone(device: &mut Saturator, amplitude: f32) -> Vec<f32> {
    render_mono(device, &sine(1_000.0, amplitude, 1.0, RATE))
}

// --------------------------------------------------------- saturator

#[test]
fn saturator_adds_odd_harmonics() {
    let clean = sine(1_000.0, 0.5, 1.0, RATE);
    assert!(harmonic_db(&clean, 3) < -100.0, "the source is a pure tone");

    for (mode, name) in MODES {
        let output = drive_tone(&mut saturator(mode, &[(Saturator::DRIVE, 18.0)]), 0.5);
        let (second, third, fifth) = (
            harmonic_db(&output, 2),
            harmonic_db(&output, 3),
            harmonic_db(&output, 5),
        );
        assert!(third > -25.0, "{name}: third harmonic at {third} dB");
        assert!(fifth > -40.0, "{name}: fifth harmonic at {fifth} dB");
        assert!(
            second < -80.0,
            "{name}: a symmetric curve adds no even harmonics, second at {second} dB"
        );
    }
}

#[test]
fn saturator_distortion_grows_with_drive() {
    for (mode, name) in MODES {
        let thirds: Vec<f64> = [0.0, 5.0, 12.0, 18.0]
            .iter()
            .map(|drive| {
                let output = drive_tone(&mut saturator(mode, &[(Saturator::DRIVE, *drive)]), 0.5);
                harmonic_db(&output, 3)
            })
            .collect();
        // The hard clip is clean until the signal reaches full scale, which
        // a 0.5 tone does past 6 dB of drive.
        let from = if mode == Saturator::HARD { 2 } else { 0 };
        if mode == Saturator::HARD {
            assert!(thirds[0] < -100.0 && thirds[1] < -100.0, "{thirds:?}");
        }
        for pair in thirds[from..].windows(2) {
            assert!(
                pair[1] > pair[0],
                "{name}: more drive, more third harmonic: {thirds:?}"
            );
        }
    }
}

#[test]
fn saturator_is_clean_and_at_unity_for_small_signals() {
    for (mode, name) in MODES {
        let mut device = saturator(mode, &[(Saturator::DRIVE, 0.0)]);
        let gain = tone_gain_db_at_level(&mut device, 1_000.0, 0.001);
        assert!(gain.abs() < 0.01, "{name}: gain {gain} dB at -60 dBFS");
        let output = drive_tone(&mut saturator(mode, &[(Saturator::DRIVE, 0.0)]), 0.001);
        let third = harmonic_db(&output, 3);
        assert!(third < -100.0, "{name}: third harmonic at {third} dB");
    }
}

#[test]
fn saturator_curves_differ_in_how_early_they_bend() {
    // Moderate level, no drive: tanh bends first, the console sine later,
    // Spiral stays clean longest and a clip does nothing at all.
    let third = |mode: usize| {
        let output = drive_tone(&mut saturator(mode, &[(Saturator::DRIVE, 0.0)]), 0.3);
        harmonic_db(&output, 3)
    };
    let (tape, spiral, console, hard) = (
        third(Saturator::TAPE),
        third(Saturator::SPIRAL),
        third(Saturator::CONSOLE),
        third(Saturator::HARD),
    );
    assert!(
        tape > console + 4.0 && console > spiral + 10.0,
        "tape {tape}, console {console}, spiral {spiral}"
    );
    assert!(hard < -100.0, "a clip below full scale is clean: {hard}");

    let input = dual_mono(&sine(1_000.0, 0.9, 0.5, RATE));
    let mut clip = saturator(Saturator::HARD, &[(Saturator::DRIVE, 0.0)]);
    let output = render_effect(&mut clip, &input, &config());
    assert_eq!(output.left, input.left, "and passes the signal exactly");
}

#[test]
fn saturator_output_never_exceeds_full_scale() {
    let loud = dual_mono(&white_noise(4, 1.0, 1.0, RATE));
    for (mode, name) in MODES {
        let mut device = saturator(mode, &[(Saturator::DRIVE, 36.0)]);
        let output = render_effect(&mut device, &loud, &config());
        let level = peak(&output.left).max(peak(&output.right));
        assert!(level <= 1.0, "{name}: peak {level}");
        // Spiral, like the Airwindows original, tops out near 0.85 and
        // settles at 0.8 when driven flat out.
        let floor = if mode == Saturator::SPIRAL { 0.79 } else { 0.9 };
        assert!(level > floor, "{name}: driven to the top of the curve");
        if mode == Saturator::SPIRAL {
            assert!(level < 0.86, "{name}: peak {level}");
        }
    }
}

#[test]
fn saturator_compresses_loud_signals() {
    // 20 dB more level in gives far less than 20 dB more out.
    for (mode, name) in MODES {
        let level = |amplitude: f32| {
            let output = drive_tone(&mut saturator(mode, &[(Saturator::DRIVE, 12.0)]), amplitude);
            rms_db(&output[output.len() / 2..])
        };
        let rise = level(1.0) - level(0.1);
        assert!(rise < 12.0 && rise > 0.0, "{name}: rose {rise} dB");
    }
}

#[test]
fn saturator_asymmetry_adds_even_harmonics_without_an_offset() {
    for (mode, name) in MODES {
        // The clip needs to reach its corners to be asymmetric.
        let drive = if mode == Saturator::HARD { 12.0 } else { 6.0 };
        let even = |asymmetry: f32| {
            let mut device = saturator(
                mode,
                &[(Saturator::DRIVE, drive), (Saturator::ASYMMETRY, asymmetry)],
            );
            drive_tone(&mut device, 0.5)
        };
        let (plain, bent) = (even(0.0), even(0.6));
        let (before, after) = (harmonic_db(&plain, 2), harmonic_db(&bent, 2));
        assert!(before < -80.0, "{name}: no second harmonic at first");
        assert!(after > -40.0, "{name}: second harmonic at {after} dB");

        let tail = &bent[bent.len() / 2..];
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 2.0e-3, "{name}: offset removed, mean {mean}");
    }

    // Silence in gives silence out, even with the curve shifted.
    let mut device = saturator(Saturator::TAPE, &[(Saturator::ASYMMETRY, 1.0)]);
    let output = render_effect(
        &mut device,
        &StereoBuffer::silent(RATE, frames(0.5)),
        &config(),
    );
    assert!(is_silent(&output.left, -140.0) && is_silent(&output.right, -140.0));
}

#[test]
fn saturator_mix_and_output_gain() {
    let input = dual_mono(&sine(1_000.0, 0.5, 0.5, RATE));

    let mut dry = saturator(
        Saturator::TAPE,
        &[(Saturator::DRIVE, 24.0), (Saturator::MIX, 0.0)],
    );
    let output = render_effect(&mut dry, &input, &config());
    assert_eq!(output.left, input.left, "mix 0 is the dry signal");

    let mut quiet = saturator(
        Saturator::TAPE,
        &[
            (Saturator::DRIVE, 24.0),
            (Saturator::MIX, 0.0),
            (Saturator::OUTPUT, -12.0),
        ],
    );
    let output = render_effect(&mut quiet, &input, &config());
    let change = rms_db(&output.left) - rms_db(&input.left);
    assert!((change + 12.0).abs() < 0.01, "output gain: {change} dB");

    let level = |mix: f32| {
        let mut device = saturator(
            Saturator::TAPE,
            &[(Saturator::DRIVE, 24.0), (Saturator::MIX, mix)],
        );
        harmonic_db(&render_mono(&mut device, &input.left), 3)
    };
    assert!(level(0.5) < level(1.0) - 1.0, "half mix, less distortion");
}

#[test]
fn saturator_treats_both_channels_alike_and_apart() {
    let left = sine(1_000.0, 0.6, 0.5, RATE);
    let right = sine(330.0, 0.2, 0.5, RATE);
    let mut device = saturator(Saturator::SPIRAL, &[(Saturator::DRIVE, 12.0)]);
    let stereo = render_effect(
        &mut device,
        &StereoBuffer::new(RATE, left.clone(), right.clone()),
        &config(),
    );
    let mut device = saturator(Saturator::SPIRAL, &[(Saturator::DRIVE, 12.0)]);
    assert_eq!(stereo.left, render_mono(&mut device, &left));
    let mut device = saturator(Saturator::SPIRAL, &[(Saturator::DRIVE, 12.0)]);
    assert_eq!(stereo.right, render_mono(&mut device, &right));
}

// ----------------------------------------------------------- utility

/// A stereo test signal with different content on each side.
fn wide_signal() -> StereoBuffer {
    StereoBuffer::new(
        RATE,
        sine(440.0, 0.4, 0.5, RATE),
        sine(660.0, 0.3, 0.5, RATE),
    )
}

/// The part of a render after the parameter glide has settled.
fn settled(buffer: &StereoBuffer) -> StereoBuffer {
    buffer.slice_seconds(0.1, 0.5)
}

#[test]
fn utility_at_its_defaults_passes_audio_bit_for_bit() {
    let input = wide_signal();
    let output = render_effect(&mut Utility::new(), &input, &config());
    assert_eq!(output.left, input.left);
    assert_eq!(output.right, input.right);
}

#[test]
fn utility_gain_is_in_decibels_and_its_minimum_is_silence() {
    let input = wide_signal();
    for db in [-24.0f32, -6.0, 6.0, 12.0] {
        let output = render_effect(&mut utility(&[(Utility::GAIN, db)]), &input, &config());
        for (out, source) in [(&output.left, &input.left), (&output.right, &input.right)] {
            let change = rms_db(out) - rms_db(source);
            assert!((change - db).abs() < 0.01, "{db} dB gave {change} dB");
        }
    }
    let mut off = utility(&[(Utility::GAIN, Utility::MIN_DB)]);
    let output = render_effect(&mut off, &input, &config());
    assert!(output.left.iter().chain(&output.right).all(|s| *s == 0.0));
}

#[test]
fn utility_pan_turns_the_far_side_down() {
    let input = wide_signal();
    let levels = |pan: f32| {
        let output = settled(&render_effect(
            &mut utility(&[(Utility::PAN, pan)]),
            &input,
            &config(),
        ));
        let source = settled(&input);
        (
            rms_db(&output.left) - rms_db(&source.left),
            rms_db(&output.right) - rms_db(&source.right),
        )
    };
    let (left, right) = levels(-1.0);
    assert!(
        left.abs() < 0.01 && right < -100.0,
        "hard left: {left}, {right}"
    );
    let (left, right) = levels(1.0);
    assert!(
        left < -100.0 && right.abs() < 0.01,
        "hard right: {left}, {right}"
    );
    // Half way: the far side is at cos(45 degrees), -3.01 dB.
    let (left, right) = levels(0.5);
    assert!(
        (left + 3.01).abs() < 0.02 && right.abs() < 0.01,
        "half right: {left}, {right}"
    );
    let (left, right) = levels(-0.5);
    assert!(
        left.abs() < 0.01 && (right + 3.01).abs() < 0.02,
        "half left: {left}, {right}"
    );
}

#[test]
fn utility_width_scales_the_side_signal() {
    let input = wide_signal();
    let side_change = |width: f32| {
        let output = settled(&render_effect(
            &mut utility(&[(Utility::WIDTH, width)]),
            &input,
            &config(),
        ));
        let source = settled(&input);
        assert_null(&output.mono(), &source.mono(), -100.0);
        rms_db(&output.side()) - rms_db(&source.side())
    };
    assert!(
        (side_change(2.0) - 6.02).abs() < 0.02,
        "width 2 doubles the side"
    );
    assert!(
        (side_change(0.5) + 6.02).abs() < 0.02,
        "width 0.5 halves it"
    );

    let output = settled(&render_effect(
        &mut utility(&[(Utility::WIDTH, 0.0)]),
        &input,
        &config(),
    ));
    assert_eq!(output.left, output.right, "width 0 is mono");
    assert_null(&output.left, &settled(&input).mono(), -100.0);

    let wider = settled(&render_effect(
        &mut utility(&[(Utility::WIDTH, 2.0)]),
        &dual_mono(&input.left),
        &config(),
    ));
    assert_eq!(
        wider.left, wider.right,
        "a mono source has no side to widen"
    );
}

#[test]
fn utility_mono_sums_both_sides() {
    let input = wide_signal();
    let mut device = utility(&[(Utility::MONO, 1.0), (Utility::WIDTH, 2.0)]);
    let output = render_effect(&mut device, &input, &config());
    assert_eq!(output.left, output.right);
    assert!(stereo_correlation(&output.left, &output.right) > 0.9999);
    assert_null(&output.left, &input.mono(), -100.0);
}

#[test]
fn utility_polarity_switches_invert_each_side() {
    let input = wide_signal();
    let negated = |signal: &[f32]| signal.iter().map(|s| -s).collect::<Vec<f32>>();

    let output = render_effect(
        &mut utility(&[(Utility::INVERT_LEFT, 1.0)]),
        &input,
        &config(),
    );
    assert_eq!(output.left, negated(&input.left));
    assert_eq!(output.right, input.right);

    let output = render_effect(
        &mut utility(&[(Utility::INVERT_RIGHT, 1.0)]),
        &input,
        &config(),
    );
    assert_eq!(output.left, input.left);
    assert_eq!(output.right, negated(&input.right));

    // One side of a mono source inverted, then summed: nothing left.
    let mono = dual_mono(&input.left);
    let mut device = utility(&[(Utility::INVERT_LEFT, 1.0), (Utility::MONO, 1.0)]);
    let output = render_effect(&mut device, &mono, &config());
    assert!(is_silent(&output.left, -140.0));
}

#[test]
fn utility_changes_glide_without_a_click() {
    // A constant input shows the gain directly. Change it between blocks
    // and the output must move in small steps.
    let mut device = Utility::new();
    prepare(&mut device, 480);
    let ones = vec![1.0f32; 480];
    let inputs: [&[f32]; 2] = [&ones, &ones];
    let first = run_block(&mut device, &stopped(), &inputs, 2, 480);
    assert!(first[0].iter().all(|s| *s == 1.0));

    device.set_param(Utility::GAIN, -12.0);
    let mut gain = Vec::new();
    for _ in 0..5 {
        gain.extend(run_block(&mut device, &stopped(), &inputs, 2, 480).remove(0));
    }
    let largest_step = gain
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(largest_step < 0.01, "largest step {largest_step}");
    assert!((gain[0] - 1.0).abs() < 0.01, "starts where it was");
    let target = 10.0f32.powf(-12.0 / 20.0);
    // 5 ms time constant: 63% of the way after 240 samples.
    let part = (1.0 - gain[239]) / (1.0 - target);
    assert!((part - 0.632).abs() < 0.03, "glide time constant: {part}");
    assert!(
        (gain[gain.len() - 1] - target).abs() < 1.0e-4,
        "and arrives"
    );
}
