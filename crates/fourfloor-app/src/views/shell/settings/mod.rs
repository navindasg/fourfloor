//! The settings window: audio and MIDI devices, MIDI controller mappings,
//! machine-learning models and hosted plugins, one tab each.

mod audio;
mod mappings;
mod models;
mod plugins;

use crate::state::SettingsTab;
use crate::views::ViewCtx;

/// Tabs in order, with their titles.
pub const TABS: [(SettingsTab, &str); 5] = [
    (SettingsTab::Audio, "Audio & MIDI"),
    (SettingsTab::Mappings, "MIDI mappings"),
    (SettingsTab::Models, "Models"),
    (SettingsTab::Plugins, "Plugins"),
    (SettingsTab::Assistant, "Assistant"),
];

/// The settings window, shown while `ctx.ui.panels.settings` is set.
pub fn settings_window(egui_ctx: &egui::Context, ctx: &mut ViewCtx<'_>) {
    let mut open = ctx.ui.panels.settings;
    egui::Window::new("Settings")
        .open(&mut open)
        .default_width(560.0)
        .default_height(420.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                for (tab, title) in TABS {
                    ui.selectable_value(&mut ctx.ui.settings_tab, tab, title);
                }
            });
            ui.separator();
            match ctx.ui.settings_tab {
                SettingsTab::Audio => audio::audio_tab(ui, ctx),
                SettingsTab::Mappings => mappings::mappings_tab(ui, ctx),
                SettingsTab::Models => models::models_tab(ui, ctx),
                SettingsTab::Plugins => plugins::plugins_tab(ui, ctx),
                SettingsTab::Assistant => crate::views::assistant::server_settings(ui, ctx),
            }
        });
    ctx.ui.panels.settings = open;
}
