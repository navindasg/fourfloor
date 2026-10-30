//! Phase 0 de-risking spikes for fourfloor.
//!
//! Each spike sits behind its own cargo feature so that a failure in one
//! third-party crate cannot break the workspace build.
//!
//! | Feature | Module | Third-party crate | Default |
//! | --- | --- | --- | --- |
//! | `stretch` | `stretch` | `signalsmith-stretch` (C++, needs a C++ compiler and libclang) | no |
//! | `hosting-truce` | `hosting_truce` | `truce-rack` (loads plugin binaries) | no |
//! | `hosting-clack` | `hosting_clack` | `clack-host` (loads CLAP binaries) | no |
//!
//! The metering (`ebur128`) and decode (`symphonia`) spikes passed and now
//! live in the `fourfloor-audio` crate.
//!
//! None of this code is meant to run on the audio thread.

#![warn(missing_docs)]

pub use fourfloor_core as core;

#[cfg(feature = "hosting-clack")]
pub mod hosting_clack;
#[cfg(feature = "hosting-truce")]
pub mod hosting_truce;
#[cfg(feature = "stretch")]
pub mod stretch;
