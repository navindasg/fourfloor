//! Running a wrapped device into a preallocated scratch buffer, for the
//! DJ devices that blend a wrapped effect with their dry input.

use fourfloor_core::{Device, EventBuffer, ProcessIo, TransportContext};
use fourfloor_engine::MAX_CHANNELS;

/// Runs `device` on `inputs`, writing `channels` channels of `frames`
/// frames into `scratch` (channel `c` at `c * stride`). No allocation.
pub(crate) fn process_into(
    device: &mut dyn Device,
    ctx: &TransportContext,
    inputs: &[&[f32]],
    scratch: &mut [f32],
    stride: usize,
    frames: usize,
    events_out: &mut EventBuffer,
) {
    let mut outputs: [&mut [f32]; MAX_CHANNELS] = Default::default();
    let mut used = 0;
    for (slot, region) in outputs.iter_mut().zip(scratch.chunks_exact_mut(stride)) {
        *slot = &mut region[..frames];
        used += 1;
    }
    device.process(
        ctx,
        &mut ProcessIo {
            inputs,
            sidechain: None,
            outputs: &mut outputs[..used],
            events: &[],
            events_out,
        },
    );
}

/// One step of a one-pole glide from `value` towards `target`. Lands
/// exactly on the target once within 1e-6, or once f32 rounding stops it
/// moving (with a small coefficient that happens well before 1e-6 near
/// unity), so a settled gain is exact.
#[inline]
pub(crate) fn glide(value: f32, target: f32, coefficient: f32) -> f32 {
    let next = value + (target - value) * coefficient;
    if next == value || (target - next).abs() < 1e-6 {
        target
    } else {
        next
    }
}

/// The `index`-th sample of channel `channel` of a scratch buffer.
#[inline]
pub(crate) fn at(scratch: &[f32], stride: usize, channel: usize, index: usize) -> f32 {
    scratch[channel * stride + index]
}

#[cfg(test)]
mod tests {
    use super::glide;

    #[test]
    fn glide_lands_exactly_on_its_target() {
        // A 5 ms glide at 48 kHz: small enough that f32 rounding stalls it
        // short of the 1e-6 window near unity.
        let coefficient = 1.0 - (-1.0f64 / 240.0).exp() as f32;
        for (start, target) in [(0.0, 1.0), (1.0, 0.5), (0.5, 4.0), (1.0, 0.0)] {
            let mut value: f32 = start;
            for _ in 0..48_000 {
                value = glide(value, target, coefficient);
            }
            assert_eq!(value, target, "{start} -> {target}");
        }
    }
}
