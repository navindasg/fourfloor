//! Settings tab: hosted plugins from the scan cache, and a rescan.

use egui::{RichText, Ui};

use crate::state::SettingsIntent;
use crate::theme;
use crate::views::ViewCtx;

/// Draws the tab.
pub fn plugins_tab(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let Some(path) = ctx.features.paths.plugins.clone() else {
        ui.label(RichText::new("Plugins are off in this run.").color(theme::TEXT_DIM));
        return;
    };
    if ctx.features.plugins.path.is_none() {
        ctx.intents.push(SettingsIntent::ReloadPlugins);
    }
    ui.label(
        RichText::new(format!("Scan cache: {}", path.display()))
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.horizontal(|ui| {
        let scanning = ctx.features.plugin_scan;
        if ui
            .add_enabled(!scanning, egui::Button::new("Scan plugins"))
            .clicked()
        {
            ctx.intents.push(SettingsIntent::ScanPlugins);
        }
        if ui.small_button("Reload list").clicked() {
            ctx.intents.push(SettingsIntent::ReloadPlugins);
        }
        if scanning {
            ui.spinner();
            ui.label("scanning…");
        }
    });
    ui.label(
        RichText::new(
            "Scanning loads each CLAP plugin and Audio Unit in this process: a plugin that crashes on load closes fourfloor. Start with --plugins to use the list.",
        )
        .small()
        .color(theme::MUTE),
    );
    let list = &ctx.features.plugins;
    if list.rows.is_empty() {
        ui.label(RichText::new("No plugins in the cache.").color(theme::TEXT_DIM));
    } else {
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .show(ui, |ui| {
                egui::Grid::new("plugins")
                    .num_columns(5)
                    .striped(true)
                    .show(ui, |ui| {
                        for title in ["Name", "Vendor", "Format", "Kind", "Loads"] {
                            ui.label(RichText::new(title).strong());
                        }
                        ui.end_row();
                        for row in &list.rows {
                            ui.label(&row.name).on_hover_text(&row.kind);
                            ui.label(&row.vendor);
                            ui.label(&row.format);
                            ui.label(&row.category);
                            ui.label(if row.loadable { "yes" } else { "no" });
                            ui.end_row();
                        }
                    });
            });
    }
    if !list.failures.is_empty() {
        egui::CollapsingHeader::new(format!("{} failed to scan", list.failures.len())).show(
            ui,
            |ui| {
                for (path, reason) in &list.failures {
                    ui.label(RichText::new(format!("{path}: {reason}")).small());
                }
            },
        );
    }
}
