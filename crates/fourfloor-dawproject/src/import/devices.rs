//! Import of device chains: plugin references with their state, fourfloor's
//! own devices, and placeholders for devices fourfloor does not have.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use fourfloor_core::{DeviceId, ParamId};
use fourfloor_model::validate::MAX_PARAMS_PER_DEVICE;
use fourfloor_model::{ContentHash, DeviceInstance, Param};

use super::{sanitize_name, Importer, Target};
use crate::container::normalize_path;
use crate::export::devices::FOURFLOOR_VENDOR;
use crate::naming;
use crate::report::Subject;
use crate::units::{kind_from_plugin, PluginElement};
use crate::xml::{boolean, child, elements, integer, number, tag, Node};

/// Parameter elements the generic built-in devices carry directly.
const TYPED_PARAMS: [&str; 8] = [
    "Attack",
    "AutoMakeup",
    "InputGain",
    "OutputGain",
    "Ratio",
    "Range",
    "Release",
    "Threshold",
];

fn plugin_element(tag: &str) -> Option<PluginElement> {
    PluginElement::ALL.into_iter().find(|e| e.element() == tag)
}

/// One parameter read from the XML, before it gets its id.
struct RawParam<'a> {
    xml_id: Option<&'a str>,
    wanted: Option<u32>,
    name: String,
    value: f32,
    min: f32,
    max: f32,
    normalized: bool,
}

fn finite(value: Option<f64>) -> Option<f32> {
    value.map(|v| v as f32).filter(|v| v.is_finite())
}

/// Reads one parameter element. `None` for elements that are not
/// parameters the model can hold (time signatures, unknown types).
fn read_param<'a>(node: Node<'a, '_>, name: String) -> Option<RawParam<'a>> {
    let kind = match tag(node) {
        "RealParameter" | "BoolParameter" | "IntegerParameter" | "EnumParameter" => tag(node),
        _ if node.has_attribute("unit") => "RealParameter",
        _ if boolean(node, "value").is_some() => "BoolParameter",
        _ => return None,
    };
    let (value, min, max) = match kind {
        "BoolParameter" => (
            if boolean(node, "value") == Some(true) {
                1.0
            } else {
                0.0
            },
            0.0,
            1.0,
        ),
        "EnumParameter" => {
            let count = integer(node, "count").unwrap_or(1).max(1);
            let value = integer(node, "value").unwrap_or(0).clamp(0, count - 1);
            (value as f32, 0.0, (count - 1) as f32)
        }
        _ => {
            let value = finite(number(node, "value"));
            let min = finite(number(node, "min"));
            let max = finite(number(node, "max"));
            let value = value.or(min).unwrap_or(0.0);
            let (low, high) = (min.unwrap_or(value.min(0.0)), max.unwrap_or(value.max(1.0)));
            let (low, high) = if low <= high {
                (low, high)
            } else {
                (high, low)
            };
            // The model needs the value inside the range; widen it rather
            // than move the value.
            (value, low.min(value), high.max(value))
        }
    };
    Some(RawParam {
        xml_id: node.attribute("id"),
        wanted: integer(node, "parameterID").map(|id| id as u32),
        name,
        value,
        min,
        max,
        normalized: node.attribute("unit") == Some("normalized"),
    })
}

impl<'a, 'input> Importer<'a, 'input> {
    pub(super) fn devices(
        &mut self,
        track: usize,
        track_name: &str,
        channel: Node<'a, 'input>,
    ) -> Vec<Arc<DeviceInstance>> {
        let Some(list) = child(channel, "Devices") else {
            return Vec::new();
        };
        elements(list)
            .map(|node| Arc::new(self.device(track, track_name, node)))
            .collect()
    }

    fn device(&mut self, track: usize, track_name: &str, node: Node<'a, 'input>) -> DeviceInstance {
        let element = tag(node);
        let device_name = node.attribute("deviceName");
        let name = sanitize_name(
            node.attribute("name").or(device_name).unwrap_or_default(),
            element,
        );
        let at = format!("{track_name} / {name}");
        let device_id = node
            .attribute("deviceID")
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let label = device_id.or(device_name).unwrap_or("unknown");
        let kind = match (plugin_element(element), device_id) {
            (Some(format), Some(id)) => {
                if format == PluginElement::Vst2 {
                    self.report.preserved(
                        Subject::Device,
                        at.clone(),
                        "fourfloor cannot host VST2 plugins; the reference and state are kept but the device does not play",
                    );
                }
                kind_from_plugin(format, id)
            }
            (None, Some(id)) if node.attribute("deviceVendor") == Some(FOURFLOOR_VENDOR) => {
                id.to_owned()
            }
            _ => {
                self.report.preserved(
                    Subject::Device,
                    at.clone(),
                    format!("fourfloor has no device for <{element}> `{label}`; it is kept as a placeholder that playback skips"),
                );
                format!("dawproject.{}:{label}", element.to_ascii_lowercase())
            }
        };
        let id = DeviceId::new(self.ids.claim(naming::number(node.attribute("id"), "d")));
        let params = self.params(track, id, node, &at);
        let enabled = child(node, "Enabled");
        if let Some(param_id) = enabled.and_then(|e| e.attribute("id")) {
            self.targets
                .insert(param_id, Target::Unsupported("device on/off"));
        }
        let bypassed = enabled.and_then(|e| boolean(e, "value")) == Some(false);
        let state_blob = child(node, "State").and_then(|state| self.state(state, &at));
        DeviceInstance {
            id,
            kind: sanitize_name(&kind, "dawproject.device"),
            name,
            bypassed,
            params,
            state_blob,
            sidechain: None,
        }
    }

    fn params(
        &mut self,
        track: usize,
        device: DeviceId,
        node: Node<'a, 'input>,
        at: &str,
    ) -> Vec<Param> {
        let mut raw = Vec::new();
        if let Some(list) = child(node, "Parameters") {
            for (index, param) in elements(list).enumerate() {
                let fallback = format!("Parameter {}", index + 1);
                let name = sanitize_name(param.attribute("name").unwrap_or_default(), &fallback);
                raw.extend(read_param(param, name));
            }
        }
        for typed in elements(node).filter(|n| TYPED_PARAMS.contains(&tag(*n))) {
            raw.extend(read_param(typed, tag(typed).to_owned()));
        }
        let bands = elements(node).filter(|n| tag(*n) == "Band");
        for (band_index, band) in bands.enumerate() {
            for part in elements(band) {
                let name = format!("Band {} {}", band_index + 1, tag(part));
                raw.extend(read_param(part, name));
            }
        }
        if raw.len() > MAX_PARAMS_PER_DEVICE {
            self.report.dropped(
                Subject::Device,
                at.to_owned(),
                format!("only the first {MAX_PARAMS_PER_DEVICE} parameters are kept"),
            );
            raw.truncate(MAX_PARAMS_PER_DEVICE);
        }
        let mut taken: HashSet<u32> = HashSet::new();
        let wanted: Vec<Option<u32>> = raw
            .iter()
            .map(|p| p.wanted.filter(|id| taken.insert(*id)))
            .collect();
        let mut next = 0u32;
        raw.into_iter()
            .zip(wanted)
            .map(|(param, wanted)| {
                let id = wanted.unwrap_or_else(|| {
                    while !taken.insert(next) {
                        next = next.wrapping_add(1);
                    }
                    next
                });
                if let Some(xml_id) = param.xml_id {
                    self.targets.insert(
                        xml_id,
                        Target::Device {
                            track,
                            device,
                            param: ParamId::new(id),
                            xml_min: f64::from(param.min),
                            xml_max: f64::from(param.max),
                            normalized: param.normalized,
                        },
                    );
                }
                Param {
                    id: ParamId::new(id),
                    name: param.name,
                    value: param.value,
                    min: param.min,
                    max: param.max,
                }
            })
            .collect()
    }

    /// Reads a file the document refers to: from the archive, or from disk
    /// next to the `.dawproject` file when it is marked external.
    pub(crate) fn referenced_file(&self, file: Node<'a, 'input>) -> Result<Vec<u8>, String> {
        let raw = file
            .attribute("path")
            .ok_or("the file reference has no path")?;
        if boolean(file, "external") == Some(true) {
            let candidate = Path::new(raw);
            let full = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                self.source_dir
                    .ok_or("the file is external and the project was not read from disk")?
                    .join(candidate)
            };
            return std::fs::read(&full)
                .map_err(|error| format!("cannot read external file {}: {error}", full.display()));
        }
        let name = normalize_path(raw).ok_or_else(|| format!("`{raw}` is not a valid path"))?;
        self.files
            .get(&name)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| format!("`{name}` is not in the archive"))
    }

    fn state(&mut self, state: Node<'a, 'input>, at: &str) -> Option<ContentHash> {
        let stored = self
            .referenced_file(state)
            .and_then(|bytes| self.states.put(&bytes).map_err(|error| error.to_string()));
        match stored {
            Ok(hash) => Some(hash),
            Err(reason) => {
                self.report.missing(
                    Subject::PluginState,
                    at.to_owned(),
                    format!("the device state is unavailable ({reason}); the device starts from its defaults"),
                );
                None
            }
        }
    }
}
