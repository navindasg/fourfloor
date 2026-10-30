//! The natural-language command layer.
//!
//! [`parser`] turns common phrases into tool calls with fixed rules and no
//! model. [`llm`] adds an optional chat backend for anything the parser
//! does not know (an OpenAI-compatible local server; with the off-by-default
//! `mistralrs` feature, also an in-process GGUF model in `mistral`). [`instruct`] tries the parser first and falls back to
//! the backend; either way the edits run as ordinary tool calls, so they
//! go through the command bus and are undoable.

pub mod llm;
#[cfg(feature = "mistralrs")]
pub mod mistral;
pub mod parser;

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::Agent;
use crate::error::AgentError;
use crate::tools::{ToolOutput, ToolSpec};
use llm::{ChatBackend, ChatMessage, Role};
pub use parser::{parse, ParseError, ToolCall};

/// An agent shared between the server, the chat loop and the caller.
pub type SharedAgent = Arc<Mutex<Agent>>;

/// Wraps an agent for sharing.
pub fn share(agent: Agent) -> SharedAgent {
    Arc::new(Mutex::new(agent))
}

/// Runs `run` on the shared agent. A poisoned lock (a panic while editing)
/// is reported, not propagated: the command bus never leaves a half-applied
/// edit, so the project inside is still consistent.
pub fn with_agent<T>(
    agent: &SharedAgent,
    run: impl FnOnce(&mut Agent) -> Result<T, AgentError>,
) -> Result<T, AgentError> {
    let mut guard = agent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    run(&mut guard)
}

/// One tool call made for an instruction, and what it returned.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The tool.
    pub tool: String,
    /// Its arguments.
    pub args: Value,
    /// What it returned, or why it failed.
    pub result: Result<ToolOutput, String>,
}

/// What an instruction did.
#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    /// `"parser"` or the chat backend's name.
    pub handled_by: String,
    /// The tool calls in order.
    pub steps: Vec<Step>,
    /// The model's closing text, if a model was used.
    pub reply: Option<String>,
}

impl Instruction {
    /// Whether every step succeeded.
    pub fn succeeded(&self) -> bool {
        self.steps.iter().all(|step| step.result.is_ok())
    }
}

fn steps_json(steps: &[Step]) -> Value {
    steps
        .iter()
        .map(|step| match &step.result {
            Ok(output) => json!({ "tool": step.tool, "args": step.args, "ok": output.text }),
            Err(error) => json!({ "tool": step.tool, "args": step.args, "error": error }),
        })
        .collect()
}

/// Parses `text` with the phrase parser and runs the calls, stopping at
/// the first failure. Each call is its own undo step.
pub fn instruct_local(agent: &mut Agent, text: &str) -> Result<ToolOutput, AgentError> {
    let calls = parse(text, agent.project()).map_err(|error| AgentError::Parse(error.0))?;
    if calls.iter().any(|call| call.name == "instruct") {
        return Err(AgentError::Parse(
            "an instruction cannot run itself".to_owned(),
        ));
    }
    let mut steps = Vec::new();
    let mut changed = false;
    for call in calls {
        let result = agent.call_tool(&call.name, call.args.clone());
        let failed = result.is_err();
        if let Ok(output) = &result {
            changed |= output.changed;
        }
        steps.push(Step {
            tool: call.name,
            args: call.args,
            result: result.map_err(|error| error.to_string()),
        });
        if failed {
            break;
        }
    }
    let text = steps
        .iter()
        .map(|step| match &step.result {
            Ok(output) => output.text.clone(),
            Err(error) => format!("{} failed: {error}", step.tool),
        })
        .collect::<Vec<_>>()
        .join("; ");
    let data = json!({ "handled_by": "parser", "steps": steps_json(&steps) });
    if let Some(Step {
        result: Err(error), ..
    }) = steps.last()
    {
        if !changed {
            return Err(AgentError::Parse(error.clone()));
        }
    }
    Ok(ToolOutput {
        text,
        data,
        changed,
    })
}

/// Tools offered to a chat model: the high-level tools plus the commands
/// people use most. Small local models do better with a short list.
pub fn chat_tools(agent: &Agent) -> Vec<ToolSpec> {
    const COMMON: &[&str] = &[
        "create_track",
        "rename_track",
        "delete_track",
        "set_tempo",
        "set_track_gain",
        "set_track_pan",
        "set_track_mute",
        "set_track_solo",
        "add_device",
        "set_param",
        "set_device_bypass",
    ];
    agent
        .tools()
        .into_iter()
        .filter(|tool| {
            tool.kind != crate::tools::ToolKind::Command || COMMON.contains(&tool.name.as_str())
        })
        .filter(|tool| tool.name != "instruct")
        .collect()
}

/// Options of the chat loop.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatOptions {
    /// Most model replies before giving up.
    pub max_rounds: usize,
    /// Offer every tool instead of [`chat_tools`].
    pub all_tools: bool,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            max_rounds: 8,
            all_tools: false,
        }
    }
}

const SYSTEM_PROMPT: &str = "You are the studio assistant of fourfloor, a DAW for house music. \
Use the tools to change the project; never invent track or device ids, read them from the \
project summary or call list_tracks. Every edit can be undone with the undo tool. Prefer the \
high-level tools (add_house_drum_pattern, add_bassline, add_chords, set_swing, sidechain, \
add_filter_riser, render) over raw commands. Answer briefly when done.";

/// Runs a chat with `backend` until it stops calling tools.
pub async fn chat(
    agent: &SharedAgent,
    backend: &dyn ChatBackend,
    text: &str,
    options: &ChatOptions,
) -> Result<Instruction, AgentError> {
    let (tools, summary) = with_agent(agent, |agent| {
        let tools = if options.all_tools {
            agent.tools()
        } else {
            chat_tools(agent)
        };
        Ok((tools, crate::resources::summary_text(agent)))
    })?;
    let mut messages = vec![
        ChatMessage::text(
            Role::System,
            format!("{SYSTEM_PROMPT}\n\nProject:\n{summary}"),
        ),
        ChatMessage::text(Role::User, text),
    ];
    let mut steps = Vec::new();
    for _ in 0..options.max_rounds.max(1) {
        let reply = backend.complete(&messages, &tools).await?;
        messages.push(reply.to_message());
        if reply.tool_calls.is_empty() {
            return Ok(Instruction {
                handled_by: backend.name().to_owned(),
                steps,
                reply: reply.content,
            });
        }
        for call in &reply.tool_calls {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            let result = if call.name == "instruct" || !tools.iter().any(|t| t.name == call.name) {
                Err(format!("tool {} is not available", call.name))
            } else {
                let shared = Arc::clone(agent);
                let name = call.name.clone();
                let tool_args = args.clone();
                tokio::task::spawn_blocking(move || {
                    with_agent(&shared, |agent| agent.call_tool(&name, tool_args))
                })
                .await
                .map_err(|error| AgentError::Unavailable(error.to_string()))
                .and_then(|result| result)
                .map_err(|error| error.to_string())
            };
            let content = match &result {
                Ok(output) => json!({ "ok": output.text, "data": output.data }).to_string(),
                Err(error) => json!({ "error": error }).to_string(),
            };
            messages.push(ChatMessage::tool_result(call.id.clone(), content));
            steps.push(Step {
                tool: call.name.clone(),
                args,
                result,
            });
        }
    }
    Ok(Instruction {
        handled_by: backend.name().to_owned(),
        steps,
        reply: Some(format!(
            "stopped after {} rounds without a final answer",
            options.max_rounds.max(1)
        )),
    })
}

/// Carries out `text`: the phrase parser first, then `backend` if the
/// parser does not understand it and a backend is given.
pub async fn instruct(
    agent: &SharedAgent,
    text: &str,
    backend: Option<&dyn ChatBackend>,
    options: &ChatOptions,
) -> Result<Instruction, AgentError> {
    let parsed = with_agent(agent, |agent| Ok(parse(text, agent.project())))?;
    match (parsed, backend) {
        (Ok(_), _) => {
            let shared = Arc::clone(agent);
            let owned = text.to_owned();
            let output = tokio::task::spawn_blocking(move || {
                with_agent(&shared, |agent| instruct_local(agent, &owned))
            })
            .await
            .map_err(|error| AgentError::Unavailable(error.to_string()))??;
            let steps = output.data["steps"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .map(|step| Step {
                            tool: step["tool"].as_str().unwrap_or_default().to_owned(),
                            args: step["args"].clone(),
                            result: match step.get("error").and_then(Value::as_str) {
                                Some(error) => Err(error.to_owned()),
                                None => Ok(ToolOutput {
                                    text: step["ok"].as_str().unwrap_or_default().to_owned(),
                                    data: Value::Null,
                                    changed: output.changed,
                                }),
                            },
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(Instruction {
                handled_by: "parser".to_owned(),
                steps,
                reply: None,
            })
        }
        (Err(_), Some(backend)) => chat(agent, backend, text, options).await,
        (Err(error), None) => Err(AgentError::Parse(error.0)),
    }
}
