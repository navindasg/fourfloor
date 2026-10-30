//! Step sequencer and piano-roll commands: write a step pattern, quantize,
//! transpose, snap to a scale, stamp a chord and apply a groove.

use fourfloor_core::{ClipId, Ticks, TrackId, TICKS_PER_BEAT};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome, WriteMidiClip};
use crate::clip::{Clip, ClipContent, MidiNote};
use crate::error::ModelError;
use crate::groove::GrooveTemplate;
use crate::music::{chord_pitches, quantize_tick, snap_pitch, ChordKind, Scale};
use crate::project::{Groove, Project};
use crate::sequencer::{StepLane, BAR_TICKS, MAX_PATTERN_BARS, STEPS_PER_BAR};
use crate::validate::{self, MAX_TICKS};

/// Most lanes one step pattern may hold.
pub const MAX_STEP_LANES: usize = 128;
/// Most steps one lane's pattern may hold before it repeats.
pub const MAX_LANE_STEPS: usize = 1_024;

fn invalid(field: &'static str, reason: impl Into<String>) -> ModelError {
    ModelError::InvalidValue {
        field,
        reason: reason.into(),
    }
}

/// A copy of the project in which a MIDI clip's notes are replaced by
/// `edit(notes, clip)`. The result is validated and stored sorted.
fn map_notes(
    project: &Project,
    id: ClipId,
    edit: impl Fn(&[MidiNote], &Clip) -> Result<Vec<MidiNote>, ModelError>,
) -> Result<Project, ModelError> {
    project.map_any_clip(id, |clip| {
        let ClipContent::Midi { notes } = &clip.content else {
            return Err(invalid("clip", format!("clip {id} is not a MIDI clip")));
        };
        let edited = edit(notes, clip)?;
        validate::notes(&edited, Ticks::new(MAX_TICKS + 1))?;
        Ok(Clip {
            content: ClipContent::Midi {
                notes: validate::sorted_notes(&edited),
            },
            ..clip.clone()
        })
    })
}

/// A groove template: one that ships with fourfloor, or a custom one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrooveSource {
    /// A built-in template by name: `straight`, `swing_16_54`,
    /// `swing_16_58`, `swing_16_62`, `swing_16_66`, `swing_8_62` or
    /// `house_push`.
    Builtin(String),
    /// A template given in full.
    Custom(GrooveTemplate),
}

impl GrooveSource {
    /// The template, checked.
    pub fn resolve(&self) -> Result<GrooveTemplate, ModelError> {
        let template = match self {
            Self::Builtin(name) => GrooveTemplate::builtin(name)
                .ok_or_else(|| invalid("groove", format!("no built-in groove named `{name}`")))?,
            Self::Custom(template) => template.clone(),
        };
        template.validate()?;
        Ok(template)
    }
}

fn default_amount() -> f64 {
    1.0
}

fn check_amount(field: &'static str, amount: f64) -> Result<(), ModelError> {
    if amount.is_finite() && (0.0..=1.0).contains(&amount) {
        Ok(())
    } else {
        Err(invalid(field, format!("{amount} is outside 0 to 1")))
    }
}

/// Write a step-sequencer pattern into a MIDI clip.
///
/// Each lane is one pitch with a row of sixteenth-note steps. A lane
/// shorter than the pattern repeats. Swing is applied first, then the
/// groove template; both are baked into the notes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteStepPattern {
    /// The MIDI track that holds the clip.
    #[schemars(with = "u64")]
    pub track: TrackId,
    /// An existing clip on that track to replace. Omit to create a new clip.
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    pub clip: Option<ClipId>,
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// Timeline position in ticks from the project start.
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// Pattern length in 4/4 bars, 1 to 64.
    pub bars: u32,
    /// The rows: at most 128, each with at most 1024 steps.
    pub lanes: Vec<StepLane>,
    /// Swing for this pattern. Omit to use the project's swing setting.
    #[serde(default)]
    pub swing: Option<Groove>,
    /// A groove template applied after the swing. Omit for none.
    #[serde(default)]
    pub groove: Option<GrooveSource>,
    /// How strongly the groove template is applied, 0 to 1. Defaults to 1.
    #[serde(default = "default_amount")]
    pub groove_amount: f64,
}

impl WriteStepPattern {
    /// The notes the pattern produces, sorted, with swing and groove baked
    /// in and every note inside the clip.
    pub fn notes(&self, project: &Project) -> Result<Vec<MidiNote>, ModelError> {
        if !(1..=MAX_PATTERN_BARS).contains(&self.bars) {
            return Err(invalid(
                "bars",
                format!("{} is outside 1 to {MAX_PATTERN_BARS}", self.bars),
            ));
        }
        if self.lanes.len() > MAX_STEP_LANES {
            return Err(invalid(
                "lanes",
                format!("a pattern holds at most {MAX_STEP_LANES} lanes"),
            ));
        }
        if self
            .lanes
            .iter()
            .any(|lane| lane.steps.len() > MAX_LANE_STEPS)
        {
            return Err(invalid(
                "lanes",
                format!("a lane holds at most {MAX_LANE_STEPS} steps"),
            ));
        }
        check_amount("groove_amount", self.groove_amount)?;
        let swing = self.swing.unwrap_or(project.groove);
        validate::groove(&swing)?;
        let template = self
            .groove
            .as_ref()
            .map(GrooveSource::resolve)
            .transpose()?;
        let length = i64::from(self.bars) * BAR_TICKS;
        let total_steps = self.bars as usize * STEPS_PER_BAR;
        let notes: Vec<MidiNote> = self
            .lanes
            .iter()
            .flat_map(|lane| lane.notes_for(total_steps, &swing))
            .map(|note| match &template {
                Some(template) => template.apply(&note, self.groove_amount),
                None => note,
            })
            .map(|note| MidiNote {
                start: Ticks::new(note.start.get().clamp(0, length - 1)),
                ..note
            })
            .collect();
        Ok(validate::sorted_notes(&notes))
    }
}

impl Apply for WriteStepPattern {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        WriteMidiClip {
            track: self.track,
            clip: self.clip,
            name: self.name.clone(),
            start: self.start,
            length: Ticks::new(i64::from(self.bars.clamp(1, MAX_PATTERN_BARS)) * BAR_TICKS),
            notes: self.notes(project)?,
        }
        .apply(project)
    }
}

/// Move a MIDI clip's notes onto a grid.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuantizeClip {
    /// The MIDI clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Grid size in ticks, 1 to 3840. 240 is a sixteenth note.
    #[schemars(with = "i64")]
    pub grid: Ticks,
    /// How far notes move towards the grid, 0 to 1. Defaults to 1.
    #[serde(default = "default_amount")]
    pub strength: f64,
    /// `true` to also round note lengths to the grid (at least one grid
    /// step). Defaults to false.
    #[serde(default)]
    pub lengths: bool,
}

impl Apply for QuantizeClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if !(1..=TICKS_PER_BEAT * 4).contains(&self.grid.get()) {
            return Err(invalid(
                "grid",
                format!("{} ticks is outside 1 to 3840", self.grid.get()),
            ));
        }
        check_amount("strength", self.strength)?;
        let updated = map_notes(project, self.clip, |notes, _| {
            Ok(notes
                .iter()
                .map(|note| MidiNote {
                    start: quantize_tick(note.start, self.grid, self.strength).max(Ticks::ZERO),
                    length: if self.lengths {
                        quantize_tick(note.length, self.grid, self.strength).max(self.grid)
                    } else {
                        note.length
                    },
                    ..*note
                })
                .collect())
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Shift every note of a MIDI clip up or down by semitones.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransposeClip {
    /// The MIDI clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Semitones to move, -127 to 127. The command fails if any note would
    /// leave the range 0 to 127.
    pub semitones: i8,
}

impl Apply for TransposeClip {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = map_notes(project, self.clip, |notes, _| {
            notes
                .iter()
                .enumerate()
                .map(|(index, note)| {
                    let moved = i16::from(note.pitch) + i16::from(self.semitones);
                    u8::try_from(moved)
                        .ok()
                        .filter(|pitch| *pitch <= 127)
                        .map(|pitch| MidiNote { pitch, ..*note })
                        .ok_or_else(|| ModelError::InvalidNote {
                            index,
                            reason: format!("pitch {moved} is outside 0 to 127"),
                        })
                })
                .collect()
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Move every note of a MIDI clip to the nearest note of a scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapClipToScale {
    /// The MIDI clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// The scale's root as a pitch class: 0 is C, 9 is A. 0 to 11.
    pub root: u8,
    /// The scale.
    pub scale: Scale,
}

impl Apply for SnapClipToScale {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if self.root > 11 {
            return Err(invalid("root", format!("{} is outside 0 to 11", self.root)));
        }
        let updated = map_notes(project, self.clip, |notes, _| {
            // Two notes that land on the same pitch at the same start would
            // be one note played twice: keep the first.
            let mut snapped: Vec<MidiNote> = Vec::with_capacity(notes.len());
            for note in notes {
                let moved = MidiNote {
                    pitch: snap_pitch(note.pitch, self.root, self.scale),
                    ..*note
                };
                let duplicate = snapped
                    .iter()
                    .any(|kept| kept.start == moved.start && kept.pitch == moved.pitch);
                if !duplicate {
                    snapped.push(moved);
                }
            }
            Ok(snapped)
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Add the notes of a chord to a MIDI clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StampChord {
    /// The MIDI clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// Where the chord starts, in ticks from the start of the clip's content.
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// How long the chord lasts, in ticks; must be positive.
    #[schemars(with = "i64")]
    pub length: Ticks,
    /// MIDI pitch of the chord's root, 0 to 127.
    pub root: u8,
    /// The chord quality.
    pub chord: ChordKind,
    /// How many times the lowest note is moved up an octave. Defaults to 0.
    #[serde(default)]
    pub inversion: u8,
    /// MIDI velocity of every note, 1 to 127.
    pub velocity: u8,
}

impl Apply for StampChord {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if self.root > 127 {
            return Err(invalid("root", format!("{} is above 127", self.root)));
        }
        let stamped: Vec<MidiNote> = chord_pitches(self.root, self.chord, self.inversion)
            .into_iter()
            .map(|pitch| MidiNote {
                start: self.start,
                length: self.length,
                pitch,
                velocity: self.velocity,
            })
            .collect();
        let updated = map_notes(project, self.clip, |notes, _| {
            // A stamped note replaces an existing one at the same start and
            // pitch.
            Ok(notes
                .iter()
                .filter(|note| {
                    !stamped
                        .iter()
                        .any(|new| new.start == note.start && new.pitch == note.pitch)
                })
                .chain(&stamped)
                .copied()
                .collect())
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Apply a groove template to a MIDI clip: shift and accent its notes step
/// by step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyGroove {
    /// The MIDI clip, on the timeline or in the launcher.
    #[schemars(with = "u64")]
    pub clip: ClipId,
    /// The template.
    pub groove: GrooveSource,
    /// How strongly it is applied, 0 to 1. Defaults to 1.
    #[serde(default = "default_amount")]
    pub amount: f64,
}

impl Apply for ApplyGroove {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        check_amount("amount", self.amount)?;
        let template = self.groove.resolve()?;
        let updated = map_notes(project, self.clip, |notes, _| {
            Ok(notes
                .iter()
                .map(|note| template.apply(note, self.amount))
                .collect())
        })?;
        Ok((updated, Outcome::Done))
    }
}
