//! Screen maths of the DJ decks: the scrolling waveform (centred on the
//! playhead, a fixed number of beats wide, beat lines from the track's
//! grid) and the whole-track overview strip.

/// Beats shown across a scrolling waveform.
pub const WINDOW_BEATS: f64 = 16.0;
/// Seconds shown when the tempo is unknown.
pub const WINDOW_SECONDS: f64 = 8.0;

/// A track's beat grid in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatGrid {
    /// Time of beat zero.
    pub first_beat: f64,
    /// Seconds per beat at normal speed.
    pub period: f64,
    /// Beats in a bar.
    pub beats_per_bar: u32,
    /// Which beat (from beat zero) is the first downbeat.
    pub downbeat_index: u32,
}

impl BeatGrid {
    /// A grid from a tempo and the first beat.
    pub fn new(bpm: f64, first_beat: f64) -> Option<Self> {
        (bpm.is_finite() && bpm > 0.0).then(|| Self {
            first_beat,
            period: 60.0 / bpm,
            beats_per_bar: 4,
            downbeat_index: 0,
        })
    }
}

/// The visible stretch of a scrolling waveform: `span` seconds centred on
/// the playhead at `center`, drawn across `width` pixels from `left`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckWindow {
    /// Track time at the centre line, seconds.
    pub center: f64,
    /// Seconds across the whole width.
    pub span: f64,
    /// Left edge, pixels.
    pub left: f32,
    /// Width, pixels.
    pub width: f32,
}

impl DeckWindow {
    /// A window of [`WINDOW_BEATS`] beats (or [`WINDOW_SECONDS`] without a
    /// tempo) around `center`.
    pub fn around(center: f64, bpm: Option<f64>, left: f32, width: f32) -> Self {
        let span = bpm
            .filter(|bpm| bpm.is_finite() && *bpm > 0.0)
            .map_or(WINDOW_SECONDS, |bpm| WINDOW_BEATS * 60.0 / bpm);
        Self {
            center,
            span,
            left,
            width: width.max(1.0),
        }
    }

    /// Track time at the left edge.
    pub fn start(&self) -> f64 {
        self.center - self.span / 2.0
    }

    /// Pixel of track time `seconds`.
    pub fn x_of(&self, seconds: f64) -> f32 {
        self.left + ((seconds - self.start()) / self.span) as f32 * self.width
    }

    /// Track time at pixel `x`.
    pub fn seconds_at(&self, x: f32) -> f64 {
        self.start() + f64::from(x - self.left) / f64::from(self.width) * self.span
    }

    /// Track frames `[from, to)` under the pixel column at `x`, clipped to
    /// a track of `frames` frames at `rate`; `None` when off the track.
    pub fn column_frames(&self, x: f32, rate: f64, frames: usize) -> Option<(usize, usize)> {
        let from = self.seconds_at(x) * rate;
        let to = self.seconds_at(x + 1.0) * rate;
        if to <= 0.0 || from >= frames as f64 {
            return None;
        }
        // Snap to whole frames first so rounding noise does not widen the
        // column by one frame.
        let from = (from.max(0.0) + 1e-6).floor() as usize;
        let to = ((to - 1e-6).ceil() as usize).clamp(from + 1, frames);
        Some((from, to))
    }

    /// Beat lines on screen: pixel and whether it starts a bar.
    pub fn beat_lines(&self, grid: &BeatGrid) -> Vec<(f32, bool)> {
        if grid.period <= 0.0 {
            return Vec::new();
        }
        let first = ((self.start() - grid.first_beat) / grid.period).ceil() as i64;
        let last = ((self.start() + self.span - grid.first_beat) / grid.period).floor() as i64;
        let bar = i64::from(grid.beats_per_bar.max(1));
        let downbeat = i64::from(grid.downbeat_index);
        (first..=last)
            .map(|beat| {
                let x = self.x_of(grid.first_beat + beat as f64 * grid.period);
                (x, (beat - downbeat).rem_euclid(bar) == 0)
            })
            .collect()
    }
}

/// Pixel of track time `seconds` on an overview of a `duration` track
/// drawn across `width` pixels from `left`.
pub fn overview_x(seconds: f64, duration: f64, left: f32, width: f32) -> f32 {
    if duration <= 0.0 {
        return left;
    }
    left + ((seconds / duration).clamp(0.0, 1.0)) as f32 * width
}

/// Track time under pixel `x` of an overview (for click-to-seek).
pub fn overview_seconds(x: f32, duration: f64, left: f32, width: f32) -> f64 {
    if width <= 0.0 {
        return 0.0;
    }
    (f64::from((x - left) / width)).clamp(0.0, 1.0) * duration.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_sixteen_beats_centred_on_the_playhead() {
        let window = DeckWindow::around(30.0, Some(120.0), 100.0, 800.0);
        assert!((window.span - 8.0).abs() < 1e-12);
        assert_eq!(window.x_of(30.0), 500.0);
        assert_eq!(window.x_of(26.0), 100.0);
        assert!((window.seconds_at(900.0) - 34.0).abs() < 1e-9);
        let unknown = DeckWindow::around(0.0, None, 0.0, 100.0);
        assert_eq!(unknown.span, WINDOW_SECONDS);
    }

    #[test]
    fn beat_lines_fall_on_the_grid_and_mark_bars() {
        let window = DeckWindow::around(30.0, Some(120.0), 0.0, 1600.0);
        let grid = BeatGrid::new(120.0, 0.25).expect("grid");
        let lines = window.beat_lines(&grid);
        assert_eq!(lines.len(), 16, "16 beats in view");
        // Beat 52 is at 26.25 s: 0.25 s into the window is 50 px.
        assert!((lines[0].0 - 50.0).abs() < 1e-3);
        assert!((lines[1].0 - lines[0].0 - 100.0).abs() < 1e-3);
        let bars = lines.iter().filter(|(_, bar)| *bar).count();
        assert_eq!(bars, 4);
        assert!(lines[0].1, "beat 52 starts bar 13");
    }

    #[test]
    fn columns_map_to_frames_and_stop_at_the_ends() {
        let window = DeckWindow::around(1.0, None, 0.0, 800.0);
        // 8 s over 800 px: 10 ms per pixel, 480 frames at 48 kHz.
        assert_eq!(window.column_frames(0.0, 48_000.0, 480_000), None);
        let (from, to) = window
            .column_frames(400.0, 48_000.0, 480_000)
            .expect("on track");
        assert_eq!((from, to), (48_000, 48_480));
        assert_eq!(window.column_frames(799.0, 48_000.0, 100), None);
    }

    #[test]
    fn the_overview_maps_time_both_ways() {
        assert_eq!(overview_x(30.0, 120.0, 10.0, 400.0), 110.0);
        assert_eq!(overview_x(500.0, 120.0, 10.0, 400.0), 410.0);
        assert!((overview_seconds(110.0, 120.0, 10.0, 400.0) - 30.0).abs() < 1e-9);
        assert_eq!(overview_seconds(-50.0, 120.0, 10.0, 400.0), 0.0);
    }
}
