//! The main-thread half of a hosted CLAP plugin.
//!
//! Everything here runs on the plugin host thread, inside a
//! [`MainBound`](crate::thread::MainBound). It may allocate and block.

use std::ffi::CString;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::latency::PluginLatency;
use clack_extensions::note_ports::{NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::ParamValueEvent;
use clack_host::prelude::*;
use clack_host::utils::Cookie;
use fourfloor_core::{ParamId, ParamInfo, ParamUnit};

use super::host::{FourfloorHost, HostMainThread, HostShared, SharedFlags};
use crate::error::HostError;
use crate::thread::{thread_token, MainIdle};

/// The processor handle a device drives on the audio thread.
pub(crate) type Processor = PluginAudioProcessor<FourfloorHost>;

/// One plugin parameter, as the device needs it.
#[derive(Clone, Debug)]
pub(crate) struct ClapParam {
    pub id: ClapId,
    /// The plugin's own shortcut to the parameter, passed back with every
    /// change.
    pub cookie: Cookie,
    pub info: ParamInfo,
}

/// How notes are sent to the plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoteInput {
    /// The plugin has no note input port.
    None,
    /// CLAP note events.
    Clap,
    /// Raw MIDI events.
    Midi,
}

/// The plugin's audio and note ports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortLayout {
    /// Channel count of each audio input port, in port order.
    pub inputs: Vec<u32>,
    /// Channel count of each audio output port, in port order.
    pub outputs: Vec<u32>,
    pub note_input: NoteInput,
    /// True when the note input also takes raw MIDI (for controllers and
    /// pitch bend).
    pub midi_input: bool,
    pub note_output: bool,
}

impl PortLayout {
    pub fn has_sidechain(&self) -> bool {
        self.inputs.len() >= 2
    }
}

/// Loads a `.clap` file or bundle.
pub(crate) fn load_entry(path: &Path) -> Result<PluginEntry, HostError> {
    // SAFETY: loading a dynamic library runs its initialisers. The caller
    // chose this path as a CLAP plugin; clack checks the entry symbol and
    // the CLAP version before using it.
    unsafe { PluginEntry::load(path) }.map_err(|error| HostError::Load {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn host_info() -> HostInfo {
    let text = |value: &str| CString::new(value).unwrap_or_default();
    HostInfo::new_from_cstring(
        text("fourfloor"),
        text("fourfloor"),
        text(""),
        text(env!("CARGO_PKG_VERSION")),
    )
}

/// The plugin instance and its main-thread extensions.
pub(crate) struct ClapMain {
    instance: PluginInstance<FourfloorHost>,
    params: Option<PluginParams>,
    state: Option<PluginState>,
    latency: Option<PluginLatency>,
    audio_ports: Option<PluginAudioPorts>,
    note_ports: Option<PluginNotePorts>,
    flags: Arc<SharedFlags>,
}

impl MainIdle for ClapMain {
    fn idle(&mut self) {
        if self.flags.callback.swap(false, Ordering::AcqRel) {
            self.instance.call_on_main_thread_callback();
        }
        // An active plugin gets its flush from the next `process` call.
        if self.flags.flush.swap(false, Ordering::AcqRel) && !self.instance.is_active() {
            self.flush_params(&[]);
        }
    }
}

impl ClapMain {
    /// Creates the plugin `id` from `entry`. Host thread only.
    pub fn create(
        entry: &PluginEntry,
        id: &str,
        flags: Arc<SharedFlags>,
    ) -> Result<Self, HostError> {
        let c_id = CString::new(id).map_err(|_| HostError::InvalidId(id.to_owned()))?;
        let main_token = thread_token();
        let shared_flags = Arc::clone(&flags);
        let mut instance = PluginInstance::<FourfloorHost>::new(
            move |_| HostShared {
                flags: shared_flags,
                main_token,
            },
            |shared| HostMainThread { shared },
            entry,
            &c_id,
            &host_info(),
        )
        .map_err(|error| match error {
            PluginInstanceError::PluginNotFound => HostError::NotFound { id: id.to_owned() },
            other => HostError::Instantiate(other.to_string()),
        })?;
        let handle = instance.plugin_handle();
        let params = handle.get_extension();
        let state = handle.get_extension();
        let latency = handle.get_extension();
        let audio_ports = handle.get_extension();
        let note_ports = handle.get_extension();
        Ok(Self {
            instance,
            params,
            state,
            latency,
            audio_ports,
            note_ports,
            flags,
        })
    }

    /// Reads the plugin's audio and note ports.
    pub fn layout(&mut self) -> PortLayout {
        let handle = self.instance.plugin_handle();
        let audio = |is_input: bool| -> Vec<u32> {
            let Some(ports) = &self.audio_ports else {
                return Vec::new();
            };
            let mut buffer = AudioPortInfoBuffer::new();
            (0..ports.count(&handle, is_input))
                .filter_map(|index| {
                    ports
                        .get(&handle, index, is_input, &mut buffer)
                        .map(|port| port.channel_count)
                })
                .collect()
        };
        let (inputs, outputs) = (audio(true), audio(false));

        let mut note_input = NoteInput::None;
        let mut midi_input = false;
        let mut note_output = false;
        if let Some(ports) = &self.note_ports {
            let mut buffer = NotePortInfoBuffer::new();
            if let Some(port) = ports.get(&handle, 0, true, &mut buffer) {
                let dialects = port.supported_dialects;
                midi_input = dialects.contains(NoteDialects::MIDI);
                note_input = if dialects.contains(NoteDialects::CLAP) {
                    NoteInput::Clap
                } else if midi_input {
                    NoteInput::Midi
                } else {
                    NoteInput::None
                };
            }
            note_output = ports.count(&handle, false) > 0;
        }
        PortLayout {
            inputs,
            outputs,
            note_input,
            midi_input,
            note_output,
        }
    }

    /// Reads the plugin's parameters. Hidden and read-only parameters are
    /// left out: the host cannot usefully set them.
    pub fn params(&mut self) -> Vec<ClapParam> {
        let Some(params) = &self.params else {
            return Vec::new();
        };
        let handle = self.instance.plugin_handle();
        let mut buffer = ParamInfoBuffer::new();
        let mut list = Vec::new();
        for index in 0..params.count(&handle) {
            let Some(info) = params.get_info(&handle, index, &mut buffer) else {
                continue;
            };
            if info
                .flags
                .intersects(ParamInfoFlags::IS_HIDDEN | ParamInfoFlags::IS_READONLY)
            {
                continue;
            }
            let (min, max) = (info.min_value as f32, info.max_value as f32);
            if !(min.is_finite() && max.is_finite() && min <= max) {
                continue;
            }
            let stepped = info.flags.contains(ParamInfoFlags::IS_STEPPED);
            let unit = match (stepped, min, max) {
                (true, 0.0, 1.0) => ParamUnit::Toggle,
                (true, _, _) => ParamUnit::Integer,
                _ => ParamUnit::Generic,
            };
            let default = (info.default_value as f32).clamp(min, max);
            let name = String::from_utf8_lossy(info.name).into_owned();
            list.push(ClapParam {
                id: info.id,
                cookie: info.cookie,
                info: ParamInfo::new(ParamId(info.id.get()), "", min, max, default, unit)
                    .with_name(name),
            });
        }
        list
    }

    /// Activates the plugin and records its latency.
    pub fn activate(&mut self, sample_rate: f64, max_block: u32) -> Result<Processor, HostError> {
        if self.instance.is_active() {
            return Err(HostError::Activate("the plugin is already active".into()));
        }
        let configuration = PluginAudioConfiguration {
            sample_rate,
            min_frames_count: 1,
            max_frames_count: max_block.max(1),
        };
        let processor = self
            .instance
            .activate(|_, _| (), configuration)
            .map_err(|error| HostError::Activate(error.to_string()))?;
        let latency = match &self.latency {
            Some(latency) => latency.get(&self.instance.plugin_handle()),
            None => 0,
        };
        self.flags.latency.store(latency, Ordering::Release);
        self.flags.latency_changed.store(false, Ordering::Release);
        self.flags.restart.store(false, Ordering::Release);
        Ok(processor.into())
    }

    /// Deactivates the plugin. `processor` must be the stopped processor of
    /// this instance; any other is dropped untouched.
    pub fn deactivate(&mut self, processor: StoppedPluginAudioProcessor<FourfloorHost>) {
        if processor.matches(&self.instance) {
            self.instance.deactivate(processor);
        }
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, HostError> {
        let state = self
            .state
            .ok_or_else(|| HostError::Unsupported("the plugin has no state extension".into()))?;
        let mut bytes = Vec::new();
        state
            .save(&self.instance.plugin_handle(), &mut bytes)
            .map_err(|error| HostError::State(error.to_string()))?;
        Ok(bytes)
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), HostError> {
        let state = self
            .state
            .ok_or_else(|| HostError::Unsupported("the plugin has no state extension".into()))?;
        state
            .load(&self.instance.plugin_handle(), &mut Cursor::new(bytes))
            .map_err(|error| HostError::State(error.to_string()))
    }

    pub fn param_value(&mut self, id: ClapId) -> Option<f64> {
        self.params?.get_value(&self.instance.plugin_handle(), id)
    }

    pub fn param_text(&mut self, id: ClapId, value: f64) -> Option<String> {
        let mut buffer = [0u8; 128];
        let text = self
            .params?
            .value_to_text(&self.instance.plugin_handle(), id, value, &mut buffer)
            .ok()?;
        Some(String::from_utf8_lossy(text).into_owned())
    }

    /// Sends parameter changes to an inactive plugin. An active plugin gets
    /// them through `process` instead, and this does nothing.
    pub fn flush_params(&mut self, changes: &[(ClapId, Cookie, f64)]) {
        let Some(params) = self.params else {
            return;
        };
        let Some(mut handle) = self.instance.inactive_plugin_handle() else {
            return;
        };
        let mut events = EventBuffer::with_capacity(changes.len());
        for &(id, cookie, value) in changes {
            let event = ParamValueEvent::new(0, id, Pckn::match_all(), value);
            // SAFETY: the cookie was given by this plugin for this very
            // parameter in `params`.
            events.push(&unsafe { event.with_cookie(cookie) });
        }
        params.flush(&mut handle, &events.as_input(), &mut OutputEvents::void());
    }
}
