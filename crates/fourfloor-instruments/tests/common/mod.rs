//! Helpers shared by the rendered-audio tests.
#![allow(dead_code)]

use fourfloor_core::{Device, SampleRate};
use fourfloor_testkit::{notes, render_instrument, RenderConfig, StereoBuffer};

pub const RATE: SampleRate = SampleRate::HZ_48000;

/// Renders `(start, length, pitch, velocity)` notes through `device`.
pub fn play(device: &mut dyn Device, specs: &[(f64, f64, u8, u8)], seconds: f64) -> StereoBuffer {
    let events = notes(specs, RATE);
    render_instrument(device, &events, seconds, &RenderConfig::default())
}

/// The part of `signal` between two times.
pub fn window(signal: &[f32], start: f64, end: f64) -> &[f32] {
    let a = ((start * RATE.as_f64()) as usize).min(signal.len());
    let b = ((end * RATE.as_f64()) as usize).min(signal.len());
    &signal[a..b]
}

/// Times, in seconds, of the upward zero crossings of `signal`.
pub fn rising_crossings(signal: &[f32]) -> Vec<f64> {
    let mut times = Vec::new();
    for (n, pair) in signal.windows(2).enumerate() {
        if pair[0] < 0.0 && pair[1] >= 0.0 {
            let frac = f64::from(-pair[0]) / f64::from(pair[1] - pair[0]);
            times.push((n as f64 + frac) / RATE.as_f64());
        }
    }
    times
}

/// Frequency of a near-sinusoidal `signal` from its zero crossings: far
/// finer than an FFT bin on a short window. 0.0 with fewer than two.
pub fn crossing_frequency(signal: &[f32]) -> f64 {
    let times = rising_crossings(signal);
    match (times.first(), times.last()) {
        (Some(first), Some(last)) if times.len() > 1 => (times.len() - 1) as f64 / (last - first),
        _ => 0.0,
    }
}

/// MIDI pitch to Hz.
pub fn hz(pitch: u8) -> f64 {
    440.0 * 2.0f64.powf((f64::from(pitch) - 69.0) / 12.0)
}

/// Difference between two frequencies in cents.
pub fn cents(found: f64, expected: f64) -> f64 {
    1_200.0 * (found / expected).log2()
}

/// Two cascaded one-pole lowpasses at `cutoff`: strips harmonics so the
/// zero crossings follow the fundamental.
pub fn lowpass(signal: &[f32], cutoff: f64) -> Vec<f32> {
    let k = 1.0 - (-std::f64::consts::TAU * cutoff / RATE.as_f64()).exp();
    let (mut a, mut b) = (0.0f64, 0.0f64);
    signal
        .iter()
        .map(|x| {
            a += k * (f64::from(*x) - a);
            b += k * (a - b);
            b as f32
        })
        .collect()
}

/// Fundamental of a harmonic tone between two times, from the zero
/// crossings of its lowpassed copy. `near` is roughly where to look.
pub fn fundamental(signal: &[f32], start: f64, end: f64, near: f64) -> f64 {
    let smooth = lowpass(signal, near);
    crossing_frequency(window(&smooth, start, end))
}
