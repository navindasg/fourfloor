//! Audio Unit v2 hosting on the AudioToolbox C API (macOS only).
//!
//! - [`list_components`] lists the installed effects and instruments and
//!   [`describe`] reads one unit's buses and parameters for the database.
//! - [`AuDevice`] wraps one unit as a [`fourfloor_core::Device`].
//!
//! # Why not truce-rack
//!
//! The Phase 0 spike hosted Apple's units through `truce-rack-au` 1.1.5.
//! Its render path is acceptable for an audio thread (the buffer list is
//! preallocated and nothing is logged), but the wrapper cannot do what the
//! engine needs, all read in its source:
//!
//! - It never reads `kAudioUnitProperty_Latency`, so delay compensation
//!   would have nothing to work from.
//! - It is fixed to one stereo input and one stereo output: no mono units,
//!   no sidechain bus.
//! - Its instance is tied to the thread that made it, with no split between
//!   a main-thread half and an audio-thread half to fit a `Send` device.
//!
//! Audio Unit v2 is a plain C API, so this module binds the dozen
//! functions it needs by hand ([`ffi`](self) is private) and adds no
//! dependency: no `objc2`, no `coreaudio-sys`.
//!
//! # What is covered
//!
//! Effects (`aufx`), music effects (`aumf`) and instruments (`aumu`): list,
//! instantiate, negotiate a mono or stereo layout, render with a main input
//! and an optional sidechain bus, MIDI in, parameter changes, latency,
//! `ClassInfo` state save and restore, and tempo and transport through the
//! host callbacks.
//!
//! # What is not
//!
//! - Units that need asynchronous instantiation (Audio Unit v3 app
//!   extensions the system cannot bridge to the v2 call) are not listed.
//! - Property listeners are not installed, so a unit that changes its own
//!   latency or parameters is not noticed until the next activation, and
//!   [`PluginControl::take_requests`](crate::PluginControl::take_requests)
//!   is always empty.
//! - Parameters outside the global scope, MIDI output and Cocoa views
//!   ([`crate::gui`]).

mod device;
mod ffi;
mod render;
mod unit;

use std::sync::Arc;

pub use device::AuDevice;
pub use unit::{component_id, list_components, parse_component_id, AuComponent};

use crate::error::HostError;
use crate::info::PluginInfo;
use crate::thread::{HostThread, MainBound};

/// Instantiates `component` once, on `thread`, to read its buses and
/// parameters, and disposes of it again.
///
/// A unit that crashes while being created takes the process with it; see
/// [`crate::sandbox`].
pub fn describe(
    thread: &Arc<HostThread>,
    component: &AuComponent,
) -> Result<PluginInfo, HostError> {
    let id = component.id.as_str();
    let main = MainBound::new(thread, move || {
        unit::AuMain::create(id, Arc::new(render::TransportShared::default()))
    })?;
    Ok(main.with(|main| main.info(component))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_component_is_not_found_and_a_bad_id_is_invalid() {
        let thread = HostThread::spawn().unwrap();
        let missing = AuComponent {
            id: "aufx.zzzz.zzzz".into(),
            name: "Missing".into(),
            vendor: String::new(),
            version: "0.0.0".into(),
        };
        assert!(matches!(
            describe(&thread, &missing),
            Err(HostError::NotFound { .. })
        ));
        let bad = AuComponent {
            id: "not an id".into(),
            ..missing
        };
        assert!(matches!(
            describe(&thread, &bad),
            Err(HostError::InvalidId(_))
        ));
    }
}
