//! Automation lanes and modulators: the project's moving parameters as an
//! engine [`ControlPlan`].

use std::collections::HashMap;

use fourfloor_core::{DeviceId, ParamId, TrackId};
use fourfloor_engine::devices::Fader;
use fourfloor_engine::{
    ControlPlan, ControlPoint, ControlTarget, LfoRate, LfoShape, ModulatorSpec,
};
use fourfloor_model as model;
use fourfloor_model::{AutomationPoint, MixerParam, ModSource, Project};

use crate::build::ParamRange;
use crate::routing::track_notes;

fn shape(shape: model::LfoShape) -> LfoShape {
    match shape {
        model::LfoShape::Sine => LfoShape::Sine,
        model::LfoShape::Triangle => LfoShape::Triangle,
        model::LfoShape::SawUp => LfoShape::SawUp,
        model::LfoShape::SawDown => LfoShape::SawDown,
        model::LfoShape::Square => LfoShape::Square,
    }
}

fn spec(project: &Project, source: &ModSource) -> ModulatorSpec {
    match *source {
        ModSource::Lfo {
            shape: lfo_shape,
            rate,
            phase,
        } => ModulatorSpec::Lfo {
            shape: shape(lfo_shape),
            rate: match rate {
                model::LfoRate::Hertz(hz) => LfoRate::Hertz(hz),
                model::LfoRate::Beats(beats) => LfoRate::Beats(beats),
            },
            phase,
        },
        ModSource::Envelope {
            trigger,
            attack_ms,
            decay_ms,
            sustain,
            release_ms,
        } => ModulatorSpec::Envelope {
            notes: project
                .track(trigger)
                .map(|track| track_notes(track))
                .unwrap_or_default()
                .iter()
                .map(|note| (note.start, note.start + note.length))
                .collect(),
            attack_ms,
            decay_ms,
            sustain,
            release_ms,
        },
    }
}

fn points(lane: &[AutomationPoint]) -> Vec<ControlPoint> {
    lane.iter()
        .map(|point| ControlPoint {
            time: point.time,
            value: point.value,
        })
        .collect()
}

/// Builds the plan for everything in `project` that moves a parameter of
/// a node that is in the graph.
///
/// `ranges` holds every parameter of every chain device that was built;
/// `faders` maps a track in the graph to its fader node. Lanes and
/// modulation aimed at anything else are left out and reported.
pub(crate) fn control_plan(
    project: &Project,
    ranges: &HashMap<(DeviceId, ParamId), ParamRange>,
    faders: &HashMap<TrackId, DeviceId>,
    warnings: &mut Vec<String>,
) -> ControlPlan {
    let mut plan = ControlPlan::default();
    let mut index: HashMap<(DeviceId, ParamId), usize> = HashMap::new();
    let mut skipped_lanes = 0usize;
    let mut skipped_modulation = 0usize;

    for track in &project.tracks {
        for lane in track
            .automation
            .iter()
            .filter(|lane| !lane.points.is_empty())
        {
            let key = (lane.device, lane.param);
            let Some(range) = ranges.get(&key) else {
                skipped_lanes += 1;
                continue;
            };
            let mut target =
                ControlTarget::new(lane.device, lane.param, range.base, range.min, range.max);
            target.lane = points(&lane.points);
            index.insert(key, plan.targets.len());
            plan.targets.push(target);
        }
        let Some(&fader) = faders.get(&track.id) else {
            skipped_lanes += track.mixer_automation.len();
            continue;
        };
        for lane in track
            .mixer_automation
            .iter()
            .filter(|lane| !lane.points.is_empty())
        {
            let mut target = match lane.param {
                MixerParam::Gain => {
                    ControlTarget::new(fader, Fader::GAIN_DB, track.gain_db, Fader::MIN_DB, 24.0)
                }
                MixerParam::Pan => ControlTarget::new(fader, Fader::PAN, track.pan, -1.0, 1.0),
            };
            target.lane = points(&lane.points);
            plan.targets.push(target);
        }
    }

    for modulator in &project.modulators {
        let live: Vec<_> = modulator
            .targets
            .iter()
            .filter(|target| ranges.contains_key(&(target.device, target.param)))
            .collect();
        skipped_modulation += modulator.targets.len() - live.len();
        if live.is_empty() {
            continue;
        }
        let modulator_index = plan.modulators.len();
        plan.modulators.push(spec(project, &modulator.source));
        for target in live {
            let key = (target.device, target.param);
            let at = *index.entry(key).or_insert_with(|| {
                let range = ranges[&key];
                plan.targets.push(ControlTarget::new(
                    target.device,
                    target.param,
                    range.base,
                    range.min,
                    range.max,
                ));
                plan.targets.len() - 1
            });
            plan.targets[at]
                .modulation
                .push((modulator_index, target.depth));
        }
    }

    if skipped_lanes > 0 {
        warnings.push(format!(
            "{skipped_lanes} automation lane(s) aim at something that is not in the graph \
             and are not played"
        ));
    }
    if skipped_modulation > 0 {
        warnings.push(format!(
            "{skipped_modulation} modulation target(s) aim at something that is not in the \
             graph and are not played"
        ));
    }
    plan
}
