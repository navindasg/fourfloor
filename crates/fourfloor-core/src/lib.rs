//! Shared contract types for fourfloor.
//!
//! This crate is the agreement between the engine, the edit model, the
//! device crates, the UI and the agent API. It holds small plain-data types,
//! one trait and the device registry:
//!
//! - [`id`]: stable id newtypes ([`TrackId`], [`ClipId`], [`DeviceId`],
//!   [`ParamId`]).
//! - [`time`]: musical time ([`Tempo`], [`Beats`], [`Ticks`], [`SampleRate`])
//!   and sample/beat conversion.
//! - [`message`]: [`AudioMessage`], the real-time-safe message sent from the
//!   UI side to the audio thread.
//! - [`event`]: [`Event`] and [`EventKind`], sample-accurate note and
//!   controller events, and the preallocated [`EventBuffer`].
//! - [`param`]: [`ParamInfo`], how a device describes its parameters.
//! - [`device`]: the [`Device`] DSP trait with its [`PrepareConfig`],
//!   [`ProcessIo`] and [`TransportContext`].
//! - [`registry`]: [`DeviceRegistry`], which maps a stable kind string such
//!   as `"drum.kick909"` to a factory and a [`DeviceDescriptor`].
//!
//! # Writing a device
//!
//! ```
//! use fourfloor_core::{
//!     Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
//! };
//!
//! /// A gain stage.
//! pub struct Volume {
//!     gain: f32,
//! }
//!
//! impl Volume {
//!     pub const GAIN: ParamId = ParamId(0);
//! }
//!
//! // A `static`, not a `const`: `params` returns a reference to it.
//! static PARAMS: [ParamInfo; 1] =
//!     [ParamInfo::new(Volume::GAIN, "Gain", 0.0, 4.0, 1.0, ParamUnit::Generic)];
//!
//! impl Device for Volume {
//!     fn prepare(&mut self, _config: &PrepareConfig) {
//!         // Allocate here, never in `process`.
//!     }
//!
//!     fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
//!         for (output, input) in io.outputs.iter_mut().zip(io.inputs) {
//!             for (o, i) in output.iter_mut().zip(input.iter()) {
//!                 *o = *i * self.gain;
//!             }
//!         }
//!     }
//!
//!     fn params(&self) -> &[ParamInfo] {
//!         &PARAMS
//!     }
//!
//!     fn set_param(&mut self, param: ParamId, value: f32) {
//!         if param == Self::GAIN && value.is_finite() {
//!             self.gain = PARAMS[0].clamp(value);
//!         }
//!     }
//! }
//! ```
//!
//! An instrument reads [`ProcessIo::events`] instead of
//! [`ProcessIo::inputs`]; see [`Device`]. To make a device available to
//! projects, register it: see [`registry`].
//!
//! The crate has no required dependencies. The optional `serde` feature
//! derives `Serialize`/`Deserialize` on the id, time, parameter and
//! descriptor types.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod device;
pub mod event;
pub mod id;
pub mod message;
pub mod param;
pub mod registry;
pub mod time;

pub use device::{Device, PrepareConfig, ProcessIo, TransportContext};
pub use event::{pitch_to_hz, velocity_to_unit, Event, EventBuffer, EventKind};
pub use id::{ClipId, DeviceId, ParamId, TrackId};
pub use message::{AudioMessage, GraphHandle};
pub use param::{ParamInfo, ParamScale, ParamUnit};
pub use registry::{
    DeviceCategory, DeviceDescriptor, DeviceError, DeviceFactory, DeviceInit, DeviceRegistry,
    RegistryError,
};
pub use time::{Beats, SamplePos, SampleRate, Tempo, Ticks, TimeError, TICKS_PER_BEAT};
