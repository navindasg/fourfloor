//! Mixer routing commands: sends, sidechains and fader automation.

use std::sync::Arc;

use fourfloor_core::{DeviceId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome};
use crate::checks;
use crate::device::{AutomationPoint, DeviceInstance, SidechainSource};
use crate::error::ModelError;
use crate::project::Project;
use crate::track::{MixerLane, MixerParam, Send, Track};

/// Send a copy of a track's signal to a bus, or change an existing send.
///
/// A bus fed by sends is a return: put a reverb or delay on it and send
/// several tracks to it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSend {
    /// The track to send from.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// The bus track that receives the send.
    #[schemars(with = "u64")]
    pub target: TrackId,
    /// Send level in decibels, -96 to 24.
    pub gain_db: f32,
    /// `true` to tap the signal before the track's fader. Defaults to false.
    #[serde(default)]
    pub pre_fader: bool,
}

impl Apply for SetSend {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if project.track(self.track).is_none() {
            return Err(ModelError::TrackNotFound(self.track));
        }
        let send = Send {
            target: self.target,
            gain_db: self.gain_db,
            pre_fader: self.pre_fader,
        };
        checks::send(project, self.track, &send)?;
        let updated = project.map_track(self.track, |track| {
            let replaced = track.send(self.target).is_some();
            let sends = track
                .sends
                .iter()
                .map(|existing| {
                    if existing.target == self.target {
                        send
                    } else {
                        *existing
                    }
                })
                .chain((!replaced).then_some(send))
                .collect();
            Ok(Track {
                sends,
                ..track.clone()
            })
        })?;
        checks::routing_graph(&updated)?;
        Ok((updated, Outcome::Done))
    }
}

/// Remove a send.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveSend {
    /// The track that sends.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// The bus the send goes to.
    #[schemars(with = "u64")]
    pub target: TrackId,
}

impl Apply for RemoveSend {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = project.map_track(self.track, |track| {
            if track.send(self.target).is_none() {
                return Err(ModelError::NotFound {
                    what: format!("send from track {} to track {}", self.track, self.target),
                });
            }
            Ok(Track {
                sends: track
                    .sends
                    .iter()
                    .filter(|send| send.target != self.target)
                    .copied()
                    .collect(),
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Route a track into a device's sidechain input, or disconnect it.
///
/// This is how a compressor on a pad ducks to the kick: the kick track is
/// the source and the compressor is the device.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDeviceSidechain {
    /// The device whose sidechain input is routed.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The track that keys the device. Omit or pass null to disconnect.
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    pub source: Option<TrackId>,
    /// `true` to tap the source before its fader, so it keys the device
    /// even when muted or turned down. Defaults to false.
    #[serde(default)]
    pub pre_fader: bool,
}

impl Apply for SetDeviceSidechain {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (owner, _) = project
            .find_device(self.device)
            .ok_or(ModelError::DeviceNotFound(self.device))?;
        let sidechain = self.source.map(|track| SidechainSource {
            track,
            pre_fader: self.pre_fader,
        });
        if let Some(source) = &sidechain {
            checks::sidechain(project, owner.id, source)?;
        }
        let updated = project.map_device(self.device, |device| {
            Ok(DeviceInstance {
                sidechain,
                ..device.clone()
            })
        })?;
        checks::routing_graph(&updated)?;
        Ok((updated, Outcome::Done))
    }
}

/// Replace the automation of a track's fader gain or pan.
///
/// Points are sorted by time; values between points are interpolated in a
/// straight line. An empty list removes the lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteTrackAutomation {
    /// The track whose mixer control is automated.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// Which control: `gain` (decibels, -96 to 24) or `pan` (-1 to 1).
    pub param: MixerParam,
    /// The lane's breakpoints.
    pub points: Vec<AutomationPoint>,
}

impl Apply for WriteTrackAutomation {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        checks::mixer_points(self.param, &self.points)?;
        let mut points = self.points.clone();
        points.sort_by_key(|point| point.time);
        let written = (!points.is_empty()).then(|| {
            Arc::new(MixerLane {
                param: self.param,
                points,
            })
        });
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                mixer_automation: track
                    .mixer_automation
                    .iter()
                    .filter(|lane| lane.param != self.param)
                    .cloned()
                    .chain(written.clone())
                    .collect(),
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}
