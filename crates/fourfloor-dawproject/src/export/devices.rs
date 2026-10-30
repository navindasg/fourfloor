//! Export of device chains: hosted plugins become `ClapPlugin`,
//! `Vst3Plugin` or `AuPlugin` with their state embedded; fourfloor's own
//! devices become `BuiltinDevice` elements that only fourfloor reads back.

use fourfloor_core::DeviceCategory;
use fourfloor_model::{DeviceInstance, PluginStateStore, Track, TrackKind};

use super::{location, Exporter};
use crate::naming;
use crate::report::Subject;
use crate::units::{plugin_from_kind, PluginElement};
use crate::xml::{El, XmlWriter};

/// `deviceVendor` of fourfloor's own devices; their `deviceID` is the kind.
pub(crate) const FOURFLOOR_VENDOR: &str = "fourfloor";

/// Kind prefixes of fourfloor's own instruments, for when no registry says.
const INSTRUMENT_PREFIXES: [&str; 8] = [
    "drum.",
    "synth.",
    "fm.",
    "poly.",
    "sampler",
    "builtin.placeholder",
    "builtin.sine",
    "builtin.metronome",
];

/// The file extension of an embedded state blob for a plugin format.
pub(crate) fn state_extension(element: Option<PluginElement>) -> &'static str {
    match element {
        Some(PluginElement::Clap) => "clap-preset",
        Some(PluginElement::Vst3) => "vstpreset",
        Some(PluginElement::Au) => "aupreset",
        Some(PluginElement::Vst2) => "fxp",
        None => "bin",
    }
}

fn role_name(category: DeviceCategory) -> &'static str {
    match category {
        DeviceCategory::Instrument => "instrument",
        DeviceCategory::NoteEffect => "noteFX",
        DeviceCategory::Effect => "audioFX",
    }
}

impl Exporter<'_> {
    fn known_category(&self, kind: &str) -> Option<DeviceCategory> {
        if let Some(descriptor) = self.options.registry.and_then(|r| r.descriptor(kind)) {
            return Some(descriptor.category);
        }
        if kind.starts_with("note.") {
            Some(DeviceCategory::NoteEffect)
        } else if INSTRUMENT_PREFIXES.iter().any(|p| kind.starts_with(p)) {
            Some(DeviceCategory::Instrument)
        } else if kind.starts_with("fx.") || kind.starts_with("builtin.") {
            Some(DeviceCategory::Effect)
        } else {
            None
        }
    }

    /// The `deviceRole` of each device of the track, in chain order. An
    /// unknown device (a plugin without a registry) on a MIDI track that has
    /// no known instrument is taken to be the instrument if it is the first
    /// such device; every other unknown device is an audio effect.
    fn roles(&self, track: &Track) -> Vec<DeviceCategory> {
        let known: Vec<Option<DeviceCategory>> = track
            .devices
            .iter()
            .map(|device| self.known_category(&device.kind))
            .collect();
        let has_instrument = known.contains(&Some(DeviceCategory::Instrument));
        let mut instrument_free = track.kind == TrackKind::Midi && !has_instrument;
        known
            .into_iter()
            .map(|category| {
                category.unwrap_or_else(|| {
                    if instrument_free {
                        instrument_free = false;
                        DeviceCategory::Instrument
                    } else {
                        DeviceCategory::Effect
                    }
                })
            })
            .collect()
    }

    pub(super) fn devices(&mut self, w: &mut XmlWriter, track: &Track) {
        if track.devices.is_empty() {
            return;
        }
        let roles = self.roles(track);
        w.open(El::new("Devices"));
        for (device, role) in track.devices.iter().zip(roles) {
            self.device(w, track, device, role);
        }
        w.close();
    }

    fn device(
        &mut self,
        w: &mut XmlWriter,
        track: &Track,
        device: &DeviceInstance,
        role: DeviceCategory,
    ) {
        let plugin = plugin_from_kind(&device.kind);
        let at = location(track, &device.name);
        let el = match &plugin {
            Some((format, id)) => El::new(format.element()).attr("deviceID", id),
            None => {
                self.report.preserved(
                    Subject::Device,
                    at.clone(),
                    format!(
                        "fourfloor device `{}` is written as a generic BuiltinDevice; other applications will not load it",
                        device.kind
                    ),
                );
                El::new("BuiltinDevice")
                    .attr("deviceID", &device.kind)
                    .attr("deviceVendor", FOURFLOOR_VENDOR)
            }
        };
        w.open(
            el.attr("id", naming::device(device.id))
                .attr("name", &device.name)
                .attr("deviceName", &device.name)
                .attr("deviceRole", role_name(role))
                .attr("loaded", true),
        );
        w.open(El::new("Parameters"));
        for param in &device.params {
            w.leaf(
                El::new("RealParameter")
                    .attr("id", naming::param(device.id, param.id))
                    .attr("name", &param.name)
                    // xs:int is signed; ids above i32::MAX wrap and are
                    // read back the same way.
                    .attr("parameterID", param.id.get() as i32)
                    .attr("unit", "linear")
                    .attr("min", param.min)
                    .attr("max", param.max)
                    .attr("value", param.value),
            );
        }
        w.close();
        w.leaf(
            El::new("Enabled")
                .attr("id", naming::device_enabled(device.id))
                .attr("name", "On/Off")
                .attr("value", !device.bypassed),
        );
        if let Some(path) = self.state_file(track, device, plugin.map(|(format, _)| format)) {
            w.leaf(El::new("State").attr("path", path));
        }
        w.close();
    }

    /// Embeds a device's state blob and returns its archive path.
    fn state_file(
        &mut self,
        track: &Track,
        device: &DeviceInstance,
        format: Option<PluginElement>,
    ) -> Option<String> {
        let hash = device.state_blob.as_ref()?;
        let at = location(track, &device.name);
        let Some(dir) = self.dir else {
            self.report.missing(
                Subject::PluginState,
                at,
                "no project folder was given, so the device state cannot be embedded",
            );
            return None;
        };
        match PluginStateStore::new(dir).get(hash) {
            Ok(bytes) => {
                let path = format!("plugins/{hash}.{}", state_extension(format));
                // A hex name is always a valid entry name.
                let _ = self.files.insert(&path, bytes);
                Some(path)
            }
            Err(error) => {
                self.report.missing(
                    Subject::PluginState,
                    at,
                    format!("the device state cannot be read ({error}); the device is written without it"),
                );
                None
            }
        }
    }
}
