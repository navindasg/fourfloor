//! Rendered-audio tests for the limiter's true-peak mode, measured with
//! the BS.1770 true-peak meter in the testkit.

mod common;

use common::*;
use fourfloor_core::{Device, ParamId, PrepareConfig};
use fourfloor_effects::Limiter;
use fourfloor_testkit::{
    assert_null, peak_db, render_effect, sine, true_peak_dbtp, white_noise, AllocDisabler,
    StereoBuffer,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn limiter(params: &[(ParamId, f32)]) -> Limiter {
    let mut device = Limiter::new();
    set(&mut device, params);
    device
}

/// Material with inter-sample peaks well above its sample peaks: a tone
/// near a quarter of the sample rate plus bright noise, driven hard.
fn hot_bright() -> StereoBuffer {
    let tone = sine(11_999.0, 0.9, 2.0, RATE);
    let noise = white_noise(7, 0.6, 2.0, RATE);
    let left: Vec<f32> = tone.iter().zip(&noise).map(|(t, n)| t + n).collect();
    let right: Vec<f32> = tone.iter().zip(&noise).map(|(t, n)| t - n).collect();
    StereoBuffer::new(RATE, left, right)
}

#[test]
fn sample_peak_mode_lets_true_peaks_through_and_true_peak_mode_holds_them() {
    let input = hot_bright();
    for ceiling in [-1.0f32, -3.0] {
        let mut plain = limiter(&[(Limiter::CEILING, ceiling)]);
        let plain_out = render_effect(&mut plain, &input, &config());
        let mut tp = limiter(&[(Limiter::CEILING, ceiling), (Limiter::TRUE_PEAK, 1.0)]);
        let tp_out = render_effect(&mut tp, &input, &config());

        let plain_tp = true_peak_dbtp(&plain_out);
        let tp_tp = true_peak_dbtp(&tp_out);
        // Without true-peak detection the sample ceiling holds but the
        // reconstructed waveform goes over it.
        assert!(peak_db(&plain_out.left) <= ceiling + 1e-3);
        assert!(plain_tp > f64::from(ceiling) + 0.5, "plain {plain_tp}");
        // With it, the meter's true peak stays at the ceiling.
        assert!(tp_tp <= f64::from(ceiling) + 0.1, "true-peak mode {tp_tp}");
        assert!(peak_db(&tp_out.left) <= ceiling + 1e-3);
        assert!(peak_db(&tp_out.right) <= ceiling + 1e-3);
    }
}

#[test]
fn true_peak_mode_adds_eight_samples_of_latency_read_in_prepare() {
    let mut device = limiter(&[(Limiter::LOOKAHEAD, 5.0)]);
    device.prepare(&PrepareConfig::stereo(RATE, 512));
    assert_eq!(device.latency_samples(), 240);
    device.set_param(Limiter::TRUE_PEAK, 1.0);
    assert_eq!(device.latency_samples(), 240, "moves only on prepare");
    device.prepare(&PrepareConfig::stereo(RATE, 512));
    assert_eq!(device.latency_samples(), 248);
}

#[test]
fn quiet_material_comes_out_exactly_delayed_by_the_reported_latency() {
    let signal = sine(997.0, 0.25, 1.0, RATE);
    let input = dual_mono(&signal);
    let mut device = limiter(&[(Limiter::TRUE_PEAK, 1.0)]);
    let output = render_effect(&mut device, &input, &config());
    let latency = device.latency_samples() as usize;
    assert_eq!(latency, 248);
    let expected: Vec<f32> = std::iter::repeat_n(0.0, latency)
        .chain(signal.iter().copied())
        .take(signal.len())
        .collect();
    assert_null(&output.left, &expected, -140.0);
    assert_null(&output.right, &expected, -140.0);
}
