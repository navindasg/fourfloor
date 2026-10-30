//! A rotary knob: drag up and down, double-click for the default.

use std::f32::consts::PI;

use egui::{pos2, vec2, Response, Sense, Stroke, Ui};

use crate::theme;

/// Pixels of vertical drag for the full travel.
pub const DRAG_PIXELS: f32 = 160.0;

/// What a knob did this frame.
#[derive(Debug)]
pub struct KnobOutput {
    /// The egui response.
    pub response: Response,
    /// New travel position while dragging (or after a reset).
    pub changed: Option<f32>,
    /// `true` when a gesture ended this frame: commit now.
    pub committed: bool,
}

/// Draws a knob at travel position `value` (0..1) labelled `label` with
/// `display` under it. `default` is restored on double-click.
pub fn knob(
    ui: &mut Ui,
    label: &str,
    display: &str,
    value: f32,
    default: f32,
    size: f32,
) -> KnobOutput {
    let desired = vec2(size + 28.0, size + 36.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let accessible = format!("{label} knob");
    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(value), &accessible));
    let mut changed = None;
    let mut committed = false;
    if response.double_clicked() {
        changed = Some(default.clamp(0.0, 1.0));
        committed = true;
    } else if response.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.1
        } else {
            1.0
        };
        let delta = -response.drag_delta().y / DRAG_PIXELS * fine;
        if delta != 0.0 {
            changed = Some((value + delta).clamp(0.0, 1.0));
        }
    }
    if response.drag_stopped() {
        committed = true;
    }
    let shown = changed.unwrap_or(value);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let center = pos2(rect.center().x, rect.top() + 6.0 + size / 2.0);
        let radius = size / 2.0;
        let start = PI * 0.75;
        let sweep = PI * 1.5;
        let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
            let steps = 24;
            (0..=steps)
                .map(|i| {
                    let a = from + (to - from) * i as f32 / steps as f32;
                    center + vec2(a.cos(), a.sin()) * radius
                })
                .collect()
        };
        painter.circle_filled(center, radius - 3.0, theme::RAISED);
        painter.add(egui::Shape::line(
            arc(start, start + sweep),
            Stroke::new(3.0, theme::GRID_BAR),
        ));
        if shown > 0.0 {
            let color = if response.hovered() || response.dragged() {
                theme::ACCENT
            } else {
                theme::dim(theme::ACCENT, 0.15)
            };
            painter.add(egui::Shape::line(
                arc(start, start + sweep * shown),
                Stroke::new(3.0, color),
            ));
        }
        let angle = start + sweep * shown;
        painter.line_segment(
            [
                center,
                center + vec2(angle.cos(), angle.sin()) * (radius - 4.0),
            ],
            Stroke::new(2.0, theme::TEXT),
        );
        let small = egui::FontId::proportional(10.0);
        painter.text(
            pos2(rect.center().x, rect.bottom() - 19.0),
            egui::Align2::CENTER_CENTER,
            truncate(label, 10),
            small.clone(),
            theme::TEXT_DIM,
        );
        painter.text(
            pos2(rect.center().x, rect.bottom() - 7.0),
            egui::Align2::CENTER_CENTER,
            display,
            small,
            theme::TEXT,
        );
    }
    let response = response.on_hover_text(format!("{label}: {display}"));
    KnobOutput {
        response,
        changed,
        committed,
    }
}

/// Shortens `text` to `max` characters with an ellipsis.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
        short.push('…');
        short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_labels_are_shortened() {
        assert_eq!(truncate("Cutoff", 12), "Cutoff");
        assert_eq!(truncate("Resonance Amount", 8), "Resonan…");
    }
}
