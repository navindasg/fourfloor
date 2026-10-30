//! Typed commands: the only way to change a project.
//!
//! Each command is a small serde struct with a stable snake_case name.
//! On the wire a command is `{"name": "<name>", "args": { ... }}`, which is
//! the shape of an MCP tool call, so a tool layer can map one tool to one
//! command. [`command_schemas`] lists every name with the JSON Schema of its
//! arguments.
//!
//! Applying a command never mutates the project it is given. It returns a
//! new [`Project`] that shares the untouched tracks with the old one.

mod arrange;
mod clip;
mod device;
mod global;
mod mixer;
mod modulation;
mod pianoroll;
mod session;
mod track;

use fourfloor_core::{AudioMessage, ClipId, DeviceId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::{MacroId, ModulatorId, SceneId};
use crate::project::Project;

pub use arrange::{
    DuplicateClip, ResizeClip, SetClipFades, SetClipGain, SetClipLoop, SetClipWarp, SplitClip,
};
pub use clip::{AddAudioClip, DeleteClip, MoveClip, WriteMidiClip};
pub use device::{
    AddDevice, RemoveDevice, SetDeviceBypass, SetDeviceState, SetParam, WriteAutomation,
};
pub use global::{RenameProject, SetSwing, SetTempo};
pub use mixer::{RemoveSend, SetDeviceSidechain, SetSend, WriteTrackAutomation};
pub use modulation::{
    AddMacro, AddModulator, MapController, RemoveMacro, RemoveMacroTarget, RemoveModulation,
    RemoveModulator, SetMacro, SetMacroTarget, SetModulation, SetModulatorSource, UnmapController,
};
pub use pianoroll::{
    ApplyGroove, GrooveSource, QuantizeClip, SnapClipToScale, StampChord, TransposeClip,
    WriteStepPattern, MAX_LANE_STEPS, MAX_STEP_LANES,
};
pub use session::{
    AddScene, ClearSlot, ClipToSlot, RemoveScene, RenameScene, SetLaunchQuantize, WriteMidiSlot,
};
pub use track::{
    CreateTrack, DeleteTrack, RenameTrack, SetTrackGain, SetTrackMute, SetTrackOutput, SetTrackPan,
    SetTrackSolo,
};

/// What a successful command produced, beyond the new project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// The change was applied; there is nothing to report.
    Done,
    /// A track was created.
    TrackCreated {
        /// Id of the new track.
        track: TrackId,
    },
    /// A device was added.
    DeviceAdded {
        /// Id of the new device.
        device: DeviceId,
    },
    /// A clip was created or its contents replaced.
    ClipWritten {
        /// Id of the clip.
        clip: ClipId,
        /// `true` if the clip is new, `false` if an existing one was replaced.
        created: bool,
    },
    /// A scene was added.
    SceneAdded {
        /// Id of the new scene.
        scene: SceneId,
    },
    /// A modulator was added.
    ModulatorAdded {
        /// Id of the new modulator.
        modulator: ModulatorId,
    },
    /// A macro was added.
    MacroAdded {
        /// Id of the new macro.
        #[serde(rename = "macro")]
        macro_id: MacroId,
    },
}

impl Outcome {
    /// The new track's id, if the command created a track.
    pub fn track(self) -> Option<TrackId> {
        match self {
            Self::TrackCreated { track } => Some(track),
            _ => None,
        }
    }

    /// The new device's id, if the command added a device.
    pub fn device(self) -> Option<DeviceId> {
        match self {
            Self::DeviceAdded { device } => Some(device),
            _ => None,
        }
    }

    /// The clip's id, if the command wrote a clip.
    pub fn clip(self) -> Option<ClipId> {
        match self {
            Self::ClipWritten { clip, .. } => Some(clip),
            _ => None,
        }
    }

    /// The new scene's id, if the command added a scene.
    pub fn scene(self) -> Option<SceneId> {
        match self {
            Self::SceneAdded { scene } => Some(scene),
            _ => None,
        }
    }

    /// The new modulator's id, if the command added a modulator.
    pub fn modulator(self) -> Option<ModulatorId> {
        match self {
            Self::ModulatorAdded { modulator } => Some(modulator),
            _ => None,
        }
    }

    /// The new macro's id, if the command added a macro.
    pub fn macro_id(self) -> Option<MacroId> {
        match self {
            Self::MacroAdded { macro_id } => Some(macro_id),
            _ => None,
        }
    }
}

/// The result of applying a command: the new project and what was produced.
pub type Applied = (Project, Outcome);

/// Implemented by every command's argument struct.
pub(crate) trait Apply {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError>;
}

/// A command's stable name with the JSON Schema of its arguments.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommandSchema {
    /// The stable snake_case name, as used in the `name` field.
    pub name: &'static str,
    /// One-line description taken from the argument struct's documentation.
    pub description: String,
    /// JSON Schema (draft 2020-12) of the `args` object.
    pub args_schema: serde_json::Value,
}

macro_rules! commands {
    ($( $variant:ident => $name:literal ),* $(,)?) => {
        /// Every edit the model supports.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "name", content = "args")]
        pub enum Command {
            $(
                #[doc = concat!("The `", $name, "` command.")]
                #[serde(rename = $name)]
                $variant($variant),
            )*
        }

        impl Command {
            /// Every command name, in a fixed order.
            pub const NAMES: &'static [&'static str] = &[$($name),*];

            /// This command's stable name.
            pub fn name(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => $name, )*
                }
            }

            /// Applies the command to `project` and returns the new project.
            ///
            /// `project` is left untouched. On error nothing has changed.
            pub fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
                match self {
                    $( Self::$variant(args) => args.apply(project), )*
                }
            }
        }

        $(
            impl From<$variant> for Command {
                fn from(args: $variant) -> Self {
                    Self::$variant(args)
                }
            }
        )*

        /// Every command name with the JSON Schema of its arguments, in the
        /// order of [`Command::NAMES`].
        pub fn command_schemas() -> Vec<CommandSchema> {
            vec![$( schema_entry($name, schemars::schema_for!($variant)) ),*]
        }
    };
}

commands! {
    CreateTrack => "create_track",
    DeleteTrack => "delete_track",
    RenameTrack => "rename_track",
    SetTrackGain => "set_track_gain",
    SetTrackPan => "set_track_pan",
    SetTrackMute => "set_track_mute",
    SetTrackSolo => "set_track_solo",
    SetTrackOutput => "set_track_output",
    AddDevice => "add_device",
    RemoveDevice => "remove_device",
    SetParam => "set_param",
    SetDeviceBypass => "set_device_bypass",
    SetDeviceState => "set_device_state",
    WriteAutomation => "write_automation",
    WriteMidiClip => "write_midi_clip",
    AddAudioClip => "add_audio_clip",
    MoveClip => "move_clip",
    DeleteClip => "delete_clip",
    SetTempo => "set_tempo",
    SetSwing => "set_swing",
    RenameProject => "rename_project",
    SetSend => "set_send",
    RemoveSend => "remove_send",
    SetDeviceSidechain => "set_device_sidechain",
    WriteTrackAutomation => "write_track_automation",
    SetClipGain => "set_clip_gain",
    SetClipFades => "set_clip_fades",
    SetClipWarp => "set_clip_warp",
    SplitClip => "split_clip",
    DuplicateClip => "duplicate_clip",
    SetClipLoop => "set_clip_loop",
    ResizeClip => "resize_clip",
    AddScene => "add_scene",
    RemoveScene => "remove_scene",
    RenameScene => "rename_scene",
    WriteMidiSlot => "write_midi_slot",
    ClipToSlot => "clip_to_slot",
    ClearSlot => "clear_slot",
    SetLaunchQuantize => "set_launch_quantize",
    WriteStepPattern => "write_step_pattern",
    QuantizeClip => "quantize_clip",
    TransposeClip => "transpose_clip",
    SnapClipToScale => "snap_clip_to_scale",
    StampChord => "stamp_chord",
    ApplyGroove => "apply_groove",
    AddModulator => "add_modulator",
    RemoveModulator => "remove_modulator",
    SetModulatorSource => "set_modulator_source",
    SetModulation => "set_modulation",
    RemoveModulation => "remove_modulation",
    AddMacro => "add_macro",
    RemoveMacro => "remove_macro",
    SetMacroTarget => "set_macro_target",
    RemoveMacroTarget => "remove_macro_target",
    SetMacro => "set_macro",
    MapController => "map_controller",
    UnmapController => "unmap_controller",
}

fn schema_entry(name: &'static str, schema: schemars::Schema) -> CommandSchema {
    let args_schema = schema.to_value();
    let description = args_schema
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    CommandSchema {
        name,
        description,
        args_schema,
    }
}

impl Command {
    /// Builds a command from a name and a JSON arguments object, as an MCP
    /// tool call supplies them. Unknown names, unknown fields and wrongly
    /// typed fields are all rejected.
    pub fn from_name_and_args(name: &str, args: serde_json::Value) -> Result<Self, ModelError> {
        let wire = serde_json::json!({ "name": name, "args": args });
        serde_json::from_value(wire).map_err(|error| ModelError::Decode {
            name: name.to_owned(),
            reason: error.to_string(),
        })
    }

    /// The audio-thread message that carries this change, if one exists.
    ///
    /// Parameter and tempo changes can go straight to the audio thread.
    /// Every other command returns `None`: the engine has to build a new
    /// graph or new clip data from the project instead.
    pub fn audio_message(&self) -> Option<AudioMessage> {
        match self {
            Self::SetParam(args) => Some(AudioMessage::SetParam {
                device: args.device,
                param: args.param,
                value: args.value,
            }),
            Self::SetTempo(args) => fourfloor_core::Tempo::new(args.bpm)
                .ok()
                .map(|tempo| AudioMessage::SetTempo { tempo }),
            _ => None,
        }
    }

    /// Every audio-thread message this change needs, given the project as
    /// the command left it.
    ///
    /// Like [`Command::audio_message`], plus the commands that move several
    /// parameters at once: `set_macro` and `set_macro_target` yield one
    /// `SetParam` per parameter the macro drives. Commands that need a new
    /// graph return an empty list.
    pub fn audio_messages(&self, after: &Project) -> Vec<AudioMessage> {
        let macro_messages = |id: MacroId| -> Vec<AudioMessage> {
            let Some(control) = after.macro_control(id) else {
                return Vec::new();
            };
            control
                .targets
                .iter()
                .map(|target| AudioMessage::SetParam {
                    device: target.device,
                    param: target.param,
                    value: target.value_at(control.value),
                })
                .collect()
        };
        match self {
            Self::SetMacro(args) => macro_messages(args.macro_id),
            Self::SetMacroTarget(args) => macro_messages(args.macro_id),
            other => other.audio_message().into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests;
