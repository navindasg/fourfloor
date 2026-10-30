//! Devices in a track's chain, their parameters and automation lanes.

use fourfloor_core::{DeviceId, ParamId, Ticks, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;

/// One parameter of a device, in the device's plain units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// Parameter id, unique within its device.
    #[schemars(with = "u32")]
    pub id: ParamId,
    /// Display name.
    pub name: String,
    /// Current value; must lie in `min..=max`.
    pub value: f32,
    /// Lowest allowed value.
    pub min: f32,
    /// Highest allowed value.
    pub max: f32,
}

/// Where a device's sidechain input comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SidechainSource {
    /// The track whose signal keys the device.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// `true` taps that track before its fader, so a muted or turned-down
    /// track can still key the device; `false` taps it after.
    #[serde(default)]
    pub pre_fader: bool,
}

/// An instrument or effect placed in a track's device chain.
///
/// This is the saved description only. The engine builds the running
/// `fourfloor_core::Device` from `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInstance {
    /// Unique id across all tracks.
    pub id: DeviceId,
    /// Which device this is, for example `builtin.sampler` or a plugin id.
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Whether the device is bypassed.
    pub bypassed: bool,
    /// Parameters with their current values.
    pub params: Vec<Param>,
    /// Opaque plugin state stored in the project's `plugin-state` directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_blob: Option<ContentHash>,
    /// The track routed to the device's sidechain input, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<SidechainSource>,
}

impl DeviceInstance {
    /// Looks up a parameter by id.
    pub fn param(&self, id: ParamId) -> Option<&Param> {
        self.params.iter().find(|param| param.id == id)
    }
}

/// One breakpoint in an automation lane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationPoint {
    /// Timeline position in ticks from the project start.
    #[schemars(with = "i64")]
    pub time: Ticks,
    /// Parameter value at that time, in the parameter's plain units.
    pub value: f32,
}

/// Automation for one parameter of one device. Lives on the device's track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationLane {
    /// The automated device.
    pub device: DeviceId,
    /// The automated parameter.
    pub param: ParamId,
    /// Breakpoints sorted by time. Two points may share a time to make a step.
    pub points: Vec<AutomationPoint>,
}
