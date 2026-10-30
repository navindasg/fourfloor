//! Removing references to things a command has just deleted.
//!
//! Deleting a track, a device or a scene can leave sends, sidechains,
//! launcher slots, modulation targets, macro targets and controller
//! mappings pointing at nothing. [`scrub`] drops every such reference, so a
//! delete command only has to remove the object itself.

use std::collections::HashSet;
use std::sync::Arc;

use fourfloor_core::{DeviceId, TrackId};

use crate::device::DeviceInstance;
use crate::ids::SceneId;
use crate::modulation::{Macro, ModSource, Modulator};
use crate::project::Project;
use crate::track::Track;

struct Living {
    tracks: HashSet<TrackId>,
    devices: HashSet<DeviceId>,
    scenes: HashSet<SceneId>,
}

fn scrub_track(track: &Arc<Track>, living: &Living) -> Arc<Track> {
    let output = track.output.filter(|bus| living.tracks.contains(bus));
    let sends: Vec<_> = track
        .sends
        .iter()
        .filter(|send| living.tracks.contains(&send.target))
        .copied()
        .collect();
    let slots: Vec<_> = track
        .slots
        .iter()
        .filter(|slot| living.scenes.contains(&slot.scene))
        .cloned()
        .collect();
    let devices: Vec<Arc<DeviceInstance>> = track
        .devices
        .iter()
        .map(|device| match device.sidechain {
            Some(source) if !living.tracks.contains(&source.track) => Arc::new(DeviceInstance {
                sidechain: None,
                ..DeviceInstance::clone(device)
            }),
            _ => Arc::clone(device),
        })
        .collect();
    let automation: Vec<_> = track
        .automation
        .iter()
        .filter(|lane| living.devices.contains(&lane.device))
        .cloned()
        .collect();
    let unchanged = output == track.output
        && sends.len() == track.sends.len()
        && slots.len() == track.slots.len()
        && automation.len() == track.automation.len()
        && devices
            .iter()
            .zip(&track.devices)
            .all(|(new, old)| Arc::ptr_eq(new, old));
    if unchanged {
        return Arc::clone(track);
    }
    Arc::new(Track {
        output,
        sends,
        slots,
        devices,
        automation,
        ..Track::clone(track)
    })
}

fn scrub_modulator(modulator: &Arc<Modulator>, living: &Living) -> Option<Arc<Modulator>> {
    if let ModSource::Envelope { trigger, .. } = modulator.source {
        if !living.tracks.contains(&trigger) {
            return None;
        }
    }
    let targets: Vec<_> = modulator
        .targets
        .iter()
        .filter(|target| living.devices.contains(&target.device))
        .copied()
        .collect();
    Some(if targets.len() == modulator.targets.len() {
        Arc::clone(modulator)
    } else {
        Arc::new(Modulator {
            targets,
            ..Modulator::clone(modulator)
        })
    })
}

fn scrub_macro(control: &Arc<Macro>, living: &Living) -> Arc<Macro> {
    let targets: Vec<_> = control
        .targets
        .iter()
        .filter(|target| living.devices.contains(&target.device))
        .copied()
        .collect();
    if targets.len() == control.targets.len() {
        Arc::clone(control)
    } else {
        Arc::new(Macro {
            targets,
            ..Macro::clone(control)
        })
    }
}

/// A copy of `project` with every dangling reference removed. Tracks,
/// modulators and macros that needed no change are shared with the input.
pub(crate) fn scrub(project: &Project) -> Project {
    let living = Living {
        tracks: project.tracks.iter().map(|track| track.id).collect(),
        devices: project
            .tracks
            .iter()
            .flat_map(|track| track.devices.iter().map(|device| device.id))
            .collect(),
        scenes: project.scenes.iter().map(|scene| scene.id).collect(),
    };
    Project {
        tracks: project
            .tracks
            .iter()
            .map(|track| scrub_track(track, &living))
            .collect(),
        modulators: project
            .modulators
            .iter()
            .filter_map(|modulator| scrub_modulator(modulator, &living))
            .collect(),
        macros: project
            .macros
            .iter()
            .map(|control| scrub_macro(control, &living))
            .collect(),
        controller_map: project
            .controller_map
            .iter()
            .filter(|mapping| living.devices.contains(&mapping.device))
            .copied()
            .collect(),
        ..project.clone()
    }
}
