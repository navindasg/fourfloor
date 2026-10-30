//! Settings tab: the project's MIDI controller mappings.

use egui::{RichText, Ui};

use crate::theme;
use crate::viewmodel::mappings::mapping_rows;
use crate::views::ViewCtx;

/// Draws the tab.
pub fn mappings_tab(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let rows = mapping_rows(ctx.project);
    ui.label(
        RichText::new(
            "Map a controller from a knob's menu (MIDI learn), then move it. Mappings are saved with the project.",
        )
        .small()
        .color(theme::TEXT_DIM),
    );
    if rows.is_empty() {
        ui.label(RichText::new("No controllers mapped.").color(theme::TEXT_DIM));
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            egui::Grid::new("midi-mappings")
                .num_columns(6)
                .striped(true)
                .show(ui, |ui| {
                    for title in ["Channel", "CC", "Track", "Device", "Parameter", "Range"] {
                        ui.label(RichText::new(title).strong());
                    }
                    ui.label("");
                    ui.end_row();
                    for row in rows {
                        ui.label(&row.channel);
                        ui.label(row.controller.to_string());
                        ui.label(&row.track);
                        ui.label(&row.device);
                        ui.label(&row.param);
                        ui.label(format!("{:.2} – {:.2}", row.range.0, row.range.1));
                        if ui
                            .small_button("Remove")
                            .on_hover_text(format!("Unmap CC {}", row.controller))
                            .clicked()
                        {
                            ctx.intents.command(row.remove.clone());
                        }
                        ui.end_row();
                    }
                });
        });
}
