//! Device chain, parameter and automation commands.

use std::sync::Arc;

use fourfloor_core::{DeviceId, ParamId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome};
use crate::cleanup;
use crate::device::{AutomationLane, AutomationPoint, DeviceInstance, Param};
use crate::error::ModelError;
use crate::hash::ContentHash;
use crate::project::Project;
use crate::track::Track;
use crate::validate;

fn map_device(
    project: &Project,
    id: DeviceId,
    update: impl Fn(&DeviceInstance) -> Result<DeviceInstance, ModelError>,
) -> Result<Project, ModelError> {
    project.map_device(id, update)
}

/// Add an instrument or effect to a track's device chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddDevice {
    /// The track that gets the device.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// Which device to add, for example `builtin.sampler` or a plugin id.
    pub kind: String,
    /// Display name. Omit to use `kind`.
    #[serde(default)]
    pub name: Option<String>,
    /// Position in the chain. Omit to add the device at the end.
    #[serde(default)]
    pub index: Option<usize>,
    /// The device's parameters with their starting values.
    #[serde(default)]
    pub params: Vec<Param>,
}

impl Apply for AddDevice {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("device kind", &self.kind)?;
        let name = self.name.as_deref().unwrap_or(&self.kind);
        validate::name("device name", name)?;
        validate::params(&self.params)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = DeviceId::new(raw_id);
        let created = Arc::new(DeviceInstance {
            id,
            kind: self.kind.clone(),
            name: name.to_owned(),
            bypassed: false,
            params: self.params.clone(),
            state_blob: None,
            sidechain: None,
        });
        let with_device = project.map_track(self.track, |track| {
            let len = track.devices.len();
            let index = self.index.unwrap_or(len);
            if index > len {
                return Err(ModelError::IndexOutOfRange { index, len });
            }
            let devices = track.devices[..index]
                .iter()
                .cloned()
                .chain(std::iter::once(Arc::clone(&created)))
                .chain(track.devices[index..].iter().cloned())
                .collect();
            Ok(Track {
                devices,
                ..track.clone()
            })
        })?;
        let updated = Project {
            next_id,
            ..with_device
        };
        Ok((updated, Outcome::DeviceAdded { device: id }))
    }
}

/// Remove a device from its chain, along with its automation lanes and
/// every modulation, macro and controller target aimed at it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveDevice {
    /// The device to remove.
    #[schemars(with = "u64")]
    pub device: DeviceId,
}

impl Apply for RemoveDevice {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (owner, _) = project
            .find_device(self.device)
            .ok_or(ModelError::DeviceNotFound(self.device))?;
        let updated = project.map_track(owner.id, |track| {
            let devices = track
                .devices
                .iter()
                .filter(|device| device.id != self.device)
                .cloned()
                .collect();
            let automation = track
                .automation
                .iter()
                .filter(|lane| lane.device != self.device)
                .cloned()
                .collect();
            Ok(Track {
                devices,
                automation,
                ..track.clone()
            })
        })?;
        Ok((cleanup::scrub(&updated), Outcome::Done))
    }
}

/// Set one parameter of a device.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetParam {
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to set.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// The new value in the parameter's plain units, within its range.
    pub value: f32,
}

impl Apply for SetParam {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = map_device(project, self.device, |device| {
            let current = device.param(self.param).ok_or(ModelError::ParamNotFound {
                device: self.device,
                param: self.param,
            })?;
            validate::param_value(self.device, current, self.value)?;
            let params = device
                .params
                .iter()
                .map(|param| {
                    if param.id == self.param {
                        Param {
                            value: self.value,
                            ..param.clone()
                        }
                    } else {
                        param.clone()
                    }
                })
                .collect();
            Ok(DeviceInstance {
                params,
                ..device.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Bypass or re-enable a device.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDeviceBypass {
    /// The device to change.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// `true` to bypass.
    pub bypassed: bool,
}

impl Apply for SetDeviceBypass {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = map_device(project, self.device, |device| {
            Ok(DeviceInstance {
                bypassed: self.bypassed,
                ..device.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Point a device at a saved plugin state blob, or clear it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDeviceState {
    /// The device to change.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// Hash of a blob in the project's plugin-state store. Omit or pass null
    /// to clear it.
    #[serde(default)]
    pub state_blob: Option<ContentHash>,
}

impl Apply for SetDeviceState {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = map_device(project, self.device, |device| {
            Ok(DeviceInstance {
                state_blob: self.state_blob.clone(),
                ..device.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Replace the automation lane of one device parameter.
///
/// Points are sorted by time. An empty list removes the lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteAutomation {
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to automate.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// The lane's breakpoints; values must lie within the parameter's range.
    pub points: Vec<AutomationPoint>,
}

impl Apply for WriteAutomation {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (owner, device) = project
            .find_device(self.device)
            .ok_or(ModelError::DeviceNotFound(self.device))?;
        let param = device.param(self.param).ok_or(ModelError::ParamNotFound {
            device: self.device,
            param: self.param,
        })?;
        validate::automation_points(self.device, param, &self.points)?;
        let written = self.sorted_lane().map(Arc::new);
        let updated = project.map_track(owner.id, |track| {
            let automation = track
                .automation
                .iter()
                .filter(|lane| !(lane.device == self.device && lane.param == self.param))
                .cloned()
                .chain(written.clone())
                .collect();
            Ok(Track {
                automation,
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

impl WriteAutomation {
    /// The lane to store, or `None` when the command clears it.
    fn sorted_lane(&self) -> Option<AutomationLane> {
        if self.points.is_empty() {
            return None;
        }
        let mut points = self.points.clone();
        points.sort_by_key(|point| point.time);
        Some(AutomationLane {
            device: self.device,
            param: self.param,
            points,
        })
    }
}
