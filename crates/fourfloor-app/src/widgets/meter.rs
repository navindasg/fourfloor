//! Level meters, faders and the spectrum, all custom painted.

use egui::{pos2, vec2, Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui};

use crate::theme;

/// Bottom of the meter scale.
pub const METER_FLOOR_DB: f32 = -60.0;
/// Top of the meter scale.
pub const METER_TOP_DB: f32 = 6.0;

/// Linear level to dB (with a floor for silence).
pub fn level_db(linear: f32) -> f32 {
    if linear > 0.0 && linear.is_finite() {
        20.0 * linear.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Fraction 0..1 of the meter height for a level in dB.
pub fn meter_fraction(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db - METER_FLOOR_DB) / (METER_TOP_DB - METER_FLOOR_DB)).clamp(0.0, 1.0)
}

fn meter_color(db: f32) -> Color32 {
    if db >= 0.0 {
        theme::METER_CLIP
    } else if db >= -6.0 {
        theme::METER_HOT
    } else {
        theme::METER_OK
    }
}

/// Paints a vertical stereo meter for linear peaks into `rect`.
pub fn paint_stereo_meter(ui: &Ui, rect: Rect, peaks: [f32; 2]) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::CANVAS);
    let gap = 2.0;
    let width = (rect.width() - gap) / 2.0;
    for (channel, peak) in peaks.iter().enumerate() {
        let db = level_db(*peak);
        let fraction = meter_fraction(db);
        let left = rect.left() + channel as f32 * (width + gap);
        let bar = Rect::from_min_max(
            pos2(left, rect.bottom() - rect.height() * fraction),
            pos2(left + width, rect.bottom()),
        );
        painter.rect_filled(bar, 1.0, meter_color(db));
    }
    // 0 dB tick.
    let zero = rect.bottom() - rect.height() * meter_fraction(0.0);
    painter.line_segment(
        [pos2(rect.left(), zero), pos2(rect.right(), zero)],
        Stroke::new(1.0, theme::TEXT_DIM),
    );
}

/// Paints a horizontal stereo meter (track headers).
pub fn paint_horizontal_meter(ui: &Ui, rect: Rect, peaks: [f32; 2]) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 1.0, theme::CANVAS);
    let height = (rect.height() - 1.0) / 2.0;
    for (channel, peak) in peaks.iter().enumerate() {
        let db = level_db(*peak);
        let top = rect.top() + channel as f32 * (height + 1.0);
        let bar = Rect::from_min_size(
            pos2(rect.left(), top),
            vec2(rect.width() * meter_fraction(db), height),
        );
        painter.rect_filled(bar, 0.0, meter_color(db));
    }
}

/// What a fader did.
#[derive(Debug)]
pub struct FaderOutput {
    /// The egui response.
    pub response: Response,
    /// New position while dragging.
    pub changed: Option<f32>,
    /// A gesture ended: commit.
    pub committed: bool,
}

/// A vertical fader at `position` (0..1). Double-click returns `default`.
pub fn fader(
    ui: &mut Ui,
    label: &str,
    position: f32,
    default: f32,
    size: egui::Vec2,
) -> FaderOutput {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let accessible = format!("{label} fader");
    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(position), &accessible));
    let mut changed = None;
    let mut committed = response.drag_stopped();
    if response.double_clicked() {
        changed = Some(default);
        committed = true;
    } else if response.dragged() {
        let delta = -response.drag_delta().y / (rect.height() - 12.0).max(1.0);
        if delta != 0.0 {
            changed = Some((position + delta).clamp(0.0, 1.0));
        }
    }
    let shown = changed.unwrap_or(position);
    let painter = ui.painter_at(rect);
    let track = Rect::from_center_size(rect.center(), vec2(4.0, rect.height() - 12.0));
    painter.rect_filled(track, 2.0, theme::CANVAS);
    let y = track.bottom() - track.height() * shown;
    let fill = Rect::from_min_max(pos2(track.left(), y), track.max);
    painter.rect_filled(fill, 2.0, theme::dim(theme::ACCENT, 0.3));
    let cap = Rect::from_center_size(pos2(rect.center().x, y), vec2(rect.width() - 4.0, 10.0));
    painter.rect_filled(cap, 2.0, theme::RAISED);
    painter.rect_stroke(
        cap,
        2.0,
        Stroke::new(1.0, theme::TEXT_DIM),
        StrokeKind::Inside,
    );
    FaderOutput {
        response,
        changed,
        committed,
    }
}

/// Paints band levels (dB, low to high) as bars into `rect`.
pub fn paint_spectrum(ui: &Ui, rect: Rect, bands_db: &[f32], floor_db: f32) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::CANVAS);
    if bands_db.is_empty() {
        return;
    }
    let width = rect.width() / bands_db.len() as f32;
    for (index, db) in bands_db.iter().enumerate() {
        let t = ((db - floor_db) / -floor_db).clamp(0.0, 1.0);
        let left = rect.left() + index as f32 * width;
        let bar = Rect::from_min_max(
            pos2(left + 0.5, rect.bottom() - rect.height() * t),
            pos2(left + width - 0.5, rect.bottom()),
        );
        painter.rect_filled(bar, 0.0, theme::dim(theme::ACCENT_2, 0.2 + 0.4 * (1.0 - t)));
    }
}

/// Paints a correlation meter (-1..1) into `rect`.
pub fn paint_correlation(ui: &Ui, rect: Rect, correlation: f32) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::CANVAS);
    let x = rect.center().x + correlation.clamp(-1.0, 1.0) * rect.width() / 2.0;
    painter.line_segment(
        [
            pos2(rect.center().x, rect.top()),
            pos2(rect.center().x, rect.bottom()),
        ],
        Stroke::new(1.0, theme::GRID_BAR),
    );
    let color = if correlation < 0.0 {
        theme::METER_CLIP
    } else {
        theme::METER_OK
    };
    let marker = Rect::from_center_size(pos2(x, rect.center().y), vec2(4.0, rect.height() - 2.0));
    painter.rect_filled(marker, 1.0, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_onto_the_meter_scale() {
        assert_eq!(level_db(1.0), 0.0);
        assert!((level_db(0.5) + 6.02).abs() < 0.01);
        assert_eq!(level_db(0.0), f32::NEG_INFINITY);
        assert_eq!(meter_fraction(f32::NEG_INFINITY), 0.0);
        assert_eq!(meter_fraction(METER_TOP_DB), 1.0);
        assert!((meter_fraction(0.0) - 60.0 / 66.0).abs() < 1e-6);
        assert_eq!(meter_color(1.0), theme::METER_CLIP);
        assert_eq!(meter_color(-20.0), theme::METER_OK);
    }
}
