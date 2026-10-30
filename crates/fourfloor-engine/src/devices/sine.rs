use std::f64::consts::TAU;

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, SampleRate, TransportContext,
};

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new(
        SineOsc::FREQUENCY,
        "Frequency",
        1.0,
        20_000.0,
        440.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        SineOsc::AMPLITUDE,
        "Amplitude",
        0.0,
        1.0,
        0.5,
        ParamUnit::Generic,
    ),
];

/// A free-running sine oscillator. Writes the same signal to every channel.
/// It ignores the transport: it sounds whether or not the transport rolls.
#[derive(Clone, Debug)]
pub struct SineOsc {
    frequency: f64,
    amplitude: f32,
    /// Phase in cycles, `0.0..1.0`.
    phase: f64,
    sample_rate: f64,
}

impl SineOsc {
    /// Frequency in Hz.
    pub const FREQUENCY: ParamId = ParamId(0);
    /// Linear amplitude.
    pub const AMPLITUDE: ParamId = ParamId(1);

    /// A sine at `frequency` Hz with linear peak `amplitude`.
    pub fn new(frequency: f32, amplitude: f32) -> Self {
        Self {
            frequency: f64::from(frequency),
            amplitude,
            phase: 0.0,
            sample_rate: SampleRate::HZ_48000.as_f64(),
        }
    }
}

impl Device for SineOsc {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.as_f64();
        self.phase = 0.0;
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let step = self.frequency / self.sample_rate;
        let Some((first, rest)) = io.outputs.split_first_mut() else {
            return;
        };
        for sample in first.iter_mut() {
            *sample = self.amplitude * (self.phase * TAU).sin() as f32;
            self.phase = (self.phase + step).fract();
        }
        for channel in rest {
            channel.copy_from_slice(first);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if !value.is_finite() {
            return;
        }
        match param {
            Self::FREQUENCY => self.frequency = f64::from(PARAMS[0].clamp(value)),
            Self::AMPLITUDE => self.amplitude = PARAMS[1].clamp(value),
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run_source, stopped};

    fn run(osc: &mut SineOsc, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![9.0; frames], vec![9.0; frames]);
        run_source(osc, &stopped(), &mut [&mut l, &mut r]);
        (l, r)
    }

    #[test]
    fn quarter_rate_sine_hits_known_samples_on_every_channel() {
        let mut osc = SineOsc::new(12_000.0, 0.5);
        osc.prepare(&PrepareConfig::stereo(SampleRate::HZ_48000, 8));
        let (l, r) = run(&mut osc, 8);
        for (i, expected) in [0.0, 0.5, 0.0, -0.5, 0.0, 0.5, 0.0, -0.5]
            .iter()
            .enumerate()
        {
            assert!((l[i] - expected).abs() < 1e-6, "sample {i} = {}", l[i]);
        }
        assert_eq!(l, r);
    }

    #[test]
    fn phase_is_continuous_across_blocks_and_reset_restarts_it() {
        let mut a = SineOsc::new(441.0, 1.0);
        a.prepare(&PrepareConfig::stereo(SampleRate::HZ_48000, 64));
        let (whole, _) = run(&mut a, 64);
        a.reset();
        let (first, _) = run(&mut a, 20);
        let (second, _) = run(&mut a, 44);
        assert_eq!(whole[..20], first[..]);
        assert_eq!(whole[20..], second[..]);
    }

    #[test]
    fn params_change_frequency_and_amplitude() {
        let mut osc = SineOsc::new(100.0, 1.0);
        osc.prepare(&PrepareConfig::stereo(SampleRate::HZ_48000, 4));
        osc.set_param(SineOsc::FREQUENCY, 12_000.0);
        osc.set_param(SineOsc::AMPLITUDE, 0.25);
        osc.set_param(SineOsc::AMPLITUDE, f32::NAN);
        osc.set_param(ParamId(99), 3.0);
        let (l, _) = run(&mut osc, 4);
        assert!((l[1] - 0.25).abs() < 1e-6);
    }
}
