use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

/// Multiplies its single input port by a linear gain, channel for channel.
/// A negative gain inverts polarity.
#[derive(Clone, Debug)]
pub struct Gain {
    gain: f32,
}

static PARAMS: [ParamInfo; 1] = [ParamInfo::new(
    Gain::GAIN,
    "Gain",
    -16.0,
    16.0,
    1.0,
    ParamUnit::Generic,
)];

impl Gain {
    /// Linear gain, -16 to 16.
    pub const GAIN: ParamId = ParamId(0);

    /// A gain stage with the given linear gain.
    pub fn new(gain: f32) -> Self {
        Self { gain }
    }
}

impl Device for Gain {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        for (channel, output) in io.outputs.iter_mut().enumerate() {
            match io.inputs.get(channel) {
                Some(input) => {
                    for (o, i) in output.iter_mut().zip(input.iter()) {
                        *o = *i * self.gain;
                    }
                }
                None => output.fill(0.0),
            }
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if param == Self::GAIN && value.is_finite() {
            self.gain = PARAMS[0].clamp(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped, RATE};

    #[test]
    fn scales_each_channel_and_silences_unfed_channels() {
        let mut gain = Gain::new(2.0);
        gain.prepare(&PrepareConfig::stereo(RATE, 2));
        gain.set_param(Gain::GAIN, -0.5);
        gain.set_param(Gain::GAIN, f32::INFINITY);
        let (mut l, mut r) = ([9.0f32; 2], [9.0f32; 2]);
        run(
            &mut gain,
            &stopped(),
            &[&[1.0, -2.0], &[4.0, 8.0]],
            None,
            &mut [&mut l, &mut r],
            &[],
        );
        assert_eq!(l, [-0.5, 1.0]);
        assert_eq!(r, [-2.0, -4.0], "the right channel is its own signal");

        run(
            &mut gain,
            &stopped(),
            &[&[1.0, -2.0]],
            None,
            &mut [&mut l, &mut r],
            &[],
        );
        assert_eq!(r, [0.0, 0.0]);
        assert_eq!(gain.params()[0].default, 1.0);
    }
}
