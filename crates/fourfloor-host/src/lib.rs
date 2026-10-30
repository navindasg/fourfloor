//! Plugin hosting: third-party plugins as [`fourfloor_core::Device`]s.
//!
//! | Format | State | Module |
//! | --- | --- | --- |
//! | CLAP | Hosted on `clack-host` | [`clap`] |
//! | Audio Unit v2 | Hosted on the AudioToolbox C API, macOS only | `au` |
//! | VST3 | Listed in the database, not loadable | [`vst3`] |
//!
//! # How the pieces fit
//!
//! 1. A scan ([`PluginDatabase::rescan`]) finds plugins and records a
//!    [`PluginInfo`] for each: id, name, category, ports and parameters.
//!    The database is cached as JSON so the app starts without loading a
//!    plugin.
//! 2. [`PluginHost::register_database`] adds each plugin to the
//!    [`DeviceRegistry`](fourfloor_core::DeviceRegistry) under the kind
//!    `plugin.<format>:<id>`, for example `plugin.clap:com.u-he.diva` or
//!    `plugin.au:aufx.dely.appl`.
//! 3. A project device with that kind then compiles like any other: the
//!    registry factory loads the plugin, restores the state blob from the
//!    project folder and returns the device.
//! 4. The app keeps a [`PluginController`] per plugin
//!    ([`PluginHost::controllers`]) to save state and read values.
//!
//! ```no_run
//! use fourfloor_core::DeviceRegistry;
//! use fourfloor_host::{PluginDatabase, PluginHost, ScanOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let host = PluginHost::new()?;
//! let mut database = PluginDatabase::new();
//! database.rescan(host.thread(), &ScanOptions::default());
//!
//! let mut registry = DeviceRegistry::new();
//! host.register_database(&mut registry, &database)?;
//! let device = registry.create_default("plugin.au:aufx.dely.appl")?;
//! # drop(device);
//! # Ok(())
//! # }
//! ```
//!
//! # Threads
//!
//! A plugin has a main-thread half and an audio-thread half. The device is
//! the audio-thread half and is real-time safe in `process`, `set_param`
//! and `reset`. The main-thread half lives on a dedicated
//! [`HostThread`]; `prepare`, loading, state calls and
//! dropping a device block on it. See [`thread`].
//!
//! # Not done
//!
//! VST3 loading ([`vst3`]), plugin editors ([`gui`]) and out-of-process
//! hosting ([`sandbox`]) are stubs with their design written down.

#![warn(missing_docs)]

#[cfg(target_os = "macos")]
pub mod au;
mod bridge;
pub mod clap;
mod database;
mod error;
pub mod gui;
mod info;
pub mod sandbox;
pub mod thread;
pub mod vst3;

pub use fourfloor_core as core;

pub use bridge::{register, HostedPlugin, PluginHost};
pub use clap::ClapDevice;
pub use database::{
    FileStamp, PluginDatabase, ScanFailure, ScanOptions, ScanReport, DATABASE_VERSION,
};
pub use error::HostError;
pub use info::{
    parse_plugin_kind, plugin_kind, PluginControl, PluginController, PluginFormat, PluginInfo,
    PluginRequests, KIND_PREFIX,
};
pub use thread::{HostThread, HostThreadError};
