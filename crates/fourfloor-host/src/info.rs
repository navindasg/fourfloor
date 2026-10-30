//! What a scan learns about one plugin, and the handle used to talk to a
//! running one from outside the audio graph.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use fourfloor_core::{DeviceCategory, DeviceDescriptor, ParamId, ParamInfo};
use serde::{Deserialize, Serialize};

use crate::error::HostError;

/// A plugin format this crate knows about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    /// CLAP, hosted on `clack-host`.
    Clap,
    /// Audio Unit v2 (macOS).
    Au,
    /// VST3. Scanned by path only; see [`crate::vst3`].
    Vst3,
}

impl PluginFormat {
    /// The lowercase tag used in device kinds and the database.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Clap => "clap",
            Self::Au => "au",
            Self::Vst3 => "vst3",
        }
    }

    /// Parses a tag written by [`tag`](Self::tag).
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "clap" => Some(Self::Clap),
            "au" => Some(Self::Au),
            "vst3" => Some(Self::Vst3),
            _ => None,
        }
    }
}

impl fmt::Display for PluginFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// The prefix of every hosted-plugin device kind.
pub const KIND_PREFIX: &str = "plugin.";

/// Builds the device kind for a plugin: `plugin.<format>:<id>`, for example
/// `plugin.clap:com.u-he.diva` or `plugin.au:aufx.dely.appl`.
///
/// Kinds are stored in project files and cannot contain whitespace, so any
/// whitespace in `id` is written as `%20`-style escapes.
pub fn plugin_kind(format: PluginFormat, id: &str) -> String {
    let mut kind = format!("{KIND_PREFIX}{}:", format.tag());
    for c in id.chars() {
        if c.is_whitespace() || c == '%' {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                kind.push_str(&format!("%{byte:02X}"));
            }
        } else {
            kind.push(c);
        }
    }
    kind
}

/// Splits a device kind made by [`plugin_kind`] back into format and id.
/// Returns `None` for any other kind.
pub fn parse_plugin_kind(kind: &str) -> Option<(PluginFormat, String)> {
    let rest = kind.strip_prefix(KIND_PREFIX)?;
    let (tag, escaped) = rest.split_once(':')?;
    let format = PluginFormat::from_tag(tag)?;
    let mut bytes = Vec::with_capacity(escaped.len());
    let raw = escaped.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            let hex = escaped.get(index + 1..index + 3)?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let id = String::from_utf8(bytes).ok()?;
    (!id.is_empty()).then_some((format, id))
}

/// One plugin found by a scan. This is what the database stores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    /// The plugin's format.
    pub format: PluginFormat,
    /// The format's own stable id: a CLAP plugin id, or an Audio Unit
    /// `type.subtype.manufacturer` triple.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Vendor, when the plugin reports one.
    #[serde(default)]
    pub vendor: String,
    /// Version string, when the plugin reports one.
    #[serde(default)]
    pub version: String,
    /// Where the plugin sits in a device chain.
    pub category: DeviceCategory,
    /// The file or bundle the plugin lives in. Empty for Audio Units, which
    /// the system finds by id.
    #[serde(default)]
    pub path: PathBuf,
    /// The format's feature or type tags, as reported.
    #[serde(default)]
    pub features: Vec<String>,
    /// The plugin's automatable parameters at scan time.
    #[serde(default)]
    pub params: Vec<ParamInfo>,
    /// Channels of the main audio input (0 for an instrument).
    #[serde(default)]
    pub audio_inputs: u32,
    /// Channels of the main audio output.
    #[serde(default)]
    pub audio_outputs: u32,
    /// True when the plugin has a second audio input, used as a sidechain.
    #[serde(default)]
    pub sidechain: bool,
    /// True when the plugin takes notes.
    #[serde(default)]
    pub note_input: bool,
    /// True when the plugin emits notes.
    #[serde(default)]
    pub note_output: bool,
    /// False when this build can list the plugin but not load it (VST3).
    #[serde(default = "yes")]
    pub loadable: bool,
}

fn yes() -> bool {
    true
}

impl PluginInfo {
    /// The device kind projects store for this plugin.
    pub fn kind(&self) -> String {
        plugin_kind(self.format, &self.id)
    }

    /// The registry descriptor for this plugin.
    pub fn descriptor(&self) -> DeviceDescriptor {
        let descriptor = DeviceDescriptor::new(self.kind(), self.name.clone(), self.category)
            .with_params(dedup_params(&self.params));
        if self.sidechain {
            descriptor.with_sidechain()
        } else {
            descriptor
        }
    }
}

/// Drops parameters that repeat an earlier id; the registry refuses those.
pub(crate) fn dedup_params(params: &[ParamInfo]) -> Vec<ParamInfo> {
    let mut seen = std::collections::HashSet::new();
    params
        .iter()
        .filter(|param| seen.insert(param.id))
        .cloned()
        .collect()
}

/// What a running plugin has asked of its host since the last check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginRequests {
    /// The plugin's latency changed; the graph should be recompiled so
    /// delay compensation picks up the new value.
    pub latency_changed: bool,
    /// The plugin wants to be deactivated and activated again.
    pub restart: bool,
    /// The plugin's state changed and the project should be marked dirty.
    pub state_dirty: bool,
    /// The plugin's parameter list or values changed outside the host.
    pub params_changed: bool,
}

/// Main-thread access to a running hosted plugin.
///
/// A hosted [`Device`](fourfloor_core::Device) lives inside the audio graph,
/// where only the audio thread can reach it. This is the other half: a
/// handle the app keeps to save and restore state, read values and learn
/// what the plugin has asked for. Every call blocks on the plugin host
/// thread, so none of them may be made from the audio thread.
pub trait PluginControl: Send + Sync {
    /// The plugin's device kind.
    fn kind(&self) -> &str;

    /// Serialises the plugin's state to a blob for the project's
    /// `plugin-state/` folder.
    fn save_state(&self) -> Result<Vec<u8>, HostError>;

    /// Restores a blob written by [`save_state`](Self::save_state).
    fn load_state(&self, state: &[u8]) -> Result<(), HostError>;

    /// The latency the plugin reported when it was last activated.
    fn latency_samples(&self) -> u32;

    /// A parameter's current value in plain units, read from the plugin.
    fn param_value(&self, param: ParamId) -> Option<f64>;

    /// A parameter value as the plugin would display it, e.g. `-6.0 dB`.
    fn param_text(&self, param: ParamId, value: f64) -> Option<String>;

    /// Returns and clears what the plugin has requested since the last
    /// call.
    fn take_requests(&self) -> PluginRequests;

    /// Blocks the plugin failed to process since it was created. A plugin
    /// that fails outputs silence (instrument) or its dry input (effect).
    fn failed_blocks(&self) -> u32;

    /// Events dropped because a block carried more than the wrapper's
    /// preallocated capacity.
    fn dropped_events(&self) -> u32;

    /// Why the last activation failed, if it did.
    fn last_error(&self) -> Option<String>;
}

/// A shared [`PluginControl`].
pub type PluginController = Arc<dyn PluginControl>;

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_core::ParamUnit;

    #[test]
    fn kinds_round_trip_including_awkward_ids() {
        for (format, id) in [
            (PluginFormat::Clap, "com.u-he.diva"),
            (PluginFormat::Au, "aufx.dely.appl"),
            (PluginFormat::Vst3, "ABCDEF0123456789ABCDEF0123456789"),
            (PluginFormat::Clap, "has space\tand%percent"),
            (PluginFormat::Clap, "unicode-é:colon"),
        ] {
            let kind = plugin_kind(format, id);
            assert!(!kind.chars().any(char::is_whitespace), "{kind}");
            assert_eq!(parse_plugin_kind(&kind), Some((format, id.to_owned())));
        }
        assert_eq!(
            plugin_kind(PluginFormat::Clap, "com.u-he.diva"),
            "plugin.clap:com.u-he.diva"
        );
    }

    #[test]
    fn other_kinds_are_not_plugin_kinds() {
        for kind in [
            "drum.kick909",
            "plugin.clap",
            "plugin.clap:",
            "plugin.lv2:urn:x",
            "plugin.clap:bad%zz",
            "plugin.clap:cut%2",
        ] {
            assert_eq!(parse_plugin_kind(kind), None, "{kind}");
        }
    }

    #[test]
    fn descriptor_carries_kind_params_and_sidechain() {
        let level = ParamInfo::new(ParamId(1), "Level", 0.0, 1.0, 0.5, ParamUnit::Generic);
        let info = PluginInfo {
            format: PluginFormat::Clap,
            id: "org.example.comp".into(),
            name: "Comp".into(),
            vendor: String::new(),
            version: String::new(),
            category: DeviceCategory::Effect,
            path: PathBuf::new(),
            features: Vec::new(),
            params: vec![level.clone(), level.clone()],
            audio_inputs: 2,
            audio_outputs: 2,
            sidechain: true,
            note_input: false,
            note_output: false,
            loadable: true,
        };
        let descriptor = info.descriptor();
        assert_eq!(descriptor.kind, "plugin.clap:org.example.comp");
        assert_eq!(descriptor.params, vec![level], "duplicate ids are dropped");
        assert!(descriptor.sidechain);
    }
}
