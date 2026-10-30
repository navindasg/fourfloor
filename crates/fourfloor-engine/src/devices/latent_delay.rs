use fourfloor_core::{Device, PrepareConfig, ProcessIo, TransportContext};

use crate::delay::DelayLine;

/// Delays its input by a fixed number of samples and reports that delay as
/// latency, like a look-ahead plugin does. It exists so delay compensation
/// can be exercised without hosting a real plugin.
#[derive(Clone, Debug)]
pub struct LatentDelay {
    line: DelayLine,
    latency: u32,
}

impl LatentDelay {
    /// A delay of `samples` samples. The delay line is allocated in
    /// `prepare`, once the channel count is known.
    pub fn new(samples: u32) -> Self {
        Self {
            line: DelayLine::new(0, samples as usize),
            latency: samples,
        }
    }
}

impl Device for LatentDelay {
    fn prepare(&mut self, config: &PrepareConfig) {
        if self.line.channels() == config.channels {
            self.line.clear();
        } else {
            self.line = DelayLine::new(config.channels, self.latency as usize);
        }
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        for (channel, output) in io.outputs.iter_mut().enumerate() {
            match io.inputs.get(channel) {
                Some(input) => self.line.run(channel, input, output, false),
                None => output.fill(0.0),
            }
        }
        self.line.advance(frames);
    }

    fn reset(&mut self) {
        self.line.clear();
    }

    fn latency_samples(&self) -> u32 {
        self.latency
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped, RATE};

    #[test]
    fn delays_every_channel_and_reports_latency() {
        let mut delay = LatentDelay::new(2);
        delay.prepare(&PrepareConfig::stereo(RATE, 4));
        assert_eq!(delay.latency_samples(), 2);
        let (mut l, mut r) = ([9.0f32; 4], [9.0f32; 4]);
        run(
            &mut delay,
            &stopped(),
            &[&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0]],
            None,
            &mut [&mut l, &mut r],
            &[],
        );
        assert_eq!(l, [0.0, 0.0, 1.0, 2.0]);
        assert_eq!(r, [0.0, 0.0, 5.0, 6.0]);
        delay.reset();
        run(
            &mut delay,
            &stopped(),
            &[&[0.0; 4], &[0.0; 4]],
            None,
            &mut [&mut l, &mut r],
            &[],
        );
        assert_eq!(l, [0.0; 4]);
    }
}
