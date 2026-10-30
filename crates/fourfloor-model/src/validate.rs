//! Validation rules shared by commands and by project loading.

use std::collections::HashSet;

use fourfloor_core::{DeviceId, Tempo, Ticks, TrackId};

use crate::checks;
use crate::clip::{AudioRef, Clip, ClipContent, MidiNote};
use crate::device::{AutomationLane, AutomationPoint, DeviceInstance, Param};
use crate::error::ModelError;
use crate::project::{Groove, Project};
use crate::track::{Track, TrackKind};

/// Longest allowed name, in characters.
pub const MAX_NAME_LEN: usize = 128;
/// Lowest fader gain in decibels.
pub const MIN_GAIN_DB: f32 = -96.0;
/// Highest fader gain in decibels.
pub const MAX_GAIN_DB: f32 = 24.0;
/// Straight timing: the off step sits exactly halfway through the pair.
pub const MIN_SWING_PERCENT: f64 = 50.0;
/// Hardest allowed swing.
pub const MAX_SWING_PERCENT: f64 = 75.0;
/// Largest timeline position or length, in ticks (about 1.1 billion beats).
pub const MAX_TICKS: i64 = 1 << 40;
/// Most notes one MIDI clip may hold.
pub const MAX_NOTES_PER_CLIP: usize = 65_536;
/// Most parameters one device may declare.
pub const MAX_PARAMS_PER_DEVICE: usize = 4_096;
/// Most breakpoints one automation lane may hold.
pub const MAX_AUTOMATION_POINTS: usize = 65_536;
/// Longest allowed audio file extension.
pub const MAX_EXTENSION_LEN: usize = 8;

fn invalid(field: &'static str, reason: impl Into<String>) -> ModelError {
    ModelError::InvalidValue {
        field,
        reason: reason.into(),
    }
}

pub(crate) fn name(field: &'static str, value: &str) -> Result<(), ModelError> {
    let reason = if value.trim().is_empty() {
        Some("must not be empty".to_owned())
    } else if value.chars().count() > MAX_NAME_LEN {
        Some(format!("must be at most {MAX_NAME_LEN} characters"))
    } else if value.chars().any(char::is_control) {
        Some("must not contain control characters".to_owned())
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ModelError::InvalidName { field, reason }),
        None => Ok(()),
    }
}

pub(crate) fn tempo(bpm: f64) -> Result<Tempo, ModelError> {
    Tempo::new(bpm).map_err(|_| ModelError::InvalidTempo { bpm })
}

pub(crate) fn swing(percent: f64) -> Result<(), ModelError> {
    if percent.is_finite() && (MIN_SWING_PERCENT..=MAX_SWING_PERCENT).contains(&percent) {
        Ok(())
    } else {
        Err(ModelError::InvalidSwing { percent })
    }
}

pub(crate) fn groove(groove: &Groove) -> Result<(), ModelError> {
    swing(groove.swing_percent)
}

pub(crate) fn gain_db(value: f32) -> Result<(), ModelError> {
    if value.is_finite() && (MIN_GAIN_DB..=MAX_GAIN_DB).contains(&value) {
        Ok(())
    } else {
        Err(invalid(
            "gain_db",
            format!("{value} is outside {MIN_GAIN_DB} to {MAX_GAIN_DB}"),
        ))
    }
}

pub(crate) fn pan(value: f32) -> Result<(), ModelError> {
    if value.is_finite() && (-1.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(invalid("pan", format!("{value} is outside -1 to 1")))
    }
}

pub(crate) fn clip_span(start: Ticks, length: Ticks) -> Result<(), ModelError> {
    if !(0..=MAX_TICKS).contains(&start.get()) {
        return Err(invalid(
            "start",
            format!("{} ticks is out of range", start.get()),
        ));
    }
    if !(1..=MAX_TICKS).contains(&length.get()) {
        return Err(invalid(
            "length",
            format!("{} ticks is not a positive length in range", length.get()),
        ));
    }
    Ok(())
}

pub(crate) fn notes(notes: &[MidiNote], clip_length: Ticks) -> Result<(), ModelError> {
    if notes.len() > MAX_NOTES_PER_CLIP {
        return Err(invalid(
            "notes",
            format!("a clip holds at most {MAX_NOTES_PER_CLIP} notes"),
        ));
    }
    notes
        .iter()
        .enumerate()
        .try_for_each(|(index, note)| match note_problem(note, clip_length) {
            Some(reason) => Err(ModelError::InvalidNote { index, reason }),
            None => Ok(()),
        })
}

pub(crate) fn note_problem(note: &MidiNote, clip_length: Ticks) -> Option<String> {
    if note.pitch > 127 {
        Some(format!("pitch {} is above 127", note.pitch))
    } else if !(1..=127).contains(&note.velocity) {
        Some(format!("velocity {} is outside 1 to 127", note.velocity))
    } else if note.start < Ticks::ZERO || note.start >= clip_length {
        Some(format!("start {} is outside the clip", note.start.get()))
    } else if !(1..=MAX_TICKS).contains(&note.length.get()) {
        Some(format!("length {} is not positive", note.length.get()))
    } else {
        None
    }
}

/// The canonical note order: by start, then pitch.
pub(crate) fn sorted_notes(notes: &[MidiNote]) -> Vec<MidiNote> {
    let mut sorted = notes.to_vec();
    sorted.sort_by_key(|note| (note.start, note.pitch));
    sorted
}

pub(crate) fn params(params: &[Param]) -> Result<(), ModelError> {
    if params.len() > MAX_PARAMS_PER_DEVICE {
        return Err(invalid(
            "params",
            format!("a device declares at most {MAX_PARAMS_PER_DEVICE} parameters"),
        ));
    }
    let mut seen = HashSet::with_capacity(params.len());
    params.iter().try_for_each(|param| {
        name("parameter name", &param.name)?;
        let id = param.id.get();
        if !seen.insert(id) {
            return Err(invalid(
                "params",
                format!("parameter id {id} appears twice"),
            ));
        }
        let finite = param.min.is_finite() && param.max.is_finite() && param.value.is_finite();
        if !finite || param.min > param.max {
            return Err(invalid(
                "params",
                format!("parameter {id} has an invalid range"),
            ));
        }
        if param.value < param.min || param.value > param.max {
            return Err(invalid(
                "params",
                format!("parameter {id} has a value outside its range"),
            ));
        }
        Ok(())
    })
}

pub(crate) fn param_value(device: DeviceId, param: &Param, value: f32) -> Result<(), ModelError> {
    if value.is_finite() && value >= param.min && value <= param.max {
        Ok(())
    } else {
        Err(ModelError::ParamOutOfRange {
            device,
            param: param.id,
            value,
            min: param.min,
            max: param.max,
        })
    }
}

pub(crate) fn automation_points(
    device: DeviceId,
    param: &Param,
    points: &[AutomationPoint],
) -> Result<(), ModelError> {
    if points.len() > MAX_AUTOMATION_POINTS {
        return Err(invalid(
            "points",
            format!("a lane holds at most {MAX_AUTOMATION_POINTS} points"),
        ));
    }
    points.iter().try_for_each(|point| {
        if !(0..=MAX_TICKS).contains(&point.time.get()) {
            return Err(invalid(
                "points",
                format!("time {} ticks is out of range", point.time.get()),
            ));
        }
        param_value(device, param, point.value)
    })
}

pub(crate) fn audio_ref(source: &AudioRef) -> Result<(), ModelError> {
    extension(&source.extension)
}

pub(crate) fn extension(extension: &str) -> Result<(), ModelError> {
    let well_formed = (1..=MAX_EXTENSION_LEN).contains(&extension.len())
        && extension
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if well_formed {
        Ok(())
    } else {
        Err(invalid(
            "extension",
            format!("must be 1 to {MAX_EXTENSION_LEN} lowercase letters or digits"),
        ))
    }
}

/// Checks that `track` may send its output to `output`.
pub(crate) fn routing(
    project: &Project,
    track: TrackId,
    output: Option<TrackId>,
) -> Result<(), ModelError> {
    let Some(target) = output else {
        return Ok(());
    };
    let reject = |reason: &str| ModelError::InvalidRouting {
        track,
        reason: reason.to_owned(),
    };
    if target == track {
        return Err(reject("a track cannot feed itself"));
    }
    let bus = project
        .track(target)
        .ok_or(ModelError::TrackNotFound(target))?;
    if bus.kind != TrackKind::Bus {
        return Err(reject("the target is not a bus"));
    }
    // Walk downstream from the target; meeting `track` again means a loop.
    let mut next = bus.output;
    for _ in 0..project.tracks.len() {
        match next {
            None => return Ok(()),
            Some(id) if id == track => return Err(reject("the route would loop")),
            Some(id) => next = project.track(id).and_then(|t| t.output),
        }
    }
    Err(reject("the route would loop"))
}

/// Checks every invariant the commands maintain. Used on load, where the
/// data did not come through the command bus.
pub(crate) fn project(project: &Project) -> Result<(), ModelError> {
    name("project name", &project.name)?;
    groove(&project.groove)?;
    let mut ids = HashSet::new();
    project.tracks.iter().try_for_each(|track| {
        claim_id(&mut ids, project.next_id, track.id.get())?;
        self::track(project, track, &mut ids)
    })?;
    let extra_ids = project
        .scenes
        .iter()
        .map(|scene| scene.id.get())
        .chain(project.modulators.iter().map(|entry| entry.id.get()))
        .chain(project.macros.iter().map(|entry| entry.id.get()));
    for id in extra_ids {
        claim_id(&mut ids, project.next_id, id)?;
    }
    checks::project_extras(project)
}

fn claim_id(ids: &mut HashSet<u64>, next_id: u64, id: u64) -> Result<(), ModelError> {
    if id < next_id && ids.insert(id) {
        Ok(())
    } else {
        Err(ModelError::InvalidId(id))
    }
}

fn track(project: &Project, track: &Track, ids: &mut HashSet<u64>) -> Result<(), ModelError> {
    name("track name", &track.name)?;
    gain_db(track.gain_db)?;
    pan(track.pan)?;
    routing(project, track.id, track.output)?;
    for clip in track
        .clips
        .iter()
        .chain(track.slots.iter().map(|slot| &slot.clip))
    {
        claim_id(ids, project.next_id, clip.id.get())?;
        self::clip(track, clip)?;
    }
    checks::track_extras(project, track)?;
    for device in &track.devices {
        claim_id(ids, project.next_id, device.id.get())?;
        self::device(device)?;
    }
    let mut lanes = HashSet::new();
    track.automation.iter().try_for_each(|lane| {
        if !lanes.insert((lane.device, lane.param)) {
            return Err(invalid(
                "automation",
                "two lanes automate the same parameter",
            ));
        }
        self::lane(track, lane)
    })
}

fn clip(track: &Track, clip: &Clip) -> Result<(), ModelError> {
    name("clip name", &clip.name)?;
    clip_span(clip.start, clip.length)?;
    checks::clip_window(clip)?;
    let expected = match &clip.content {
        ClipContent::Midi { .. } => TrackKind::Midi,
        ClipContent::Audio { .. } => TrackKind::Audio,
    };
    if track.kind != expected {
        return Err(ModelError::WrongTrackKind {
            track: track.id,
            expected,
            actual: track.kind,
        });
    }
    match &clip.content {
        ClipContent::Audio { source, .. } => audio_ref(source),
        ClipContent::Midi { notes: clip_notes } => {
            // A clip is a window: notes may lie outside it, but not outside the
            // timeline's range.
            notes(clip_notes, Ticks::new(MAX_TICKS + 1))?;
            let sorted = clip_notes
                .windows(2)
                .all(|pair| (pair[0].start, pair[0].pitch) <= (pair[1].start, pair[1].pitch));
            if sorted {
                Ok(())
            } else {
                Err(invalid("notes", "notes must be sorted by start then pitch"))
            }
        }
    }
}

fn device(device: &DeviceInstance) -> Result<(), ModelError> {
    name("device kind", &device.kind)?;
    name("device name", &device.name)?;
    params(&device.params)
}

fn lane(track: &Track, lane: &AutomationLane) -> Result<(), ModelError> {
    let device = track
        .device(lane.device)
        .ok_or(ModelError::DeviceNotFound(lane.device))?;
    let param = device.param(lane.param).ok_or(ModelError::ParamNotFound {
        device: lane.device,
        param: lane.param,
    })?;
    automation_points(lane.device, param, &lane.points)?;
    if lane
        .points
        .windows(2)
        .all(|pair| pair[0].time <= pair[1].time)
    {
        Ok(())
    } else {
        Err(invalid(
            "points",
            "automation points must be sorted by time",
        ))
    }
}
