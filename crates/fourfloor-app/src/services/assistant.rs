//! The AI assistant: `fourfloor-agent` driven from the app.
//!
//! The UI's document stays the only truth, and it changes only through its
//! command bus. So the agent never edits it directly: each request runs on
//! a worker against a *sandbox* agent holding a copy of the project, and
//! the steps the sandbox's bus recorded (label plus commands) are then
//! replayed into the document, each as its own undo step. Because commands
//! are deterministic and the copy starts from the same project, the replay
//! reproduces the sandbox's result exactly (checked after every run).
//!
//! The MCP server ([`McpServer`]) serves a shared agent over Streamable
//! HTTP on localhost. [`McpServer::sync`] pulls the client's edits into the
//! document the same way, and re-seeds the agent from the document when the
//! app edited the project in between.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use fourfloor_agent::nl::llm::OpenAiCompatible;
use fourfloor_agent::nl::{instruct, instruct_local, ChatOptions};
use fourfloor_agent::{share, Agent, AgentSettings, HttpServer, SharedAgent};
use fourfloor_core::DeviceRegistry;
use fourfloor_model::{Command, CommandBus, Project};
use serde_json::Value;

use super::Progress;
use crate::state::Document;

/// An OpenAI-compatible chat server on this machine (llama.cpp, Ollama,
/// LM Studio...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmConfig {
    /// Base URL, e.g. `http://127.0.0.1:11434/v1`.
    pub url: String,
    /// Model name the server knows.
    pub model: String,
}

/// One step the sandbox's command bus recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedStep {
    /// Undo label (the tool or command name).
    pub label: String,
    /// The commands, in order.
    pub commands: Vec<Command>,
}

/// One tool call made for a request.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolStep {
    /// The tool.
    pub tool: String,
    /// What it said (or why it failed).
    pub text: String,
    /// Whether it succeeded.
    pub ok: bool,
}

/// What a request did in the sandbox.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRun {
    /// What was asked.
    pub prompt: String,
    /// `"parser"`, `"button"` or the chat backend's name.
    pub handled_by: String,
    /// Tool calls in order.
    pub tools: Vec<ToolStep>,
    /// Edits to replay, in order.
    pub steps: Vec<RecordedStep>,
    /// The model's closing words, if a model was used.
    pub reply: Option<String>,
    /// The project the sandbox ended with, to check the replay.
    pub result: Box<Project>,
    /// Document revision the sandbox started from.
    pub start_revision: u64,
}

/// The steps a bus recorded, oldest first.
pub fn recorded_steps(bus: &CommandBus) -> Vec<RecordedStep> {
    let mut bus = bus.clone();
    let mut steps = Vec::new();
    while let Ok(entry) = bus.undo() {
        steps.push(RecordedStep {
            label: entry.label.clone(),
            commands: entry.commands.clone(),
        });
    }
    steps.reverse();
    steps
}

/// The `n` newest steps a bus recorded, oldest first.
pub fn newest_steps(bus: &CommandBus, n: usize) -> Vec<RecordedStep> {
    let all = recorded_steps(bus);
    let skip = all.len().saturating_sub(n);
    all.into_iter().skip(skip).collect()
}

/// A sandbox agent on a copy of `project`. Renders go to `out_dir`.
pub fn sandbox(
    project: Project,
    dir: Option<PathBuf>,
    out_dir: PathBuf,
    registry: Arc<DeviceRegistry>,
) -> Agent {
    let settings = AgentSettings {
        autosave: false,
        output_dir: Some(out_dir),
        ..AgentSettings::default()
    };
    let agent = Agent::new(project, registry).with_settings(settings);
    match dir {
        Some(dir) => agent.with_project_dir(dir),
        None => agent,
    }
}

fn tool_steps(data: &Value) -> Vec<ToolStep> {
    data["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|step| {
                    let tool = step["tool"].as_str().unwrap_or_default().to_owned();
                    match step.get("error").and_then(Value::as_str) {
                        Some(error) => ToolStep {
                            tool,
                            text: error.to_owned(),
                            ok: false,
                        },
                        None => ToolStep {
                            tool,
                            text: step["ok"].as_str().unwrap_or_default().to_owned(),
                            ok: true,
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Everything a request job needs.
#[derive(Clone, Debug)]
pub struct AgentRequest {
    /// The project to work on.
    pub project: Project,
    /// Its folder.
    pub dir: Option<PathBuf>,
    /// Where renders go without a folder.
    pub out_dir: PathBuf,
    /// The device registry.
    pub registry: Arc<DeviceRegistry>,
    /// Document revision of `project`.
    pub revision: u64,
}

fn finish(agent: &Agent, request: &AgentRequest, prompt: String) -> AgentRun {
    AgentRun {
        prompt,
        handled_by: String::new(),
        tools: Vec::new(),
        steps: recorded_steps(agent.bus()),
        reply: None,
        result: Box::new(agent.project().clone()),
        start_revision: request.revision,
    }
}

/// Job: carry out a typed instruction. The phrase parser always runs
/// first; anything it does not know goes to `llm` when one is set.
pub fn instruction_job(
    request: AgentRequest,
    text: String,
    llm: Option<LlmConfig>,
) -> impl FnOnce(&Progress) -> Result<AgentRun, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        let mut agent = sandbox(
            request.project.clone(),
            request.dir.clone(),
            request.out_dir.clone(),
            Arc::clone(&request.registry),
        );
        let parsed = fourfloor_agent::nl::parse(&text, agent.project());
        match (parsed, llm) {
            (Ok(_), _) | (Err(_), None) => {
                let output = instruct_local(&mut agent, &text).map_err(|e| e.to_string())?;
                let mut run = finish(&agent, &request, text);
                run.handled_by = "parser".to_owned();
                run.tools = tool_steps(&output.data);
                Ok(run)
            }
            (Err(_), Some(llm)) => {
                let backend =
                    OpenAiCompatible::new(&llm.url, &llm.model).map_err(|e| e.to_string())?;
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let shared = share(agent);
                let instruction = runtime
                    .block_on(instruct(
                        &shared,
                        &text,
                        Some(&backend),
                        &ChatOptions::default(),
                    ))
                    .map_err(|e| e.to_string())?;
                let agent = shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut run = finish(&agent, &request, text);
                run.handled_by = instruction.handled_by;
                run.reply = instruction.reply;
                run.tools = instruction
                    .steps
                    .into_iter()
                    .map(|step| match step.result {
                        Ok(output) => ToolStep {
                            tool: step.tool,
                            text: output.text,
                            ok: true,
                        },
                        Err(error) => ToolStep {
                            tool: step.tool,
                            text: error,
                            ok: false,
                        },
                    })
                    .collect();
                Ok(run)
            }
        }
    }
}

/// Job: run one agent tool (the panel's buttons).
pub fn tool_job(
    request: AgentRequest,
    tool: String,
    args: Value,
) -> impl FnOnce(&Progress) -> Result<AgentRun, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        let mut agent = sandbox(
            request.project.clone(),
            request.dir.clone(),
            request.out_dir.clone(),
            Arc::clone(&request.registry),
        );
        let output = agent
            .call_tool(&tool, args)
            .map_err(|e| format!("{tool}: {e}"))?;
        let mut run = finish(&agent, &request, tool.clone());
        run.handled_by = "button".to_owned();
        run.tools = vec![ToolStep {
            tool,
            text: output.text,
            ok: true,
        }];
        Ok(run)
    }
}

/// One edit the assistant made in the document, with what undoing it
/// needs.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedStep {
    /// Undo label.
    pub label: String,
    /// Names of its commands.
    pub commands: Vec<String>,
    /// Undo depth right after it went in.
    pub depth: usize,
}

impl AppliedStep {
    /// Whether this step is the newest undo step now (so its undo button
    /// undoes exactly it).
    pub fn is_undoable(&self, doc: &Document) -> bool {
        doc.bus().undo_depth() == self.depth && doc.bus().undo_label() == Some(self.label.as_str())
    }
}

/// Replays recorded steps into `doc`, each as one undo step. Stops at the
/// first that fails.
pub fn replay(doc: &mut Document, steps: &[RecordedStep]) -> Result<Vec<AppliedStep>, String> {
    let mut applied = Vec::new();
    for step in steps {
        doc.transaction(&step.label, step.commands.clone())
            .map_err(|error| format!("{}: {error}", step.label))?;
        applied.push(AppliedStep {
            label: step.label.clone(),
            commands: step.commands.iter().map(|c| c.name().to_owned()).collect(),
            depth: doc.bus().undo_depth(),
        });
    }
    Ok(applied)
}

/// A chat entry as the AI panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatEntry {
    /// What was asked.
    pub prompt: String,
    /// Who handled it.
    pub handled_by: String,
    /// Tool calls.
    pub tools: Vec<ToolStep>,
    /// Edits made in the document.
    pub applied: Vec<AppliedStep>,
    /// Model reply.
    pub reply: Option<String>,
    /// What went wrong, if anything.
    pub error: Option<String>,
}

/// What [`McpServer::sync`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncReport {
    /// Client edits applied to the document.
    pub applied: Vec<AppliedStep>,
    /// Client undos applied to the document.
    pub undone: usize,
    /// The agent was given the document's project afresh.
    pub reseeded: bool,
    /// A problem to show.
    pub warning: Option<String>,
}

/// A running MCP server on localhost.
pub struct McpServer {
    runtime: tokio::runtime::Runtime,
    server: Option<HttpServer>,
    agent: SharedAgent,
    registry: Arc<DeviceRegistry>,
    out_dir: PathBuf,
    dir: Option<PathBuf>,
    agent_revision: u64,
    agent_depth: usize,
    doc_revision: u64,
    /// The endpoint, `http://127.0.0.1:port/mcp`.
    pub url: String,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl McpServer {
    /// Starts serving the document's project at `addr` (loopback only).
    pub fn start(
        doc: &Document,
        registry: Arc<DeviceRegistry>,
        out_dir: PathBuf,
        addr: SocketAddr,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fourfloor-mcp")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let agent = share(sandbox(
            doc.project().clone(),
            doc.dir.clone(),
            out_dir.clone(),
            Arc::clone(&registry),
        ));
        let server = runtime
            .block_on(fourfloor_agent::serve_http(Arc::clone(&agent), addr))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            url: server.url(),
            runtime,
            server: Some(server),
            agent,
            registry,
            out_dir,
            dir: doc.dir.clone(),
            agent_revision: 0,
            agent_depth: 0,
            doc_revision: doc.revision(),
        })
    }

    /// The agent the server edits (tests drive it directly).
    pub fn agent(&self) -> &SharedAgent {
        &self.agent
    }

    /// Stops the server.
    pub fn stop(mut self) {
        if let Some(server) = self.server.take() {
            let _ = self.runtime.block_on(server.shutdown());
        }
    }

    fn reseed(&mut self, agent: &mut Agent, doc: &Document) {
        *agent = sandbox(
            doc.project().clone(),
            doc.dir.clone().or_else(|| self.dir.clone()),
            self.out_dir.clone(),
            Arc::clone(&self.registry),
        );
        self.agent_revision = agent.bus().revision();
        self.agent_depth = 0;
        self.doc_revision = doc.revision();
    }

    /// Brings client edits into `doc` and app edits to the agent. Skips a
    /// frame when a client call holds the agent.
    pub fn sync(&mut self, doc: &mut Document) -> SyncReport {
        let agent = Arc::clone(&self.agent);
        let Ok(mut guard) = agent.try_lock() else {
            return SyncReport::default();
        };
        let mut report = SyncReport::default();
        let doc_changed = doc.revision() != self.doc_revision;
        let agent_changed = guard.bus().revision() != self.agent_revision;
        if agent_changed && !doc_changed {
            let depth = guard.bus().undo_depth();
            let result = if depth > self.agent_depth {
                replay(doc, &newest_steps(guard.bus(), depth - self.agent_depth))
            } else {
                // The client undid steps (and maybe made one new edit).
                let undo = self.agent_depth - depth + usize::from(depth == self.agent_depth);
                let mut outcome: Result<Vec<AppliedStep>, String> = Ok(Vec::new());
                for _ in 0..undo {
                    if let Err(error) = doc.undo() {
                        outcome = Err(error.to_string());
                        break;
                    }
                    report.undone += 1;
                }
                let redo = usize::from(depth == self.agent_depth);
                outcome.and_then(|_| replay(doc, &newest_steps(guard.bus(), redo)))
            };
            match result {
                Ok(applied) => report.applied = applied,
                Err(error) => report.warning = Some(format!("MCP edit not applied: {error}")),
            }
            if doc.project() == guard.project() {
                self.agent_revision = guard.bus().revision();
                self.agent_depth = guard.bus().undo_depth();
                self.doc_revision = doc.revision();
                return report;
            }
            report.warning.get_or_insert_with(|| {
                "the MCP client and the app disagreed; the client now sees the app's project"
                    .to_owned()
            });
        } else if agent_changed {
            report.warning = Some(
                "an MCP edit arrived while the project was being edited here; the app's version was kept"
                    .to_owned(),
            );
        }
        if doc_changed || agent_changed {
            self.reseed(&mut guard, doc);
            report.reseeded = true;
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::{CreateTrack, SetTempo};
    use fourfloor_model::TrackKind;

    #[test]
    fn recorded_steps_come_back_oldest_first_with_their_commands() {
        let mut bus = CommandBus::new(Project::new("Test"));
        bus.execute(SetTempo { bpm: 126.0 }).expect("tempo");
        bus.transaction("two tracks", |tx| {
            for name in ["A", "B"] {
                tx.execute(CreateTrack {
                    name: name.to_owned(),
                    kind: TrackKind::Midi,
                    index: None,
                })?;
            }
            Ok(())
        })
        .expect("tracks");
        let steps = recorded_steps(&bus);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].label, "set_tempo");
        assert_eq!(steps[1].label, "two tracks");
        assert_eq!(steps[1].commands.len(), 2);
        assert_eq!(newest_steps(&bus, 1)[0].label, "two tracks");
        // Replaying onto the same start gives the same project.
        let mut doc = Document::new(Project::new("Test"));
        let applied = replay(&mut doc, &steps).expect("replays");
        assert_eq!(doc.project(), bus.project());
        assert!(applied[1].is_undoable(&doc));
        assert!(!applied[0].is_undoable(&doc));
        doc.undo().expect("undo");
        assert!(applied[0].is_undoable(&doc));
    }
}
