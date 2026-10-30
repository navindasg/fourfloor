//! Shared helpers for tools that edit, and the high-level `set_swing`.

use std::sync::Arc;

use fourfloor_core::{DeviceId, DeviceRegistry, ParamId, Ticks, TrackId};
use fourfloor_model::command::{AddDevice, CreateTrack, SetSwing, WriteMidiClip};
use fourfloor_model::sequencer::{apply_swing, BAR_TICKS};
use fourfloor_model::validate::{MAX_SWING_PERCENT, MIN_SWING_PERCENT};
use fourfloor_model::{Groove, MidiNote, Param, Project, SwingGrid, TrackKind, Transaction};
use serde_json::json;

use super::args::{SetSwingArgs, TrackRef};
use super::ToolOutput;
use crate::agent::Agent;
use crate::error::AgentError;

/// Tick position of a bar.
pub(crate) fn bar_tick(bar: u32) -> Ticks {
    Ticks::new(i64::from(bar) * BAR_TICKS)
}

/// The parameters a new device of `kind` starts with: every parameter of
/// its registry descriptor at its default, with `overrides` applied (and
/// clamped to range).
pub(crate) fn device_params(
    registry: &DeviceRegistry,
    kind: &str,
    overrides: &[(ParamId, f32)],
) -> Result<Vec<Param>, AgentError> {
    let descriptor = registry.descriptor(kind).ok_or_else(|| {
        AgentError::NotFound(format!(
            "no device kind \"{kind}\" in the registry; list_device_kinds shows them"
        ))
    })?;
    Ok(descriptor
        .params
        .iter()
        .map(|info| {
            let value = overrides
                .iter()
                .find(|(id, _)| *id == info.id)
                .map_or(info.default, |(_, value)| info.clamp(*value));
            Param {
                id: info.id,
                name: info.name.to_string(),
                value,
                min: info.min,
                max: info.max,
            }
        })
        .collect())
}

/// Adds a device of `kind` with registry parameters to `track`.
pub(crate) fn add_device(
    tx: &mut Transaction,
    registry: &DeviceRegistry,
    track: TrackId,
    kind: &str,
    name: Option<&str>,
    overrides: &[(ParamId, f32)],
) -> Result<DeviceId, AgentError> {
    let params = device_params(registry, kind, overrides)?;
    let outcome = tx.execute(AddDevice {
        track,
        kind: kind.to_owned(),
        name: name.map(str::to_owned),
        index: None,
        params,
    })?;
    outcome
        .device()
        .ok_or_else(|| AgentError::Unavailable("add_device returned no device".to_owned()))
}

/// The MIDI track a part goes on: `existing` resolved, or a new track
/// named `name` with the instrument `instrument`. Returns the id and
/// whether it was created.
pub(crate) fn midi_track(
    tx: &mut Transaction,
    registry: &DeviceRegistry,
    existing: Option<&TrackRef>,
    name: &str,
    instrument: &str,
) -> Result<(TrackId, bool), AgentError> {
    if let Some(reference) = existing {
        let id = reference.resolve(tx.project())?;
        let kind = tx.project().track(id).map(|track| track.kind);
        if kind != Some(TrackKind::Midi) {
            return Err(AgentError::Unavailable(format!(
                "track {} is not a MIDI track",
                id.get()
            )));
        }
        return Ok((id, false));
    }
    // Check the instrument before creating anything.
    device_params(registry, instrument, &[])?;
    let track = tx
        .execute(CreateTrack {
            name: name.to_owned(),
            kind: TrackKind::Midi,
            index: None,
        })?
        .track()
        .ok_or_else(|| AgentError::Unavailable("create_track returned no track".to_owned()))?;
    add_device(tx, registry, track, instrument, None, &[])?;
    Ok((track, true))
}

/// Moves note starts with `map`, keeping notes that did not overlap the
/// next onset from overlapping it afterwards (so a monophonic line does
/// not start sliding, and repeated chords stay separate).
pub(crate) fn retime(notes: &[MidiNote], map: impl Fn(Ticks) -> Ticks) -> Vec<MidiNote> {
    let mut onsets: Vec<i64> = notes.iter().map(|note| note.start.get()).collect();
    onsets.sort_unstable();
    onsets.dedup();
    let mut moved: Vec<MidiNote> = notes
        .iter()
        .map(|note| {
            let start = map(note.start);
            let mut length = note.length;
            let end = note.start.get() + note.length.get();
            let next = onsets
                .iter()
                .copied()
                .find(|onset| *onset > note.start.get());
            if let Some(next) = next {
                if end <= next {
                    let gap = (next - end).min(20);
                    let limit = map(Ticks::new(next)).get() - start.get() - gap;
                    length = Ticks::new(length.get().min(limit).max(1));
                }
            }
            MidiNote {
                start,
                length,
                ..*note
            }
        })
        .collect();
    moved.sort_by_key(|note| (note.start, note.pitch));
    moved
}

/// Applies `groove` to straight notes.
pub(crate) fn swing_notes(notes: &[MidiNote], groove: &Groove) -> Vec<MidiNote> {
    retime(notes, |time| apply_swing(time, groove))
}

/// Puts notes that sit on a swung off-step back on the straight grid.
///
/// A note counts as a swung off-step when it lies between the off-step and
/// half a step after it (75 % swing, the most the model allows, moves it by
/// half a step). Everything else is left alone.
fn unswing(time: Ticks, grid: SwingGrid) -> Ticks {
    let step = grid.step_ticks();
    let pair = step * 2;
    let within = time.get().rem_euclid(pair);
    if (step..=step + step / 2).contains(&within) {
        Ticks::new(time.get() - within + step)
    } else {
        time
    }
}

/// The high-level `set_swing`.
pub(crate) fn set_swing(agent: &mut Agent, args: SetSwingArgs) -> Result<ToolOutput, AgentError> {
    if !args.percent.is_finite() || !(MIN_SWING_PERCENT..=MAX_SWING_PERCENT).contains(&args.percent)
    {
        return Err(AgentError::invalid(
            "set_swing",
            format!("swing {} is outside 50 to 75 percent", args.percent),
        ));
    }
    let project = agent.project().clone();
    let old = project.groove;
    let new = Groove {
        swing_percent: args.percent,
        grid: args.grid.unwrap_or(old.grid),
    };
    let targets: Vec<TrackId> = match &args.track {
        Some(reference) => vec![reference.resolve(&project)?],
        None => project
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Midi)
            .map(|track| track.id)
            .collect(),
    };
    let set_project = args.track.is_none();
    let retime_clips = args.retime;
    let result = agent.transaction("set_swing", |tx| {
        if set_project {
            tx.execute(SetSwing {
                percent: args.percent,
                grid: args.grid,
            })?;
        }
        let mut rewritten = 0usize;
        let mut skipped = 0usize;
        if retime_clips {
            for track_id in &targets {
                let Some(track) = tx.project().track(*track_id).map(Arc::clone) else {
                    continue;
                };
                for clip in track.clips.iter().filter(|clip| clip.is_midi()) {
                    if clip.offset != Ticks::ZERO || clip.loop_length.is_some() {
                        skipped += 1;
                        continue;
                    }
                    let notes = retime(clip.notes(), |time| {
                        apply_swing(unswing(time, old.grid), &new)
                    });
                    if notes.as_slice() == clip.notes() {
                        continue;
                    }
                    tx.execute(WriteMidiClip {
                        track: *track_id,
                        clip: Some(clip.id),
                        name: clip.name.clone(),
                        start: clip.start,
                        length: clip.length,
                        notes,
                    })?;
                    rewritten += 1;
                }
            }
        }
        Ok((rewritten, skipped))
    });
    let (rewritten, skipped) = result?;
    let scope = match &args.track {
        Some(_) => format!("on {} track", targets.len()),
        None => "for the project".to_owned(),
    };
    let changed = set_project || rewritten > 0;
    let text = format!(
        "swing {:.1}% {scope}; re-timed {rewritten} clip(s){}",
        args.percent,
        if skipped > 0 {
            format!(", skipped {skipped} looped or offset clip(s)")
        } else {
            String::new()
        }
    );
    let data = json!({
        "percent": args.percent,
        "grid": new.grid,
        "tracks": targets.iter().map(|id| id.get()).collect::<Vec<_>>(),
        "clips_retimed": rewritten,
        "clips_skipped": skipped,
        "project_swing_set": set_project,
    });
    Ok(if changed {
        ToolOutput::changed(text, data)
    } else {
        ToolOutput::unchanged(text, data)
    })
}

/// The project's length rounded up to whole bars (at least one).
pub(crate) fn project_bars(project: &Project) -> u32 {
    let length = fourfloor_compile::project_length(project).get();
    let bars = (length + BAR_TICKS - 1) / BAR_TICKS;
    bars.clamp(1, i64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: Ticks::new(start),
            length: Ticks::new(length),
            pitch,
            velocity: 100,
        }
    }

    #[test]
    fn swinging_keeps_a_mono_line_from_overlapping() {
        // Rolling bass: sixteenths 1, 2, 3 held a step each.
        let notes = [note(240, 220, 33), note(480, 220, 33), note(720, 220, 40)];
        let groove = Groove {
            swing_percent: 75.0,
            grid: SwingGrid::Sixteenth,
        };
        let swung = swing_notes(&notes, &groove);
        assert_eq!(swung[0].start.get(), 360);
        assert_eq!(swung[1].start.get(), 480);
        assert_eq!(swung[2].start.get(), 840);
        for pair in swung.windows(2) {
            assert!(pair[0].start + pair[0].length <= pair[1].start, "{swung:?}");
        }
    }

    #[test]
    fn unswing_then_swing_moves_off_steps_to_the_new_amount() {
        let at_58 = Groove {
            swing_percent: 58.0,
            grid: SwingGrid::Sixteenth,
        };
        let at_66 = Groove {
            swing_percent: 66.0,
            grid: SwingGrid::Sixteenth,
        };
        for straight in [0i64, 240, 480, 720, 960 + 240] {
            let swung = apply_swing(Ticks::new(straight), &at_58);
            let back = unswing(swung, SwingGrid::Sixteenth);
            assert_eq!(back.get(), straight, "{straight}");
            assert_eq!(
                apply_swing(back, &at_66),
                apply_swing(Ticks::new(straight), &at_66)
            );
        }
    }
}
