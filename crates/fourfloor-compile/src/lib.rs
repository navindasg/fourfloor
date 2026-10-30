//! The project compiler: turns a [`fourfloor_model::Project`] into a
//! [`fourfloor_engine::CompiledGraph`], building every device through a
//! [`fourfloor_core::DeviceRegistry`].
//!
//! ```
//! use fourfloor_compile::{compile_project, CompileOptions};
//! use fourfloor_core::{DeviceRegistry, SampleRate};
//! use fourfloor_model::Project;
//!
//! let mut registry = DeviceRegistry::new();
//! fourfloor_engine::devices::register(&mut registry).unwrap();
//! // Device crates add theirs the same way:
//! // fourfloor_instruments::register(&mut registry).unwrap();
//!
//! let options = CompileOptions::new(SampleRate::HZ_48000, 512);
//! let compiled = compile_project(&Project::new("Empty"), &registry, &options).unwrap();
//! assert_eq!(compiled.graph.order(), [compiled.master]);
//! ```
//!
//! # How a project maps to a graph
//!
//! Every graph is stereo. Per track, in display order:
//!
//! - **MIDI track.** The notes its timeline clips sound (offsets and loops
//!   applied) go into one
//!   [`NotePlayer`](fourfloor_engine::devices::NotePlayer). If the track
//!   has clip-launcher slots, a
//!   [`ClipLauncher`](fourfloor_engine::devices::ClipLauncher) follows,
//!   with one row per project scene. The events then run through the
//!   track's note effects, in chain order, into the track's instrument.
//!   The instrument's audio runs through the track's effects, in chain
//!   order, into the track's [`Fader`](fourfloor_engine::devices::Fader)
//!   (gain, pan, mute).
//! - **Audio track.** Its audio clips are decoded from the project's audio
//!   pool, converted to the graph's sample rate, warped to the project
//!   tempo if they ask for it, and played by one
//!   [`AudioClipPlayer`](fourfloor_engine::devices::AudioClipPlayer)
//!   (position, gain, fades, loop), then effects and the fader. This needs
//!   [`CompileOptions::project_dir`].
//! - **Bus track.** A unity [`Gain`](fourfloor_engine::devices::Gain) sums
//!   everything routed or sent to the bus, then the bus's effects and its
//!   fader. Groups and returns are both buses.
//!
//! A fader feeds the bus named by the track's `output`, or the master
//! `Gain` (the graph output) when there is none.
//!
//! - **Sends.** Each send is a `Gain` node at the send level, fed from the
//!   track's signal before or after its fader, feeding the target bus.
//! - **Sidechains.** A device whose descriptor takes a sidechain and whose
//!   instance names a source track gets a sidechain edge from that track's
//!   signal, before or after its fader.
//! - **Automation and modulation.** Device automation lanes, track gain
//!   and pan lanes, and modulators (LFOs, note-triggered envelopes) become
//!   the graph's [`ControlPlan`](fourfloor_engine::ControlPlan): values
//!   are worked out once per block. A lane sets a parameter's centre and
//!   modulators swing around it by `depth` times the parameter's range.
//! - **Macros.** A parameter a macro drives starts at the value the
//!   macro's position gives it. Live changes travel as
//!   `Command::audio_messages`.
//!
//! The engine's delay compensation covers audio, send and sidechain
//! edges, so a latent device on one track keeps everything aligned.
//!
//! # Warping
//!
//! A warped audio clip is stretched before the graph is built, so that
//! material at the clip's `source_bpm` plays in time at the project tempo.
//! Without key lock it is resampled (pitch follows speed). With key lock
//! it is time-stretched by Signalsmith Stretch when the crate is built
//! with the `stretch` feature (channels are stretched one at a time); in a
//! build without it, the clip falls back to resampling and a warning says
//! so. Changing the project tempo means compiling again.
//!
//! # How a device chain entry maps to a registry kind
//!
//! A [`DeviceInstance`](fourfloor_model::DeviceInstance) is looked up by
//! its `kind` string, unchanged, with [`DeviceRegistry::descriptor`]. The
//! descriptor's [`DeviceCategory`](fourfloor_core::DeviceCategory) decides
//! where the device is wired (note effect, instrument or effect); its
//! position among devices of the same category is kept. The device is
//! built with [`DeviceRegistry::create`], given the instance's parameter
//! values and, when [`CompileOptions::project_dir`] is set, its saved state
//! blob. The graph node takes the instance's own [`DeviceId`], so an
//! `AudioMessage::SetParam` addressed to the model's device id reaches the
//! running device.
//!
//! - A bypassed device is left out of the graph.
//! - A kind that is not registered is left out and reported as a warning.
//! - The first instrument in a MIDI track's chain is used; others are
//!   reported and left out. A MIDI track with no instrument gets
//!   [`CompileOptions::default_instrument`]
//!   (`builtin.placeholder` unless changed), so a new project makes sound.
//!
//! Mute silences a track's fader. When any track is soloed, a fader stays
//! open only if its track is soloed, feeds a soloed bus, or is a bus that
//! carries a soloed track.
//!
//! # Rendering and export
//!
//! [`render_project`] renders the master offline; [`RenderOptions`] can
//! launch scenes at given times. [`render_stems`] renders the master and
//! one stem per track or bus that feeds the master, in a single pass, so
//! the stems add up to the master. [`normalize_loudness`] and
//! [`RenderedStems::normalize`] bring a render to a loudness target, and
//! [`RenderedStems::write_wav`] writes the files.
//!
//! # MIDI input
//!
//! [`controller_map`] and [`midi_router`] hand the project's controller
//! mappings to the engine's MIDI input; [`learned_command`] turns a MIDI
//! learn result into the `map_controller` command that saves it.
//!
//! Not compiled yet, each reported as a warning: audio clips in launcher
//! slots.
//!
//! [`DeviceRegistry::descriptor`]: fourfloor_core::DeviceRegistry::descriptor
//! [`DeviceRegistry::create`]: fourfloor_core::DeviceRegistry::create
//! [`DeviceId`]: fourfloor_core::DeviceId

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod audio;
mod build;
mod compile;
mod control;
mod export;
mod midi;
mod options;
mod render;
mod routing;

pub use audio::warp_ratio;
pub use compile::compile_project;
pub use export::{normalize_loudness, render_stems, ExportError, Normalized, RenderedStems, Stem};
pub use fourfloor_audio::SampleFormat;
pub use midi::{controller_map, learned_command, midi_router};
pub use options::{
    CompileError, CompileOptions, CompiledProject, CompiledTrack, CHANNELS, DEFAULT_INSTRUMENT,
};
pub use render::{render_project, RenderError, RenderOptions, RenderedProject, SceneLaunch};
pub use routing::{db_to_linear, project_length, track_notes};
