//! Real-time audio engine for fourfloor: graph, transport and the audio
//! callback.
//!
//! The pieces, in the order data flows through them:
//!
//! - [`devices`] holds the built-in devices, including the per-track
//!   [`devices::NotePlayer`] that turns clip notes into events and the
//!   [`devices::Fader`] that gives each track gain and pan.
//! - [`graph::GraphBuilder`] describes devices and their audio, sidechain
//!   and event connections. Its
//!   [`compile`](graph::GraphBuilder::compile) step (off the audio thread)
//!   sorts the nodes, works out delay compensation and preallocates every
//!   buffer, producing an immutable [`graph::CompiledGraph`].
//! - [`engine::engine`] creates the two halves of a running engine: an
//!   [`engine::EngineController`] for the UI/command side and an
//!   [`engine::AudioProcessor`] for the audio thread. They talk through
//!   `rtrb` queues (commands and graphs in, retired graphs out) and atomics
//!   (playhead and peak meters out).
//! - [`offline::render`] drives a processor without an audio device; the
//!   optional [`backend`] module drives it from cpal.
//! - [`midi`] parses MIDI, maps controllers to parameters (with MIDI
//!   learn) and generates MIDI clock; with the `midi` feature it also
//!   opens real ports through midir.
//!
//! Rule for everything that runs on the audio thread: no allocation, no
//! locks, no logging. [`engine::AudioProcessor::process_interleaved`] runs
//! inside `assert_no_alloc`, which aborts on allocation in debug builds when
//! the binary installs [`AllocDisabler`] as its global allocator.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod backend;
pub mod delay;
pub mod devices;
pub mod engine;
pub mod graph;
pub mod midi;
pub mod offline;
pub mod transport;

pub use fourfloor_core as core;

/// Allocator that enforces the no-allocation rule. Install it in a binary
/// to make the engine's allocation guard active:
///
/// ```
/// use fourfloor_engine::AllocDisabler;
///
/// #[global_allocator]
/// static ALLOCATOR: AllocDisabler = AllocDisabler;
/// ```
///
/// It forwards to the system allocator. In debug builds the engine marks
/// the audio callback as allocation-free, and this allocator aborts the
/// process if anything allocates or frees inside it. In release builds the
/// engine never sets that mark, so nothing aborts.
pub use assert_no_alloc::AllocDisabler;

pub use engine::{engine, AudioProcessor, EngineConfig, EngineController, EngineError};
pub use graph::{
    CompiledGraph, ControlPlan, ControlPoint, ControlTarget, GraphBuilder, GraphError, Input,
    LfoRate, LfoShape, ModulatorSpec, DEFAULT_EVENT_CAPACITY, MAX_CHANNELS, MAX_INPUT_PORTS,
};
pub use transport::{LoopRegion, Transport};

// Unit tests run with the guard live, so any render in a test that allocates
// on the processing path aborts the test binary.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: AllocDisabler = AllocDisabler;
