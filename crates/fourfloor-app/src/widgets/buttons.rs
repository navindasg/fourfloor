//! Small square toggle buttons (mute, solo, arm, step cells) with their
//! own accessible labels, so tests can find "Mute Kick" rather than "M".

use egui::{vec2, Color32, Response, Sense, Stroke, StrokeKind, Ui};

use crate::theme;

/// A toggle showing `text`, filled with `on_color` when `on`. `label` is
/// what screen readers and tests see.
pub fn toggle(
    ui: &mut Ui,
    text: &str,
    label: &str,
    on: bool,
    on_color: Color32,
    size: f32,
) -> Response {
    pad(ui, text, label, on, on_color, vec2(size, size * 0.8))
}

/// A button of any size showing `text`, lit with `on_color` when `on`,
/// with its own accessible `label` (unique per deck, per track...).
pub fn pad(
    ui: &mut Ui,
    text: &str,
    label: &str,
    on: bool,
    on_color: Color32,
    size: egui::Vec2,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let accessible = label.to_owned();
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &accessible));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if on {
            on_color
        } else if response.hovered() {
            theme::GRID_BAR
        } else {
            theme::RAISED
        };
        painter.rect_filled(rect, 2.0, fill);
        painter.rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, theme::GRID_BEAT),
            StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional((size.y * 0.5).clamp(9.0, 14.0)),
            if on { theme::CANVAS } else { theme::TEXT_DIM },
        );
    }
    response.on_hover_text(label)
}
