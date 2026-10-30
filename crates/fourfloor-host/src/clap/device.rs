//! A hosted CLAP plugin as a fourfloor [`Device`].
//!
//! # Real-time safety
//!
//! `process`, `set_param` and `reset` do not allocate, lock or log:
//!
//! - Audio goes through scratch buffers allocated in `prepare`, one per
//!   plugin channel, so the plugin never sees the graph's own buffers and
//!   channel layouts that differ (a mono plugin on a stereo track) are
//!   mapped with plain copies.
//! - The CLAP port descriptors (`AudioPorts`) and the event buffer are
//!   created with their final capacity in `prepare`, and event pushes are
//!   bounded by [`EventWriter`].
//! - A parameter change is stored in a slot per parameter and sent as one
//!   event at the start of the next block; setting it twice keeps the last
//!   value.
//! - Everything the plugin can call back into on the audio thread ends in
//!   an atomic store (see [`super::host`]).
//!
//! What the plugin itself does inside `process` is outside the host's
//! control.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use clack_host::events::event_types::ParamValueEvent;
use clack_host::prelude::*;
use clack_host::utils::Cookie;
use fourfloor_core::{Device, ParamId, ParamInfo, PrepareConfig, ProcessIo, TransportContext};

use super::events::{self, EventWriter, HeldNotes, NoteSink};
use super::host::SharedFlags;
use super::instance::{load_entry, ClapMain, ClapParam, NoteInput, PortLayout, Processor};
use crate::error::HostError;
use crate::info::{plugin_kind, PluginControl, PluginController, PluginFormat, PluginRequests};
use crate::thread::{HostThread, MainBound};

/// Note, controller and all-notes-off events a block can carry on top of
/// one change per parameter.
const EVENT_CAPACITY: usize = 4096;

type LastError = Arc<Mutex<Option<String>>>;

fn set_error(slot: &LastError, error: Option<String>) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = error;
}

/// Audio-thread state that exists only while the plugin is active.
struct Active {
    processor: Processor,
    /// `inputs[port][channel]`, each `max_block` frames.
    inputs: Vec<Vec<Vec<f32>>>,
    outputs: Vec<Vec<Vec<f32>>>,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    events: EventBuffer,
    event_capacity: u32,
    max_block: usize,
    steady_time: u64,
}

/// A CLAP plugin wrapped as a [`Device`].
pub struct ClapDevice {
    kind: String,
    main: Arc<MainBound<ClapMain>>,
    flags: Arc<SharedFlags>,
    last_error: LastError,
    layout: PortLayout,
    infos: Vec<ParamInfo>,
    params: Vec<ClapParam>,
    /// `(parameter id, index into params)`, sorted by id.
    lookup: Vec<(u32, usize)>,
    /// A value waiting to be sent, per parameter.
    pending: Vec<Option<f64>>,
    pending_any: bool,
    held: HeldNotes,
    active: Option<Active>,
    /// Kept by the device so the handle lives exactly as long as it does.
    control: PluginController,
}

impl ClapDevice {
    /// Loads the plugin `id` from the `.clap` file or bundle at `path`,
    /// restoring `state` if given.
    ///
    /// The plugin is created on `thread` and is inactive until `prepare`.
    /// A state blob the plugin rejects does not fail the load: the plugin
    /// starts from its defaults and the reason is kept in
    /// [`PluginControl::last_error`].
    pub fn load(
        thread: &Arc<HostThread>,
        path: &Path,
        id: &str,
        state: Option<&[u8]>,
    ) -> Result<Self, HostError> {
        let flags = Arc::new(SharedFlags::default());
        let main_flags = Arc::clone(&flags);
        let main = MainBound::new(thread, move || {
            let entry = load_entry(path)?;
            ClapMain::create(&entry, id, main_flags)
        })?;
        let (state_result, layout, params) = main.with(|main| {
            let state_result = state.map_or(Ok(()), |bytes| main.load_state(bytes));
            (state_result, main.layout(), main.params())
        })?;

        let mut lookup: Vec<(u32, usize)> = params
            .iter()
            .enumerate()
            .map(|(index, param)| (param.id.get(), index))
            .collect();
        lookup.sort_unstable();
        lookup.dedup_by_key(|(id, _)| *id);
        let last_error = LastError::default();
        set_error(&last_error, state_result.err().map(|e| e.to_string()));
        let kind = plugin_kind(PluginFormat::Clap, id);
        let main = Arc::new(main);
        let control: PluginController = Arc::new(ClapControl {
            kind: kind.clone(),
            main: Arc::clone(&main),
            flags: Arc::clone(&flags),
            last_error: Arc::clone(&last_error),
        });
        Ok(Self {
            kind,
            main,
            flags,
            last_error,
            control,
            layout,
            infos: params.iter().map(|param| param.info.clone()).collect(),
            pending: vec![None; params.len()],
            params,
            lookup,
            pending_any: false,
            held: HeldNotes::default(),
            active: None,
        })
    }

    /// The device kind, `plugin.clap:<id>`.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// A handle for saving state and reading values off the audio thread.
    /// Every call returns the same handle, which the device also keeps.
    pub fn controller(&self) -> PluginController {
        Arc::clone(&self.control)
    }

    /// Channels of the plugin's main audio input; 0 for an instrument.
    pub fn audio_inputs(&self) -> u32 {
        self.layout.inputs.first().copied().unwrap_or(0)
    }

    /// Channels of the plugin's main audio output.
    pub fn audio_outputs(&self) -> u32 {
        self.layout.outputs.first().copied().unwrap_or(0)
    }

    /// True when the plugin has a second audio input, fed from the
    /// device's sidechain.
    pub fn has_sidechain(&self) -> bool {
        self.layout.has_sidechain()
    }

    /// True when the plugin takes notes.
    pub fn takes_notes(&self) -> bool {
        self.layout.note_input != NoteInput::None
    }

    /// True when the plugin emits notes.
    pub fn emits_notes(&self) -> bool {
        self.layout.note_output
    }

    /// Stops and deactivates the plugin, if it is active.
    fn deactivate(&mut self) {
        if let Some(active) = self.active.take() {
            // `stop_processing` belongs to the audio thread. Whoever calls
            // `prepare` or drops the device owns it exclusively, so this
            // thread is standing in for the audio thread here.
            let stopped = active.processor.into_stopped();
            let _ = self.main.with(move |main| main.deactivate(stopped));
        }
        self.held.clear();
    }

    fn activate(&mut self, config: &PrepareConfig) -> Result<Active, HostError> {
        let changes: Vec<(ClapId, Cookie, f64)> = self
            .pending
            .iter_mut()
            .zip(&self.params)
            .filter_map(|(slot, param)| slot.take().map(|value| (param.id, param.cookie, value)))
            .collect();
        self.pending_any = false;
        let sample_rate = config.sample_rate.as_f64();
        let max_block = config.max_block.max(1);
        let block = u32::try_from(max_block)
            .map_err(|_| HostError::Activate("the block size is too large".into()))?;
        let (processor, layout) = self.main.with(move |main| {
            main.flush_params(&changes);
            let layout = main.layout();
            main.activate(sample_rate, block)
                .map(|processor| (processor, layout))
        })??;
        self.layout = layout;

        let buffers = |ports: &[u32]| -> Vec<Vec<Vec<f32>>> {
            ports
                .iter()
                .map(|&channels| vec![vec![0.0; max_block]; channels as usize])
                .collect()
        };
        let ports = |ports: &[u32]| {
            let channels: usize = ports.iter().map(|&c| c as usize).sum();
            AudioPorts::with_capacity(channels, ports.len())
        };
        let event_capacity = EVENT_CAPACITY + self.params.len();
        Ok(Active {
            processor,
            inputs: buffers(&self.layout.inputs),
            outputs: buffers(&self.layout.outputs),
            input_ports: ports(&self.layout.inputs),
            output_ports: ports(&self.layout.outputs),
            events: EventBuffer::with_capacity(event_capacity),
            event_capacity: u32::try_from(event_capacity).unwrap_or(u32::MAX),
            max_block,
            steady_time: 0,
        })
    }
}

impl Drop for ClapDevice {
    fn drop(&mut self) {
        self.deactivate();
    }
}

/// Copies one graph bus into one plugin input port.
///
/// A mono port on a wider bus gets the average of the bus; a wider port on a
/// mono bus gets the bus on every channel; otherwise channels map one to
/// one and extra port channels are silent.
pub(crate) fn fill_port(port: &mut [Vec<f32>], source: Option<&[&[f32]]>, frames: usize) {
    let Some(source) = source.filter(|source| !source.is_empty()) else {
        for channel in port {
            channel[..frames].fill(0.0);
        }
        return;
    };
    if let ([mono], true) = (&mut *port, source.len() > 1) {
        let scale = 1.0 / source.len() as f32;
        let mono = &mut mono[..frames];
        mono.fill(0.0);
        for channel in source {
            for (out, sample) in mono.iter_mut().zip(&channel[..frames]) {
                *out += sample * scale;
            }
        }
        return;
    }
    for (index, channel) in port.iter_mut().enumerate() {
        match source.get(index).or(if source.len() == 1 {
            source.first()
        } else {
            None
        }) {
            Some(input) => channel[..frames].copy_from_slice(&input[..frames]),
            None => channel[..frames].fill(0.0),
        }
    }
}

/// Copies a plugin output port to the graph bus, replacing anything that is
/// not a finite number with silence.
pub(crate) fn drain_port<C: AsRef<[f32]>>(port: &[C], outputs: &mut [&mut [f32]], frames: usize) {
    for (index, output) in outputs.iter_mut().enumerate() {
        let source = port
            .get(index)
            .or(if port.len() == 1 { port.first() } else { None });
        match source {
            Some(source) => {
                let source = source.as_ref();
                for (out, sample) in output[..frames].iter_mut().zip(&source[..frames]) {
                    *out = if sample.is_finite() { *sample } else { 0.0 };
                }
            }
            None => output[..frames].fill(0.0),
        }
    }
}

impl Device for ClapDevice {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.deactivate();
        match self.activate(config) {
            Ok(active) => {
                self.active = Some(active);
                set_error(&self.last_error, None);
            }
            Err(error) => {
                self.flags.latency.store(0, Ordering::Release);
                set_error(&self.last_error, Some(error.to_string()));
            }
        }
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        if frames == 0 {
            return;
        }
        let Self {
            active,
            layout,
            params,
            pending,
            pending_any,
            held,
            flags,
            ..
        } = self;
        let Some(active) = active.as_mut().filter(|active| frames <= active.max_block) else {
            // Not activated: an effect passes its input on, anything else
            // is silent.
            if layout.inputs.is_empty() {
                io.silence_outputs();
            } else {
                io.pass_through();
            }
            return;
        };

        // Audio in. Port 0 is the main input, port 1 the sidechain.
        let bus = io.channels();
        for (index, port) in active.inputs.iter_mut().enumerate() {
            let source = match index {
                0 => io.inputs.get(..bus.min(io.inputs.len())),
                1 => io.sidechain,
                _ => None,
            };
            fill_port(port, source, frames);
        }

        // Events in: parameter changes first, then the block's events.
        active.events.clear();
        let mut writer = EventWriter::new(&mut active.events, active.event_capacity);
        if *pending_any {
            for (slot, param) in pending.iter_mut().zip(params.iter()) {
                if let Some(value) = slot.take() {
                    let event = ParamValueEvent::new(0, param.id, Pckn::match_all(), value);
                    // SAFETY: the cookie was given by this plugin for this
                    // parameter.
                    writer.push(&unsafe { event.with_cookie(param.cookie) });
                }
            }
            *pending_any = false;
        }
        for event in io.events {
            writer.push_core(event, frames, layout.note_input, layout.midi_input, held);
        }
        if writer.dropped > 0 {
            flags
                .dropped_events
                .fetch_add(writer.dropped, Ordering::Relaxed);
        }

        let transport = events::transport(ctx);
        let mut sink = NoteSink {
            out: &mut *io.events_out,
            last_frame: u32::try_from(frames - 1).unwrap_or(u32::MAX),
        };
        let mut events_out = if layout.note_output {
            OutputEvents::from_buffer(&mut sink)
        } else {
            OutputEvents::void()
        };

        let result = match active.processor.ensure_processing_started() {
            Ok(started) => {
                let mut audio_in =
                    active
                        .input_ports
                        .with_input_buffers(active.inputs.iter_mut().map(|port| AudioPortBuffer {
                            latency: 0,
                            channels:
                                AudioPortBufferType::f32_input_only(
                                    port.iter_mut().map(|channel| {
                                        InputChannel::variable(&mut channel[..frames])
                                    }),
                                ),
                        }));
                let mut audio_out =
                    active
                        .output_ports
                        .with_output_buffers(active.outputs.iter_mut().map(|port| {
                            AudioPortBuffer {
                                latency: 0,
                                channels: AudioPortBufferType::f32_output_only(
                                    port.iter_mut().map(|channel| &mut channel[..frames]),
                                ),
                            }
                        }));
                if layout.inputs.is_empty() && layout.outputs.is_empty() {
                    // With no audio ports clack cannot infer the block
                    // length, and would tell the plugin the block is empty.
                    // SAFETY: an empty buffer list has no pointers to be
                    // invalid; only the frame count is used.
                    audio_in = unsafe {
                        InputAudioBuffers::from_raw_buffers(
                            &[],
                            u32::try_from(frames).unwrap_or(u32::MAX),
                        )
                    };
                }
                started
                    .process(
                        &audio_in,
                        &mut audio_out,
                        &active.events.as_input(),
                        &mut events_out,
                        Some(active.steady_time),
                        Some(&transport),
                    )
                    .is_ok()
            }
            Err(_) => false,
        };
        active.steady_time = active.steady_time.wrapping_add(frames as u64);

        if result {
            match active.outputs.first() {
                Some(port) => drain_port(port, io.outputs, frames),
                // No audio output: a note effect. Keep the bus silent.
                None => io.silence_outputs(),
            }
        } else {
            flags.failed_blocks.fetch_add(1, Ordering::Relaxed);
            if layout.inputs.is_empty() {
                io.silence_outputs();
            } else {
                io.pass_through();
            }
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &self.infos
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        let Ok(found) = self
            .lookup
            .binary_search_by_key(&param.get(), |(id, _)| *id)
        else {
            return;
        };
        let index = self.lookup[found].1;
        let value = self.infos[index].clamp(value);
        self.pending[index] = Some(f64::from(value));
        self.pending_any = true;
    }

    fn reset(&mut self) {
        if let Some(active) = &mut self.active {
            active.processor.reset();
        }
        self.held.clear();
    }

    fn latency_samples(&self) -> u32 {
        self.flags.latency.load(Ordering::Acquire)
    }
}

/// The [`PluginControl`] of a [`ClapDevice`].
struct ClapControl {
    kind: String,
    main: Arc<MainBound<ClapMain>>,
    flags: Arc<SharedFlags>,
    last_error: LastError,
}

impl PluginControl for ClapControl {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn save_state(&self) -> Result<Vec<u8>, HostError> {
        self.main.with(|main| main.save_state())?
    }

    fn load_state(&self, state: &[u8]) -> Result<(), HostError> {
        self.main.with(|main| main.load_state(state))?
    }

    fn latency_samples(&self) -> u32 {
        self.flags.latency.load(Ordering::Acquire)
    }

    fn param_value(&self, param: ParamId) -> Option<f64> {
        let id = ClapId::from_raw(param.get())?;
        self.main.with(|main| main.param_value(id)).ok()?
    }

    fn param_text(&self, param: ParamId, value: f64) -> Option<String> {
        let id = ClapId::from_raw(param.get())?;
        self.main.with(|main| main.param_text(id, value)).ok()?
    }

    fn take_requests(&self) -> PluginRequests {
        self.flags.take_requests()
    }

    fn failed_blocks(&self) -> u32 {
        self.flags.failed_blocks.load(Ordering::Relaxed)
    }

    fn dropped_events(&self) -> u32 {
        self.flags.dropped_events.load(Ordering::Relaxed)
    }

    fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(channels: usize) -> Vec<Vec<f32>> {
        vec![vec![9.0; 4]; channels]
    }

    #[test]
    fn stereo_bus_to_stereo_port_copies_channel_for_channel() {
        let (left, right) = ([1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        let mut buffers = port(2);
        fill_port(&mut buffers, Some(&[&left, &right]), 3);
        assert_eq!(
            buffers[0],
            [1.0, 2.0, 3.0, 9.0],
            "frames past the block untouched"
        );
        assert_eq!(buffers[1], [5.0, 6.0, 7.0, 9.0]);
    }

    #[test]
    fn stereo_bus_to_mono_port_is_averaged() {
        let (left, right) = ([1.0; 4], [0.0; 4]);
        let mut buffers = port(1);
        fill_port(&mut buffers, Some(&[&left, &right]), 4);
        assert_eq!(buffers[0], [0.5; 4]);
    }

    #[test]
    fn mono_bus_feeds_every_port_channel_and_missing_input_is_silence() {
        let mono = [0.25; 4];
        let mut buffers = port(2);
        fill_port(&mut buffers, Some(&[&mono]), 4);
        assert_eq!(buffers, vec![vec![0.25; 4]; 2]);

        fill_port(&mut buffers, None, 4);
        assert_eq!(buffers, vec![vec![0.0; 4]; 2]);

        let (left, right) = ([1.0; 4], [2.0; 4]);
        let mut wide = port(3);
        fill_port(&mut wide, Some(&[&left, &right]), 4);
        assert_eq!(wide[2], [0.0; 4], "a third port channel has no source");
    }

    #[test]
    fn outputs_are_mapped_and_sanitised() {
        let mono = vec![vec![1.0, f32::NAN, f32::INFINITY, -1.0]];
        let (mut left, mut right) = ([7.0; 4], [7.0; 4]);
        drain_port(&mono, &mut [&mut left, &mut right], 4);
        assert_eq!(left, [1.0, 0.0, 0.0, -1.0]);
        assert_eq!(right, left, "a mono port fills both bus channels");

        let stereo = vec![vec![1.0; 4], vec![2.0; 4]];
        let (mut a, mut b, mut c) = ([7.0; 4], [7.0; 4], [7.0; 4]);
        drain_port(&stereo, &mut [&mut a, &mut b, &mut c], 4);
        assert_eq!((a, b, c), ([1.0; 4], [2.0; 4], [0.0; 4]));
    }
}
