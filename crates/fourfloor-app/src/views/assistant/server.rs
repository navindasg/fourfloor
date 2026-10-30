//! The MCP server switch (with its address) and the local chat model
//! setting. Shown in the panel and in the settings window's Assistant tab.

use egui::{RichText, Ui};

use crate::state::AssistantIntent;
use crate::theme;
use crate::viewmodel::assistant::{llm_config, parse_mcp_addr, DEFAULT_LLM_URL, DEFAULT_MCP_ADDR};
use crate::views::ViewCtx;

use super::hint;

/// What is typed into the server fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFields {
    /// Address the MCP server listens on.
    pub mcp_addr: String,
    /// OpenAI-compatible chat server.
    pub llm_url: String,
    /// Its model name.
    pub llm_model: String,
    /// The last field that did not parse.
    pub error: Option<String>,
}

impl Default for ServerFields {
    fn default() -> Self {
        Self {
            mcp_addr: DEFAULT_MCP_ADDR.to_owned(),
            llm_url: DEFAULT_LLM_URL.to_owned(),
            llm_model: String::new(),
            error: None,
        }
    }
}

/// Draws the MCP and chat-model settings (their fields live in the UI
/// state, shared by the panel and the settings window).
pub fn server_settings(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let mut taken = std::mem::take(&mut ctx.ui.assistant_fields);
    let fields = &mut taken;
    mcp(fields, ui, ctx);
    ui.add_space(6.0);
    chat_model(fields, ui, ctx);
    if let Some(error) = &fields.error {
        ui.label(RichText::new(error).color(theme::METER_CLIP));
    }
    ctx.ui.assistant_fields = taken;
}

fn mcp(fields: &mut ServerFields, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.label(RichText::new("MCP server").strong());
    match &ctx.features.mcp {
        Some(server) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Serving at").color(theme::METER_OK));
                ui.label(RichText::new(&server.url).monospace().color(theme::TEXT));
            });
            ui.horizontal(|ui| {
                if ui.button("Copy address").clicked() {
                    ui.ctx().copy_text(server.url.clone());
                }
                if ui.button("Stop server").clicked() {
                    ctx.intents.push(AssistantIntent::StopMcp);
                }
            });
            hint(
                ui,
                "Edits from MCP clients appear in the log and can be undone.",
            );
        }
        None => {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut fields.mcp_addr)
                        .desired_width(150.0)
                        .hint_text("127.0.0.1:8765"),
                )
                .on_hover_text("Address to listen on (this machine only)");
                if ui.button("Start server").clicked() {
                    match parse_mcp_addr(&fields.mcp_addr) {
                        Ok(addr) => {
                            fields.error = None;
                            ctx.intents.push(AssistantIntent::StartMcp(addr));
                        }
                        Err(error) => fields.error = Some(error),
                    }
                }
            });
            hint(
                ui,
                "Lets Claude or another MCP client edit this project over Streamable HTTP.",
            );
        }
    }
}

fn chat_model(fields: &mut ServerFields, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.label(RichText::new("Local chat model").strong());
    if let Some(llm) = &ctx.features.llm {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} at {}", llm.model, llm.url)).color(theme::TEXT));
            if ui.button("Turn off").clicked() {
                ctx.intents.push(AssistantIntent::SetLlm(None));
            }
        });
        return;
    }
    egui::Grid::new("assistant-llm")
        .num_columns(2)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("URL");
            ui.add(egui::TextEdit::singleline(&mut fields.llm_url).desired_width(200.0));
            ui.end_row();
            ui.label("Model");
            ui.add(
                egui::TextEdit::singleline(&mut fields.llm_model)
                    .desired_width(200.0)
                    .hint_text("e.g. qwen2.5:7b"),
            );
            ui.end_row();
        });
    if ui
        .button("Use this model")
        .on_hover_text("Requests the phrase parser does not know go to this server")
        .clicked()
    {
        match llm_config(&fields.llm_url, &fields.llm_model) {
            Ok(config) => {
                fields.error = None;
                ctx.intents.push(AssistantIntent::SetLlm(Some(config)));
            }
            Err(error) => fields.error = Some(error),
        }
    }
    hint(
        ui,
        "Any OpenAI-compatible server on this machine: Ollama, llama.cpp, LM Studio.",
    );
}
