//! `org.fourfloor.test.delay`: a fixed delay that reports itself as latency.
//!
//! The output is the input moved [`DELAY_SAMPLES`] frames later, bit for
//! bit, so a host that compensates the reported latency correctly gets a
//! perfect null against the undelayed signal.

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_plugin::plugin::features::{AUDIO_EFFECT, STEREO};
use clack_plugin::prelude::*;

use crate::InstanceGuard;

/// The delay, and the reported latency, in frames. Deliberately not a power
/// of two and not a block size.
pub const DELAY_SAMPLES: usize = 441;
const CHANNELS: usize = 2;

pub fn descriptor() -> PluginDescriptor {
    PluginDescriptor::new("org.fourfloor.test.delay", "Fourfloor Test Latent Delay")
        .with_vendor("fourfloor")
        .with_version("0.1.0")
        .with_features([AUDIO_EFFECT, STEREO])
}

pub struct DelayPlugin;

impl Plugin for DelayPlugin {
    type AudioProcessor<'a> = DelayProcessor;
    type Shared<'a> = ();
    type MainThread<'a> = DelayMainThread;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&()>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginLatency>();
    }
}

pub struct DelayMainThread {
    _guard: InstanceGuard,
}

impl DelayMainThread {
    pub fn new() -> Self {
        Self {
            _guard: InstanceGuard::new(),
        }
    }
}

impl PluginMainThread<'_, ()> for DelayMainThread {}

impl PluginLatencyImpl for DelayMainThread {
    fn get(&self) -> u32 {
        DELAY_SAMPLES as u32
    }
}

impl PluginAudioPortsImpl for DelayMainThread {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"main",
                channel_count: CHANNELS as u32,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

/// One channel's delay memory: a ring of exactly [`DELAY_SAMPLES`] frames.
struct Ring {
    samples: Vec<f32>,
    position: usize,
}

impl Ring {
    fn new() -> Self {
        Self {
            samples: vec![0.0; DELAY_SAMPLES],
            position: 0,
        }
    }

    fn clear(&mut self) {
        self.samples.fill(0.0);
        self.position = 0;
    }

    /// Stores `input` and returns the sample from `DELAY_SAMPLES` frames ago.
    fn tick(&mut self, input: f32) -> f32 {
        let output = std::mem::replace(&mut self.samples[self.position], input);
        self.position = (self.position + 1) % DELAY_SAMPLES;
        output
    }
}

pub struct DelayProcessor {
    rings: [Ring; CHANNELS],
}

impl<'a> PluginAudioProcessor<'a, (), DelayMainThread> for DelayProcessor {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &DelayMainThread,
        _shared: &'a (),
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            rings: [Ring::new(), Ring::new()],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let Some(mut port_pair) = audio.port_pair(0) else {
            return Ok(ProcessStatus::Continue);
        };
        let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
            return Ok(ProcessStatus::Continue);
        };
        for (channel_pair, ring) in channel_pairs.into_iter().zip(&mut self.rings) {
            match channel_pair {
                ChannelPair::InputOnly(_) => {}
                ChannelPair::OutputOnly(output) => {
                    for sample in output {
                        *sample = ring.tick(0.0);
                    }
                }
                ChannelPair::InputOutput(input, output) => {
                    for (input, output) in input.iter().zip(output) {
                        *output = ring.tick(*input);
                    }
                }
                ChannelPair::InPlace(buffer) => {
                    for sample in buffer {
                        *sample = ring.tick(*sample);
                    }
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        for ring in &mut self.rings {
            ring.clear();
        }
    }
}
