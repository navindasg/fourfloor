use fourfloor_core::{Device, ParamId, PrepareConfig, ProcessIo, TransportContext};

/// Sums a fixed number of input ports, each with its own linear gain,
/// channel for channel.
///
/// Add it to the graph with the same number of input ports it was created
/// with. The gain of port `n` is parameter `ParamId(n)`.
#[derive(Clone, Debug)]
pub struct Mixer {
    gains: Vec<f32>,
}

impl Mixer {
    /// A mixer with `ports` inputs, all at unity gain.
    pub fn new(ports: usize) -> Self {
        Self {
            gains: vec![1.0; ports],
        }
    }

    /// The parameter that sets the gain of input port `port`.
    pub fn port_gain(port: usize) -> ParamId {
        ParamId(port as u32)
    }

    /// Number of input ports.
    pub fn ports(&self) -> usize {
        self.gains.len()
    }
}

impl Device for Mixer {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let channels = io.outputs.len();
        for (channel, output) in io.outputs.iter_mut().enumerate() {
            output.fill(0.0);
            for (port, gain) in self.gains.iter().enumerate() {
                let Some(input) = io.inputs.get(port * channels + channel) else {
                    break;
                };
                for (o, i) in output.iter_mut().zip(input.iter()) {
                    *o += *i * *gain;
                }
            }
        }
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if let (Some(gain), true) = (self.gains.get_mut(param.get() as usize), value.is_finite()) {
            *gain = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped};

    #[test]
    fn sums_ports_per_channel_with_gains() {
        let mut mixer = Mixer::new(2);
        assert_eq!(mixer.ports(), 2);
        mixer.set_param(Mixer::port_gain(1), 0.5);
        mixer.set_param(Mixer::port_gain(7), 0.5); // ignored
        let (mut l, mut r) = ([9.0f32; 2], [9.0f32; 2]);
        // port 0: L=[1,2] R=[3,4]; port 1: L=[10,20] R=[30,40]
        let inputs: [&[f32]; 4] = [&[1.0, 2.0], &[3.0, 4.0], &[10.0, 20.0], &[30.0, 40.0]];
        run(
            &mut mixer,
            &stopped(),
            &inputs,
            None,
            &mut [&mut l, &mut r],
            &[],
        );
        assert_eq!(l, [6.0, 12.0]);
        assert_eq!(r, [18.0, 24.0]);
    }

    #[test]
    fn missing_ports_count_as_silence() {
        let mut mixer = Mixer::new(3);
        let mut out = [9.0f32; 2];
        run(
            &mut mixer,
            &stopped(),
            &[&[1.0, 1.0]],
            None,
            &mut [&mut out],
            &[],
        );
        assert_eq!(out, [1.0, 1.0]);
    }
}
