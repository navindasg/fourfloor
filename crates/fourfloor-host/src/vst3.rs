//! VST3: found and listed, **not hosted yet**.
//!
//! # What works
//!
//! [`default_vst3_dirs`] and [`find_vst3_bundles`] find `.vst3` bundles, and
//! [`describe_bundle`] lists each one in the plugin database without
//! loading any code: from the bundle's `moduleinfo.json` when it has one
//! (VST3 SDK 3.7.5 and later write it), otherwise from the file name. Every
//! entry has [`PluginInfo::loadable`] set to `false`.
//!
//! # What is a stub, and why
//!
//! [`load`] always returns [`HostError::Unsupported`]. A Rust path exists
//! but is not small, and could not be tested here:
//!
//! - The `vst3` crate (0.3.0, MIT OR Apache-2.0) is the raw COM bindings
//!   generated from the SDK headers and nothing more. A host has to
//!   implement, as COM objects, `IHostApplication`, `IComponentHandler`,
//!   `IParameterChanges` with one `IParamValueQueue` per parameter,
//!   `IEventList` and `IBStream`; load the module and call its
//!   `bundleEntry`/`InitDll` and `GetPluginFactory`; create the processor
//!   and the separate edit controller class and connect them through
//!   `IConnectionPoint`; and negotiate bus arrangements. That is about as
//!   much code as the CLAP and Audio Unit hosts together.
//! - `truce-rack-vst3` does that work, but its process path allocates a
//!   `Vec` of channel pointers on every block and it has no latency query
//!   (see the README), so it cannot be used on the audio thread as it is.
//! - No VST3 plugin is installed on the development machine, and unlike
//!   CLAP there is no light way to build a fixture plugin in this
//!   repository, so a host written now could only be compiled, not run.
//!
//! The plan: write the host on the `vst3` crate with the same split as
//! [`crate::clap`] (main-thread half in a `MainBound`, preallocated
//! parameter queues and event list on the audio thread), and test it
//! against a fixture built with a VST3-capable Rust plugin framework.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fourfloor_core::{Device, DeviceCategory};

use crate::error::HostError;
use crate::info::{PluginFormat, PluginInfo};
use crate::thread::HostThread;

/// The conventional VST3 install directories for this OS.
pub fn default_vst3_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        if let Some(home) = &home {
            dirs.push(home.join("Library/Audio/Plug-Ins/VST3"));
        }
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    } else if cfg!(target_os = "windows") {
        if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
            dirs.push(PathBuf::from(common).join("VST3"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs/Common/VST3"));
        }
    } else {
        if let Some(home) = &home {
            dirs.push(home.join(".vst3"));
        }
        dirs.push(PathBuf::from("/usr/lib/vst3"));
        dirs.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    dirs
}

/// Every path ending in `.vst3` under `dir`, sorted. Bundles are not
/// searched inside.
pub fn find_vst3_bundles(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    crate::clap::collect_bundles(dir, "vst3", 0, &mut found);
    found.sort();
    found
}

/// The first audio class in a bundle's `moduleinfo.json`.
struct ModuleClass {
    cid: String,
    name: String,
    vendor: String,
    version: String,
    sub_categories: Vec<String>,
}

fn read_module_info(bundle: &Path) -> Option<ModuleClass> {
    let text = [
        "Contents/Resources/moduleinfo.json",
        "Contents/moduleinfo.json",
    ]
    .iter()
    .find_map(|relative| std::fs::read_to_string(bundle.join(relative)).ok())?;
    // The file is JSON5 in principle; the SDK's own writer emits plain
    // JSON, which is all that is accepted here.
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let class = json.get("Classes")?.as_array()?.iter().find(|class| {
        class.get("Category").and_then(|c| c.as_str()) == Some("Audio Module Class")
    })?;
    let text = |value: Option<&serde_json::Value>| {
        value
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let cid = text(class.get("CID"));
    if cid.is_empty() || cid.chars().any(char::is_whitespace) {
        return None;
    }
    Some(ModuleClass {
        cid,
        name: text(class.get("Name")),
        vendor: text(class.get("Vendor"))
            .if_empty(|| text(json.get("Factory Info").and_then(|info| info.get("Vendor")))),
        version: text(class.get("Version")),
        sub_categories: class
            .get("Sub Categories")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

trait IfEmpty {
    fn if_empty(self, fallback: impl FnOnce() -> String) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: impl FnOnce() -> String) -> String {
        if self.is_empty() {
            fallback()
        } else {
            self
        }
    }
}

/// Describes a `.vst3` bundle without loading it.
///
/// With a `moduleinfo.json` the id is the processor class id. Without one
/// the id is `file:<bundle name>`, which is only as stable as the file
/// name; a real host must replace it with the class id once it can load
/// the module.
pub fn describe_bundle(bundle: &Path) -> PluginInfo {
    let stem = bundle
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let module = read_module_info(bundle);
    let (id, name, vendor, version, features) = match module {
        Some(class) => (
            class.cid,
            class.name.if_empty(|| stem.clone()),
            class.vendor,
            class.version,
            class.sub_categories,
        ),
        None => (
            format!("file:{stem}"),
            stem,
            String::new(),
            String::new(),
            Vec::new(),
        ),
    };
    let category = if features.iter().any(|f| f == "Instrument") {
        DeviceCategory::Instrument
    } else {
        DeviceCategory::Effect
    };
    PluginInfo {
        format: PluginFormat::Vst3,
        id,
        name,
        vendor,
        version,
        category,
        path: bundle.to_path_buf(),
        features,
        params: Vec::new(),
        audio_inputs: 0,
        audio_outputs: 0,
        sidechain: false,
        note_input: false,
        note_output: false,
        loadable: false,
    }
}

/// **Stub.** Loading a VST3 plugin is not implemented; see the module
/// documentation for the reason and the plan. Always returns
/// [`HostError::Unsupported`].
pub fn load(
    _thread: &Arc<HostThread>,
    info: &PluginInfo,
    _state: Option<&[u8]>,
) -> Result<Box<dyn Device>, HostError> {
    Err(HostError::Unsupported(format!(
        "VST3 hosting is not implemented yet, so \"{}\" cannot be loaded; \
         use the plugin's CLAP or Audio Unit version",
        info.name
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fourfloor-host-vst3-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bundles_are_found_and_described_from_their_name() {
        let dir = scratch_dir("name");
        std::fs::create_dir_all(dir.join("Vendor/Wide Synth.vst3/Contents/MacOS")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        let found = find_vst3_bundles(&dir);
        assert_eq!(found, vec![dir.join("Vendor/Wide Synth.vst3")]);

        let info = describe_bundle(&found[0]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(info.id, "file:Wide Synth");
        assert_eq!(info.name, "Wide Synth");
        assert_eq!(info.kind(), "plugin.vst3:file:Wide%20Synth");
        assert!(!info.loadable);
    }

    #[test]
    fn module_info_supplies_the_class_id_name_and_category() {
        let dir = scratch_dir("moduleinfo");
        let bundle = dir.join("Synth.vst3");
        std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        std::fs::write(
            bundle.join("Contents/Resources/moduleinfo.json"),
            r#"{
              "Name": "Synth",
              "Factory Info": { "Vendor": "Example Audio" },
              "Classes": [
                { "CID": "11111111111111111111111111111111", "Category": "Component Controller Class", "Name": "Synth Controller" },
                { "CID": "ABCDEF0123456789ABCDEF0123456789", "Category": "Audio Module Class", "Name": "Example Synth",
                  "Version": "2.0.1", "Sub Categories": ["Instrument", "Synth"] }
              ]
            }"#,
        )
        .unwrap();
        let info = describe_bundle(&bundle);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(info.id, "ABCDEF0123456789ABCDEF0123456789");
        assert_eq!(info.name, "Example Synth");
        assert_eq!(info.vendor, "Example Audio");
        assert_eq!(info.version, "2.0.1");
        assert_eq!(info.category, DeviceCategory::Instrument);
        assert_eq!(info.kind(), "plugin.vst3:ABCDEF0123456789ABCDEF0123456789");
    }

    #[test]
    fn loading_is_an_unsupported_error_that_names_the_plugin() {
        let thread = HostThread::spawn().unwrap();
        let info = describe_bundle(Path::new("/nowhere/Some Reverb.vst3"));
        let error = load(&thread, &info, None).err().unwrap();
        assert!(matches!(error, HostError::Unsupported(_)));
        assert!(error.to_string().contains("Some Reverb"), "{error}");
    }
}
