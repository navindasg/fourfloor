//! The window shell: menu bar, transport bar, view tabs, status bar and
//! the settings window.

pub mod dialogs;
pub mod settings;
mod toasts;

pub use settings::settings_window;
pub use toasts::toasts;

use egui::Ui;
use fourfloor_model::command::{SetSwing, SetTempo};

use crate::geometry::bar_beat_label;
use crate::state::{FileIntent, Intent, PathPurpose, TransportIntent, ViewId};
use crate::theme;

use super::{View, ViewCtx};
use dialogs::choose_path;

/// The menu bar.
pub fn menu_bar(ui: &mut Ui, ctx: &mut ViewCtx<'_>, views: &[Box<dyn View>]) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.button("New").clicked() {
                ctx.intents.push(Intent::File(FileIntent::New));
                ui.close();
            }
            if ui.button("Open demo").clicked() {
                ctx.intents.push(Intent::File(FileIntent::Demo));
                ui.close();
            }
            if ui.button("Open…").clicked() {
                choose_path(ctx, PathPurpose::Open);
                ui.close();
            }
            if ui.button("Save").clicked() {
                ctx.intents.push(Intent::File(FileIntent::Save));
                ui.close();
            }
            if ui.button("Save as…").clicked() {
                choose_path(ctx, PathPurpose::SaveAs);
                ui.close();
            }
            ui.separator();
            if ui.button("Export WAV…").clicked() {
                choose_path(ctx, PathPurpose::ExportWav);
                ui.close();
            }
            if ui.button("Export stems…").clicked() {
                choose_path(ctx, PathPurpose::ExportStems);
                ui.close();
            }
            ui.separator();
            if ui.button("Import DAWproject…").clicked() {
                choose_path(ctx, PathPurpose::ImportDawproject);
                ui.close();
            }
            if ui.button("Export DAWproject…").clicked() {
                choose_path(ctx, PathPurpose::ExportDawproject);
                ui.close();
            }
        });
        ui.menu_button("Edit", |ui| {
            let undo = ctx.live.undo_label.clone();
            if ui
                .add_enabled(
                    undo.is_some(),
                    egui::Button::new(format!("Undo {}", undo.unwrap_or_default())),
                )
                .clicked()
            {
                ctx.intents.push(Intent::Undo);
                ui.close();
            }
            let redo = ctx.live.redo_label.clone();
            if ui
                .add_enabled(
                    redo.is_some(),
                    egui::Button::new(format!("Redo {}", redo.unwrap_or_default())),
                )
                .clicked()
            {
                ctx.intents.push(Intent::Redo);
                ui.close();
            }
        });
        ui.menu_button("View", |ui| {
            for (index, view) in views.iter().enumerate() {
                let label = format!("{}  ({}+{})", view.title(), MOD_KEY, index + 1);
                if ui
                    .selectable_label(ctx.ui.view == view.id(), label)
                    .clicked()
                {
                    ctx.ui.switch_view(view.id());
                    ui.close();
                }
            }
            ui.separator();
            ui.checkbox(&mut ctx.ui.panels.devices, "Device dock");
            ui.checkbox(&mut ctx.ui.panels.master, "Master meters");
            ui.checkbox(&mut ctx.ui.panels.library, "Sample browser  (B)");
            ui.checkbox(&mut ctx.ui.panels.assistant, "Assistant");
        });
        ui.menu_button("Options", |ui| {
            for (tab, title) in settings::TABS {
                if ui.button(format!("{title}…")).clicked() {
                    ctx.ui.settings_tab = tab;
                    ctx.ui.panels.settings = true;
                    ui.close();
                }
            }
        });
    });
}

fn tempo_and_swing(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    tempo_edit: &mut Option<f64>,
    swing_edit: &mut Option<f64>,
) {
    let mut bpm = tempo_edit.unwrap_or(ctx.project.tempo.bpm());
    let response = ui
        .add(
            egui::DragValue::new(&mut bpm)
                .range(20.0..=999.0)
                .speed(0.2)
                .fixed_decimals(1)
                .suffix(" BPM"),
        )
        .on_hover_text("Tempo");
    if response.changed() {
        *tempo_edit = Some(bpm);
    }
    if response.drag_stopped()
        || response.lost_focus()
        || (response.changed() && !response.dragged() && !response.has_focus())
    {
        if let Some(bpm) = tempo_edit.take() {
            if (bpm - ctx.project.tempo.bpm()).abs() > 1e-6 {
                ctx.intents.command(SetTempo { bpm });
            }
        }
    }
    let mut swing = swing_edit.unwrap_or(ctx.project.groove.swing_percent);
    let response = ui
        .add(
            egui::DragValue::new(&mut swing)
                .range(50.0..=75.0)
                .speed(0.2)
                .fixed_decimals(0)
                .prefix("swing ")
                .suffix("%"),
        )
        .on_hover_text("Project swing (applied when patterns are written)");
    if response.changed() {
        *swing_edit = Some(swing);
    }
    if response.drag_stopped()
        || response.lost_focus()
        || (response.changed() && !response.dragged() && !response.has_focus())
    {
        if let Some(percent) = swing_edit.take() {
            if (percent - ctx.project.groove.swing_percent).abs() > 1e-6 {
                ctx.intents.command(SetSwing {
                    percent,
                    grid: None,
                });
            }
        }
    }
}

/// Edits in progress in the transport bar.
#[derive(Debug, Default)]
pub struct TransportBar {
    tempo_edit: Option<f64>,
    swing_edit: Option<f64>,
}

impl TransportBar {
    /// Draws the transport bar.
    pub fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.horizontal(|ui| {
            let playing = ctx.live.transport.playing;
            let play = egui::Button::new(if playing { "Pause" } else { "Play" }).selected(playing);
            if ui.add(play).on_hover_text("Space").clicked() {
                ctx.intents.push(Intent::Transport(TransportIntent::Toggle));
            }
            if ui.button("Stop").on_hover_text("Stop and return to the start").clicked() {
                ctx.intents.push(Intent::Transport(TransportIntent::Stop));
                ctx.intents.push(Intent::Transport(TransportIntent::Seek(0)));
            }
            let looping = ctx.ui.loop_on;
            if ui.add(egui::Button::new("Loop").selected(looping)).on_hover_text("L").clicked() {
                ctx.intents.push(Intent::Transport(TransportIntent::ToggleLoop));
            }
            let metronome = ctx.ui.metronome;
            if ui.add(egui::Button::new("Click").selected(metronome)).on_hover_text("Metronome").clicked() {
                ctx.intents.push(Intent::Transport(TransportIntent::Metronome(!metronome)));
            }
            ui.separator();
            let position = ctx.live.transport.position.max(0.0) as i64;
            ui.label(egui::RichText::new(bar_beat_label(position)).monospace().size(16.0).color(theme::ACCENT));
            ui.separator();
            tempo_and_swing(ui, ctx, &mut self.tempo_edit, &mut self.swing_edit);
            ui.separator();
            if ui.add_enabled(ctx.live.undo_label.is_some(), egui::Button::new("Undo")).clicked() {
                ctx.intents.push(Intent::Undo);
            }
            if ui.add_enabled(ctx.live.redo_label.is_some(), egui::Button::new("Redo")).clicked() {
                ctx.intents.push(Intent::Redo);
            }
            ui.separator();
            let assistant = ctx.ui.panels.assistant;
            if ui
                .add(egui::Button::new("AI").selected(assistant))
                .on_hover_text("Assistant panel")
                .clicked()
            {
                ctx.ui.panels.assistant = !assistant;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let load = ctx.live.cpu_load * 100.0;
                let color = if load > 80.0 { theme::METER_CLIP } else { theme::TEXT_DIM };
                ui.label(egui::RichText::new(format!("CPU {load:>3.0}%  xruns {}", ctx.live.xruns)).monospace().color(color))
                    .on_hover_text("Audio callback load (time rendering / buffer length) and dropouts estimated from callback timing");
            });
        });
    }
}

/// The shortcut modifier as the user's keyboard labels it.
pub const MOD_KEY: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// The view tabs, on their own row so they stay visible at any window width.
pub fn view_tabs(ui: &mut Ui, ctx: &mut ViewCtx<'_>, views: &[Box<dyn View>]) {
    ui.horizontal_wrapped(|ui| {
        if let Some(previous) = ctx.ui.previous_view {
            let label = format!("< Back to {}", view_title(views, previous));
            if ui.button(label).on_hover_text("Esc").clicked() {
                ctx.ui.go_back();
            }
            ui.separator();
        }
        for (index, view) in views.iter().enumerate() {
            let hint = format!("{}+{}", MOD_KEY, index + 1);
            if ui
                .selectable_label(ctx.ui.view == view.id(), view.title())
                .on_hover_text(hint)
                .clicked()
            {
                ctx.ui.switch_view(view.id());
            }
        }
    });
}

/// The status bar: message, jobs, output and warnings.
pub fn status_bar(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.horizontal(|ui| {
        if let Some(status) = &ctx.ui.status {
            let color = if status.error {
                theme::METER_CLIP
            } else {
                theme::TEXT
            };
            ui.label(egui::RichText::new(&status.text).color(color));
        }
        for (label, progress) in &ctx.live.jobs {
            ui.add(
                egui::ProgressBar::new(*progress)
                    .desired_width(120.0)
                    .text(label.as_str()),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let dirty = if ctx.live.dirty { " *" } else { "" };
            ui.label(
                egui::RichText::new(format!("{}{dirty}", ctx.project.name)).color(theme::TEXT_DIM),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} Hz",
                    ctx.live.output_name, ctx.live.sample_rate
                ))
                .color(theme::TEXT_DIM),
            );
            if let Some(error) = &ctx.live.compile_error {
                ui.label(
                    egui::RichText::new(format!("compile failed: {error}"))
                        .color(theme::METER_CLIP),
                );
            } else if !ctx.live.warnings.is_empty() {
                let count = ctx.live.warnings.len();
                ui.label(
                    egui::RichText::new(format!("{count} warning(s)")).color(theme::METER_HOT),
                )
                .on_hover_text(ctx.live.warnings.join("\n"));
            }
        });
    });
}

/// Tab label of the current view, for the window title.
pub fn view_title(views: &[Box<dyn View>], id: ViewId) -> &str {
    views
        .iter()
        .find(|v| v.id() == id)
        .map_or("", |v| v.title())
}
