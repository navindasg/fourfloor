//! CLAP hosting on `clack-host`.
//!
//! - [`default_clap_dirs`], [`find_clap_bundles`] and [`scan_bundle`] find
//!   plugins and describe them.
//! - [`ClapDevice`] wraps one plugin as a [`fourfloor_core::Device`].
//!
//! # Why not truce-rack
//!
//! The plan named truce-rack for CLAP. Its CLAP wrapper allocates a `Vec`
//! of channel pointers on every block and reports a plugin panic with
//! `eprintln!` from inside `process`, has no latency query, and is fixed to
//! stereo in and out (all read in the 1.1.5 source; see the README). This
//! module drives `clack-host` directly instead, with every buffer the
//! process path needs allocated in `prepare`.
//!
//! # What is covered
//!
//! Scan, load, activate, process 32-bit audio with a main input, a
//! sidechain input and a main output, note and MIDI events in, note events
//! out, parameter changes, latency, state save and restore, and the host
//! side of the log, thread-check, params, state, latency, audio-ports and
//! note-ports extensions.
//!
//! # What is not
//!
//! - 64-bit audio, more than one output port (extra outputs are rendered
//!   and discarded) and surround layouts.
//! - Parameter changes the plugin makes itself (from its editor) are not
//!   read back; modulation and per-note parameter events are not sent.
//! - A latency change while running is only flagged
//!   ([`PluginRequests::latency_changed`](crate::PluginRequests)); the graph
//!   has to be recompiled to pick it up.
//! - Editors ([`crate::gui`]) and out-of-process hosting
//!   ([`crate::sandbox`]).

mod device;
mod events;
mod host;
mod instance;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fourfloor_core::DeviceCategory;

pub use device::ClapDevice;
#[cfg(target_os = "macos")]
pub(crate) use device::{drain_port, fill_port};
#[cfg(target_os = "macos")]
pub(crate) use events::HeldNotes;

use crate::error::HostError;
use crate::info::{PluginFormat, PluginInfo};
use crate::thread::{HostThread, MainBound, MainIdle};
use host::SharedFlags;
use instance::{load_entry, ClapMain, NoteInput};

/// How deep [`find_clap_bundles`] looks below a plugin directory.
const MAX_SCAN_DEPTH: usize = 6;

/// The conventional CLAP install directories for this OS, followed by any
/// in the `CLAP_PATH` environment variable.
pub fn default_clap_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        if let Some(home) = &home {
            dirs.push(home.join("Library/Audio/Plug-Ins/CLAP"));
        }
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    } else if cfg!(target_os = "windows") {
        if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
            dirs.push(PathBuf::from(common).join("CLAP"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs/Common/CLAP"));
        }
    } else {
        if let Some(home) = &home {
            dirs.push(home.join(".clap"));
        }
        dirs.push(PathBuf::from("/usr/lib/clap"));
    }
    if let Some(extra) = std::env::var_os("CLAP_PATH") {
        dirs.extend(std::env::split_paths(&extra));
    }
    dirs
}

/// Every path ending in `.clap` under `dir`, sorted. A `.clap` directory (a
/// macOS bundle) is returned as it is and not searched. A missing or
/// unreadable directory yields nothing.
pub fn find_clap_bundles(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_bundles(dir, "clap", 0, &mut found);
    found.sort();
    found
}

pub(crate) fn collect_bundles(dir: &Path, extension: &str, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == extension) {
            found.push(path);
        } else if path.is_dir() && depth < MAX_SCAN_DEPTH {
            collect_bundles(&path, extension, depth + 1, found);
        }
    }
}

/// What one bundle holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BundleScan {
    /// Plugins that could be created and described.
    pub plugins: Vec<PluginInfo>,
    /// `(plugin id, reason)` for plugins the bundle lists but that could
    /// not be created.
    pub failures: Vec<(String, String)>,
}

/// What a bundle's factory says about one plugin, before it is created.
struct Listed {
    id: String,
    name: String,
    vendor: String,
    version: String,
    features: Vec<String>,
}

/// Nothing to keep alive between calls: the scan does its work in one go.
struct ScanJob;

impl MainIdle for ScanJob {}

/// Lists and describes the plugins in one `.clap` file or bundle.
///
/// This loads the library and creates each plugin once, on `thread`, to
/// read its ports and parameters. A plugin that crashes while being created
/// takes the process with it; see [`crate::sandbox`] for the plan to move
/// scanning out of process.
pub fn scan_bundle(thread: &Arc<HostThread>, bundle: &Path) -> Result<BundleScan, HostError> {
    let job = MainBound::new(thread, || Ok::<_, HostError>(ScanJob))?;
    job.with(|_| scan_on_host_thread(bundle))?
}

fn scan_on_host_thread(bundle: &Path) -> Result<BundleScan, HostError> {
    let entry = load_entry(bundle)?;
    let factory = entry.get_plugin_factory().ok_or_else(|| HostError::Load {
        path: bundle.to_path_buf(),
        reason: "the file has no CLAP plugin factory".to_owned(),
    })?;
    let text = |value: Option<&std::ffi::CStr>| {
        value
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let listed: Vec<Listed> = factory
        .plugin_descriptors()
        .filter(|descriptor| descriptor.id().is_some_and(|id| !id.is_empty()))
        .map(|descriptor| Listed {
            id: text(descriptor.id()),
            name: text(descriptor.name()),
            vendor: text(descriptor.vendor()),
            version: text(descriptor.version()),
            features: descriptor
                .features()
                .map(|feature| feature.to_string_lossy().into_owned())
                .collect(),
        })
        .collect();

    let mut scan = BundleScan::default();
    for plugin in listed {
        let created = ClapMain::create(&entry, &plugin.id, Arc::new(SharedFlags::default()));
        let mut main = match created {
            Ok(main) => main,
            Err(error) => {
                scan.failures.push((plugin.id, error.to_string()));
                continue;
            }
        };
        let layout = main.layout();
        let params = main.params().into_iter().map(|param| param.info).collect();
        drop(main);

        let has = |feature: &str| plugin.features.iter().any(|f| f == feature);
        let takes_notes = layout.note_input != NoteInput::None;
        let category = if has("instrument") || (layout.inputs.is_empty() && takes_notes) {
            if layout.outputs.is_empty() {
                DeviceCategory::NoteEffect
            } else {
                DeviceCategory::Instrument
            }
        } else if has("note-effect") {
            DeviceCategory::NoteEffect
        } else {
            DeviceCategory::Effect
        };
        scan.plugins.push(PluginInfo {
            format: PluginFormat::Clap,
            name: if plugin.name.is_empty() {
                plugin.id.clone()
            } else {
                plugin.name
            },
            id: plugin.id,
            vendor: plugin.vendor,
            version: plugin.version,
            category,
            path: bundle.to_path_buf(),
            features: plugin.features,
            params,
            audio_inputs: layout.inputs.first().copied().unwrap_or(0),
            audio_outputs: layout.outputs.first().copied().unwrap_or(0),
            sidechain: layout.has_sidechain(),
            note_input: takes_notes,
            note_output: layout.note_output,
            loadable: true,
        });
    }
    Ok(scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fourfloor-host-clap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_dirs_are_not_empty() {
        assert!(!default_clap_dirs().is_empty());
    }

    #[test]
    fn bundles_are_found_recursively_and_not_descended_into() {
        let dir = scratch_dir("find");
        std::fs::create_dir_all(dir.join("Vendor/Synth.clap/Contents/MacOS")).unwrap();
        std::fs::write(dir.join("Vendor/Synth.clap/Contents/MacOS/inner.clap"), b"").unwrap();
        std::fs::write(dir.join("flat.clap"), b"").unwrap();
        std::fs::write(dir.join("readme.txt"), b"").unwrap();

        let found = find_clap_bundles(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            found,
            vec![dir.join("Vendor/Synth.clap"), dir.join("flat.clap")]
        );
    }

    #[test]
    fn a_missing_directory_has_no_bundles() {
        assert!(find_clap_bundles(Path::new("/definitely/not/a/real/clap/dir")).is_empty());
    }

    #[test]
    fn a_file_that_is_not_a_plugin_is_an_error_not_a_crash() {
        let dir = scratch_dir("bogus");
        let bogus = dir.join("bogus.clap");
        std::fs::write(&bogus, b"this is not a dynamic library").unwrap();
        let thread = HostThread::spawn().unwrap();
        let result = scan_bundle(&thread, &bogus);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            matches!(&result, Err(HostError::Load { path, .. }) if path == &bogus),
            "{result:?}"
        );
    }

    #[test]
    fn loading_a_missing_plugin_is_an_error() {
        let thread = HostThread::spawn().unwrap();
        let result = ClapDevice::load(&thread, Path::new("/definitely/not/here.clap"), "x", None);
        assert!(matches!(result, Err(HostError::Load { .. })));
    }
}
