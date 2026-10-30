//! The chat log (one card per request: the tools it ran, the edits it made
//! with an undo button each, the model's or report's words) and the input
//! row.

use egui::{RichText, Ui};

use crate::services::assistant::{AppliedStep, ChatEntry};
use crate::state::AssistantIntent;
use crate::theme;
use crate::viewmodel::assistant::{prompt_title, step_state, tool_summary, StepState};
use crate::views::ViewCtx;

use super::{hint, AssistantPanel};

/// Example phrases the parser understands, shown while the log is empty.
pub const EXAMPLES: [&str; 5] = [
    "set tempo to 124",
    "swing the hats to 56%",
    "sidechain the pad to the kick",
    "add a deep house drum pattern",
    "mute the bass",
];

/// The text box and Send button.
pub fn input_row(panel: &mut AssistantPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.add_space(4.0);
    let busy = ctx.features.assistant_busy;
    let mut send = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 64.0).max(80.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut panel.input)
                .hint_text("Tell fourfloor what to do…")
                .desired_width(width),
        );
        let entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if entered {
            response.request_focus();
        }
        let clicked = ui
            .add_enabled(
                !busy && !panel.input.trim().is_empty(),
                egui::Button::new("Send"),
            )
            .clicked();
        send = (entered || clicked) && !busy && !panel.input.trim().is_empty();
    });
    if send {
        let text = std::mem::take(&mut panel.input);
        ctx.intents.push(AssistantIntent::Instruct(text));
    }
    let backend = match &ctx.features.llm {
        Some(llm) => format!("Phrase commands, then {} for the rest", llm.model),
        None => "Phrase commands only (set a local chat model below the tools)".to_owned(),
    };
    hint(ui, backend);
    ui.add_space(2.0);
}

/// The log, newest at the bottom.
pub fn log(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    if ctx.features.chat.is_empty() && !ctx.features.assistant_busy {
        empty_state(ui, ctx);
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for (index, entry) in ctx.features.chat.iter().enumerate() {
                ui.push_id(index, |ui| entry_card(ui, entry, ctx.live, ctx.intents));
                ui.add_space(4.0);
            }
            if let Some(pending) = &ctx.features.assistant_pending {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(format!("Working on \"{pending}\"…")).color(theme::TEXT_DIM),
                    );
                });
            }
        });
}

fn empty_state(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.add_space(8.0);
    ui.label(RichText::new("Nothing asked yet.").color(theme::TEXT_DIM));
    hint(ui, "Every change the assistant makes is an undo step. Try:");
    for example in EXAMPLES {
        if ui
            .link(RichText::new(example).color(theme::ACCENT_2))
            .on_hover_text("Run this")
            .clicked()
        {
            ctx.intents
                .push(AssistantIntent::Instruct(example.to_owned()));
        }
    }
}

fn entry_card(
    ui: &mut Ui,
    entry: &ChatEntry,
    live: &crate::state::LiveReadout,
    intents: &mut crate::state::Intents,
) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(prompt_title(&entry.prompt))
                        .strong()
                        .color(theme::TEXT),
                );
                if !entry.handled_by.is_empty() {
                    ui.label(
                        RichText::new(format!("via {}", entry.handled_by))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                }
            });
            for tool in &entry.tools {
                // Successful steps already show as edits below; only words
                // worth reading (or failures) get a line.
                let summary = tool_summary(&tool.tool, &tool.text);
                match (tool.ok, summary) {
                    (true, Some(text)) => {
                        ui.label(RichText::new(text).small().color(theme::TEXT_DIM));
                    }
                    (false, summary) => {
                        let why = summary.unwrap_or_else(|| tool.text.clone());
                        ui.label(
                            RichText::new(format!("{} failed: {why}", tool.tool))
                                .small()
                                .color(theme::METER_CLIP),
                        );
                    }
                    (true, None) => {}
                }
            }
            for (index, step) in entry.applied.iter().enumerate() {
                ui.push_id(index, |ui| applied_row(ui, step, live, intents));
            }
            if let Some(reply) = &entry.reply {
                ui.label(RichText::new(reply).color(theme::TEXT));
            }
            if let Some(error) = &entry.error {
                ui.label(RichText::new(error).color(theme::METER_CLIP));
            }
            if entry.applied.is_empty() && entry.error.is_none() && entry.reply.is_none() {
                hint(ui, "No change to the project.");
            }
        });
}

fn applied_row(
    ui: &mut Ui,
    step: &AppliedStep,
    live: &crate::state::LiveReadout,
    intents: &mut crate::state::Intents,
) {
    ui.horizontal(|ui| {
        let state = step_state(step, live.undo_depth, live.undo_label.as_deref());
        let (enabled, tip) = match state {
            StepState::Undoable => (true, "Undo this edit"),
            StepState::Undone => (false, "Already undone"),
            StepState::Buried => (false, "Undo the newer edits first"),
        };
        let label = if state == StepState::Undone {
            "Undone"
        } else {
            "Undo edit"
        };
        let button = ui
            .add_enabled(enabled, egui::Button::new(label).small())
            .on_hover_text(tip)
            .on_disabled_hover_text(tip);
        if button.clicked() {
            intents.push(AssistantIntent::UndoStep {
                depth: step.depth,
                label: step.label.clone(),
            });
        }
        let commands = step.commands.join(", ");
        let color = if state == StepState::Undone {
            theme::TEXT_DIM
        } else {
            theme::ACCENT
        };
        ui.label(RichText::new(&step.label).color(color))
            .on_hover_text(format!("Commands: {commands}"));
        ui.label(
            RichText::new(format!("{} cmd", step.commands.len()))
                .small()
                .color(theme::TEXT_DIM),
        );
    });
}
