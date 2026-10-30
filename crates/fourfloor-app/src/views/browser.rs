//! The add-device browser: every registry device, grouped, searchable.

use egui::Ui;

use crate::viewmodel::mixer::{add_device, browser_groups};

use super::ViewCtx;

/// The browser window's search text.
#[derive(Debug, Default)]
pub struct DeviceBrowser {
    /// Filter text.
    pub search: String,
}

impl DeviceBrowser {
    /// Shows the browser window when `ctx.ui.panels.browser` is set.
    pub fn window(&mut self, egui_ctx: &egui::Context, ctx: &mut ViewCtx<'_>) {
        let mut open = ctx.ui.panels.browser;
        egui::Window::new("Add device")
            .open(&mut open)
            .default_width(320.0)
            .default_height(420.0)
            .show(egui_ctx, |ui| self.contents(ui, ctx));
        ctx.ui.panels.browser = open && ctx.ui.panels.browser;
    }

    fn contents(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        let Some(track) = ctx.selected_track().map(|t| (t.id, t.name.clone())) else {
            ui.label("Select a track first.");
            return;
        };
        ui.label(format!("Add to {}", track.1));
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("search"));
        let needle = self.search.to_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (group, devices) in browser_groups(ctx.registry) {
                let matching: Vec<_> = devices
                    .into_iter()
                    .filter(|d| {
                        needle.is_empty()
                            || d.name.to_lowercase().contains(&needle)
                            || d.kind.contains(&needle)
                    })
                    .collect();
                if matching.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(group)
                    .default_open(true)
                    .show(ui, |ui| {
                        for descriptor in matching {
                            let text = format!("{}  ·  {}", descriptor.name, descriptor.kind);
                            if ui.button(text).clicked() {
                                match add_device(
                                    ctx.project,
                                    ctx.registry,
                                    track.0,
                                    &descriptor.kind,
                                ) {
                                    Some(command) => {
                                        ctx.intents.command(command);
                                        ctx.ui.panels.browser = false;
                                        ctx.ui.panels.devices = true;
                                    }
                                    None => ctx.ui.error(format!("cannot add {}", descriptor.kind)),
                                }
                            }
                        }
                    });
            }
        });
    }
}
