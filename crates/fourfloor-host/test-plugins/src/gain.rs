//! `org.fourfloor.test.gain`: stereo gain with one parameter and saved state.

use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::plugin::features::{AUDIO_EFFECT, STEREO};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};

use crate::{AtomicF64, InstanceGuard};

/// Id of the gain parameter.
pub const PARAM_GAIN: u32 = 1;
/// The gain parameter's range and default (linear factor).
pub const GAIN_RANGE: (f64, f64, f64) = (0.0, 4.0, 1.0);
/// First bytes of a saved state, so a foreign blob is refused.
const STATE_MAGIC: &[u8; 4] = b"FFG1";

pub fn descriptor() -> PluginDescriptor {
    PluginDescriptor::new("org.fourfloor.test.gain", "Fourfloor Test Gain")
        .with_vendor("fourfloor")
        .with_version("0.1.0")
        .with_features([AUDIO_EFFECT, STEREO])
}

pub struct GainPlugin;

impl Plugin for GainPlugin {
    type AudioProcessor<'a> = GainProcessor<'a>;
    type Shared<'a> = GainShared;
    type MainThread<'a> = GainMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&GainShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

pub struct GainShared {
    gain: AtomicF64,
}

impl GainShared {
    pub fn new() -> Self {
        Self {
            gain: AtomicF64::new(GAIN_RANGE.2),
        }
    }
}

impl PluginShared<'_> for GainShared {}

pub struct GainMainThread<'a> {
    shared: &'a GainShared,
    _guard: InstanceGuard,
}

impl<'a> GainMainThread<'a> {
    pub fn new(shared: &'a GainShared) -> Self {
        Self {
            shared,
            _guard: InstanceGuard::new(),
        }
    }
}

impl<'a> PluginMainThread<'a, GainShared> for GainMainThread<'a> {}

/// Applies every gain change in `events` to `shared`.
fn apply_param_events(shared: &GainShared, events: &InputEvents) {
    for event in events {
        if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event() {
            if change.param_id() == Some(ClapId::new(PARAM_GAIN)) {
                shared
                    .gain
                    .set(change.value().clamp(GAIN_RANGE.0, GAIN_RANGE.1));
            }
        }
    }
}

impl PluginMainThreadParams for GainMainThread<'_> {
    fn count(&self) -> u32 {
        1
    }

    fn get_info(&self, param_index: u32, info: &mut ParamInfoWriter) {
        if param_index == 0 {
            info.set(&ParamInfo {
                id: ClapId::new(PARAM_GAIN),
                flags: ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Default::default(),
                name: b"Gain",
                module: b"",
                min_value: GAIN_RANGE.0,
                max_value: GAIN_RANGE.1,
                default_value: GAIN_RANGE.2,
            });
        }
    }

    fn get_value(&self, param_id: ClapId) -> Option<f64> {
        (param_id == PARAM_GAIN).then(|| self.shared.gain.get())
    }

    fn value_to_text(
        &self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> core::fmt::Result {
        if param_id == PARAM_GAIN {
            write!(writer, "{value:.2} x")
        } else {
            Err(core::fmt::Error)
        }
    }

    fn text_to_value(&self, param_id: ClapId, text: &CStr) -> Option<f64> {
        if param_id != PARAM_GAIN {
            return None;
        }
        text.to_str()
            .ok()?
            .trim_end_matches(" x")
            .trim()
            .parse()
            .ok()
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        apply_param_events(self.shared, input);
    }
}

impl PluginStateImpl for GainMainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(STATE_MAGIC)?;
        output.write_all(&self.shared.gain.get().to_le_bytes())?;
        Ok(())
    }

    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut magic = [0u8; 4];
        input.read_exact(&mut magic)?;
        if &magic != STATE_MAGIC {
            return Err(PluginError::Message("not a test gain state"));
        }
        let mut value = [0u8; 8];
        input.read_exact(&mut value)?;
        let gain = f64::from_le_bytes(value);
        if !gain.is_finite() {
            return Err(PluginError::Message("gain is not a number"));
        }
        self.shared.gain.set(gain.clamp(GAIN_RANGE.0, GAIN_RANGE.1));
        Ok(())
    }
}

impl PluginAudioPortsImpl for GainMainThread<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

pub struct GainProcessor<'a> {
    shared: &'a GainShared,
}

impl<'a> PluginAudioProcessor<'a, GainShared, GainMainThread<'a>> for GainProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &GainMainThread<'a>,
        shared: &'a GainShared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self { shared })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let start_gain = self.shared.gain.get();
        // Each channel replays the block's gain changes at their own frame,
        // so a change lands on the same sample in every channel.
        let gain_at = |frame: usize, cursor: &mut u32, gain: &mut f64| {
            while let Some(event) = events.input.get(*cursor) {
                if event.header().time() as usize > frame {
                    break;
                }
                if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event() {
                    if change.param_id() == Some(ClapId::new(PARAM_GAIN)) {
                        *gain = change.value().clamp(GAIN_RANGE.0, GAIN_RANGE.1);
                    }
                }
                *cursor += 1;
            }
            *gain as f32
        };

        for mut port_pair in &mut audio {
            let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                continue;
            };
            for channel_pair in channel_pairs {
                let (mut cursor, mut gain) = (0u32, start_gain);
                match channel_pair {
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(output) => output.fill(0.0),
                    ChannelPair::InputOutput(input, output) => {
                        for (frame, (input, output)) in input.iter().zip(output).enumerate() {
                            *output = input * gain_at(frame, &mut cursor, &mut gain);
                        }
                    }
                    ChannelPair::InPlace(buffer) => {
                        for (frame, sample) in buffer.iter_mut().enumerate() {
                            *sample *= gain_at(frame, &mut cursor, &mut gain);
                        }
                    }
                }
            }
        }

        apply_param_events(self.shared, events.input);
        Ok(ProcessStatus::ContinueIfNotQuiet)
    }
}

impl PluginAudioProcessorParams for GainProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        apply_param_events(self.shared, input);
    }
}
