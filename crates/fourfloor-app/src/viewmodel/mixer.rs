//! Mixer logic: the fader law, send targets, adding devices from the
//! registry, macros and modulation assignment.

use fourfloor_core::{
    DeviceCategory, DeviceDescriptor, DeviceId, DeviceRegistry, ParamId, TrackId,
};
use fourfloor_model::command::{AddDevice, AddModulator, SetMacroTarget, SetModulation};
use fourfloor_model::{LfoRate, LfoShape, ModSource, Project, TrackKind};

use crate::services::registry::device_params;

/// Fader bottom, treated as silence.
pub const FADER_FLOOR_DB: f32 = -60.0;
/// Fader top.
pub const FADER_TOP_DB: f32 = 6.0;
/// The model's lowest gain.
pub const MIN_GAIN_DB: f32 = -96.0;

/// Fader position (0..1) for a gain in dB. A square-root law gives the
/// top 20 dB most of the travel; 0 dB sits at about 83 %.
pub fn fader_position(gain_db: f32) -> f32 {
    if gain_db <= FADER_FLOOR_DB {
        return 0.0;
    }
    let t = (gain_db - FADER_FLOOR_DB) / (FADER_TOP_DB - FADER_FLOOR_DB);
    t.clamp(0.0, 1.0).powi(2)
}

/// Gain in dB for a fader position; the bottom is [`MIN_GAIN_DB`].
pub fn fader_gain(position: f32) -> f32 {
    if position <= 0.0 {
        return MIN_GAIN_DB;
    }
    let t = position.clamp(0.0, 1.0).sqrt();
    FADER_FLOOR_DB + t * (FADER_TOP_DB - FADER_FLOOR_DB)
}

/// Formats a gain for a strip label.
pub fn gain_label(gain_db: f32) -> String {
    if gain_db <= FADER_FLOOR_DB {
        "-inf".to_owned()
    } else {
        format!("{gain_db:+.1}")
    }
}

/// Bus tracks `track` may send to: every bus except itself and buses that
/// (directly or through other buses) feed it, which would loop.
pub fn send_targets(project: &Project, track: TrackId) -> Vec<TrackId> {
    project
        .tracks
        .iter()
        .filter(|candidate| candidate.kind == TrackKind::Bus && candidate.id != track)
        .filter(|candidate| !feeds(project, candidate.id, track))
        .map(|candidate| candidate.id)
        .collect()
}

/// `true` if `from` reaches `to` through outputs or sends.
pub fn feeds(project: &Project, from: TrackId, to: TrackId) -> bool {
    let mut stack = vec![from];
    let mut seen = Vec::new();
    while let Some(current) = stack.pop() {
        if current == to {
            return true;
        }
        if seen.contains(&current) {
            continue;
        }
        seen.push(current);
        if let Some(track) = project.track(current) {
            stack.extend(track.output);
            stack.extend(track.sends.iter().map(|send| send.target));
        }
    }
    false
}

/// Registry devices grouped for the browser: instruments, note effects,
/// effects (each sorted by name).
pub fn browser_groups(registry: &DeviceRegistry) -> Vec<(&'static str, Vec<&DeviceDescriptor>)> {
    let group = |category: DeviceCategory| {
        let mut list: Vec<&DeviceDescriptor> = registry
            .descriptors()
            .filter(|d| d.category == category)
            .filter(|d| !d.kind.starts_with("builtin.fader"))
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    };
    vec![
        ("Instruments", group(DeviceCategory::Instrument)),
        ("Note effects", group(DeviceCategory::NoteEffect)),
        ("Effects", group(DeviceCategory::Effect)),
    ]
}

/// The command that adds `kind` at the end of `track`'s chain, with every
/// parameter at its default. Instruments and note effects go before the
/// first effect, so the chain stays playable.
pub fn add_device(
    project: &Project,
    registry: &DeviceRegistry,
    track: TrackId,
    kind: &str,
) -> Option<AddDevice> {
    let descriptor = registry.descriptor(kind)?;
    let owner = project.track(track)?;
    let params = device_params(registry, kind, &[]).ok()?;
    let index = match descriptor.category {
        DeviceCategory::Effect => None,
        DeviceCategory::Instrument | DeviceCategory::NoteEffect => {
            owner.devices.iter().position(|device| {
                registry
                    .descriptor(&device.kind)
                    .is_some_and(|d| d.category == DeviceCategory::Effect)
            })
        }
    };
    Some(AddDevice {
        track,
        kind: kind.to_owned(),
        name: Some(descriptor.name.clone()),
        index,
        params,
    })
}

/// Maps a parameter onto a macro over its whole range.
pub fn macro_target(
    macro_id: fourfloor_model::MacroId,
    device: DeviceId,
    param: ParamId,
    min: f32,
    max: f32,
) -> SetMacroTarget {
    SetMacroTarget {
        macro_id,
        device,
        param,
        min,
        max,
    }
}

/// A new tempo-synced sine LFO.
pub fn new_lfo(name: &str, beats: f64) -> AddModulator {
    AddModulator {
        name: name.to_owned(),
        source: ModSource::Lfo {
            shape: LfoShape::Sine,
            rate: LfoRate::Beats(beats),
            phase: 0.0,
        },
    }
}

/// Assigns a modulator to a parameter at `depth` (a fraction of the range).
pub fn modulate(
    modulator: fourfloor_model::ModulatorId,
    device: DeviceId,
    param: ParamId,
    depth: f32,
) -> SetModulation {
    SetModulation {
        modulator,
        device,
        param,
        depth: depth.clamp(-1.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::build_demo_project;
    use crate::services::registry::build_registry;
    use fourfloor_model::CommandBus;

    #[test]
    fn the_fader_law_round_trips_and_has_unity_high_up() {
        for db in [-48.0f32, -12.0, -3.0, 0.0, 6.0] {
            assert!((fader_gain(fader_position(db)) - db).abs() < 1e-3, "{db}");
        }
        assert!((0.8..0.86).contains(&fader_position(0.0)));
        assert_eq!(fader_position(-96.0), 0.0);
        assert_eq!(fader_gain(0.0), MIN_GAIN_DB);
        assert_eq!(gain_label(-96.0), "-inf");
        assert_eq!(gain_label(-3.0), "-3.0");
    }

    #[test]
    fn sends_never_loop_and_devices_land_in_a_playable_place() {
        let registry = build_registry(false).expect("registry");
        let project = build_demo_project(&registry).expect("demo");
        let id = |name: &str| {
            project
                .tracks
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.id)
                .expect(name)
        };
        let targets = send_targets(&project, id("Stabs"));
        assert!(targets.contains(&id("Reverb")));
        assert!(!targets.contains(&id("Stabs")));
        // The reverb return feeds the mix bus, so the mix bus may not
        // send back into the reverb.
        assert!(!send_targets(&project, id("Mix")).contains(&id("Reverb")));
        assert!(feeds(&project, id("Kick"), id("Mix")));
        // Bass is [acid, saturator, compressor]: a note effect goes in
        // front of the first effect.
        let command = add_device(&project, &registry, id("Bass"), "note.arp").expect("arp");
        assert_eq!(command.index, Some(1));
        let mut bus = CommandBus::new(project.clone());
        bus.execute(add_device(&project, &registry, id("Bass"), "fx.chorus").expect("chorus"))
            .expect("added");
        let bass = bus.project().track(id("Bass")).expect("bass");
        assert_eq!(
            bass.devices.last().map(|d| d.kind.as_str()),
            Some("fx.chorus")
        );
        let groups = browser_groups(&registry);
        assert!(groups[0].1.iter().any(|d| d.kind == "drum.kick909"));
        assert!(groups[2].1.iter().any(|d| d.kind == "fx.reverb"));
    }
}
