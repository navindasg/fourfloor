//! Arrangement commands: clip gain, fades and warp, and split, duplicate,
//! loop and resize.

use std::sync::Arc;

use fourfloor_core::{ClipId, Ticks, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::clip::{require_kind, with_clip_appended, with_clip_replaced};
use super::{Applied, Apply, Outcome};
use crate::checks;
use crate::clip::{Clip, ClipContent, Warp};
use crate::error::ModelError;
use crate::project::Project;
use crate::validate;

fn not_audio(clip: ClipId) -> ModelError {
    ModelError::InvalidValue {
        field: "clip",
        reason: format!("clip {clip} is not an audio clip"),
    }
}

/// The fades of a clip of `length` ticks, shortened so they still fit.
fn fitted_fades(fade_in: Ticks, fade_out: Ticks, length: Ticks) -> (Ticks, Ticks) {
    let fade_in = fade_in.min(length);
    (fade_in, fade_out.min(length - fade_in))
}

/// A copy of an audio clip's content with some settings replaced.
fn with_audio(
    clip: &Clip,
    change: impl Fn(&mut f32, &mut Ticks, &mut Ticks, &mut Option<Warp>),
) -> Result<ClipContent, ModelError> {
    match &clip.content {
        ClipContent::Audio {
            source,
            source_offset_frames,
            gain_db,
            fade_in,
            fade_out,
            warp,
        } => {
            let (mut gain_db, mut fade_in, mut fade_out, mut warp) =
                (*gain_db, *fade_in, *fade_out, *warp);
            change(&mut gain_db, &mut fade_in, &mut fade_out, &mut warp);
            Ok(ClipContent::Audio {
                source: source.clone(),
                source_offset_frames: *source_offset_frames,
                gain_db,
                fade_in,
                fade_out,
                warp,
            })
        }
        ClipContent::Midi { .. } => Err(not_audio(clip.id)),
    }
}

/// A copy of a clip with a new length; an audio clip's fades are shortened
/// to fit.
fn with_length(clip: &Clip, length: Ticks, keep_in: bool, keep_out: bool) -> Clip {
    let content = with_audio(clip, |_, fade_in, fade_out, _| {
        let wanted_in = if keep_in { *fade_in } else { Ticks::ZERO };
        let wanted_out = if keep_out { *fade_out } else { Ticks::ZERO };
        (*fade_in, *fade_out) = fitted_fades(wanted_in, wanted_out, length);
    })
    .unwrap_or_else(|_| clip.content.clone());
    Clip {
        length,
        content,
        ..clip.clone()
    }
}

/// Set an audio clip's gain.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetClipGain {
    /// The audio clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Gain in decibels, -96 to 24.
    pub gain_db: f32,
}

impl Apply for SetClipGain {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::gain_db(self.gain_db)?;
        let updated = project.map_any_clip(self.clip, |clip| {
            Ok(Clip {
                content: with_audio(clip, |gain_db, _, _, _| *gain_db = self.gain_db)?,
                ..clip.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Set an audio clip's fade-in and fade-out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetClipFades {
    /// The audio clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Length of the linear fade-in, in ticks. 0 for none.
    #[schemars(with = "i64")]
    pub fade_in: Ticks,
    /// Length of the linear fade-out, in ticks. 0 for none. Together the
    /// fades may not be longer than the clip.
    #[schemars(with = "i64")]
    pub fade_out: Ticks,
}

impl Apply for SetClipFades {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = project.map_any_clip(self.clip, |clip| {
            let content = with_audio(clip, |_, fade_in, fade_out, _| {
                (*fade_in, *fade_out) = (self.fade_in, self.fade_out);
            })?;
            checks::fades(self.fade_in, self.fade_out, clip.length)?;
            Ok(Clip {
                content,
                ..clip.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Make an audio clip follow the project tempo, or stop it doing so.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetClipWarp {
    /// The audio clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// The tempo of the recording and whether to keep its pitch. Omit or
    /// pass null to play the file at its own speed.
    #[serde(default)]
    pub warp: Option<Warp>,
}

impl Apply for SetClipWarp {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if let Some(warp) = &self.warp {
            checks::warp(warp)?;
        }
        let updated = project.map_any_clip(self.clip, |clip| {
            Ok(Clip {
                content: with_audio(clip, |_, _, _, warp| *warp = self.warp)?,
                ..clip.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Cut a timeline clip in two.
///
/// The left part keeps the clip's id; the right part is a new clip that
/// carries on where the left one stops. Both keep the whole content, so
/// either can be lengthened again later.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitClip {
    /// The clip to cut.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Where to cut, in ticks from the project start. Must lie strictly
    /// inside the clip.
    #[schemars(with = "i64")]
    pub at: Ticks,
}

impl Apply for SplitClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (owner, clip) = project
            .find_clip(self.clip)
            .ok_or(ModelError::ClipNotFound(self.clip))?;
        if self.at <= clip.start || self.at >= clip.end() {
            return Err(ModelError::InvalidValue {
                field: "at",
                reason: format!(
                    "{} is not strictly inside the clip ({} to {})",
                    self.at.get(),
                    clip.start.get(),
                    clip.end().get()
                ),
            });
        }
        let left_length = self.at - clip.start;
        let (raw_id, next_id) = project.reserve_id()?;
        let right_id = ClipId::new(raw_id);
        let offset = clip.offset + left_length;
        let left = Arc::new(with_length(clip, left_length, true, false));
        let right = Arc::new(Clip {
            id: right_id,
            start: self.at,
            offset: match clip.loop_length {
                Some(period) => Ticks::new(offset.get().rem_euclid(period.get().max(1))),
                None => offset,
            },
            ..with_length(clip, clip.length - left_length, false, true)
        });
        let with_parts = project.map_track(owner.id, |track| {
            Ok(with_clip_appended(
                &with_clip_replaced(track, Arc::clone(&left)),
                Arc::clone(&right),
            ))
        })?;
        Ok((
            Project {
                next_id,
                ..with_parts
            },
            Outcome::ClipWritten {
                clip: right_id,
                created: true,
            },
        ))
    }
}

/// Copy a timeline clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateClip {
    /// The clip to copy.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Where the copy starts, in ticks. Omit to put it right after the
    /// original.
    #[serde(default)]
    #[schemars(with = "Option<i64>")]
    pub start: Option<Ticks>,
    /// Track for the copy, of the same kind. Omit to keep it on the
    /// original's track.
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    pub track: Option<TrackId>,
}

impl Apply for DuplicateClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let (source, clip) = project
            .find_clip(self.clip)
            .ok_or(ModelError::ClipNotFound(self.clip))?;
        let start = self.start.unwrap_or_else(|| clip.end());
        validate::clip_span(start, clip.length)?;
        let target_id = self.track.unwrap_or(source.id);
        let target = project
            .track(target_id)
            .ok_or(ModelError::TrackNotFound(target_id))?;
        require_kind(target, source.kind)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = ClipId::new(raw_id);
        let copy = Arc::new(Clip {
            id,
            start,
            ..Clip::clone(clip)
        });
        let with_copy =
            project.map_track(target_id, |track| Ok(with_clip_appended(track, copy)))?;
        Ok((
            Project {
                next_id,
                ..with_copy
            },
            Outcome::ClipWritten {
                clip: id,
                created: true,
            },
        ))
    }
}

/// Make a clip repeat the first part of its content, or stop it looping.
///
/// With a loop of one bar, a four-bar clip plays its first bar four times;
/// use `resize_clip` to make the clip longer and get more repeats.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetClipLoop {
    /// The clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Length of the repeating region in ticks, measured from the start of
    /// the content. Omit or pass null to stop looping.
    #[serde(default)]
    #[schemars(with = "Option<i64>")]
    pub loop_length: Option<Ticks>,
}

impl Apply for SetClipLoop {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if let Some(period) = self.loop_length {
            checks::loop_length(period)?;
        }
        let updated = project.map_any_clip(self.clip, |clip| {
            Ok(Clip {
                loop_length: self.loop_length,
                // Keep the offset inside the loop it now belongs to.
                offset: match self.loop_length {
                    Some(period) => Ticks::new(clip.offset.get().rem_euclid(period.get())),
                    None => clip.offset,
                },
                ..clip.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Change a clip's length, keeping its start. An audio clip's fades are
/// shortened if they no longer fit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeClip {
    /// The clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// The new length in ticks; must be positive.
    #[schemars(with = "i64")]
    pub length: Ticks,
}

impl Apply for ResizeClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = project.map_any_clip(self.clip, |clip| {
            validate::clip_span(clip.start, self.length)?;
            Ok(with_length(clip, self.length, true, true))
        })?;
        Ok((updated, Outcome::Done))
    }
}
