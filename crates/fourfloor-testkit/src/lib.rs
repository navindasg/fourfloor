//! Test helpers for everything in fourfloor that makes or changes sound.
//!
//! The rule in this workspace is **verify sound with code**: every audio
//! feature has tests that render audio offline and assert on things that
//! can be measured. This crate is the shared toolbox for those tests. Add
//! it as a dev-dependency:
//!
//! ```toml
//! [dev-dependencies]
//! fourfloor-testkit = { workspace = true }
//! ```
//!
//! # Render
//!
//! | Function | Renders |
//! | --- | --- |
//! | [`render_instrument`] | a [`Device`](fourfloor_core::Device) fed note events ([`note`], [`notes`]) |
//! | [`render_effect`] | a `Device` fed a [`StereoBuffer`] |
//! | [`render_effect_with_sidechain`] | the same, with a key signal on its sidechain bus |
//! | [`render_device`] | the general form: audio, sidechain and events in; audio and events out |
//! | [`render_graph`] | a compiled engine graph, through the real engine |
//! | [`render_project`] | a whole project, compiled through a device registry |
//!
//! All of them return a [`StereoBuffer`] (`left`, `right`, `sample_rate`),
//! which can be saved with [`StereoBuffer::write_wav`] to listen to what a
//! failing test heard. [`RenderConfig`] sets sample rate, block size, tempo
//! and whether the transport rolls.
//!
//! # Make test signals
//!
//! [`sine`], [`white_noise`], [`impulse`], [`click_train`], [`silence`],
//! [`dc`], [`mix`]. All deterministic.
//!
//! # Measure and assert
//!
//! | Question | Function |
//! | --- | --- |
//! | What pitch is it? | [`dominant_frequency`] |
//! | Where is the energy? | [`spectrum_band_energy`], [`spectrum_band_energy_db`], [`spectrum`] |
//! | How loud, when? | [`rms`], [`rms_window`], [`rms_window_seconds`], [`peak`], [`peak_db`], [`rms_db`] |
//! | When do sounds start? | [`onset_times`], [`onset_times_with`], [`times_match`] |
//! | What shape is the level? | [`envelope`] |
//! | Programme loudness? | [`lufs`], [`true_peak_dbtp`], [`loudness`] |
//! | Mono, wide or out of phase? | [`stereo_correlation`], [`StereoBuffer::side`] |
//! | Are two renders the same? | [`null_test`], [`null_residual_db`], [`assert_null`] |
//! | Is it silent? | [`is_silent`] |
//! | Is it broken? | [`assert_no_nan_or_clipping`], [`assert_buffer_ok`], [`find_nan_or_clipping`] |
//!
//! Functions take plain `&[f32]` slices and a
//! [`SampleRate`](fourfloor_core::SampleRate); pass `&buffer.left`,
//! `&buffer.right` or `&buffer.mono()`.
//!
//! # Example
//!
//! ```
//! use fourfloor_core::{
//!     pitch_to_hz, Device, EventKind, PrepareConfig, ProcessIo, TransportContext,
//! };
//! use fourfloor_testkit::{
//!     assert_buffer_ok, dominant_frequency, is_silent, note, null_test, render_instrument,
//!     rms_window_seconds, RenderConfig,
//! };
//!
//! /// A one-voice sine synth: the device under test.
//! #[derive(Default)]
//! struct Beep {
//!     step: f32,
//!     phase: f32,
//!     rate: f32,
//! }
//!
//! impl Device for Beep {
//!     fn prepare(&mut self, config: &PrepareConfig) {
//!         self.rate = config.sample_rate.hz() as f32;
//!     }
//!
//!     fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
//!         let mut events = io.events.iter().peekable();
//!         for frame in 0..io.frames() {
//!             while let Some(event) = events.next_if(|e| e.frame as usize <= frame) {
//!                 match event.kind {
//!                     EventKind::NoteOn { pitch, .. } => {
//!                         self.step = pitch_to_hz(f32::from(pitch)) / self.rate;
//!                     }
//!                     EventKind::NoteOff { .. } | EventKind::AllNotesOff => self.step = 0.0,
//!                     _ => {}
//!                 }
//!             }
//!             let sample = if self.step > 0.0 {
//!                 0.5 * (std::f32::consts::TAU * self.phase).sin()
//!             } else {
//!                 0.0
//!             };
//!             self.phase = (self.phase + self.step).fract();
//!             for channel in io.outputs.iter_mut() {
//!                 channel[frame] = sample;
//!             }
//!         }
//!     }
//! }
//!
//! let config = RenderConfig::default();
//! let rate = config.sample_rate;
//! // A4 from 0.25 s to 0.75 s, in a one-second render.
//! let events = note(0.25, 0.5, 69, 100, rate);
//! let out = render_instrument(&mut Beep::default(), &events, 1.0, &config);
//!
//! assert_buffer_ok(&out);
//! assert_eq!(out.frames(), 48_000);
//! // Silent before the note, sounding at 440 Hz during it, silent after.
//! assert!(is_silent(&out.slice_seconds(0.0, 0.25).left, -120.0));
//! let during = out.slice_seconds(0.3, 0.7);
//! assert!((dominant_frequency(&during.left, rate) - 440.0).abs() < 1.0);
//! assert!(rms_window_seconds(&out.left, rate, 0.3, 0.4) > 0.3);
//! assert!(is_silent(&out.slice_seconds(0.76, 1.0).left, -120.0));
//! // Dual mono: left and right null against each other.
//! assert!(null_test(&out.left, &out.right, -120.0));
//!
//! // The result must not depend on the block size.
//! let small = render_instrument(&mut Beep::default(), &events, 1.0, &config.with_block(37));
//! assert!(null_test(&out.left, &small.left, -120.0));
//! ```
//!
//! # Catching allocation on the audio path
//!
//! [`render_device`] runs every `process` call inside `assert_no_alloc`.
//! That only bites when the test binary installs the guard allocator:
//!
//! ```ignore
//! #[cfg(test)]
//! #[global_allocator]
//! static ALLOCATOR: fourfloor_testkit::AllocDisabler = fourfloor_testkit::AllocDisabler;
//! ```
//!
//! With it, a device that allocates, frees or reallocates inside `process`
//! aborts the test with "memory allocation of N bytes failed". Put it in a
//! crate's `lib.rs` (under `cfg(test)`) or at the top of an integration
//! test file.
//!
//! # Limits
//!
//! Tests built on this crate need no audio device, display or installed
//! plugin. Everything here allocates and is meant for tests only.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod buffer;
mod check;
mod level;
mod onset;
mod render;
mod signal;
mod spectrum;

pub use assert_no_alloc::AllocDisabler;
pub use buffer::StereoBuffer;
pub use check::{
    assert_buffer_ok, assert_no_nan_or_clipping, assert_null, find_nan_or_clipping, loudness, lufs,
    null_residual_db, null_test, true_peak_dbtp, Fault,
};
pub use level::{
    envelope, from_db, is_silent, peak, peak_db, rms, rms_db, rms_window, rms_window_seconds,
    stereo_correlation, to_db,
};
pub use onset::{onset_samples_with, onset_times, onset_times_with, times_match, OnsetOptions};
pub use render::{
    note, notes, render_device, render_effect, render_effect_with_sidechain, render_graph,
    render_instrument, render_project, render_project_with, DeviceInput, DeviceOutput,
    RenderConfig, TimedEvent,
};
pub use signal::{click_train, dc, impulse, mix, seconds_to_frames, silence, sine, white_noise};
pub use spectrum::{
    dominant_frequency, spectrum, spectrum_band_energy, spectrum_band_energy_db, Spectrum,
};

pub use fourfloor_audio::Loudness;
pub use fourfloor_compile::{RenderError, RenderOptions};
