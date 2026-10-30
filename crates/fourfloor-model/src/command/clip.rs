//! Clip commands.

use std::sync::Arc;

use fourfloor_core::{ClipId, Ticks, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome};
use crate::clip::{AudioRef, Clip, ClipContent, MidiNote};
use crate::error::ModelError;
use crate::project::Project;
use crate::track::{Track, TrackKind};
use crate::validate;

pub(super) fn require_kind(track: &Track, expected: TrackKind) -> Result<(), ModelError> {
    if track.kind == expected {
        Ok(())
    } else {
        Err(ModelError::WrongTrackKind {
            track: track.id,
            expected,
            actual: track.kind,
        })
    }
}

pub(super) fn without_clip(track: &Track, clip: ClipId) -> Track {
    Track {
        clips: track
            .clips
            .iter()
            .filter(|existing| existing.id != clip)
            .cloned()
            .collect(),
        ..track.clone()
    }
}

pub(super) fn with_clip_appended(track: &Track, clip: Arc<Clip>) -> Track {
    Track {
        clips: track
            .clips
            .iter()
            .cloned()
            .chain(std::iter::once(clip))
            .collect(),
        ..track.clone()
    }
}

pub(super) fn with_clip_replaced(track: &Track, clip: Arc<Clip>) -> Track {
    Track {
        clips: track
            .clips
            .iter()
            .map(|existing| {
                Arc::clone(if existing.id == clip.id {
                    &clip
                } else {
                    existing
                })
            })
            .collect(),
        ..track.clone()
    }
}

/// Write a MIDI clip: create a new one, or replace an existing clip's name,
/// position, length and notes. Replacing a clip also clears its offset and
/// its loop.
///
/// Notes are stored sorted by start then pitch. Clips on one track may overlap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteMidiClip {
    /// The MIDI track that holds the clip.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// An existing clip on that track to replace. Omit to create a new clip.
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    pub clip: Option<ClipId>,
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// Timeline position in ticks from the project start (960 per beat).
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// Clip length in ticks; must be positive.
    #[schemars(with = "i64")]
    pub length: Ticks,
    /// The notes, with starts relative to the clip start and inside the clip.
    pub notes: Vec<MidiNote>,
}

impl Apply for WriteMidiClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let track = project
            .track(self.track)
            .ok_or(ModelError::TrackNotFound(self.track))?;
        require_kind(track, TrackKind::Midi)?;
        validate::name("clip name", &self.name)?;
        validate::clip_span(self.start, self.length)?;
        validate::notes(&self.notes, self.length)?;
        let content = ClipContent::Midi {
            notes: validate::sorted_notes(&self.notes),
        };
        match self.clip {
            Some(id) => self.replace(project, id, content),
            None => self.create(project, content),
        }
    }
}

impl WriteMidiClip {
    fn clip(&self, id: ClipId, content: ClipContent) -> Arc<Clip> {
        Arc::new(Clip {
            id,
            name: self.name.clone(),
            start: self.start,
            length: self.length,
            offset: Ticks::ZERO,
            loop_length: None,
            content,
        })
    }

    fn create(&self, project: &Project, content: ClipContent) -> Result<Applied, ModelError> {
        let (raw_id, next_id) = project.reserve_id()?;
        let id = ClipId::new(raw_id);
        let clip = self.clip(id, content);
        let with_clip =
            project.map_track(self.track, |track| Ok(with_clip_appended(track, clip)))?;
        let updated = Project {
            next_id,
            ..with_clip
        };
        Ok((
            updated,
            Outcome::ClipWritten {
                clip: id,
                created: true,
            },
        ))
    }

    fn replace(
        &self,
        project: &Project,
        id: ClipId,
        content: ClipContent,
    ) -> Result<Applied, ModelError> {
        let (owner, _) = project.find_clip(id).ok_or(ModelError::ClipNotFound(id))?;
        if owner.id != self.track {
            return Err(ModelError::ClipNotOnTrack {
                clip: id,
                track: self.track,
            });
        }
        let clip = self.clip(id, content);
        let updated = project.map_track(self.track, |track| Ok(with_clip_replaced(track, clip)))?;
        Ok((
            updated,
            Outcome::ClipWritten {
                clip: id,
                created: false,
            },
        ))
    }
}

/// Place a region of an audio pool file on an audio track.
///
/// The command does not check that the file is present in the pool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddAudioClip {
    /// The audio track that gets the clip.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// Timeline position in ticks from the project start (960 per beat).
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// Clip length in ticks; must be positive.
    #[schemars(with = "i64")]
    pub length: Ticks,
    /// The pool file to play.
    pub source: AudioRef,
    /// Frames to skip at the start of the file. Defaults to 0.
    #[serde(default)]
    pub source_offset_frames: u64,
}

impl Apply for AddAudioClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let track = project
            .track(self.track)
            .ok_or(ModelError::TrackNotFound(self.track))?;
        require_kind(track, TrackKind::Audio)?;
        validate::name("clip name", &self.name)?;
        validate::clip_span(self.start, self.length)?;
        validate::audio_ref(&self.source)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = ClipId::new(raw_id);
        let clip = Arc::new(Clip {
            id,
            name: self.name.clone(),
            start: self.start,
            length: self.length,
            offset: Ticks::ZERO,
            loop_length: None,
            content: ClipContent::audio(self.source.clone(), self.source_offset_frames),
        });
        let with_clip =
            project.map_track(self.track, |track| Ok(with_clip_appended(track, clip)))?;
        let updated = Project {
            next_id,
            ..with_clip
        };
        Ok((
            updated,
            Outcome::ClipWritten {
                clip: id,
                created: true,
            },
        ))
    }
}

/// Move a clip along the timeline, optionally onto another track of the
/// same kind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveClip {
    /// The clip to move.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// New timeline position in ticks from the project start.
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// Track to move the clip to. Omit to keep it on its current track.
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    pub track: Option<TrackId>,
}

impl Apply for MoveClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (source, clip) = project
            .find_clip(self.clip)
            .ok_or(ModelError::ClipNotFound(self.clip))?;
        validate::clip_span(self.start, clip.length)?;
        let target_id = self.track.unwrap_or(source.id);
        let target = project
            .track(target_id)
            .ok_or(ModelError::TrackNotFound(target_id))?;
        require_kind(target, source.kind)?;
        let moved = Arc::new(Clip {
            start: self.start,
            ..Clip::clone(clip)
        });
        if target_id == source.id {
            let updated =
                project.map_track(target_id, |track| Ok(with_clip_replaced(track, moved)))?;
            return Ok((updated, Outcome::Done));
        }
        let source_id = source.id;
        let tracks = project
            .tracks
            .iter()
            .map(|track| {
                if track.id == source_id {
                    Arc::new(without_clip(track, self.clip))
                } else if track.id == target_id {
                    Arc::new(with_clip_appended(track, Arc::clone(&moved)))
                } else {
                    Arc::clone(track)
                }
            })
            .collect();
        Ok((project.with_tracks(tracks), Outcome::Done))
    }
}

/// Delete a clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteClip {
    /// The clip to delete.
    #[schemars(with = "u64")]
    pub clip: ClipId,
}

impl Apply for DeleteClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (owner, _) = project
            .find_clip(self.clip)
            .ok_or(ModelError::ClipNotFound(self.clip))?;
        let updated = project.map_track(owner.id, |track| Ok(without_clip(track, self.clip)))?;
        Ok((updated, Outcome::Done))
    }
}
