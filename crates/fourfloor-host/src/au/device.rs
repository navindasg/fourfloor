//! A hosted Audio Unit as a fourfloor [`Device`].
//!
//! # Real-time safety
//!
//! `process`, `set_param` and `reset` do not allocate, lock or log on the
//! host side:
//!
//! - The unit pulls its input from scratch buffers allocated when it was
//!   activated ([`InputContext`](super::render::InputContext)); the output
//!   buffer list lives on the stack and points at scratch buffers too.
//! - Parameter changes go straight to `AudioUnitSetParameter` and MIDI to
//!   `MusicDeviceMIDIEvent`, the two calls the Audio Unit API provides for
//!   use from the render thread.
//! - The transport is handed over through atomics.
//!
//! What the unit itself does inside those calls is outside the host's
//! control.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use fourfloor_core::{
    Device, Event, EventKind, ParamId, ParamInfo, PrepareConfig, ProcessIo, TransportContext,
};

use super::ffi::{self, AudioBufferList, AudioTimeStamp, AudioUnit, MAX_CHANNELS};
use super::render::TransportShared;
use super::unit::{Activated, Activation, AuMain, AudioHandles};
use crate::clap::{drain_port, fill_port, HeldNotes};
use crate::error::HostError;
use crate::info::{plugin_kind, PluginControl, PluginController, PluginFormat, PluginRequests};
use crate::thread::{HostThread, MainBound};

type LastError = Arc<Mutex<Option<String>>>;

fn set_error(slot: &LastError, error: Option<String>) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = error;
}

/// Counters shared by the device and its controller.
#[derive(Debug, Default)]
struct Counters {
    latency: AtomicU32,
    failed_blocks: AtomicU32,
}

/// Audio-thread state that exists only while the unit is initialised.
struct Active {
    layout: Activated,
    /// One scratch buffer per output channel, each `max_block` frames.
    outputs: Vec<Vec<f32>>,
    max_block: usize,
    /// Frames rendered since activation: the unit's render clock.
    sample_time: f64,
}

/// An Audio Unit wrapped as a [`Device`].
pub struct AuDevice {
    kind: String,
    main: Arc<MainBound<AuMain>>,
    handles: AudioHandles,
    transport: Arc<TransportShared>,
    counters: Arc<Counters>,
    last_error: LastError,
    infos: Arc<[ParamInfo]>,
    /// Parameter ids, sorted, with the index into `infos`.
    lookup: Vec<(u32, usize)>,
    instrument: bool,
    takes_midi: bool,
    held: HeldNotes,
    active: Option<Active>,
    /// Kept by the device so the handle lives exactly as long as it does.
    control: PluginController,
}

impl AuDevice {
    /// Instantiates the unit `id` (`type.subtype.manufacturer`, for example
    /// `aufx.dely.appl`), restoring `state` if given.
    ///
    /// The unit is created on `thread` and is uninitialised until
    /// `prepare`. A state blob the unit rejects does not fail the load: the
    /// unit starts from its defaults and the reason is kept in
    /// [`PluginControl::last_error`].
    pub fn load(
        thread: &Arc<HostThread>,
        id: &str,
        state: Option<&[u8]>,
    ) -> Result<Self, HostError> {
        let transport = Arc::new(TransportShared::default());
        let main_transport = Arc::clone(&transport);
        let main = MainBound::new(thread, move || AuMain::create(id, main_transport))?;
        let (state_result, handles, infos, instrument, takes_midi) = main.with(|main| {
            let state_result = state.map_or(Ok(()), |bytes| main.load_state(bytes));
            (
                state_result,
                main.handles(),
                main.params(),
                main.is_instrument(),
                main.takes_midi(),
            )
        })?;
        let infos = crate::info::dedup_params(&infos);
        let mut lookup: Vec<(u32, usize)> = infos
            .iter()
            .enumerate()
            .map(|(index, info)| (info.id.get(), index))
            .collect();
        lookup.sort_unstable();
        let last_error = LastError::default();
        set_error(&last_error, state_result.err().map(|e| e.to_string()));
        let kind = plugin_kind(PluginFormat::Au, id);
        let main = Arc::new(main);
        let counters = Arc::new(Counters::default());
        let infos: Arc<[ParamInfo]> = infos.into();
        let control: PluginController = Arc::new(AuControl {
            kind: kind.clone(),
            main: Arc::clone(&main),
            counters: Arc::clone(&counters),
            last_error: Arc::clone(&last_error),
            infos: Arc::clone(&infos),
        });
        Ok(Self {
            kind,
            main,
            handles,
            transport,
            counters,
            last_error,
            infos,
            control,
            lookup,
            instrument,
            takes_midi,
            held: HeldNotes::default(),
            active: None,
        })
    }

    /// The device kind, `plugin.au:<id>`.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// A handle for saving state and reading values off the audio thread.
    /// Every call returns the same handle, which the device also keeps.
    pub fn controller(&self) -> PluginController {
        Arc::clone(&self.control)
    }

    /// True for an instrument (`aumu`), which has no audio input.
    pub fn is_instrument(&self) -> bool {
        self.instrument
    }

    /// The channel layout the unit agreed to at the last `prepare`, as
    /// `(main input, output, sidechain)` channel counts; `None` before
    /// `prepare` or when activation failed.
    pub fn layout(&self) -> Option<(u32, u32, u32)> {
        self.active.as_ref().map(|active| {
            (
                active.layout.inputs,
                active.layout.outputs,
                active.layout.sidechain,
            )
        })
    }

    fn deactivate(&mut self) {
        if self.active.take().is_some() {
            let _ = self.main.with(AuMain::deactivate);
        }
        self.held.clear();
    }

    fn activate(&mut self, config: &PrepareConfig) -> Result<Active, HostError> {
        let max_block = config.max_block.max(1);
        let request = Activation {
            sample_rate: config.sample_rate.as_f64(),
            max_block: u32::try_from(max_block)
                .map_err(|_| HostError::Activate("the block size is too large".into()))?,
            bus_channels: u32::try_from(config.channels).unwrap_or(2),
        };
        let layout = self.main.with(move |main| main.activate(&request))??;
        Ok(Active {
            layout,
            outputs: vec![vec![0.0; max_block]; (layout.outputs as usize).min(MAX_CHANNELS)],
            max_block,
            sample_time: 0.0,
        })
    }
}

impl Drop for AuDevice {
    fn drop(&mut self) {
        self.deactivate();
    }
}

/// Sends one MIDI message to the unit. The status is ignored: an effect
/// that takes no MIDI just refuses it.
fn midi(unit: AudioUnit, status: u8, data1: u8, data2: u8, offset: u32) {
    // SAFETY: `unit` is a live instance; the call is meant for the render
    // thread.
    let _ = unsafe {
        ffi::MusicDeviceMIDIEvent(
            unit,
            u32::from(status),
            u32::from(data1 & 0x7F),
            u32::from(data2 & 0x7F),
            offset,
        )
    };
}

/// Translates one fourfloor event to MIDI for the unit.
fn send_event(unit: AudioUnit, event: &Event, frames: usize, held: &mut HeldNotes) {
    let last = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
    let offset = event.frame.min(last);
    match event.kind {
        EventKind::NoteOn {
            pitch,
            velocity,
            channel,
        } if velocity > 0 => {
            midi(
                unit,
                0x90 | (channel & 0x0F),
                pitch,
                velocity.min(127),
                offset,
            );
            held.set(channel, pitch, true);
        }
        EventKind::NoteOn { pitch, channel, .. } => {
            midi(unit, 0x80 | (channel & 0x0F), pitch, 0, offset);
            held.set(channel, pitch, false);
        }
        EventKind::NoteOff {
            pitch,
            velocity,
            channel,
        } => {
            midi(
                unit,
                0x80 | (channel & 0x0F),
                pitch,
                velocity.min(127),
                offset,
            );
            held.set(channel, pitch, false);
        }
        EventKind::ControlChange {
            controller,
            value,
            channel,
        } => midi(unit, 0xB0 | (channel & 0x0F), controller, value, offset),
        EventKind::PitchBend { value, channel } => {
            let bend = (i32::from(value) + 8192).clamp(0, 16383) as u16;
            midi(
                unit,
                0xE0 | (channel & 0x0F),
                (bend & 0x7F) as u8,
                (bend >> 7) as u8,
                offset,
            );
        }
        EventKind::AllNotesOff => {
            held.drain(|channel, pitch| midi(unit, 0x80 | (channel & 0x0F), pitch, 0, offset));
        }
    }
}

impl Device for AuDevice {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.deactivate();
        match self.activate(config) {
            Ok(active) => {
                self.counters
                    .latency
                    .store(active.layout.latency, Ordering::Release);
                self.active = Some(active);
                set_error(&self.last_error, None);
            }
            Err(error) => {
                self.counters.latency.store(0, Ordering::Release);
                set_error(&self.last_error, Some(error.to_string()));
            }
        }
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        if frames == 0 {
            return;
        }
        let unit = self.handles.unit;
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| frames <= active.max_block)
        else {
            // Not initialised: an effect passes its input on, an instrument
            // is silent.
            if self.instrument {
                io.silence_outputs();
            } else {
                io.pass_through();
            }
            return;
        };

        self.transport.store(ctx);
        if self.takes_midi {
            for event in io.events {
                send_event(unit, event, frames, &mut self.held);
            }
        }

        if active.layout.inputs > 0 {
            // SAFETY: the context is owned by the `AuMain` this device keeps
            // alive. The host thread touches it only inside `activate`,
            // which runs from `prepare` and so never alongside `process`;
            // the unit's input callback reads it only during the
            // `AudioUnitRender` call below, after this reference is gone.
            let input = unsafe { &mut *self.handles.input };
            let bus = io.channels().min(io.inputs.len());
            let [main_port, sidechain_port] = &mut input.ports;
            fill_port(main_port, io.inputs.get(..bus), frames);
            fill_port(sidechain_port, io.sidechain, frames);
        }

        let channels = active.outputs.len();
        let bytes = (frames * std::mem::size_of::<f32>()) as u32;
        let mut list = AudioBufferList::empty();
        list.mNumberBuffers = channels as u32;
        for (buffer, channel) in list.mBuffers.iter_mut().zip(active.outputs.iter_mut()) {
            buffer.mNumberChannels = 1;
            buffer.mDataByteSize = bytes;
            buffer.mData = channel.as_mut_ptr().cast();
        }
        let stamp = AudioTimeStamp {
            mSampleTime: active.sample_time,
            mRateScalar: 1.0,
            mFlags: ffi::TIMESTAMP_SAMPLE_TIME_VALID,
            ..AudioTimeStamp::default()
        };
        let mut flags = 0u32;
        // SAFETY: `unit` is live and initialised (there is an `Active`);
        // the list holds `channels` buffers of `frames` samples each, and
        // `frames` does not exceed the maximum the unit was told.
        let status = unsafe {
            ffi::AudioUnitRender(
                unit,
                &mut flags,
                &stamp,
                0,
                u32::try_from(frames).unwrap_or(u32::MAX),
                &mut list,
            )
        };
        active.sample_time += frames as f64;

        // The unit may have swapped in its own buffers, so read the result
        // through the list and not through `active.outputs`.
        let mut planes: [&[f32]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
        let mut rendered = status == 0 && channels > 0;
        if rendered {
            for (plane, buffer) in planes.iter_mut().zip(&list.mBuffers[..channels]) {
                let data = buffer.mData.cast::<f32>().cast_const();
                if data.is_null() || buffer.mDataByteSize < bytes {
                    rendered = false;
                    break;
                }
                // SAFETY: the unit reported at least `frames` samples at
                // `data`, valid until the next call into the unit.
                *plane = unsafe { std::slice::from_raw_parts(data, frames) };
            }
        }
        if rendered {
            drain_port(&planes[..channels], io.outputs, frames);
        } else {
            self.counters.failed_blocks.fetch_add(1, Ordering::Relaxed);
            if self.instrument {
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
        let value = self.infos[self.lookup[found].1].clamp(value);
        // SAFETY: live unit; `AudioUnitSetParameter` may be called from the
        // render thread, before or after initialisation.
        let _ = unsafe {
            ffi::AudioUnitSetParameter(
                self.handles.unit,
                param.get(),
                ffi::SCOPE_GLOBAL,
                0,
                value,
                0,
            )
        };
    }

    fn reset(&mut self) {
        if self.active.is_some() {
            // SAFETY: live, initialised unit.
            let _ = unsafe { ffi::AudioUnitReset(self.handles.unit, ffi::SCOPE_GLOBAL, 0) };
        }
        self.held.clear();
    }

    fn latency_samples(&self) -> u32 {
        self.counters.latency.load(Ordering::Acquire)
    }
}

/// The [`PluginControl`] of an [`AuDevice`].
struct AuControl {
    kind: String,
    main: Arc<MainBound<AuMain>>,
    counters: Arc<Counters>,
    last_error: LastError,
    infos: Arc<[ParamInfo]>,
}

impl PluginControl for AuControl {
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
        self.counters.latency.load(Ordering::Acquire)
    }

    fn param_value(&self, param: ParamId) -> Option<f64> {
        self.infos.iter().find(|info| info.id == param)?;
        self.main.with(|main| main.param_value(param.get())).ok()?
    }

    /// The value with the parameter's unit; the unit's own value strings
    /// (`kAudioUnitProperty_ParameterStringFromValue`) are not read.
    fn param_text(&self, param: ParamId, value: f64) -> Option<String> {
        let info = self.infos.iter().find(|info| info.id == param)?;
        let suffix = info.unit.suffix();
        Some(if suffix.is_empty() {
            format!("{value:.2}")
        } else {
            format!("{value:.2} {suffix}")
        })
    }

    fn take_requests(&self) -> PluginRequests {
        // No property listeners are installed; see the module docs.
        PluginRequests::default()
    }

    fn failed_blocks(&self) -> u32 {
        self.counters.failed_blocks.load(Ordering::Relaxed)
    }

    fn dropped_events(&self) -> u32 {
        0
    }

    fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
