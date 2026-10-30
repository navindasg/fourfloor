//! Helpers shared by the effect tests.
#![allow(dead_code)]

use fourfloor_core::{
    Device, EventBuffer, ParamId, PrepareConfig, ProcessIo, SampleRate, Tempo, TransportContext,
};
use fourfloor_testkit::{
    dominant_frequency, envelope, render_effect, rms, sine, RenderConfig, StereoBuffer,
};

pub const RATE: SampleRate = SampleRate::HZ_48000;

/// Seconds to a frame index at [`RATE`].
pub fn frames(seconds: f64) -> usize {
    (seconds * RATE.as_f64()).round() as usize
}

pub fn config() -> RenderConfig {
    RenderConfig::default()
}

pub fn at_bpm(bpm: f64) -> RenderConfig {
    RenderConfig::default().with_tempo(Tempo::new(bpm).expect("valid tempo"))
}

/// Sets several parameters.
pub fn set(device: &mut dyn Device, params: &[(ParamId, f32)]) {
    for &(id, value) in params {
        device.set_param(id, value);
    }
}

/// The same signal on both channels.
pub fn dual_mono(signal: &[f32]) -> StereoBuffer {
    StereoBuffer::from_mono(RATE, signal)
}

/// Renders `signal` (dual mono) through the device and returns the left
/// channel.
pub fn render_mono(device: &mut dyn Device, signal: &[f32]) -> Vec<f32> {
    render_effect(device, &dual_mono(signal), &config()).left
}

/// Steady-state gain of the device for a sine at `hz`, in decibels,
/// measured over the second half of a one-second render.
pub fn tone_gain_db(device: &mut dyn Device, hz: f64) -> f32 {
    tone_gain_db_at_level(device, hz, 0.25)
}

pub fn tone_gain_db_at_level(device: &mut dyn Device, hz: f64, amplitude: f32) -> f32 {
    let input = sine(hz, amplitude, 1.0, RATE);
    let output = render_mono(device, &input);
    let half = input.len() / 2;
    20.0 * (rms(&output[half..]) / rms(&input[half..])).log10()
}

/// How many times a second the level of `signal` rises and falls: the
/// strongest frequency of its envelope.
pub fn modulation_rate_hz(signal: &[f32], release_ms: f32) -> f32 {
    let level = envelope(signal, RATE, 1.0, release_ms);
    dominant_frequency(&level, RATE)
}

/// Index of the largest absolute sample in `signal[start..end]`.
pub fn peak_index(signal: &[f32], start: usize, end: usize) -> usize {
    let end = end.min(signal.len());
    (start..end)
        .max_by(|a, b| signal[*a].abs().total_cmp(&signal[*b].abs()))
        .unwrap_or(start)
}

/// Drives a device directly, one block, with any number of channels.
/// `inputs` may be empty (nothing connected). Returns the output channels.
pub fn run_block(
    device: &mut dyn Device,
    ctx: &TransportContext,
    inputs: &[&[f32]],
    channels: usize,
    block: usize,
) -> Vec<Vec<f32>> {
    let mut out = vec![vec![f32::NAN; block]; channels];
    let mut events_out = EventBuffer::with_capacity(16);
    {
        let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(|c| c.as_mut_slice()).collect();
        let mut io = ProcessIo {
            inputs,
            sidechain: None,
            outputs: &mut outputs,
            events: &[],
            events_out: &mut events_out,
        };
        device.process(ctx, &mut io);
    }
    out
}

pub fn stopped() -> TransportContext {
    TransportContext::stopped(RATE, Tempo::DEFAULT)
}

pub fn prepare(device: &mut dyn Device, block: usize) {
    device.prepare(&PrepareConfig::stereo(RATE, block));
}
