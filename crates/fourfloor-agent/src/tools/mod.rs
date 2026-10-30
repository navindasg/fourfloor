//! The tool catalog: one tool per model command, generated from
//! [`fourfloor_model::command_schemas`], plus higher-level tools that write
//! whole parts, route sidechains, render and report.
//!
//! Every editing tool runs through the agent's command bus and is one undo
//! step. Tools that remove or replace content carry `destructive: true`
//! (the MCP `destructiveHint`).

pub mod args;
mod edit;
mod mix;
mod music;
pub(crate) mod output;

use fourfloor_model::{command_schemas, Command};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::error::AgentError;

pub use args::TrackRef;

/// Where a tool comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Exactly one model command; the arguments are the command's.
    Command,
    /// A higher-level tool that may run several commands as one step.
    HighLevel,
    /// Reads the project or the session and changes nothing.
    Query,
}

/// A tool's name, description and argument schema.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolSpec {
    /// Stable snake_case name.
    pub name: String,
    /// Short human title.
    pub title: String,
    /// What it does.
    pub description: String,
    /// JSON Schema of the arguments object.
    pub input_schema: Value,
    /// Where it comes from.
    pub kind: ToolKind,
    /// It changes nothing in the project.
    pub read_only: bool,
    /// It can remove or replace content (still undoable for project edits;
    /// file outputs are overwritten).
    pub destructive: bool,
}

/// What a tool returned.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolOutput {
    /// One-line summary for people.
    pub text: String,
    /// Structured result.
    pub data: Value,
    /// Whether the project changed.
    pub changed: bool,
}

impl ToolOutput {
    pub(crate) fn changed(text: impl Into<String>, data: Value) -> Self {
        Self {
            text: text.into(),
            data,
            changed: true,
        }
    }

    pub(crate) fn unchanged(text: impl Into<String>, data: Value) -> Self {
        Self {
            text: text.into(),
            data,
            changed: false,
        }
    }
}

/// Model commands that remove or replace content.
pub const DESTRUCTIVE_COMMANDS: &[&str] = &[
    "delete_track",
    "remove_device",
    "delete_clip",
    "remove_send",
    "remove_scene",
    "clear_slot",
    "remove_modulator",
    "remove_modulation",
    "remove_macro",
    "remove_macro_target",
    "unmap_controller",
    "set_device_state",
    "write_midi_clip",
    "write_step_pattern",
    "write_automation",
    "write_track_automation",
    "write_midi_slot",
    "clip_to_slot",
];

/// Model commands that a high-level tool of the same name replaces. The
/// high-level `set_swing` takes the command's arguments plus a track and a
/// re-time switch.
const SUPERSEDED_COMMANDS: &[&str] = &["set_swing"];

struct HighLevel {
    name: &'static str,
    title: &'static str,
    kind: ToolKind,
    destructive: bool,
    schema: fn() -> Value,
}

fn schema_of<T: JsonSchema>() -> Value {
    schemars::schema_for!(T).to_value()
}

const HIGH_LEVEL: &[HighLevel] = &[
    HighLevel {
        name: "add_house_drum_pattern",
        title: "Add house drum pattern",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::AddHouseDrumPatternArgs>,
    },
    HighLevel {
        name: "add_bassline",
        title: "Add bassline",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::AddBasslineArgs>,
    },
    HighLevel {
        name: "add_chords",
        title: "Add chord part",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::AddChordsArgs>,
    },
    HighLevel {
        name: "set_swing",
        title: "Set swing",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::SetSwingArgs>,
    },
    HighLevel {
        name: "sidechain",
        title: "Sidechain duck",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::SidechainArgs>,
    },
    HighLevel {
        name: "add_filter_riser",
        title: "Add filter riser",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::AddFilterRiserArgs>,
    },
    HighLevel {
        name: "render",
        title: "Render to WAV",
        kind: ToolKind::Query,
        destructive: true,
        schema: schema_of::<args::RenderArgs>,
    },
    HighLevel {
        name: "export_stems",
        title: "Export stems",
        kind: ToolKind::Query,
        destructive: true,
        schema: schema_of::<args::ExportStemsArgs>,
    },
    HighLevel {
        name: "mix_report",
        title: "Mix analysis report",
        kind: ToolKind::Query,
        destructive: false,
        schema: schema_of::<args::MixReportArgs>,
    },
    HighLevel {
        name: "project_summary",
        title: "Project summary",
        kind: ToolKind::Query,
        destructive: false,
        schema: schema_of::<args::NoArgs>,
    },
    HighLevel {
        name: "list_tracks",
        title: "List tracks, devices and parameters",
        kind: ToolKind::Query,
        destructive: false,
        schema: schema_of::<args::NoArgs>,
    },
    HighLevel {
        name: "list_device_kinds",
        title: "List available devices",
        kind: ToolKind::Query,
        destructive: false,
        schema: schema_of::<args::NoArgs>,
    },
    HighLevel {
        name: "undo",
        title: "Undo",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::NoArgs>,
    },
    HighLevel {
        name: "redo",
        title: "Redo",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::NoArgs>,
    },
    HighLevel {
        name: "save_project",
        title: "Save project",
        kind: ToolKind::Query,
        destructive: false,
        schema: schema_of::<args::NoArgs>,
    },
    HighLevel {
        name: "instruct",
        title: "Plain-English instruction",
        kind: ToolKind::HighLevel,
        destructive: false,
        schema: schema_of::<args::InstructArgs>,
    },
];

/// Every tool: model commands (generated from their schemas) first, then
/// the higher-level tools.
pub fn catalog() -> Vec<ToolSpec> {
    let commands = command_schemas()
        .into_iter()
        .filter(|schema| !SUPERSEDED_COMMANDS.contains(&schema.name))
        .map(|schema| {
            let destructive = DESTRUCTIVE_COMMANDS.contains(&schema.name);
            let mut description = if schema.description.is_empty() {
                format!("The `{}` model command.", schema.name)
            } else {
                schema.description.clone()
            };
            description.push_str(" One undoable step.");
            if destructive {
                description.push_str(" Destructive: removes or replaces content.");
            }
            ToolSpec {
                name: schema.name.to_owned(),
                title: schema.name.replace('_', " "),
                description,
                input_schema: schema.args_schema,
                kind: ToolKind::Command,
                read_only: false,
                destructive,
            }
        });
    let high_level = HIGH_LEVEL.iter().map(|tool| {
        let input_schema = (tool.schema)();
        let description = input_schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(tool.title)
            .to_owned();
        let read_only = tool.kind == ToolKind::Query;
        ToolSpec {
            name: tool.name.to_owned(),
            title: tool.title.to_owned(),
            description,
            input_schema,
            kind: tool.kind,
            read_only,
            destructive: tool.destructive,
        }
    });
    commands.chain(high_level).collect()
}

/// Reads a tool's arguments; `null` counts as `{}`.
pub(crate) fn parse_args<T: DeserializeOwned>(tool: &str, args: Value) -> Result<T, AgentError> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|error| AgentError::invalid(tool, error.to_string()))
}

/// Runs the tool `name` on `agent`.
pub fn call(agent: &mut Agent, name: &str, args: Value) -> Result<ToolOutput, AgentError> {
    match name {
        "add_house_drum_pattern" => music::add_house_drum_pattern(agent, parse_args(name, args)?),
        "add_bassline" => music::add_bassline(agent, parse_args(name, args)?),
        "add_chords" => music::add_chords(agent, parse_args(name, args)?),
        "set_swing" => edit::set_swing(agent, parse_args(name, args)?),
        "sidechain" => mix::sidechain(agent, parse_args(name, args)?),
        "add_filter_riser" => mix::add_filter_riser(agent, parse_args(name, args)?),
        "render" => output::render(agent, parse_args(name, args)?),
        "export_stems" => output::export_stems(agent, parse_args(name, args)?),
        "mix_report" => {
            let args: args::MixReportArgs = parse_args(name, args)?;
            let report = crate::resources::mix_report(agent, args.bars)?;
            Ok(ToolOutput::unchanged(
                crate::resources::mix_report_text(&report),
                report,
            ))
        }
        "project_summary" => {
            parse_args::<args::NoArgs>(name, args)?;
            let summary = crate::resources::project_summary(agent);
            Ok(ToolOutput::unchanged(
                crate::resources::summary_text(agent),
                summary,
            ))
        }
        "list_tracks" => {
            parse_args::<args::NoArgs>(name, args)?;
            Ok(ToolOutput::unchanged(
                "tracks, devices and parameters",
                crate::resources::track_listing(agent),
            ))
        }
        "list_device_kinds" => {
            parse_args::<args::NoArgs>(name, args)?;
            Ok(ToolOutput::unchanged(
                "device kinds in the registry",
                crate::resources::device_listing(agent),
            ))
        }
        "undo" => {
            parse_args::<args::NoArgs>(name, args)?;
            let label = agent.undo()?;
            Ok(ToolOutput::changed(
                format!("undid {label}"),
                json!({ "undone": label }),
            ))
        }
        "redo" => {
            parse_args::<args::NoArgs>(name, args)?;
            let label = agent.redo()?;
            Ok(ToolOutput::changed(
                format!("redid {label}"),
                json!({ "redone": label }),
            ))
        }
        "save_project" => {
            parse_args::<args::NoArgs>(name, args)?;
            let dir = agent.save()?;
            Ok(ToolOutput::unchanged(
                format!("saved to {}", dir.display()),
                json!({ "dir": dir }),
            ))
        }
        "instruct" => {
            let args: args::InstructArgs = parse_args(name, args)?;
            crate::nl::instruct_local(agent, &args.text)
        }
        _ => command_tool(agent, name, args),
    }
}

fn command_tool(agent: &mut Agent, name: &str, args: Value) -> Result<ToolOutput, AgentError> {
    if !Command::NAMES.contains(&name) {
        return Err(AgentError::UnknownTool(name.to_owned()));
    }
    let args = if args.is_null() { json!({}) } else { args };
    let command = Command::from_name_and_args(name, args)
        .map_err(|error| AgentError::invalid(name, error.to_string()))?;
    let outcome = agent.execute(command)?;
    let data = serde_json::to_value(outcome).unwrap_or(Value::Null);
    Ok(ToolOutput::changed(format!("{name}: {data}"), data))
}
