//! CLAP-only hosting fallback on `clack-host` 0.2.0.
//!
//! This is the fallback path in case `truce-rack` (see `hosting_truce`)
//! turns out not to be usable. It scans the conventional CLAP directories,
//! lists the plugins inside each bundle, and can push one silent stereo block
//! through a plugin.
//!
//! Everything here is main-thread, allocating code. The spike assumes one
//! stereo input port and one stereo output port because it does not pull in
//! `clack-extensions` to query the plugin's real audio-port layout.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use clack_host::prelude::*;

/// One plugin found inside a `.clap` bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapPluginInfo {
    /// Path of the `.clap` bundle or file the plugin lives in.
    pub bundle: PathBuf,
    /// The plugin's stable id, e.g. `com.u-he.diva`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Vendor, when the plugin reports one.
    pub vendor: String,
    /// CLAP feature strings (`instrument`, `audio-effect`, ...).
    pub features: Vec<String>,
}

/// A bundle that could not be opened or had no plugin factory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapScanFailure {
    /// Path of the bundle.
    pub bundle: PathBuf,
    /// Why it was skipped.
    pub reason: String,
}

/// The result of a CLAP scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClapScan {
    /// Every plugin found.
    pub plugins: Vec<ClapPluginInfo>,
    /// Every bundle that failed to load.
    pub failures: Vec<ClapScanFailure>,
}

/// The conventional CLAP install directories for this OS, plus anything in
/// the `CLAP_PATH` environment variable.
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

/// Recursively collect every path ending in `.clap` under `dir`. A `.clap`
/// directory (a macOS bundle) is returned as-is and not descended into.
/// Unreadable or missing directories yield nothing.
pub fn find_clap_bundles(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_bundles(dir, 0, &mut found);
    found.sort();
    found
}

const MAX_SCAN_DEPTH: usize = 6;

fn collect_bundles(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "clap") {
            found.push(path);
        } else if path.is_dir() && depth < MAX_SCAN_DEPTH {
            collect_bundles(&path, depth + 1, found);
        }
    }
}

/// List the plugins inside one `.clap` bundle.
///
/// This loads and runs code from the bundle, which is why it is the only
/// place that needs `unsafe`.
pub fn describe_bundle(bundle: &Path) -> Result<Vec<ClapPluginInfo>, String> {
    // SAFETY: loading a dynamic library runs its initialisers. The caller
    // chose to scan this path as a CLAP plugin; clack validates the entry
    // symbol and the CLAP version before use.
    let entry = unsafe { PluginEntry::load(bundle) }.map_err(|e| e.to_string())?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| "bundle has no plugin factory".to_string())?;
    let lossy = |s: Option<&std::ffi::CStr>| {
        s.map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    Ok(factory
        .plugin_descriptors()
        .filter(|d| d.id().is_some())
        .map(|d| ClapPluginInfo {
            bundle: bundle.to_path_buf(),
            id: lossy(d.id()),
            name: lossy(d.name()),
            vendor: lossy(d.vendor()),
            features: d
                .features()
                .map(|f| f.to_string_lossy().into_owned())
                .collect(),
        })
        .collect())
}

/// Scan the given directories for CLAP plugins.
pub fn scan_dirs(dirs: &[PathBuf]) -> ClapScan {
    let mut scan = ClapScan::default();
    for bundle in dirs.iter().flat_map(|d| find_clap_bundles(d)) {
        match describe_bundle(&bundle) {
            Ok(plugins) => scan.plugins.extend(plugins),
            Err(reason) => scan.failures.push(ClapScanFailure { bundle, reason }),
        }
    }
    scan
}

/// Scan [`default_clap_dirs`].
pub fn scan_default() -> ClapScan {
    scan_dirs(&default_clap_dirs())
}

/// What a plugin did with one block of silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClapSilentBlockReport {
    /// Frames processed.
    pub frames: usize,
    /// Status returned by the plugin.
    pub status: ProcessStatus,
    /// Largest absolute output sample.
    pub peak: f32,
    /// True when every output sample is finite.
    pub all_finite: bool,
}

struct SpikeShared;

impl<'a> SharedHandler<'a> for SpikeShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

struct SpikeHost;

impl HostHandlers for SpikeHost {
    type Shared<'a> = SpikeShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

/// Instantiate `plugin`, activate it, and process one block of stereo
/// silence.
pub fn process_silent_block(
    plugin: &ClapPluginInfo,
    sample_rate: f64,
    frames: u32,
) -> Result<ClapSilentBlockReport, String> {
    if frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }
    let id = CString::new(plugin.id.as_str()).map_err(|e| e.to_string())?;
    let host_info = HostInfo::new(
        "fourfloor-spikes",
        "fourfloor",
        "https://example.invalid/fourfloor",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|e| e.to_string())?;

    // SAFETY: see `describe_bundle`.
    let entry = unsafe { PluginEntry::load(&plugin.bundle) }.map_err(|e| e.to_string())?;
    let mut instance =
        PluginInstance::<SpikeHost>::new(|_| SpikeShared, |_| (), &entry, &id, &host_info)
            .map_err(|e| format!("instantiate failed: {e}"))?;

    let configuration = PluginAudioConfiguration {
        sample_rate,
        min_frames_count: frames,
        max_frames_count: frames,
    };
    let stopped = instance
        .activate(|_, _| (), configuration)
        .map_err(|e| format!("activate failed: {e}"))?;

    let frame_count = frames as usize;
    let mut input_data = [vec![0.0_f32; frame_count], vec![0.0_f32; frame_count]];
    let mut output_data = [vec![f32::NAN; frame_count], vec![f32::NAN; frame_count]];
    let mut input_ports = AudioPorts::with_capacity(2, 1);
    let mut output_ports = AudioPorts::with_capacity(2, 1);

    let mut started = match stopped.start_processing() {
        Ok(started) => started,
        Err(e) => return Err(format!("start_processing failed: {e}")),
    };
    let result = {
        let input_audio = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input_data.iter_mut().map(InputChannel::constant),
            ),
        }]);
        let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output_data.iter_mut().map(|b| b.as_mut_slice()),
            ),
        }]);
        started.process(
            &input_audio,
            &mut output_audio,
            &InputEvents::empty(),
            &mut OutputEvents::void(),
            None,
            None,
        )
    };
    instance.deactivate(started.stop_processing());

    let status = result.map_err(|e| format!("process failed: {e}"))?;
    let all_finite = output_data.iter().flatten().all(|s| s.is_finite());
    let peak = output_data
        .iter()
        .flatten()
        .filter(|s| s.is_finite())
        .fold(0.0_f32, |acc, s| acc.max(s.abs()));
    Ok(ClapSilentBlockReport {
        frames: frame_count,
        status,
        peak,
        all_finite,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fourfloor-spike-clack-{name}-{}",
            std::process::id()
        ));
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
    fn missing_directory_scans_as_empty() {
        let scan = scan_dirs(&[PathBuf::from("/definitely/not/a/real/clap/dir")]);
        assert_eq!(scan, ClapScan::default());
    }

    #[test]
    fn a_file_that_is_not_a_plugin_is_a_failure_not_a_crash() {
        let dir = scratch_dir("bogus");
        std::fs::write(dir.join("bogus.clap"), b"this is not a dynamic library").unwrap();
        let scan = scan_dirs(std::slice::from_ref(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(scan.plugins.is_empty());
        assert_eq!(scan.failures.len(), 1);
        assert!(scan.failures[0].bundle.ends_with("bogus.clap"));
    }

    #[test]
    fn processing_a_missing_plugin_is_an_error() {
        let info = ClapPluginInfo {
            bundle: PathBuf::from("/definitely/not/here.clap"),
            id: "nope".into(),
            name: String::new(),
            vendor: String::new(),
            features: Vec::new(),
        };
        assert!(process_silent_block(&info, 48_000.0, 64).is_err());
        assert!(process_silent_block(&info, 48_000.0, 0).is_err());
    }

    /// Needs installed CLAP plugins; drives the first one found.
    #[test]
    #[ignore = "loads and runs installed CLAP plugins"]
    fn scan_installed_clap_plugins_and_run_the_first() {
        let scan = scan_default();
        let first = scan
            .plugins
            .first()
            .expect("no CLAP plugins are installed on this machine");
        let report = process_silent_block(first, 48_000.0, 256).unwrap();
        assert!(report.all_finite);
    }
}
