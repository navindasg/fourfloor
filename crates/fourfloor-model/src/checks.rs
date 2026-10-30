//! Validation of the mixer routing, clip windows, the launcher and the
//! modulation system. Shared by commands and by project loading, like
//! [`crate::validate`].

use std::collections::HashSet;

use fourfloor_core::{DeviceId, ParamId, Tempo, Ticks, TrackId};

use crate::clip::{Clip, ClipContent, Warp};
use crate::device::{AutomationPoint, Param, SidechainSource};
use crate::error::ModelError;
use crate::modulation::{ControllerMapping, LfoRate, Macro, MacroTarget, ModSource, Modulator};
use crate::project::Project;
use crate::track::{MixerLane, MixerParam, Send, Track, TrackKind};
use crate::validate::{self, MAX_AUTOMATION_POINTS, MAX_TICKS};

/// Most targets one modulator or macro may drive.
pub const MAX_TARGETS: usize = 256;
/// Longest envelope stage, in milliseconds.
pub const MAX_ENVELOPE_MS: f64 = 60_000.0;
/// Coarsest launch quantization, in ticks: sixteen 4/4 bars.
pub const MAX_LAUNCH_QUANTIZE: i64 = 960 * 4 * 16;

fn invalid(field: &'static str, reason: impl Into<String>) -> ModelError {
    ModelError::InvalidValue {
        field,
        reason: reason.into(),
    }
}

fn routing_error(track: TrackId, reason: &str) -> ModelError {
    ModelError::InvalidRouting {
        track,
        reason: reason.to_owned(),
    }
}

pub(crate) fn send(project: &Project, track: TrackId, send: &Send) -> Result<(), ModelError> {
    validate::gain_db(send.gain_db)?;
    if send.target == track {
        return Err(routing_error(track, "a track cannot send to itself"));
    }
    let target = project
        .track(send.target)
        .ok_or(ModelError::TrackNotFound(send.target))?;
    if target.kind != TrackKind::Bus {
        return Err(routing_error(track, "the send target is not a bus"));
    }
    Ok(())
}

pub(crate) fn sidechain(
    project: &Project,
    owner: TrackId,
    source: &SidechainSource,
) -> Result<(), ModelError> {
    if project.track(source.track).is_none() {
        return Err(ModelError::TrackNotFound(source.track));
    }
    if source.track == owner {
        return Err(routing_error(
            owner,
            "a device cannot be keyed by its own track",
        ));
    }
    Ok(())
}

/// The tracks whose signal `track` feeds: its output bus, its send targets
/// and every track holding a device it keys.
fn feeds(project: &Project, track: &Track) -> Vec<TrackId> {
    let keyed = project.tracks.iter().filter_map(|other| {
        other
            .devices
            .iter()
            .any(|device| {
                device
                    .sidechain
                    .is_some_and(|source| source.track == track.id)
            })
            .then_some(other.id)
    });
    track
        .output
        .into_iter()
        .chain(track.sends.iter().map(|send| send.target))
        .chain(keyed)
        .collect()
}

/// Checks that outputs, sends and sidechains together form no loop.
pub(crate) fn routing_graph(project: &Project) -> Result<(), ModelError> {
    // Depth-first search with the usual three colours.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let index = |id: TrackId| project.track_index(id);
    let edges: Vec<Vec<usize>> = project
        .tracks
        .iter()
        .map(|track| {
            feeds(project, track)
                .into_iter()
                .filter_map(index)
                .collect()
        })
        .collect();
    let mut marks = vec![Mark::New; project.tracks.len()];
    for root in 0..project.tracks.len() {
        if marks[root] != Mark::New {
            continue;
        }
        // (node, next edge to follow)
        let mut stack = vec![(root, 0usize)];
        marks[root] = Mark::Open;
        while let Some((node, edge)) = stack.pop() {
            match edges[node].get(edge).copied() {
                None => marks[node] = Mark::Done,
                Some(next) => {
                    stack.push((node, edge + 1));
                    match marks[next] {
                        Mark::Open => {
                            return Err(routing_error(
                                project.tracks[next].id,
                                "the route would loop",
                            ));
                        }
                        Mark::New => {
                            marks[next] = Mark::Open;
                            stack.push((next, 0));
                        }
                        Mark::Done => {}
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn warp(warp: &Warp) -> Result<(), ModelError> {
    Tempo::new(warp.source_bpm)
        .map(|_| ())
        .map_err(|_| ModelError::InvalidTempo {
            bpm: warp.source_bpm,
        })
}

pub(crate) fn fades(fade_in: Ticks, fade_out: Ticks, length: Ticks) -> Result<(), ModelError> {
    let sane = fade_in >= Ticks::ZERO && fade_out >= Ticks::ZERO;
    if sane && fade_in.get().saturating_add(fade_out.get()) <= length.get() {
        Ok(())
    } else {
        Err(invalid(
            "fades",
            "fades must not be negative or longer than the clip together",
        ))
    }
}

/// Checks a clip's offset and loop, and the audio settings of an audio clip.
pub(crate) fn clip_window(clip: &Clip) -> Result<(), ModelError> {
    if !(0..=MAX_TICKS).contains(&clip.offset.get()) {
        return Err(invalid(
            "offset",
            format!("{} ticks is out of range", clip.offset.get()),
        ));
    }
    if let Some(period) = clip.loop_length {
        loop_length(period)?;
    }
    if let ClipContent::Audio {
        gain_db,
        fade_in,
        fade_out,
        warp: clip_warp,
        ..
    } = &clip.content
    {
        validate::gain_db(*gain_db)?;
        fades(*fade_in, *fade_out, clip.length)?;
        if let Some(clip_warp) = clip_warp {
            warp(clip_warp)?;
        }
    }
    Ok(())
}

pub(crate) fn loop_length(period: Ticks) -> Result<(), ModelError> {
    if (1..=MAX_TICKS).contains(&period.get()) {
        Ok(())
    } else {
        Err(invalid(
            "loop_length",
            format!("{} ticks is not a positive length in range", period.get()),
        ))
    }
}

pub(crate) fn launch_quantize(ticks: Ticks) -> Result<(), ModelError> {
    if (0..=MAX_LAUNCH_QUANTIZE).contains(&ticks.get()) {
        Ok(())
    } else {
        Err(invalid(
            "launch_quantize",
            format!(
                "{} ticks is outside 0 to {MAX_LAUNCH_QUANTIZE}",
                ticks.get()
            ),
        ))
    }
}

pub(crate) fn mixer_points(
    param: MixerParam,
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
        match param {
            MixerParam::Gain => validate::gain_db(point.value),
            MixerParam::Pan => validate::pan(point.value),
        }
    })
}

fn mixer_lane(lane: &MixerLane) -> Result<(), ModelError> {
    mixer_points(lane.param, &lane.points)?;
    if lane.points.is_empty() {
        return Err(invalid("points", "a stored lane must have points"));
    }
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

/// The parameter a target names, or the error saying what is missing.
pub(crate) fn param_ref(
    project: &Project,
    device: DeviceId,
    param: ParamId,
) -> Result<&Param, ModelError> {
    let (_, instance) = project
        .find_device(device)
        .ok_or(ModelError::DeviceNotFound(device))?;
    instance
        .param(param)
        .ok_or(ModelError::ParamNotFound { device, param })
}

pub(crate) fn unit(field: &'static str, value: f32) -> Result<(), ModelError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(invalid(field, format!("{value} is outside 0 to 1")))
    }
}

pub(crate) fn depth(value: f32) -> Result<(), ModelError> {
    if value.is_finite() && (-1.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(invalid("depth", format!("{value} is outside -1 to 1")))
    }
}

pub(crate) fn mod_source(project: &Project, source: &ModSource) -> Result<(), ModelError> {
    let within = |field: &'static str, value: f64, low: f64, high: f64| {
        if value.is_finite() && (low..=high).contains(&value) {
            Ok(())
        } else {
            Err(invalid(
                field,
                format!("{value} is outside {low} to {high}"),
            ))
        }
    };
    match *source {
        ModSource::Lfo { rate, phase, .. } => {
            match rate {
                LfoRate::Hertz(hz) => within("rate", hz, 0.001, 1_000.0)?,
                LfoRate::Beats(beats) => within("rate", beats, 1.0 / 64.0, 1_024.0)?,
            }
            within("phase", phase, 0.0, 1.0)
        }
        ModSource::Envelope {
            trigger,
            attack_ms,
            decay_ms,
            sustain,
            release_ms,
        } => {
            let track = project
                .track(trigger)
                .ok_or(ModelError::TrackNotFound(trigger))?;
            if track.kind != TrackKind::Midi {
                return Err(ModelError::WrongTrackKind {
                    track: trigger,
                    expected: TrackKind::Midi,
                    actual: track.kind,
                });
            }
            within("attack_ms", attack_ms, 0.0, MAX_ENVELOPE_MS)?;
            within("decay_ms", decay_ms, 0.0, MAX_ENVELOPE_MS)?;
            within("sustain", sustain, 0.0, 1.0)?;
            within("release_ms", release_ms, 0.0, MAX_ENVELOPE_MS)
        }
    }
}

pub(crate) fn macro_target(project: &Project, target: &MacroTarget) -> Result<(), ModelError> {
    let param = param_ref(project, target.device, target.param)?;
    validate::param_value(target.device, param, target.min)?;
    validate::param_value(target.device, param, target.max)
}

pub(crate) fn controller(project: &Project, mapping: &ControllerMapping) -> Result<(), ModelError> {
    if mapping.channel.is_some_and(|channel| channel > 15) {
        return Err(invalid("channel", "MIDI channels run from 0 to 15"));
    }
    if mapping.controller > 127 {
        return Err(invalid(
            "controller",
            "controller numbers run from 0 to 127",
        ));
    }
    let param = param_ref(project, mapping.device, mapping.param)?;
    validate::param_value(mapping.device, param, mapping.min)?;
    validate::param_value(mapping.device, param, mapping.max)
}

fn too_many_targets(count: usize) -> Result<(), ModelError> {
    if count > MAX_TARGETS {
        Err(invalid(
            "targets",
            format!("at most {MAX_TARGETS} targets are allowed"),
        ))
    } else {
        Ok(())
    }
}

fn modulator(project: &Project, modulator: &Modulator) -> Result<(), ModelError> {
    validate::name("modulator name", &modulator.name)?;
    mod_source(project, &modulator.source)?;
    too_many_targets(modulator.targets.len())?;
    let mut seen = HashSet::new();
    modulator.targets.iter().try_for_each(|target| {
        param_ref(project, target.device, target.param)?;
        depth(target.depth)?;
        if seen.insert((target.device, target.param)) {
            Ok(())
        } else {
            Err(invalid("targets", "a parameter is listed twice"))
        }
    })
}

fn macro_control(project: &Project, control: &Macro) -> Result<(), ModelError> {
    validate::name("macro name", &control.name)?;
    unit("value", control.value)?;
    too_many_targets(control.targets.len())?;
    let mut seen = HashSet::new();
    control.targets.iter().try_for_each(|target| {
        macro_target(project, target)?;
        if seen.insert((target.device, target.param)) {
            Ok(())
        } else {
            Err(invalid("targets", "a parameter is listed twice"))
        }
    })
}

/// Checks a track's sends, sidechains, mixer automation and launcher slots.
/// Slot clips are checked by the caller, which owns the id bookkeeping.
pub(crate) fn track_extras(project: &Project, track: &Track) -> Result<(), ModelError> {
    let mut targets = HashSet::new();
    for entry in &track.sends {
        send(project, track.id, entry)?;
        if !targets.insert(entry.target) {
            return Err(invalid("sends", "two sends share a target"));
        }
    }
    for device in &track.devices {
        if let Some(source) = &device.sidechain {
            sidechain(project, track.id, source)?;
        }
    }
    let mut lanes = HashSet::new();
    for lane in &track.mixer_automation {
        mixer_lane(lane)?;
        if !lanes.insert(lane.param) {
            return Err(invalid(
                "mixer_automation",
                "two lanes automate the same control",
            ));
        }
    }
    let mut scenes = HashSet::new();
    for slot in &track.slots {
        if project.scene(slot.scene).is_none() {
            return Err(invalid(
                "slots",
                format!("scene {} does not exist", slot.scene),
            ));
        }
        if !scenes.insert(slot.scene) {
            return Err(invalid("slots", "two slots share a scene"));
        }
        if track.kind == TrackKind::Bus {
            return Err(invalid("slots", "a bus holds no clips"));
        }
    }
    Ok(())
}

/// Checks everything that hangs off the project rather than a track.
/// Scene, modulator and macro ids are claimed by the caller.
pub(crate) fn project_extras(project: &Project) -> Result<(), ModelError> {
    launch_quantize(project.launch_quantize)?;
    project
        .scenes
        .iter()
        .try_for_each(|scene| validate::name("scene name", &scene.name))?;
    project
        .modulators
        .iter()
        .try_for_each(|entry| modulator(project, entry))?;
    project
        .macros
        .iter()
        .try_for_each(|entry| macro_control(project, entry))?;
    let mut bound = HashSet::new();
    for mapping in &project.controller_map {
        controller(project, mapping)?;
        if !bound.insert((mapping.channel, mapping.controller)) {
            return Err(invalid("controller_map", "a controller is mapped twice"));
        }
    }
    routing_graph(project)
}
