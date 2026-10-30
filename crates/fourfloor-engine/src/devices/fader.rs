use std::f32::consts::FRAC_PI_2;

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

/// A channel strip's fader: gain in decibels, stereo balance and mute.
///
/// Balance law: at centre both channels pass at unity. Panning left leaves
/// the left channel alone and turns the right channel down along a cosine,
/// reaching silence at hard left (and the mirror image to the right). With
/// one channel, or more than two, the pan is ignored.
///
/// Parameter changes are smoothed over about 5 ms so automation and live
/// tweaks do not click.
#[derive(Clone, Debug)]
pub struct Fader {
    gain_db: f32,
    pan: f32,
    mute: bool,
    /// Gain currently applied to the left and right channels.
    current: [f32; 2],
    /// One-pole smoothing coefficient per sample.
    smoothing: f32,
    primed: bool,
}

static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new(
        Fader::GAIN_DB,
        "Gain",
        Fader::MIN_DB,
        24.0,
        0.0,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(Fader::PAN, "Pan", -1.0, 1.0, 0.0, ParamUnit::Generic),
    ParamInfo::toggle(Fader::MUTE, "Mute", false),
];

impl Default for Fader {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

impl Fader {
    /// Gain in decibels, -96 to 24. -96 is silence.
    pub const GAIN_DB: ParamId = ParamId(0);
    /// Balance, -1 (left) to 1 (right).
    pub const PAN: ParamId = ParamId(1);
    /// Mute: 1 silences the output.
    pub const MUTE: ParamId = ParamId(2);

    /// The bottom of the fader; this value and below are silence.
    pub const MIN_DB: f32 = -96.0;
    const SMOOTHING_SECONDS: f32 = 0.005;

    /// A fader at `gain_db` decibels with the given pan.
    pub fn new(gain_db: f32, pan: f32) -> Self {
        Self {
            gain_db: PARAMS[0].clamp(gain_db),
            pan: PARAMS[1].clamp(pan),
            mute: false,
            current: [0.0; 2],
            smoothing: 1.0,
            primed: false,
        }
    }

    /// The same fader, muted or not.
    pub fn with_mute(mut self, mute: bool) -> Self {
        self.mute = mute;
        self
    }

    /// Linear gains for the left and right channels.
    fn targets(&self) -> [f32; 2] {
        if self.mute || self.gain_db <= Self::MIN_DB {
            return [0.0; 2];
        }
        let gain = 10.0f32.powf(self.gain_db / 20.0);
        let left = (self.pan.max(0.0) * FRAC_PI_2).cos();
        let right = ((-self.pan).max(0.0) * FRAC_PI_2).cos();
        [gain * left, gain * right]
    }
}

impl Device for Fader {
    fn prepare(&mut self, config: &PrepareConfig) {
        let samples = Self::SMOOTHING_SECONDS * config.sample_rate.hz() as f32;
        self.smoothing = 1.0 - (-1.0 / samples.max(1.0)).exp();
        self.primed = false;
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let targets = self.targets();
        if !self.primed {
            // Start at the target: a fresh graph must not fade in.
            self.current = targets;
            self.primed = true;
        }
        let stereo = io.outputs.len() == 2;
        let mut reached = self.current;
        for (channel, output) in io.outputs.iter_mut().enumerate() {
            let Some(input) = io.inputs.get(channel) else {
                output.fill(0.0);
                continue;
            };
            let side = if stereo { channel } else { 0 };
            let target = if stereo {
                targets[side]
            } else {
                targets[0].max(targets[1])
            };
            let mut gain = self.current[side];
            if gain == target {
                for (o, i) in output.iter_mut().zip(input.iter()) {
                    *o = *i * gain;
                }
            } else {
                for (o, i) in output.iter_mut().zip(input.iter()) {
                    gain += (target - gain) * self.smoothing;
                    *o = *i * gain;
                }
                if (gain - target).abs() < 1e-6 {
                    gain = target;
                }
            }
            reached[side] = gain;
        }
        self.current = reached;
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if !value.is_finite() {
            return;
        }
        match param {
            Self::GAIN_DB => self.gain_db = PARAMS[0].clamp(value),
            Self::PAN => self.pan = PARAMS[1].clamp(value),
            Self::MUTE => self.mute = value >= 0.5,
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.primed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped, RATE};

    fn pass(fader: &mut Fader, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let input = vec![1.0f32; frames];
        let (mut l, mut r) = (vec![9.0f32; frames], vec![9.0f32; frames]);
        run(
            fader,
            &stopped(),
            &[&input, &input],
            None,
            &mut [&mut l, &mut r],
            &[],
        );
        (l, r)
    }

    fn prepared(gain_db: f32, pan: f32) -> Fader {
        let mut fader = Fader::new(gain_db, pan);
        fader.prepare(&PrepareConfig::stereo(RATE, 4096));
        fader
    }

    #[test]
    fn centre_is_unity_and_gain_is_in_decibels() {
        let (l, r) = pass(&mut prepared(0.0, 0.0), 8);
        assert_eq!((l[0], r[0]), (1.0, 1.0));
        let (l, r) = pass(&mut prepared(-6.0, 0.0), 8);
        assert!((l[0] - 0.501_187).abs() < 1e-5);
        assert_eq!(l, r);
        let (l, _) = pass(&mut prepared(Fader::MIN_DB, 0.0), 8);
        assert_eq!(l[0], 0.0, "the bottom of the fader is silence");
    }

    #[test]
    fn pan_turns_down_the_opposite_channel_only() {
        let (l, r) = pass(&mut prepared(0.0, -1.0), 8);
        assert_eq!(l[0], 1.0);
        assert!(r[0].abs() < 1e-6, "hard left silences the right: {}", r[0]);
        let (l, r) = pass(&mut prepared(0.0, 0.5), 8);
        assert_eq!(r[0], 1.0);
        assert!((l[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn parameter_changes_are_smoothed_and_settle() {
        let mut fader = prepared(0.0, 0.0);
        pass(&mut fader, 8);
        fader.set_param(Fader::MUTE, 1.0);
        let (l, _) = pass(&mut fader, 4096);
        assert!(l[0] > 0.9, "no jump on the first sample: {}", l[0]);
        assert!(l.windows(2).all(|w| w[1] <= w[0]), "monotonic fade");
        assert!(l[4095] < 1e-3);
        let (l, _) = pass(&mut fader, 4096);
        assert_eq!(l[4095], 0.0, "settles exactly");

        fader.set_param(Fader::MUTE, 0.0);
        fader.set_param(Fader::PAN, f32::NAN);
        fader.set_param(Fader::GAIN_DB, 100.0);
        assert_eq!(fader.gain_db, 24.0, "clamped");
        fader.reset();
        let (l, _) = pass(&mut fader, 4);
        assert!((l[0] - 15.848_93).abs() < 1e-3, "reset jumps to the target");
    }

    #[test]
    fn mono_ignores_pan_and_missing_inputs_are_silent() {
        let mut fader = Fader::new(0.0, -1.0).with_mute(false);
        fader.prepare(&PrepareConfig::new(RATE, 4, 1));
        let mut out = [9.0f32; 4];
        run(
            &mut fader,
            &stopped(),
            &[&[0.5; 4]],
            None,
            &mut [&mut out],
            &[],
        );
        assert_eq!(out, [0.5; 4]);
        run(&mut fader, &stopped(), &[], None, &mut [&mut out], &[]);
        assert_eq!(out, [0.0; 4]);
        assert_eq!(fader.params().len(), 3);
    }
}
