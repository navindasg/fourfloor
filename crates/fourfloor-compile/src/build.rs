//! Building devices through the registry: one track's chain at a time.

use std::collections::HashMap;

use fourfloor_core::{Device, DeviceCategory, DeviceId, DeviceInit, DeviceRegistry, ParamId};
use fourfloor_model::{
    DeviceInstance, PluginStateStore, Project, SidechainSource, Track, TrackKind,
};

use crate::options::{CompileError, CompileOptions};

/// A device ready to become a graph node.
pub(crate) struct Built {
    pub(crate) id: DeviceId,
    pub(crate) device: Box<dyn Device>,
}

/// A track's chain, built and sorted by role.
#[derive(Default)]
pub(crate) struct Chain {
    pub(crate) note_effects: Vec<Built>,
    pub(crate) instrument: Option<Built>,
    pub(crate) effects: Vec<Built>,
    /// Sidechain requests of the devices above that can take one.
    pub(crate) sidechains: Vec<(DeviceId, SidechainSource)>,
}

/// A parameter of a built device: where it sits and how far it may move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParamRange {
    pub(crate) base: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
}

pub(crate) struct Compiler<'a> {
    pub(crate) project: &'a Project,
    pub(crate) registry: &'a DeviceRegistry,
    pub(crate) options: &'a CompileOptions,
    pub(crate) next_id: u64,
    pub(crate) warnings: Vec<String>,
    /// Every parameter of every chain device that was built.
    pub(crate) ranges: HashMap<(DeviceId, ParamId), ParamRange>,
}

impl<'a> Compiler<'a> {
    pub(crate) fn new(
        project: &'a Project,
        registry: &'a DeviceRegistry,
        options: &'a CompileOptions,
    ) -> Self {
        Self {
            project,
            registry,
            options,
            next_id: project.next_id,
            warnings: Vec::new(),
            ranges: HashMap::new(),
        }
    }

    /// Graph nodes the model does not know take ids above every id the
    /// project has handed out, so they cannot collide with device ids.
    pub(crate) fn fresh_id(&mut self) -> Result<DeviceId, CompileError> {
        let id = DeviceId::new(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(CompileError::IdExhausted)?;
        Ok(id)
    }

    pub(crate) fn warn(&mut self, track: &Track, message: impl std::fmt::Display) {
        self.warnings
            .push(format!("track \"{}\": {message}", track.name));
    }

    fn load_state(&mut self, track: &Track, instance: &DeviceInstance) -> Option<Vec<u8>> {
        let hash = instance.state_blob.as_ref()?;
        let Some(dir) = &self.options.project_dir else {
            self.warn(
                track,
                format_args!(
                    "device \"{}\" has saved state but no project folder was given; \
                     it starts from its parameters",
                    instance.name
                ),
            );
            return None;
        };
        match PluginStateStore::new(dir).get(hash) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                self.warn(
                    track,
                    format_args!(
                        "cannot load the state of device \"{}\": {error}",
                        instance.name
                    ),
                );
                None
            }
        }
    }

    /// The values a device starts with: its saved parameters, with every
    /// parameter a macro drives set from the macro's position.
    fn start_values(&self, instance: &DeviceInstance) -> Vec<(ParamId, f32)> {
        let mut values: Vec<(ParamId, f32)> = instance
            .params
            .iter()
            .map(|param| (param.id, param.value))
            .collect();
        for control in &self.project.macros {
            for target in control.targets.iter().filter(|t| t.device == instance.id) {
                let value = target.value_at(control.value);
                match values.iter_mut().find(|(id, _)| *id == target.param) {
                    Some(entry) => entry.1 = value,
                    None => values.push((target.param, value)),
                }
            }
        }
        values
    }

    /// Builds one chain entry through the registry. `None` (with a
    /// warning) when its kind is unknown or its factory fails.
    fn build(
        &mut self,
        track: &Track,
        instance: &DeviceInstance,
    ) -> Option<(DeviceCategory, bool, Built)> {
        let Some(descriptor) = self.registry.descriptor(&instance.kind) else {
            self.warn(
                track,
                format_args!(
                    "device kind \"{}\" is not registered; \"{}\" is left out",
                    instance.kind, instance.name
                ),
            );
            return None;
        };
        let (category, sidechain) = (descriptor.category, descriptor.sidechain);
        let values = self.start_values(instance);
        let state = self.load_state(track, instance);
        let init = DeviceInit::new().with_params(&values);
        let init = match &state {
            Some(bytes) => init.with_state(bytes),
            None => init,
        };
        let device = match self.registry.create(&instance.kind, &init) {
            Ok(device) => device,
            Err(error) => {
                self.warn(track, format_args!("{error}; it is left out"));
                return None;
            }
        };
        // A parameter's range is the one the project saved with it (the
        // range edits were checked against); parameters the project does
        // not list take the registry's.
        let start = |id: ParamId, fallback: f32| {
            values
                .iter()
                .find(|(own, _)| *own == id)
                .map_or(fallback, |(_, value)| *value)
        };
        for param in &instance.params {
            self.ranges.insert(
                (instance.id, param.id),
                ParamRange {
                    base: start(param.id, param.value),
                    min: param.min,
                    max: param.max,
                },
            );
        }
        for info in &descriptor.params {
            self.ranges
                .entry((instance.id, info.id))
                .or_insert(ParamRange {
                    base: start(info.id, info.default),
                    min: info.min,
                    max: info.max,
                });
        }
        Some((
            category,
            sidechain,
            Built {
                id: instance.id,
                device,
            },
        ))
    }

    /// Builds the devices of a track's chain that can sit on that kind of
    /// track; the rest are reported.
    pub(crate) fn chain(&mut self, track: &Track) -> Chain {
        let mut chain = Chain::default();
        for instance in track.devices.iter().filter(|device| !device.bypassed) {
            let Some((category, takes_sidechain, built)) = self.build(track, instance) else {
                continue;
            };
            let id = built.id;
            let midi = track.kind == TrackKind::Midi;
            let placed = match category {
                DeviceCategory::Effect => {
                    chain.effects.push(built);
                    true
                }
                DeviceCategory::NoteEffect if midi => {
                    chain.note_effects.push(built);
                    true
                }
                DeviceCategory::Instrument if midi && chain.instrument.is_none() => {
                    chain.instrument = Some(built);
                    true
                }
                DeviceCategory::Instrument if midi => {
                    self.warn(
                        track,
                        format_args!(
                            "\"{}\" is a second instrument; only the first plays",
                            instance.name
                        ),
                    );
                    false
                }
                other => {
                    self.warn(
                        track,
                        format_args!(
                            "\"{}\" is a {other} and cannot sit on a {} track; it is left out",
                            instance.name, track.kind
                        ),
                    );
                    false
                }
            };
            if !placed {
                self.ranges.retain(|(device, _), _| *device != id);
                continue;
            }
            match instance.sidechain {
                Some(source) if takes_sidechain => chain.sidechains.push((id, source)),
                Some(_) => self.warn(
                    track,
                    format_args!(
                        "\"{}\" has no sidechain input; its sidechain routing is ignored",
                        instance.name
                    ),
                ),
                None => {}
            }
        }
        chain
    }

    /// The instrument for a MIDI track whose chain has none.
    pub(crate) fn default_instrument(
        &mut self,
        track: &Track,
    ) -> Result<Option<Built>, CompileError> {
        let Some(kind) = self.options.default_instrument.as_deref() else {
            self.warn(track, "has no instrument; it is silent");
            return Ok(None);
        };
        let is_instrument = self
            .registry
            .descriptor(kind)
            .is_some_and(|descriptor| descriptor.category == DeviceCategory::Instrument);
        let device = match self.registry.create_default(kind) {
            Ok(device) if is_instrument => device,
            _ => {
                self.warn(
                    track,
                    format_args!(
                        "has no instrument and the default \"{kind}\" is not a registered \
                         instrument; it is silent"
                    ),
                );
                return Ok(None);
            }
        };
        Ok(Some(Built {
            id: self.fresh_id()?,
            device,
        }))
    }
}
