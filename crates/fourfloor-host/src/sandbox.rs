//! Out-of-process hosting: **design notes and a stub API. Every plugin
//! runs in the fourfloor process today.**
//!
//! # Why
//!
//! In-process, one plugin that crashes takes the session with it, and
//! scanning (which loads every plugin once) is where that is most likely.
//! The plan moves plugins into helper processes in two steps.
//!
//! # Step 1: scan in a helper
//!
//! A `fourfloor-plugin-scan <format> <path>` helper prints a
//! [`BundleScan`] as JSON on stdout. The app runs
//! one helper per file with a timeout; a crash or a hang becomes a
//! [`ScanFailure`](crate::ScanFailure) for that file and the scan carries
//! on. This needs no shared memory and no changes to the device code, so
//! it comes first. [`scan_out_of_process`] is its entry point.
//!
//! # Step 2: process in a helper
//!
//! One helper process per plugin (or per [`SandboxGroup`]), holding the
//! real format wrapper. In the app the plugin is a proxy device:
//!
//! - **Audio** crosses in a shared-memory region allocated in `prepare`:
//!   input and output buffers for one block, an event ring in each
//!   direction and a small header (frame count, transport, a sequence
//!   number).
//! - **Wake-up** is a pair of semaphores (futex / Mach semaphore / Win32
//!   event). `process` writes the block, signals the helper and waits with
//!   a deadline of a fraction of the block's duration. No allocation, no
//!   pipe I/O and no unbounded wait on the audio thread.
//! - **A missed deadline or a dead helper** makes the proxy output silence
//!   (instrument) or its dry input (effect) for that block and flag the
//!   failure, the same contract as [`PluginControl::failed_blocks`](crate::PluginControl::failed_blocks)
//!   (crate::PluginControl::failed_blocks). The app restarts the helper
//!   and restores the last saved state.
//! - **Main-thread calls** (state, parameter text, editor) go over a
//!   length-prefixed message pipe handled by the helper's main thread.
//! - **Latency**: the round trip is synchronous within the block, so it
//!   adds scheduling jitter but no fixed latency as long as the helper's
//!   thread has real-time priority. If that proves unreliable the fallback
//!   is one block of buffering, reported through `latency_samples` so
//!   delay compensation hides it. That cost is why sandboxing stays
//!   optional per plugin, as in Bitwig.
//!
//! # Stub API
//!
//! [`SandboxMode`] is the setting the app will store per plugin, and the
//! two functions below are the entry points. Both return
//! [`HostError::Unsupported`].

use std::path::Path;

use crate::bridge::HostedPlugin;
use crate::clap::BundleScan;
use crate::error::HostError;
use crate::info::{PluginFormat, PluginInfo};

/// Which helper process a plugin shares.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SandboxGroup {
    /// A process for this plugin instance alone.
    PerInstance,
    /// One process for every instance of the same plugin.
    PerPlugin,
    /// One process for every plugin of the same vendor.
    PerVendor(String),
}

/// Where a plugin runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum SandboxMode {
    /// Inside the fourfloor process. The only mode that works today.
    #[default]
    InProcess,
    /// In a helper process. Not implemented.
    OutOfProcess(SandboxGroup),
}

/// **Stub.** Scans one plugin file in a helper process, so a crashing
/// plugin cannot take the app down. Always returns
/// [`HostError::Unsupported`]; use [`crate::clap::scan_bundle`], which
/// scans in-process.
pub fn scan_out_of_process(_format: PluginFormat, path: &Path) -> Result<BundleScan, HostError> {
    Err(HostError::Unsupported(format!(
        "out-of-process scanning is not implemented yet ({})",
        path.display()
    )))
}

/// **Stub.** Loads a plugin according to `mode`. [`SandboxMode::InProcess`]
/// is refused too: call [`PluginHost::load`](crate::PluginHost::load) for
/// that. Always returns [`HostError::Unsupported`].
pub fn load_sandboxed(
    info: &PluginInfo,
    mode: &SandboxMode,
    _state: Option<&[u8]>,
) -> Result<HostedPlugin, HostError> {
    Err(HostError::Unsupported(match mode {
        SandboxMode::InProcess => format!(
            "load_sandboxed does not load in-process; use PluginHost::load for {}",
            info.kind()
        ),
        SandboxMode::OutOfProcess(_) => format!(
            "out-of-process hosting is not implemented yet ({})",
            info.kind()
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stubs_say_they_are_not_implemented() {
        let error = scan_out_of_process(PluginFormat::Clap, Path::new("/x/y.clap")).unwrap_err();
        assert!(matches!(error, HostError::Unsupported(_)));
        assert!(error.to_string().contains("/x/y.clap"));
        assert_eq!(SandboxMode::default(), SandboxMode::InProcess);

        let info = crate::vst3::describe_bundle(Path::new("/x/Thing.vst3"));
        for mode in [
            SandboxMode::InProcess,
            SandboxMode::OutOfProcess(SandboxGroup::PerInstance),
        ] {
            let error = load_sandboxed(&info, &mode, None).err().unwrap();
            assert!(matches!(error, HostError::Unsupported(_)), "{error}");
        }
    }
}
