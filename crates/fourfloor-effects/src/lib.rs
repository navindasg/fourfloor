//! Effects: the stock devices that process audio.
//!
//! Every effect implements [`fourfloor_core::Device`], is written for
//! stereo (a mono bus is handled, further channels are silenced) and
//! describes its parameters. [`register`] adds them all to a
//! [`DeviceRegistry`] under these kinds, which are stored in project files
//! and never change:
//!
//! | Kind | Device | What it is |
//! | --- | --- | --- |
//! | `fx.eq` | [`ParametricEq`] | Four bands plus high-pass and low-pass |
//! | `fx.isolator` | [`Isolator`] | DJ three-band isolator with kills |
//! | `fx.filter` | [`Filter`] | Resonant LP/HP/BP with a tempo-synced LFO |
//! | `fx.compressor` | [`Compressor`] | Compressor with external sidechain |
//! | `fx.glue` | [`Compressor::glue`] | The compressor with bus settings |
//! | `fx.limiter` | [`Limiter`] | Lookahead brickwall limiter (latent) |
//! | `fx.pump` | [`Pump`] | Tempo-synced volume shaper |
//! | `fx.delay` | [`Delay`] | Tempo-synced stereo / ping-pong delay |
//! | `fx.reverb` | [`Reverb`] | Plate and hall reverb |
//! | `fx.chorus` | [`Chorus`] | Juno-style stereo chorus |
//! | `fx.saturator` | [`Saturator`] | Tape, spiral, console and hard drive |
//! | `fx.utility` | [`Utility`] | Gain, pan, width, mono, polarity |
//!
//! The DSP is hand-written Rust. The build plan proposed generating it from
//! Faust; plain Rust keeps the build free of a code generator and lets each
//! device follow the engine's real-time rules directly.
//!
//! A device's parameter ids are the positions in its
//! [`params`](fourfloor_core::Device::params) list.

#![warn(missing_docs)]

mod biquad;
mod chorus;
mod clock;
mod compressor;
mod delay;
mod delay_line;
mod eq;
mod filter;
mod isolator;
mod limiter;
mod pump;
mod reverb;
mod saturator;
pub mod true_peak;
mod util;
mod utility;

pub use fourfloor_core as core;

pub use chorus::Chorus;
pub use compressor::Compressor;
pub use delay::Delay;
pub use eq::ParametricEq;
pub use filter::Filter;
pub use isolator::Isolator;
pub use limiter::Limiter;
pub use pump::Pump;
pub use reverb::Reverb;
pub use saturator::Saturator;
pub use utility::Utility;

use fourfloor_core::{DeviceCategory, DeviceDescriptor, DeviceRegistry, RegistryError};

/// Every kind [`register`] adds, in alphabetical order.
pub const KINDS: [&str; 12] = [
    "fx.chorus",
    "fx.compressor",
    "fx.delay",
    "fx.eq",
    "fx.filter",
    "fx.glue",
    "fx.isolator",
    "fx.limiter",
    "fx.pump",
    "fx.reverb",
    "fx.saturator",
    "fx.utility",
];

/// Registers a compressor kind, which is the only one marked as reading a
/// sidechain.
fn register_compressor(
    registry: &mut DeviceRegistry,
    kind: &str,
    name: &str,
    make: fn() -> Compressor,
) -> Result<(), RegistryError> {
    let descriptor =
        DeviceDescriptor::describe(kind, name, DeviceCategory::Effect, &make()).with_sidechain();
    registry.register(descriptor, move |_init| Ok(Box::new(make())))
}

/// Adds this crate's devices to `registry`. The command line and the app
/// call this at start-up.
pub fn register(registry: &mut DeviceRegistry) -> Result<(), RegistryError> {
    let effect = DeviceCategory::Effect;
    registry.register_device("fx.eq", "Parametric EQ", effect, ParametricEq::new)?;
    registry.register_device("fx.isolator", "Isolator", effect, Isolator::new)?;
    registry.register_device("fx.filter", "Filter", effect, Filter::new)?;
    register_compressor(registry, "fx.compressor", "Compressor", Compressor::new)?;
    register_compressor(registry, "fx.glue", "Glue Compressor", Compressor::glue)?;
    registry.register_device("fx.limiter", "Limiter", effect, Limiter::new)?;
    registry.register_device("fx.pump", "Pump", effect, Pump::new)?;
    registry.register_device("fx.delay", "Delay", effect, Delay::new)?;
    registry.register_device("fx.reverb", "Reverb", effect, Reverb::new)?;
    registry.register_device("fx.chorus", "Chorus", effect, Chorus::new)?;
    registry.register_device("fx.saturator", "Saturator", effect, Saturator::new)?;
    registry.register_device("fx.utility", "Utility", effect, Utility::new)?;
    Ok(())
}
