//! Modulator, macro and MIDI controller mapping commands.

use std::sync::Arc;

use fourfloor_core::{DeviceId, ParamId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome};
use crate::checks;
use crate::device::{DeviceInstance, Param};
use crate::error::ModelError;
use crate::ids::{MacroId, ModulatorId};
use crate::modulation::{ControllerMapping, Macro, MacroTarget, ModSource, ModTarget, Modulator};
use crate::project::Project;
use crate::validate;

fn map_modulator(
    project: &Project,
    id: ModulatorId,
    update: impl Fn(&Modulator) -> Result<Modulator, ModelError>,
) -> Result<Project, ModelError> {
    if project.modulator(id).is_none() {
        return Err(ModelError::ModulatorNotFound(id));
    }
    let modulators = project
        .modulators
        .iter()
        .map(|modulator| {
            if modulator.id == id {
                update(modulator).map(Arc::new)
            } else {
                Ok(Arc::clone(modulator))
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(Project {
        modulators,
        ..project.clone()
    })
}

fn map_macro(
    project: &Project,
    id: MacroId,
    update: impl Fn(&Macro) -> Result<Macro, ModelError>,
) -> Result<Project, ModelError> {
    if project.macro_control(id).is_none() {
        return Err(ModelError::MacroNotFound(id));
    }
    let macros = project
        .macros
        .iter()
        .map(|control| {
            if control.id == id {
                update(control).map(Arc::new)
            } else {
                Ok(Arc::clone(control))
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(Project {
        macros,
        ..project.clone()
    })
}

/// A copy of the project in which one parameter has a new value.
fn with_param_value(
    project: &Project,
    device: DeviceId,
    param: ParamId,
    value: f32,
) -> Result<Project, ModelError> {
    project.map_device(device, |instance| {
        let current = instance
            .param(param)
            .ok_or(ModelError::ParamNotFound { device, param })?;
        validate::param_value(device, current, value)?;
        Ok(DeviceInstance {
            params: instance
                .params
                .iter()
                .map(|existing| Param {
                    value: if existing.id == param {
                        value
                    } else {
                        existing.value
                    },
                    ..existing.clone()
                })
                .collect(),
            ..instance.clone()
        })
    })
}

/// Add a modulator: an LFO or an envelope. Assign it to parameters with
/// `set_modulation`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddModulator {
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// What the modulator produces.
    pub source: ModSource,
}

impl Apply for AddModulator {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("modulator name", &self.name)?;
        checks::mod_source(project, &self.source)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = ModulatorId::new(raw_id);
        let created = Arc::new(Modulator {
            id,
            name: self.name.clone(),
            source: self.source,
            targets: Vec::new(),
        });
        let modulators = project
            .modulators
            .iter()
            .cloned()
            .chain(std::iter::once(created))
            .collect();
        Ok((
            Project {
                modulators,
                next_id,
                ..project.clone()
            },
            Outcome::ModulatorAdded { modulator: id },
        ))
    }
}

/// Remove a modulator and everything it was assigned to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveModulator {
    /// The modulator to remove.
    pub modulator: ModulatorId,
}

impl Apply for RemoveModulator {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if project.modulator(self.modulator).is_none() {
            return Err(ModelError::ModulatorNotFound(self.modulator));
        }
        let modulators = project
            .modulators
            .iter()
            .filter(|modulator| modulator.id != self.modulator)
            .cloned()
            .collect();
        Ok((
            Project {
                modulators,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}

/// Change what a modulator produces, keeping its assignments.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetModulatorSource {
    /// The modulator to change.
    pub modulator: ModulatorId,
    /// The new LFO or envelope settings.
    pub source: ModSource,
}

impl Apply for SetModulatorSource {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        checks::mod_source(project, &self.source)?;
        let updated = map_modulator(project, self.modulator, |modulator| {
            Ok(Modulator {
                source: self.source,
                ..modulator.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Assign a modulator to a parameter, or change how far it moves it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetModulation {
    /// The modulator.
    pub modulator: ModulatorId,
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to move.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// How far the parameter moves, as a fraction of its range: -1 to 1.
    pub depth: f32,
}

impl Apply for SetModulation {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        checks::param_ref(project, self.device, self.param)?;
        checks::depth(self.depth)?;
        let entry = ModTarget {
            device: self.device,
            param: self.param,
            depth: self.depth,
        };
        let updated = map_modulator(project, self.modulator, |modulator| {
            let replaced = modulator.target(self.device, self.param).is_some();
            if !replaced && modulator.targets.len() >= checks::MAX_TARGETS {
                return Err(ModelError::InvalidValue {
                    field: "targets",
                    reason: format!("at most {} targets are allowed", checks::MAX_TARGETS),
                });
            }
            Ok(Modulator {
                targets: modulator
                    .targets
                    .iter()
                    .map(|target| {
                        if target.device == self.device && target.param == self.param {
                            entry
                        } else {
                            *target
                        }
                    })
                    .chain((!replaced).then_some(entry))
                    .collect(),
                ..modulator.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Stop a modulator moving a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveModulation {
    /// The modulator.
    pub modulator: ModulatorId,
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to release.
    #[schemars(with = "u32")]
    pub param: ParamId,
}

impl Apply for RemoveModulation {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = map_modulator(project, self.modulator, |modulator| {
            if modulator.target(self.device, self.param).is_none() {
                return Err(ModelError::NotFound {
                    what: format!(
                        "modulation of parameter {} of device {}",
                        self.param, self.device
                    ),
                });
            }
            Ok(Modulator {
                targets: modulator
                    .targets
                    .iter()
                    .filter(|target| !(target.device == self.device && target.param == self.param))
                    .copied()
                    .collect(),
                ..modulator.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Add a macro: one control that sets several parameters. Give it
/// parameters with `set_macro_target` and move it with `set_macro`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddMacro {
    /// Display name: 1 to 128 characters.
    pub name: String,
    /// Starting position, 0 to 1. Defaults to 0.
    #[serde(default)]
    pub value: f32,
}

impl Apply for AddMacro {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("macro name", &self.name)?;
        checks::unit("value", self.value)?;
        let (raw_id, next_id) = project.reserve_id()?;
        let id = MacroId::new(raw_id);
        let created = Arc::new(Macro {
            id,
            name: self.name.clone(),
            value: self.value,
            targets: Vec::new(),
        });
        let macros = project
            .macros
            .iter()
            .cloned()
            .chain(std::iter::once(created))
            .collect();
        Ok((
            Project {
                macros,
                next_id,
                ..project.clone()
            },
            Outcome::MacroAdded { macro_id: id },
        ))
    }
}

/// Remove a macro. The parameters it drove keep their current values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveMacro {
    /// The macro to remove.
    #[serde(rename = "macro")]
    pub macro_id: MacroId,
}

impl Apply for RemoveMacro {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        if project.macro_control(self.macro_id).is_none() {
            return Err(ModelError::MacroNotFound(self.macro_id));
        }
        let macros = project
            .macros
            .iter()
            .filter(|control| control.id != self.macro_id)
            .cloned()
            .collect();
        Ok((
            Project {
                macros,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}

/// Make a macro drive a parameter, or change the range it sweeps. The
/// parameter is set at once to the value for the macro's current position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMacroTarget {
    /// The macro.
    #[serde(rename = "macro")]
    pub macro_id: MacroId,
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to drive.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// The parameter's value when the macro is at 0.
    pub min: f32,
    /// The parameter's value when the macro is at 1. May be below `min`.
    pub max: f32,
}

impl Apply for SetMacroTarget {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let entry = MacroTarget {
            device: self.device,
            param: self.param,
            min: self.min,
            max: self.max,
        };
        checks::macro_target(project, &entry)?;
        let with_target = map_macro(project, self.macro_id, |control| {
            let replaced = control.target(self.device, self.param).is_some();
            if !replaced && control.targets.len() >= checks::MAX_TARGETS {
                return Err(ModelError::InvalidValue {
                    field: "targets",
                    reason: format!("at most {} targets are allowed", checks::MAX_TARGETS),
                });
            }
            Ok(Macro {
                targets: control
                    .targets
                    .iter()
                    .map(|target| {
                        if target.device == self.device && target.param == self.param {
                            entry
                        } else {
                            *target
                        }
                    })
                    .chain((!replaced).then_some(entry))
                    .collect(),
                ..control.clone()
            })
        })?;
        let position = with_target
            .macro_control(self.macro_id)
            .map_or(0.0, |control| control.value);
        let updated = with_param_value(
            &with_target,
            self.device,
            self.param,
            entry.value_at(position),
        )?;
        Ok((updated, Outcome::Done))
    }
}

/// Stop a macro driving a parameter. The parameter keeps its value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveMacroTarget {
    /// The macro.
    #[serde(rename = "macro")]
    pub macro_id: MacroId,
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to release.
    #[schemars(with = "u32")]
    pub param: ParamId,
}

impl Apply for RemoveMacroTarget {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let updated = map_macro(project, self.macro_id, |control| {
            if control.target(self.device, self.param).is_none() {
                return Err(ModelError::NotFound {
                    what: format!(
                        "macro target on parameter {} of device {}",
                        self.param, self.device
                    ),
                });
            }
            Ok(Macro {
                targets: control
                    .targets
                    .iter()
                    .filter(|target| !(target.device == self.device && target.param == self.param))
                    .copied()
                    .collect(),
                ..control.clone()
            })
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Move a macro. Every parameter it drives is set to the matching value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMacro {
    /// The macro to move.
    #[serde(rename = "macro")]
    pub macro_id: MacroId,
    /// The new position, 0 to 1.
    pub value: f32,
}

impl Apply for SetMacro {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        checks::unit("value", self.value)?;
        let moved = map_macro(project, self.macro_id, |control| {
            Ok(Macro {
                value: self.value,
                ..control.clone()
            })
        })?;
        let targets = moved
            .macro_control(self.macro_id)
            .map(|control| control.targets.clone())
            .unwrap_or_default();
        let updated = targets.iter().try_fold(moved, |project, target| {
            with_param_value(
                &project,
                target.device,
                target.param,
                target.value_at(self.value),
            )
        })?;
        Ok((updated, Outcome::Done))
    }
}

/// Bind a MIDI controller to a parameter, replacing any earlier binding of
/// the same controller. This is what MIDI learn stores.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapController {
    /// MIDI channel 0 to 15. Omit or pass null to accept any channel.
    #[serde(default)]
    pub channel: Option<u8>,
    /// MIDI controller number, 0 to 127.
    pub controller: u8,
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter to control.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// Parameter value at controller value 0. Omit for the parameter's
    /// minimum.
    #[serde(default)]
    pub min: Option<f32>,
    /// Parameter value at controller value 127. Omit for the parameter's
    /// maximum.
    #[serde(default)]
    pub max: Option<f32>,
}

impl Apply for MapController {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let param = checks::param_ref(project, self.device, self.param)?;
        let mapping = ControllerMapping {
            channel: self.channel,
            controller: self.controller,
            device: self.device,
            param: self.param,
            min: self.min.unwrap_or(param.min),
            max: self.max.unwrap_or(param.max),
        };
        checks::controller(project, &mapping)?;
        let controller_map = project
            .controller_map
            .iter()
            .filter(|existing| {
                !(existing.channel == self.channel && existing.controller == self.controller)
            })
            .copied()
            .chain(std::iter::once(mapping))
            .collect();
        Ok((
            Project {
                controller_map,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}

/// Remove a MIDI controller binding.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnmapController {
    /// The channel the binding was made with (null for any channel).
    #[serde(default)]
    pub channel: Option<u8>,
    /// MIDI controller number, 0 to 127.
    pub controller: u8,
}

impl Apply for UnmapController {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let bound = |mapping: &ControllerMapping| {
            mapping.channel == self.channel && mapping.controller == self.controller
        };
        if !project.controller_map.iter().any(bound) {
            return Err(ModelError::NotFound {
                what: format!("a mapping for controller {}", self.controller),
            });
        }
        Ok((
            Project {
                controller_map: project
                    .controller_map
                    .iter()
                    .filter(|mapping| !bound(mapping))
                    .copied()
                    .collect(),
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}
