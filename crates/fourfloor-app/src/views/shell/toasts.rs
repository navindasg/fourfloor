//! Toasts: short messages stacked over the bottom-right corner. Errors
//! anywhere in the app end up here instead of panicking.

use egui::{Align2, RichText};

use crate::state::ui::TOAST_SECONDS;
use crate::theme;
use crate::views::ViewCtx;

/// Width of one toast.
const TOAST_WIDTH: f32 = 340.0;

/// Draws the toasts; clicking one dismisses it.
pub fn toasts(egui_ctx: &egui::Context, ctx: &mut ViewCtx<'_>) {
    if ctx.ui.toasts.is_empty() {
        return;
    }
    let mut dismiss = None;
    egui::Area::new(egui::Id::unique("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -36.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(egui_ctx, |ui| {
            ui.set_max_width(TOAST_WIDTH);
            for (index, toast) in ctx.ui.toasts.iter().enumerate() {
                let fade = ((TOAST_SECONDS - toast.age) / 0.5).clamp(0.2, 1.0);
                let stroke = if toast.error {
                    theme::METER_CLIP
                } else {
                    theme::ACCENT_2
                };
                let response = egui::Frame::new()
                    .fill(theme::RAISED.gamma_multiply(fade.max(0.85)))
                    .stroke(egui::Stroke::new(1.0, stroke.gamma_multiply(fade)))
                    .corner_radius(4.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(TOAST_WIDTH - 16.0);
                        let title = if toast.error { "Error" } else { "Note" };
                        ui.label(RichText::new(title).small().strong().color(stroke));
                        ui.label(RichText::new(&toast.text).color(theme::TEXT));
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_text("Click to dismiss");
                if response.clicked() {
                    dismiss = Some(index);
                }
                ui.add_space(4.0);
            }
        });
    if let Some(index) = dismiss {
        ctx.ui.toasts.remove(index);
    }
    egui_ctx.request_repaint_after(std::time::Duration::from_millis(250));
}
