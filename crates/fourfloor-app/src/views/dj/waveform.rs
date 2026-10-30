//! Deck waveforms: the scrolling, beat-aligned view around the playhead and
//! the whole-track overview with cues and the loop.

use egui::{pos2, Color32, Painter, Rect, Stroke};
use fourfloor_dj::DeckState;

use crate::geometry::deck::{overview_x, BeatGrid, DeckWindow};
use crate::services::dj::DeckMedia;
use crate::theme;

/// The grid of a loaded track in seconds.
pub fn grid_of(media: &DeckMedia) -> Option<BeatGrid> {
    let grid = media.track.grid()?;
    let rate = media.track.sample_rate().as_f64();
    Some(BeatGrid {
        first_beat: grid.first_beat / rate,
        period: 60.0 / grid.bpm,
        beats_per_bar: grid.beats_per_bar,
        downbeat_index: grid.downbeat_index,
    })
}

fn column(painter: &Painter, x: f32, mid: f32, half: f32, min: f32, max: f32, color: Color32) {
    painter.line_segment(
        [
            pos2(x, mid - max.clamp(-1.0, 1.0) * half),
            pos2(x, mid - min.clamp(-1.0, 1.0) * half),
        ],
        Stroke::new(1.0, color),
    );
}

/// Paints the scrolling waveform of `media` around `state`'s playhead.
pub fn paint_scrolling(
    painter: &Painter,
    rect: Rect,
    media: &DeckMedia,
    state: &DeckState,
    color: Color32,
) {
    painter.rect_filled(rect, 3.0, theme::CANVAS);
    let rate = media.track.sample_rate().as_f64();
    let bpm = media.track.grid().map(|g| g.bpm);
    let window = DeckWindow::around(state.position_seconds, bpm, rect.left(), rect.width());
    if let Some(active) = state.loop_state {
        let x0 = window.x_of(active.start_seconds).max(rect.left());
        let x1 = window.x_of(active.end_seconds).min(rect.right());
        if x1 > x0 {
            let shade = Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom()));
            painter.rect_filled(shade, 0.0, theme::dim(theme::ACCENT_2, 0.8));
        }
    }
    if let Some(grid) = grid_of(media) {
        for (x, bar) in window.beat_lines(&grid) {
            let line = if bar {
                theme::GRID_BAR
            } else {
                theme::GRID_BEAT
            };
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(if bar { 1.5 } else { 1.0 }, line),
            );
        }
    }
    let overview = &media.overview;
    let mid = rect.center().y;
    let half = rect.height() / 2.0 - 3.0;
    let mut x = rect.left().floor();
    while x < rect.right() {
        if let Some((from, to)) = window.column_frames(x, rate, overview.frames) {
            let peak = overview.range(from, to);
            let played = window.seconds_at(x) < state.position_seconds;
            let shade = if played {
                theme::dim(color, 0.45)
            } else {
                color
            };
            column(painter, x, mid, half, peak.min, peak.max, shade);
        }
        x += 1.0;
    }
    for (slot, cue) in state.hot_cues.iter().enumerate() {
        if let Some(seconds) = cue {
            let cx = window.x_of(*seconds);
            if rect.x_range().contains(cx) {
                painter.line_segment(
                    [pos2(cx, rect.top()), pos2(cx, rect.bottom())],
                    Stroke::new(1.5, theme::track_color(slot)),
                );
            }
        }
    }
    let centre = rect.center().x;
    painter.line_segment(
        [pos2(centre, rect.top()), pos2(centre, rect.bottom())],
        Stroke::new(2.0, theme::TEXT),
    );
}

/// Paints the whole-track overview with playhead, cues and loop.
pub fn paint_overview(
    painter: &Painter,
    rect: Rect,
    media: &DeckMedia,
    state: &DeckState,
    color: Color32,
) {
    painter.rect_filled(rect, 2.0, theme::CANVAS);
    let overview = &media.overview;
    let columns = rect.width().max(1.0) as usize;
    let peaks = overview.columns(0, overview.frames, columns);
    let duration = media.track.seconds();
    let played = overview_x(state.position_seconds, duration, rect.left(), rect.width());
    let mid = rect.center().y;
    let half = rect.height() / 2.0 - 1.0;
    for (index, peak) in peaks.iter().enumerate() {
        let x = rect.left() + index as f32;
        let shade = if x < played {
            theme::dim(color, 0.55)
        } else {
            theme::dim(color, 0.15)
        };
        column(painter, x, mid, half, peak.min, peak.max, shade);
    }
    if let Some(active) = state.loop_state {
        let x0 = overview_x(active.start_seconds, duration, rect.left(), rect.width());
        let x1 = overview_x(active.end_seconds, duration, rect.left(), rect.width());
        let band = Rect::from_min_max(pos2(x0, rect.top()), pos2(x1.max(x0 + 2.0), rect.bottom()));
        painter.rect_filled(band, 0.0, theme::dim(theme::ACCENT_2, 0.6));
    }
    for (slot, cue) in state.hot_cues.iter().enumerate() {
        if let Some(seconds) = cue {
            let x = overview_x(*seconds, duration, rect.left(), rect.width());
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.top() + 6.0)],
                Stroke::new(2.0, theme::track_color(slot)),
            );
        }
    }
    let cue_x = overview_x(state.cue_seconds, duration, rect.left(), rect.width());
    painter.line_segment(
        [pos2(cue_x, rect.bottom() - 5.0), pos2(cue_x, rect.bottom())],
        Stroke::new(2.0, theme::METER_HOT),
    );
    painter.line_segment(
        [pos2(played, rect.top()), pos2(played, rect.bottom())],
        Stroke::new(1.5, theme::TEXT),
    );
}
