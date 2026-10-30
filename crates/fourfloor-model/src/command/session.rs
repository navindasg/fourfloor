//! Clip-launcher commands: scenes, slots and launch quantization.
//!
//! These edit what the launcher holds. Launching itself is a performance
//! action, not an edit: it goes straight to the engine and is not undoable.

use std::sync::Arc;

use fourfloor_core::{ClipId, Ticks, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::clip::require_kind;
use super::{Applied, Apply, Outcome};
use crate::checks;
use crate::cleanup;
use crate::clip::{Clip, ClipContent, MidiNote};
use crate::error::ModelError;
use crate::ids::SceneId;
use crate::modulation::Scene;
use crate::project::Project;
use crate::track::{SessionSlot, Track, TrackKind};
use crate::validate;

fn require_scene(project: &Project, scene: SceneId) -> Result<(), ModelError> {
    project
        .scene(scene)
        .map(|_| ())
        .ok_or(ModelError::SceneNotFound(scene))
}

/// A copy of a track whose slot for `scene` holds `clip` (or nothing).
fn with_slot(track: &Track, scene: SceneId, clip: Option<Arc<Clip>>) -> Track {
    Track {
        slots: track
            .slots
            .iter()
            .filter(|slot| slot.scene != scene)
            .cloned()
            .chain(clip.map(|clip| SessionSlot { scene, clip }))
            .collect(),
        ..track.clone()
    }
}

/// Add a scene: a row of the clip launcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddScene {
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// Position in the scene list. Omit to add the scene at the end.
    #[serde(default)]
    pub index: Option<usize>,
}

impl Apply for AddScene {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("scene name", &self.name)?;
        let len = project.scenes.len();
        let index = self.index.unwrap_or(len);
        if index > len {
            return Err(ModelError::IndexOutOfRange { index, len });
        }
        let (raw_id, next_id) = project.reserve_id()?;
        let id = SceneId::new(raw_id);
        let created = Scene {
            id,
            name: self.name.clone(),
        };
        let scenes = project.scenes[..index]
            .iter()
            .cloned()
            .chain(std::iter::once(created))
            .chain(project.scenes[index..].iter().cloned())
            .collect();
        Ok((
            Project {
                scenes,
                next_id,
                ..project.clone()
            },
            Outcome::SceneAdded { scene: id },
        ))
    }
}

/// Remove a scene and every clip in its row.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveScene {
    /// The scene to remove.
    pub scene: SceneId,
}

impl Apply for RemoveScene {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        require_scene(project, self.scene)?;
        let without = Project {
            scenes: project
                .scenes
                .iter()
                .filter(|scene| scene.id != self.scene)
                .cloned()
                .collect(),
            ..project.clone()
        };
        Ok((cleanup::scrub(&without), Outcome::Done))
    }
}

/// Rename a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameScene {
    /// The scene to rename.
    pub scene: SceneId,
    /// The new name: 1 to 128 characters.
    pub name: String,
}

impl Apply for RenameScene {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("scene name", &self.name)?;
        require_scene(project, self.scene)?;
        let scenes = project
            .scenes
            .iter()
            .map(|scene| Scene {
                id: scene.id,
                name: if scene.id == self.scene {
                    self.name.clone()
                } else {
                    scene.name.clone()
                },
            })
            .collect();
        Ok((
            Project {
                scenes,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}

/// Write a MIDI clip into a launcher slot, replacing whatever was there.
///
/// A launched clip repeats from its start every `length` ticks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteMidiSlot {
    /// The MIDI track (the column).
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// The scene (the row).
    pub scene: SceneId,
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// Clip length in ticks; must be positive.
    #[schemars(with = "i64")]
    pub length: Ticks,
    /// The notes, with starts relative to the clip start and inside the clip.
    pub notes: Vec<MidiNote>,
}

impl Apply for WriteMidiSlot {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let track = project
            .track(self.track)
            .ok_or(ModelError::TrackNotFound(self.track))?;
        require_kind(track, TrackKind::Midi)?;
        require_scene(project, self.scene)?;
        validate::name("clip name", &self.name)?;
        validate::clip_span(Ticks::ZERO, self.length)?;
        validate::notes(&self.notes, self.length)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = ClipId::new(raw_id);
        let clip = Arc::new(Clip {
            id,
            name: self.name.clone(),
            start: Ticks::ZERO,
            length: self.length,
            offset: Ticks::ZERO,
            loop_length: None,
            content: ClipContent::Midi {
                notes: validate::sorted_notes(&self.notes),
            },
        });
        let with_clip = project.map_track(self.track, |track| {
            Ok(with_slot(track, self.scene, Some(clip)))
        })?;
        Ok((
            Project {
                next_id,
                ..with_clip
            },
            Outcome::ClipWritten {
                clip: id,
                created: true,
            },
        ))
    }
}

/// Copy a timeline clip (MIDI or audio) into a launcher slot on the same
/// track, replacing whatever was there. The timeline clip stays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipToSlot {
    /// The timeline clip to copy.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// The scene (the row) to put the copy in.
    pub scene: SceneId,
}

impl Apply for ClipToSlot {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (owner, clip) = project
            .find_clip(self.clip)
            .ok_or(ModelError::ClipNotFound(self.clip))?;
        require_scene(project, self.scene)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = ClipId::new(raw_id);
        let copy = Arc::new(Clip {
            id,
            start: Ticks::ZERO,
            ..Clip::clone(clip)
        });
        let with_clip = project.map_track(owner.id, |track| {
            Ok(with_slot(track, self.scene, Some(copy)))
        })?;
        Ok((
            Project {
                next_id,
                ..with_clip
            },
            Outcome::ClipWritten {
                clip: id,
                created: true,
            },
        ))
    }
}

/// Empty a launcher slot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearSlot {
    /// The track (the column).
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// The scene (the row).
    pub scene: SceneId,
}

impl Apply for ClearSlot {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = project.map_track(self.track, |track| {
            if track.slot(self.scene).is_none() {
                return Err(ModelError::NotFound {
                    what: format!("a clip in scene {} on track {}", self.scene, self.track),
                });
            }
            Ok(with_slot(track, self.scene, None))
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Set the grid launched clips wait for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLaunchQuantize {
    /// Grid size in ticks: a launched clip starts on the next multiple.
    /// 3840 is one 4/4 bar, 960 one beat, 0 launches at once.
    #[schemars(with = "i64")]
    pub ticks: Ticks,
}

impl Apply for SetLaunchQuantize {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        checks::launch_quantize(self.ticks)?;
        Ok((
            Project {
                launch_quantize: self.ticks,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}
