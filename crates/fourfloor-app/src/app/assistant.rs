//! AI panel intents: instructions and tool buttons run against a sandbox
//! agent on a worker, then replayed into the document as undo steps; clip
//! stems and audio-to-MIDI; the mix report; the MCP server.

use std::sync::Arc;

use fourfloor_core::AudioMessage;
use fourfloor_model::Command;

use crate::project::JobOutput;
use crate::services::assistant::{
    instruction_job, replay, tool_job, AgentRequest, AgentRun, AppliedStep, ChatEntry, McpServer,
    RecordedStep, ToolStep,
};
use crate::services::mix_report::{mix_report_job, MixReport, ReferenceInput};
use crate::services::ml::{clip_audio, separate_job, transcribe_job, Separated, Transcribed};
use crate::state::document::is_live_only;
use crate::state::{AssistantIntent, MlIntent};

use super::FourfloorApp;

/// Job labels of the AI panel, so a failed job can clear its busy flag.
pub mod labels {
    /// An instruction or tool button.
    pub const AGENT: &str = "Assistant";
    /// Stem separation of a clip.
    pub const STEMS: &str = "Separating stems";
    /// Audio-to-MIDI of a clip.
    pub const MIDI: &str = "Audio to MIDI";
    /// The mix report.
    pub const REPORT: &str = "Mix report";
}

/// Most chat entries kept.
pub const MAX_CHAT: usize = 200;

impl FourfloorApp {
    pub(super) fn assistant_intent(&mut self, intent: AssistantIntent) {
        match intent {
            AssistantIntent::Instruct(text) => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    return;
                }
                let llm = self.features.llm.clone();
                let job = instruction_job(self.agent_request(), text.clone(), llm);
                self.start_agent_job(text, job);
            }
            AssistantIntent::Tool(tool, args) => {
                let job = tool_job(self.agent_request(), tool.clone(), args);
                self.start_agent_job(tool, job);
            }
            AssistantIntent::UndoStep { depth, label } => self.undo_step(depth, &label),
            AssistantIntent::MixReport => self.start_mix_report(),
            AssistantIntent::StartMcp(addr) => self.start_mcp(addr),
            AssistantIntent::StopMcp => {
                if let Some(server) = self.features.mcp.take() {
                    server.stop();
                    self.ui_state.info("MCP server stopped");
                }
            }
            AssistantIntent::SetLlm(config) => {
                match &config {
                    Some(llm) => self
                        .ui_state
                        .info(format!("Chat model: {} at {}", llm.model, llm.url)),
                    None => self.ui_state.info("Chat model off: phrase commands only"),
                }
                self.features.llm = config;
            }
            AssistantIntent::ClearLog => self.features.chat.clear(),
        }
    }

    fn agent_request(&self) -> AgentRequest {
        AgentRequest {
            project: self.doc.project().clone(),
            dir: self.doc.dir.clone(),
            out_dir: self.features.paths.scratch.clone(),
            registry: Arc::clone(&self.registry),
            revision: self.doc.revision(),
        }
    }

    fn start_agent_job(
        &mut self,
        prompt: String,
        job: impl FnOnce(&crate::services::Progress) -> Result<AgentRun, String> + Send + 'static,
    ) {
        if self.features.assistant_busy {
            self.ui_state
                .error("The assistant is still working on the last request");
            return;
        }
        self.features.assistant_busy = true;
        self.features.assistant_pending = Some(prompt);
        self.jobs.spawn(labels::AGENT, move |progress| {
            job(progress).map(|run| JobOutput::Agent(Box::new(run)))
        });
    }

    /// Replays recorded steps into the document and sends live parameter
    /// moves to the engine (they do not recompile).
    fn replay_steps(&mut self, steps: &[RecordedStep]) -> Result<Vec<AppliedStep>, String> {
        let applied = replay(&mut self.doc, steps);
        let messages: Vec<AudioMessage> = steps
            .iter()
            .flat_map(|step| step.commands.iter())
            .filter(|command| is_live_only(command))
            .flat_map(|command| command.audio_messages(self.doc.project()))
            .collect();
        self.host.send_all(messages);
        applied
    }

    pub(super) fn agent_finished(&mut self, run: AgentRun) {
        self.features.assistant_busy = false;
        self.features.assistant_pending = None;
        let moved = self.doc.revision() != run.start_revision;
        let mut entry = ChatEntry {
            prompt: run.prompt.clone(),
            handled_by: run.handled_by.clone(),
            tools: run.tools.clone(),
            applied: Vec::new(),
            reply: run.reply.clone(),
            error: None,
        };
        match self.replay_steps(&run.steps) {
            Ok(applied) => {
                entry.applied = applied;
                if !moved && self.doc.project() != run.result.as_ref() {
                    entry.error = Some(
                        "the edits did not reproduce exactly here; check the result".to_owned(),
                    );
                }
            }
            Err(error) => {
                let why = if moved {
                    format!("{error} (the project changed while the assistant worked)")
                } else {
                    error
                };
                self.ui_state.error(format!("Assistant: {why}"));
                entry.error = Some(why);
            }
        }
        let failed = entry.tools.iter().filter(|t| !t.ok).count();
        if entry.error.is_none() {
            let edits = entry.applied.len();
            self.ui_state.info(match (edits, failed) {
                (0, 0) => format!("Assistant: {}", short(&entry.prompt)),
                (_, 0) => format!("Assistant made {edits} edit(s)"),
                _ => format!("Assistant: {failed} step(s) failed"),
            });
        }
        self.push_chat(entry);
    }

    fn push_chat(&mut self, entry: ChatEntry) {
        self.features.chat.push(entry);
        let excess = self.features.chat.len().saturating_sub(MAX_CHAT);
        self.features.chat.drain(..excess);
    }

    /// A failed AI-panel job: clear its busy flag and log the error.
    pub(super) fn assistant_job_failed(&mut self, label: &str, error: &str) {
        let prompt = match label {
            labels::AGENT => {
                self.features.assistant_busy = false;
                self.features
                    .assistant_pending
                    .take()
                    .unwrap_or_else(|| "request".to_owned())
            }
            labels::STEMS | labels::MIDI => {
                self.features.ml_busy = false;
                label.to_owned()
            }
            labels::REPORT => {
                self.features.report_busy = false;
                label.to_owned()
            }
            _ => return,
        };
        self.push_chat(ChatEntry {
            prompt,
            handled_by: String::new(),
            tools: Vec::new(),
            applied: Vec::new(),
            reply: None,
            error: Some(error.to_owned()),
        });
    }

    fn undo_step(&mut self, depth: usize, label: &str) {
        let bus = self.doc.bus();
        if bus.undo_depth() != depth || bus.undo_label() != Some(label) {
            self.ui_state
                .error(format!("Undo the newer edits first to undo \"{label}\""));
            return;
        }
        match self.doc.undo() {
            Ok(label) => self.ui_state.info(format!("Undid {label}")),
            Err(error) => self.ui_state.error(error.to_string()),
        }
    }

    /// Stem separation and audio-to-MIDI of a clip.
    pub(super) fn clip_ml(&mut self, intent: MlIntent) {
        if self.features.ml_busy {
            self.ui_state
                .error("A separation or transcription is still running");
            return;
        }
        let clip = match &intent {
            MlIntent::Separate { clip, .. } | MlIntent::Transcribe(clip) => *clip,
            MlIntent::Download(_) | MlIntent::Refresh => return,
        };
        let audio = match clip_audio(self.doc.project(), self.doc.dir.clone(), clip) {
            Ok(audio) => audio,
            Err(error) => {
                self.ui_state.error(error);
                return;
            }
        };
        let project = self.doc.project().clone();
        let models = self.features.paths.models.clone();
        self.features.ml_busy = true;
        match intent {
            MlIntent::Separate { prefer_model, .. } => {
                let job = separate_job(audio, project, models, prefer_model);
                self.jobs.spawn(labels::STEMS, move |progress| {
                    job(progress).map(|s| JobOutput::Separated(Box::new(s)))
                });
            }
            _ => {
                let job = transcribe_job(audio, project, models);
                self.jobs.spawn(labels::MIDI, move |progress| {
                    job(progress).map(|t| JobOutput::Transcribed(Box::new(t)))
                });
            }
        }
    }

    /// Applies a planned clip edit as one step and logs it in the chat.
    fn apply_planned(&mut self, prompt: String, label: &str, commands: Vec<Command>, note: String) {
        let step = RecordedStep {
            label: label.to_owned(),
            commands,
        };
        let mut entry = ChatEntry {
            prompt,
            handled_by: "button".to_owned(),
            tools: vec![ToolStep {
                tool: label.to_owned(),
                text: note.clone(),
                ok: true,
            }],
            applied: Vec::new(),
            reply: None,
            error: None,
        };
        match self.replay_steps(&[step]) {
            Ok(applied) => {
                entry.applied = applied;
                self.ui_state.notify(note);
            }
            Err(error) => {
                self.ui_state.error(error.clone());
                entry.error = Some(error);
            }
        }
        self.push_chat(entry);
    }

    pub(super) fn separated(&mut self, separated: Separated) {
        self.features.ml_busy = false;
        let how = if separated.fallback {
            format!(
                "{} (DSP fallback; download Spleeter in Settings, Models for better stems)",
                separated.separator
            )
        } else {
            separated.separator.clone()
        };
        let note = format!("Separated into {} with {how}", separated.stems.join(", "));
        self.apply_planned(
            "Stem-separate clip".to_owned(),
            "separate_stems",
            separated.commands,
            note,
        );
    }

    pub(super) fn transcribed(&mut self, transcribed: Transcribed) {
        self.features.ml_busy = false;
        let note = format!(
            "Transcribed {} notes to a new MIDI track",
            transcribed.notes
        );
        self.apply_planned(
            "Audio to MIDI".to_owned(),
            "audio_to_midi",
            transcribed.commands,
            note,
        );
    }

    fn start_mix_report(&mut self) {
        if self.features.report_busy {
            return;
        }
        let reference = self.reference.as_ref().map(|loaded| ReferenceInput {
            name: loaded
                .path
                .file_name()
                .map_or("the reference".to_owned(), |n| {
                    n.to_string_lossy().into_owned()
                }),
            audio: Arc::clone(&loaded.audio),
        });
        self.features.report_busy = true;
        let job = mix_report_job(
            self.doc.project().clone(),
            self.doc.dir.clone(),
            Arc::clone(&self.registry),
            self.host.sample_rate(),
            reference,
        );
        self.jobs.spawn(labels::REPORT, move |progress| {
            job(progress).map(|r| JobOutput::MixReport(Box::new(r)))
        });
    }

    pub(super) fn mix_reported(&mut self, report: MixReport) {
        self.features.report_busy = false;
        let prompt = match &report.reference_name {
            Some(name) => format!("Mix report vs {name}"),
            None => "Mix report".to_owned(),
        };
        self.push_chat(ChatEntry {
            prompt,
            handled_by: "analysis".to_owned(),
            tools: Vec::new(),
            applied: Vec::new(),
            reply: Some(report.lines.join("\n")),
            error: None,
        });
        self.features.last_report = Some(report);
    }

    fn start_mcp(&mut self, addr: std::net::SocketAddr) {
        if self.features.mcp.is_some() {
            return;
        }
        let scratch = self.features.paths.scratch.clone();
        match McpServer::start(&self.doc, Arc::clone(&self.registry), scratch, addr) {
            Ok(server) => {
                self.ui_state
                    .notify(format!("MCP server at {}", server.url));
                self.features.mcp = Some(server);
            }
            Err(error) => self.ui_state.error(format!("MCP server: {error}")),
        }
    }

    /// Pulls MCP client edits into the document (every frame).
    pub(super) fn tick_mcp(&mut self) {
        let Some(mut server) = self.features.mcp.take() else {
            return;
        };
        let report = server.sync(&mut self.doc);
        self.features.mcp = Some(server);
        if let Some(warning) = report.warning {
            self.ui_state.error(warning);
        }
        if report.applied.is_empty() && report.undone == 0 {
            return;
        }
        // Client parameter moves keep the graph in the document's eyes but
        // were never sent to the engine: rebuild so it hears them.
        self.doc.invalidate_graph();
        let tools = if report.undone > 0 {
            vec![ToolStep {
                tool: "undo".to_owned(),
                text: format!("undid {} step(s)", report.undone),
                ok: true,
            }]
        } else {
            Vec::new()
        };
        self.push_chat(ChatEntry {
            prompt: "Edit from an MCP client".to_owned(),
            handled_by: "mcp".to_owned(),
            tools,
            applied: report.applied,
            reply: None,
            error: None,
        });
    }
}

/// The first few words of a prompt, for the status bar.
fn short(text: &str) -> String {
    let mut out: String = text.chars().take(48).collect();
    if text.chars().count() > 48 {
        out.push('…');
    }
    out
}
