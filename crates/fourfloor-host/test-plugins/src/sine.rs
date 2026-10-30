//! `org.fourfloor.test.sine`: a polyphonic sine instrument.
//!
//! Each held note is one sine at the note's equal-tempered frequency, with a
//! short linear attack and release so notes do not click. Notes arrive as
//! CLAP note events or raw MIDI, and start on the exact frame of the event.

use std::f64::consts::TAU;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::UnknownEvent;
use clack_plugin::plugin::features::{INSTRUMENT, STEREO, SYNTHESIZER};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};

use crate::{AtomicF64, InstanceGuard};

/// Id of the output level parameter.
pub const PARAM_LEVEL: u32 = 1;
/// The level parameter's range and default (linear amplitude of one note at
/// full velocity).
pub const LEVEL_RANGE: (f64, f64, f64) = (0.0, 1.0, 0.5);
const VOICES: usize = 16;
/// Attack and release time.
const RAMP_SECONDS: f64 = 0.003;
const STATE_MAGIC: &[u8; 4] = b"FFS1";

pub fn descriptor() -> PluginDescriptor {
    PluginDescriptor::new("org.fourfloor.test.sine", "Fourfloor Test Sine")
        .with_vendor("fourfloor")
        .with_version("0.1.0")
        .with_features([INSTRUMENT, SYNTHESIZER, STEREO])
}

pub struct SinePlugin;

impl Plugin for SinePlugin {
    type AudioProcessor<'a> = SineProcessor<'a>;
    type Shared<'a> = SineShared;
    type MainThread<'a> = SineMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&SineShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

pub struct SineShared {
    level: AtomicF64,
}

impl SineShared {
    pub fn new() -> Self {
        Self {
            level: AtomicF64::new(LEVEL_RANGE.2),
        }
    }

    fn apply(&self, event: &UnknownEvent) {
        if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event() {
            if change.param_id() == Some(ClapId::new(PARAM_LEVEL)) {
                self.level
                    .set(change.value().clamp(LEVEL_RANGE.0, LEVEL_RANGE.1));
            }
        }
    }
}

impl PluginShared<'_> for SineShared {}

pub struct SineMainThread<'a> {
    shared: &'a SineShared,
    _guard: InstanceGuard,
}

impl<'a> SineMainThread<'a> {
    pub fn new(shared: &'a SineShared) -> Self {
        Self {
            shared,
            _guard: InstanceGuard::new(),
        }
    }
}

impl<'a> PluginMainThread<'a, SineShared> for SineMainThread<'a> {}

impl PluginMainThreadParams for SineMainThread<'_> {
    fn count(&self) -> u32 {
        1
    }

    fn get_info(&self, param_index: u32, info: &mut ParamInfoWriter) {
        if param_index == 0 {
            info.set(&ParamInfo {
                id: ClapId::new(PARAM_LEVEL),
                flags: ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Default::default(),
                name: b"Level",
                module: b"",
                min_value: LEVEL_RANGE.0,
                max_value: LEVEL_RANGE.1,
                default_value: LEVEL_RANGE.2,
            });
        }
    }

    fn get_value(&self, param_id: ClapId) -> Option<f64> {
        (param_id == PARAM_LEVEL).then(|| self.shared.level.get())
    }

    fn value_to_text(
        &self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> core::fmt::Result {
        if param_id == PARAM_LEVEL {
            write!(writer, "{value:.3}")
        } else {
            Err(core::fmt::Error)
        }
    }

    fn text_to_value(&self, param_id: ClapId, text: &CStr) -> Option<f64> {
        if param_id != PARAM_LEVEL {
            return None;
        }
        text.to_str().ok()?.trim().parse().ok()
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.apply(event);
        }
    }
}

impl PluginStateImpl for SineMainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(STATE_MAGIC)?;
        output.write_all(&self.shared.level.get().to_le_bytes())?;
        Ok(())
    }

    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut magic = [0u8; 4];
        input.read_exact(&mut magic)?;
        if &magic != STATE_MAGIC {
            return Err(PluginError::Message("not a test sine state"));
        }
        let mut value = [0u8; 8];
        input.read_exact(&mut value)?;
        let level = f64::from_le_bytes(value);
        if !level.is_finite() {
            return Err(PluginError::Message("level is not a number"));
        }
        self.shared
            .level
            .set(level.clamp(LEVEL_RANGE.0, LEVEL_RANGE.1));
        Ok(())
    }
}

impl PluginAudioPortsImpl for SineMainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        u32::from(!is_input)
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && !is_input {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"out",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for SineMainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        u32::from(is_input)
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && is_input {
            writer.set(&NotePortInfo {
                id: ClapId::new(1),
                name: b"notes",
                supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    key: u8,
    held: bool,
    phase: f64,
    increment: f64,
    velocity: f64,
    /// Envelope position, 0 (silent) to 1 (full).
    envelope: f64,
}

impl Voice {
    fn is_free(&self) -> bool {
        !self.held && self.envelope <= 0.0
    }
}

pub struct SineProcessor<'a> {
    shared: &'a SineShared,
    voices: [Voice; VOICES],
    sample_rate: f64,
    /// Envelope change per frame.
    ramp: f64,
    mono: Vec<f32>,
}

impl SineProcessor<'_> {
    fn note_on(&mut self, key: u8, velocity: f64) {
        let index = self
            .voices
            .iter()
            .position(|v| v.held && v.key == key)
            .or_else(|| self.voices.iter().position(Voice::is_free))
            .unwrap_or(0);
        let hz = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0);
        self.voices[index] = Voice {
            key,
            held: true,
            phase: 0.0,
            increment: TAU * hz / self.sample_rate,
            velocity: velocity.clamp(0.0, 1.0),
            envelope: 0.0,
        };
    }

    fn note_off(&mut self, key: Option<u8>) {
        for voice in &mut self.voices {
            if key.is_none_or(|key| voice.key == key) {
                voice.held = false;
            }
        }
    }

    fn handle(&mut self, event: &UnknownEvent) {
        match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(note)) => {
                if let Some(key) = note.key().into_specific() {
                    self.note_on(key.min(127) as u8, note.velocity());
                }
            }
            Some(CoreEventSpace::NoteOff(note)) => {
                self.note_off(note.key().into_specific().map(|key| key.min(127) as u8));
            }
            Some(CoreEventSpace::NoteChoke(note)) => {
                let key = note.key().into_specific().map(|key| key.min(127) as u8);
                for voice in &mut self.voices {
                    if key.is_none_or(|key| voice.key == key) {
                        *voice = Voice::default();
                    }
                }
            }
            Some(CoreEventSpace::Midi(midi)) => {
                let [status, key, velocity] = midi.data();
                match status & 0xF0 {
                    0x90 if velocity > 0 => self.note_on(key & 0x7F, f64::from(velocity) / 127.0),
                    0x80 | 0x90 => self.note_off(Some(key & 0x7F)),
                    // All sound off / all notes off.
                    0xB0 if key == 120 || key == 123 => self.note_off(None),
                    _ => {}
                }
            }
            _ => self.shared.apply(event),
        }
    }

    fn next_sample(&mut self) -> f32 {
        let level = self.shared.level.get();
        let mut sum = 0.0;
        for voice in &mut self.voices {
            if voice.is_free() {
                continue;
            }
            let target = if voice.held { 1.0 } else { 0.0 };
            voice.envelope = if voice.envelope < target {
                (voice.envelope + self.ramp).min(target)
            } else {
                (voice.envelope - self.ramp).max(target)
            };
            sum += voice.phase.sin() * voice.velocity * voice.envelope;
            voice.phase = (voice.phase + voice.increment) % TAU;
        }
        (sum * level) as f32
    }
}

impl<'a> PluginAudioProcessor<'a, SineShared, SineMainThread<'a>> for SineProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &SineMainThread<'a>,
        shared: &'a SineShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            shared,
            voices: [Voice::default(); VOICES],
            sample_rate: audio_config.sample_rate,
            ramp: 1.0 / (RAMP_SECONDS * audio_config.sample_rate).max(1.0),
            mono: vec![0.0; audio_config.max_frames_count as usize],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let frames = (audio.frames_count() as usize).min(self.mono.len());
        let mut cursor = 0u32;
        for frame in 0..frames {
            while let Some(event) = events.input.get(cursor) {
                if event.header().time() as usize > frame {
                    break;
                }
                self.handle(event);
                cursor += 1;
            }
            self.mono[frame] = self.next_sample();
        }
        while let Some(event) = events.input.get(cursor) {
            self.handle(event);
            cursor += 1;
        }

        if let Some(mut port) = audio.output_port(0) {
            if let Some(mut channels) = port.channels()?.into_f32() {
                for channel in channels.iter_mut() {
                    let n = channel.len().min(frames);
                    channel[..n].copy_from_slice(&self.mono[..n]);
                    channel[n..].fill(0.0);
                }
            }
        }

        if self.voices.iter().all(Voice::is_free) {
            Ok(ProcessStatus::Sleep)
        } else {
            Ok(ProcessStatus::Continue)
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
    }
}

impl PluginAudioProcessorParams for SineProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.apply(event);
        }
    }
}
