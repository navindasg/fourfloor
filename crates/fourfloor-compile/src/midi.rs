//! Glue between the project's controller mappings and the engine's MIDI
//! handling.

use fourfloor_core::DeviceId;
use fourfloor_engine::midi::{ControllerBinding, ControllerMap, MidiRouter};
use fourfloor_model::command::MapController;
use fourfloor_model::Project;

/// The project's controller mappings in the form the engine's MIDI input
/// uses.
pub fn controller_map(project: &Project) -> ControllerMap {
    ControllerMap::new(
        project
            .controller_map
            .iter()
            .map(|mapping| ControllerBinding {
                channel: mapping.channel,
                controller: mapping.controller,
                device: mapping.device,
                param: mapping.param,
                min: mapping.min,
                max: mapping.max,
            })
            .collect(),
    )
}

/// A router for incoming MIDI: the project's mapped controllers move
/// their parameters, and everything else plays `instrument` (a compiled
/// track's instrument node).
pub fn midi_router(project: &Project, instrument: Option<DeviceId>) -> MidiRouter {
    MidiRouter::new(controller_map(project), instrument)
}

/// The command that stores a binding MIDI learn produced, so the mapping
/// is saved with the project and can be undone.
pub fn learned_command(binding: &ControllerBinding) -> MapController {
    MapController {
        channel: binding.channel,
        controller: binding.controller,
        device: binding.device,
        param: binding.param,
        min: Some(binding.min),
        max: Some(binding.max),
    }
}
