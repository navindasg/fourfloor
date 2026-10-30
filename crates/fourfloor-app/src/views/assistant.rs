//! The AI panel: a chat box wired to `fourfloor-agent` (the phrase parser
//! always works; a local chat model takes what it does not know), the log
//! of what each request did with an undo button per edit, buttons for stem
//! separation, audio-to-MIDI, drum and bass generation and the mix report,
//! and the MCP server switch.
//!
//! Like every view it changes nothing itself: requests are
//! [`AssistantIntent`]s and [`MlIntent`]s.

mod actions;
mod chat;
pub mod server;

use egui::Ui;

use crate::state::AssistantIntent;
use crate::theme;

use super::ViewCtx;

pub use server::server_settings;

/// Default width of the panel.
pub const PANEL_WIDTH: f32 = 360.0;

/// The panel's own state: what is being typed and the button choices.
#[derive(Debug)]
pub struct AssistantPanel {
    /// The chat box.
    pub input: String,
    /// Bars for generated parts.
    pub bars: u32,
    /// Drum flavour.
    pub drum_style: &'static str,
    /// Bass style.
    pub bass_style: &'static str,
    /// Bass key.
    pub key: &'static str,
    /// Seed of the next generated part (bumped after each).
    pub seed: u64,
    /// Use Spleeter (when downloaded) rather than the DSP split.
    pub prefer_model: bool,
}

impl Default for AssistantPanel {
    fn default() -> Self {
        Self {
            input: String::new(),
            bars: 4,
            drum_style: "classic",
            bass_style: "offbeat",
            key: "A minor",
            seed: 1,
            prefer_model: true,
        }
    }
}

impl AssistantPanel {
    /// Draws the panel.
    pub fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Assistant");
            if ctx.features.assistant_busy {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Close").clicked() {
                    ctx.ui.panels.assistant = false;
                }
                if ui
                    .add_enabled(
                        !ctx.features.chat.is_empty(),
                        egui::Button::new("Clear").small(),
                    )
                    .on_hover_text("Clear the log (the edits stay)")
                    .clicked()
                {
                    ctx.intents.push(AssistantIntent::ClearLog);
                }
            });
        });
        ui.separator();
        egui::CollapsingHeader::new("Tools")
            .default_open(true)
            .show(ui, |ui| actions::actions(self, ui, ctx));
        egui::CollapsingHeader::new("MCP server and chat model")
            .id_salt("assistant-server")
            .default_open(false)
            .show(ui, |ui| server::server_settings(ui, ctx));
        ui.separator();
        // The input sits at the bottom; the log fills what is left.
        egui::Panel::bottom("assistant-input")
            .show_separator_line(false)
            .show(ui, |ui| chat::input_row(self, ui, ctx));
        egui::CentralPanel::no_frame().show(ui, |ui| chat::log(ui, ctx));
    }
}

/// Small dim text.
fn hint(ui: &mut Ui, text: impl Into<String>) {
    ui.label(
        egui::RichText::new(text.into())
            .small()
            .color(theme::TEXT_DIM),
    );
}
