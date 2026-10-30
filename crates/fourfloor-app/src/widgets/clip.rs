//! Clip bodies: a note preview for MIDI clips, a waveform for audio clips.

use egui::{pos2, Color32, Painter, Rect, Stroke, StrokeKind};
use fourfloor_analysis::WaveformOverview;
use fourfloor_model::{Clip, ClipContent};

use crate::geometry::TimeMap;
use crate::theme;

/// How to draw one clip.
#[derive(Clone, Copy, Debug)]
pub struct ClipStyle<'a> {
    /// The track colour.
    pub color: Color32,
    /// Draw the selection outline.
    pub selected: bool,
    /// The decoded overview of an audio clip's file, once ready.
    pub waveform: Option<&'a WaveformOverview>,
    /// Audio frames per tick at the project tempo (for waveforms).
    pub frames_per_tick: f64,
}

/// Paints `clip` into `rect` (its full timeline extent).
pub fn paint_clip(
    painter: &Painter,
    rect: Rect,
    clip: &Clip,
    map: &TimeMap,
    style: &ClipStyle<'_>,
) {
    let ClipStyle {
        color,
        selected,
        waveform,
        frames_per_tick: sample_rate_ratio,
    } = *style;
    painter.rect_filled(rect, 3.0, theme::dim(color, 0.6));
    let header = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 14.0));
    painter.rect_filled(header, 3.0, theme::dim(color, 0.25));
    painter.text(
        pos2(rect.min.x + 4.0, rect.min.y + 7.0),
        egui::Align2::LEFT_CENTER,
        &clip.name,
        egui::FontId::proportional(10.5),
        theme::CANVAS,
    );
    let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + 15.0), rect.max);
    match &clip.content {
        ClipContent::Midi { .. } => paint_notes(painter, body, clip, color, map),
        ClipContent::Audio {
            source_offset_frames,
            ..
        } => {
            if let Some(overview) = waveform {
                paint_waveform(
                    painter,
                    body,
                    overview,
                    *source_offset_frames,
                    clip,
                    map,
                    sample_rate_ratio,
                );
            } else {
                painter.text(
                    body.center(),
                    egui::Align2::CENTER_CENTER,
                    "loading…",
                    egui::FontId::proportional(10.0),
                    theme::TEXT_DIM,
                );
            }
        }
    }
    let stroke = if selected {
        Stroke::new(2.0, theme::TEXT)
    } else {
        Stroke::new(1.0, theme::dim(color, 0.2))
    };
    painter.rect_stroke(rect, 3.0, stroke, StrokeKind::Inside);
}

fn paint_notes(painter: &Painter, body: Rect, clip: &Clip, color: Color32, map: &TimeMap) {
    let notes = clip.sounding_notes();
    let Some(low) = notes.iter().map(|n| n.pitch).min() else {
        return;
    };
    let high = notes.iter().map(|n| n.pitch).max().unwrap_or(low);
    let span = f32::from(high - low).max(1.0) + 1.0;
    let row = (body.height() - 4.0) / span;
    for note in &notes {
        let x0 = map.x_of((clip.start + note.start).get() as f64);
        let x1 = map.x_of((clip.start + note.start + note.length).get() as f64);
        let y = body.bottom() - 2.0 - (f32::from(note.pitch - low) + 1.0) * row;
        let rect = Rect::from_min_max(pos2(x0, y), pos2(x1.max(x0 + 1.0), y + row.max(1.5)));
        painter.rect_filled(rect.intersect(body), 0.0, color);
    }
}

fn paint_waveform(
    painter: &Painter,
    body: Rect,
    overview: &WaveformOverview,
    offset_frames: u64,
    clip: &Clip,
    map: &TimeMap,
    frames_per_tick: f64,
) {
    // Only the columns on screen, one overview query per pixel.
    let visible = body.intersect(painter.clip_rect());
    if visible.width() <= 0.0 || frames_per_tick <= 0.0 {
        return;
    }
    let clip_start = clip.start.get() as f64;
    let frame_at = |x: f32| -> usize {
        let ticks = (map.ticks_at(x) - clip_start).max(0.0);
        offset_frames as usize + (ticks * frames_per_tick) as usize
    };
    let mid = body.center().y;
    let half = body.height() / 2.0 - 2.0;
    let mut x = visible.left().floor();
    while x < visible.right() {
        let (from, to) = (frame_at(x), frame_at(x + 1.0));
        if from >= overview.frames {
            break;
        }
        let peak = overview.range(from, to.max(from + 1).min(overview.frames));
        painter.line_segment(
            [
                pos2(x, mid - peak.max.clamp(-1.0, 1.0) * half),
                pos2(x, mid - peak.min.clamp(-1.0, 1.0) * half),
            ],
            Stroke::new(1.0, theme::TEXT),
        );
        x += 1.0;
    }
}
