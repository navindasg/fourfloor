//! Track commands.

use std::sync::Arc;

use fourfloor_core::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome};
use crate::checks;
use crate::cleanup;
use crate::error::ModelError;
use crate::project::Project;
use crate::track::{Track, TrackKind};
use crate::validate;

/// Create an empty audio, MIDI or bus track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTrack {
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// What the track carries.
    pub kind: TrackKind,
    /// Position in the track list. Omit to add the track at the end.
    #[serde(default)]
    pub index: Option<usize>,
}

impl Apply for CreateTrack {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("track name", &self.name)?;
        let len = project.tracks.len();
        let index = self.index.unwrap_or(len);
        if index > len {
            return Err(ModelError::IndexOutOfRange { index, len });
        }
        let (raw_id, next_id) = project.reserve_id()?;
        let id = TrackId::new(raw_id);
        let created = Arc::new(Track::new(id, self.name.clone(), self.kind));
        let tracks = project.tracks[..index]
            .iter()
            .cloned()
            .chain(std::iter::once(created))
            .chain(project.tracks[index..].iter().cloned())
            .collect();
        let updated = Project {
            next_id,
            ..project.with_tracks(tracks)
        };
        Ok((updated, Outcome::TrackCreated { track: id }))
    }
}

/// Delete a track with its clips, devices and automation.
///
/// Tracks that were routed to the deleted track fall back to the master.
/// Sends to it, sidechains keyed by it, envelopes it triggers and every
/// modulation, macro and controller target on its devices are removed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteTrack {
    /// The track to delete.
    #[schemars(with = "u64")]
    pub track: TrackId,
}

impl Apply for DeleteTrack {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if project.track(self.track).is_none() {
            return Err(ModelError::TrackNotFound(self.track));
        }
        let tracks = project
            .tracks
            .iter()
            .filter(|track| track.id != self.track)
            .cloned()
            .collect();
        // Outputs, sends, sidechains and envelope triggers that named the
        // deleted track, and anything aimed at its devices, go with it.
        let updated = cleanup::scrub(&project.with_tracks(tracks));
        Ok((updated, Outcome::Done))
    }
}

/// Rename a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameTrack {
    /// The track to rename.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// The new name: 1 to 128 characters.
    pub name: String,
}

impl Apply for RenameTrack {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("track name", &self.name)?;
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                name: self.name.clone(),
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Set a track's fader gain.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTrackGain {
    /// The track to change.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// Gain in decibels, -96 to 24.
    pub gain_db: f32,
}

impl Apply for SetTrackGain {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::gain_db(self.gain_db)?;
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                gain_db: self.gain_db,
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Set a track's pan position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTrackPan {
    /// The track to change.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// Pan from -1 (left) to 1 (right).
    pub pan: f32,
}

impl Apply for SetTrackPan {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::pan(self.pan)?;
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                pan: self.pan,
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Mute or unmute a track.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTrackMute {
    /// The track to change.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// `true` to mute.
    pub mute: bool,
}

impl Apply for SetTrackMute {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                mute: self.mute,
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Solo or unsolo a track.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTrackSolo {
    /// The track to change.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// `true` to solo.
    pub solo: bool,
}

impl Apply for SetTrackSolo {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                solo: self.solo,
                ..track.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Route a track's output to a bus, or back to the master.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTrackOutput {
    /// The track to route.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// The bus track to feed. Omit or pass null for the master output.
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    pub output: Option<TrackId>,
}

impl Apply for SetTrackOutput {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if project.track(self.track).is_none() {
            return Err(ModelError::TrackNotFound(self.track));
        }
        validate::routing(project, self.track, self.output)?;
        let updated = project.map_track(self.track, |track| {
            Ok(Track {
                output: self.output,
                ..track.clone()
            })
        })?;
        // Sends and sidechains can close a loop the output chain alone
        // does not show.
        checks::routing_graph(&updated)?;
        Ok((updated, Outcome::Done))
    }
}
