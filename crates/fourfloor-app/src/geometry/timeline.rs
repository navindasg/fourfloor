//! Time on the horizontal axis: ticks to pixels and back, zoom, scroll and
//! snapping. Shared by the arrangement, the piano roll and the automation
//! lanes so a clip, a note and a breakpoint line up at the same x.

use fourfloor_core::{Ticks, TICKS_PER_BEAT};

/// Ticks in one 4/4 bar (the model has no time signature yet).
pub const BAR_TICKS: i64 = TICKS_PER_BEAT * 4;
/// Ticks in one sixteenth note.
pub const SIXTEENTH_TICKS: i64 = TICKS_PER_BEAT / 4;

/// The narrowest and widest zoom, in pixels per beat.
pub const MIN_PX_PER_BEAT: f32 = 2.0;
/// See [`MIN_PX_PER_BEAT`].
pub const MAX_PX_PER_BEAT: f32 = 640.0;

/// What a drag or a click snaps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Snap {
    /// No snapping: one tick resolution.
    Off,
    /// A whole bar.
    Bar,
    /// A beat.
    #[default]
    Beat,
    /// An eighth note.
    Eighth,
    /// A sixteenth note.
    Sixteenth,
}

impl Snap {
    /// Every choice, in menu order.
    pub const ALL: [Snap; 5] = [
        Snap::Off,
        Snap::Bar,
        Snap::Beat,
        Snap::Eighth,
        Snap::Sixteenth,
    ];

    /// The grid size in ticks (1 when off).
    pub const fn ticks(self) -> i64 {
        match self {
            Snap::Off => 1,
            Snap::Bar => BAR_TICKS,
            Snap::Beat => TICKS_PER_BEAT,
            Snap::Eighth => TICKS_PER_BEAT / 2,
            Snap::Sixteenth => SIXTEENTH_TICKS,
        }
    }

    /// Short label for menus.
    pub const fn label(self) -> &'static str {
        match self {
            Snap::Off => "Off",
            Snap::Bar => "Bar",
            Snap::Beat => "1/4",
            Snap::Eighth => "1/8",
            Snap::Sixteenth => "1/16",
        }
    }

    /// Rounds `ticks` to the nearest grid line.
    pub fn round(self, ticks: i64) -> i64 {
        let grid = self.ticks();
        if grid <= 1 {
            return ticks;
        }
        (ticks as f64 / grid as f64).round() as i64 * grid
    }

    /// Rounds `ticks` down to the grid line at or before it.
    pub fn floor(self, ticks: i64) -> i64 {
        let grid = self.ticks();
        ticks.div_euclid(grid) * grid
    }
}

/// Maps ticks to x coordinates for one view.
///
/// `origin_x` is the screen x of the view's left edge, `scroll` the tick at
/// that edge and `px_per_beat` the zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeMap {
    /// Screen x of the left edge of the time area.
    pub origin_x: f32,
    /// Tick shown at `origin_x`.
    pub scroll: f64,
    /// Zoom: pixels per beat.
    pub px_per_beat: f32,
}

impl Default for TimeMap {
    fn default() -> Self {
        Self {
            origin_x: 0.0,
            scroll: 0.0,
            px_per_beat: 24.0,
        }
    }
}

impl TimeMap {
    /// A map with the given left edge, scroll and zoom (zoom is clamped).
    pub fn new(origin_x: f32, scroll: f64, px_per_beat: f32) -> Self {
        Self {
            origin_x,
            scroll: scroll.max(0.0),
            px_per_beat: px_per_beat.clamp(MIN_PX_PER_BEAT, MAX_PX_PER_BEAT),
        }
    }

    /// Pixels per tick.
    pub fn px_per_tick(&self) -> f64 {
        f64::from(self.px_per_beat) / TICKS_PER_BEAT as f64
    }

    /// Screen x of a tick position.
    pub fn x_of(&self, ticks: f64) -> f32 {
        self.origin_x + ((ticks - self.scroll) * self.px_per_tick()) as f32
    }

    /// Screen x of a [`Ticks`] value.
    pub fn x_of_ticks(&self, ticks: Ticks) -> f32 {
        self.x_of(ticks.get() as f64)
    }

    /// Width in pixels of a span of ticks.
    pub fn width_of(&self, ticks: i64) -> f32 {
        (ticks as f64 * self.px_per_tick()) as f32
    }

    /// The tick (fractional) under a screen x.
    pub fn ticks_at(&self, x: f32) -> f64 {
        self.scroll + f64::from(x - self.origin_x) / self.px_per_tick()
    }

    /// The tick under `x`, snapped to the nearest grid line and clamped at 0.
    pub fn snapped_at(&self, x: f32, snap: Snap) -> i64 {
        snap.round(self.ticks_at(x).round() as i64).max(0)
    }

    /// Ticks covered by `dx` pixels, snapped to the grid.
    pub fn snapped_delta(&self, dx: f32, snap: Snap) -> i64 {
        snap.round((f64::from(dx) / self.px_per_tick()).round() as i64)
    }

    /// Zooms by `factor`, keeping the tick under `anchor_x` where it is.
    pub fn zoomed_around(&self, anchor_x: f32, factor: f32) -> Self {
        let anchor = self.ticks_at(anchor_x);
        let px_per_beat = (self.px_per_beat * factor).clamp(MIN_PX_PER_BEAT, MAX_PX_PER_BEAT);
        let px_per_tick = f64::from(px_per_beat) / TICKS_PER_BEAT as f64;
        let scroll = anchor - f64::from(anchor_x - self.origin_x) / px_per_tick;
        Self {
            origin_x: self.origin_x,
            scroll: scroll.max(0.0),
            px_per_beat,
        }
    }

    /// Scrolls by `dx` pixels (positive moves later in time).
    pub fn scrolled_by(&self, dx: f32) -> Self {
        Self {
            scroll: (self.scroll + f64::from(dx) / self.px_per_tick()).max(0.0),
            ..*self
        }
    }

    /// The finest grid worth drawing at this zoom: lines at least `min_px`
    /// apart, chosen from sixteenths, beats, bars and groups of bars.
    pub fn grid_step(&self, min_px: f32) -> i64 {
        let candidates = [
            SIXTEENTH_TICKS,
            TICKS_PER_BEAT,
            BAR_TICKS,
            BAR_TICKS * 2,
            BAR_TICKS * 4,
            BAR_TICKS * 8,
            BAR_TICKS * 16,
        ];
        candidates
            .into_iter()
            .find(|step| self.width_of(*step) >= min_px)
            .unwrap_or(BAR_TICKS * 32)
    }

    /// Grid lines between screen x `left` and `right` at `step` ticks.
    pub fn grid_lines(&self, left: f32, right: f32, step: i64) -> Vec<GridLine> {
        let step = step.max(1);
        let first = (self.ticks_at(left).max(0.0) as i64).div_euclid(step) * step;
        let last = self.ticks_at(right).ceil() as i64;
        (0..)
            .map(|i| first + i * step)
            .take_while(|ticks| *ticks <= last)
            .map(|ticks| GridLine {
                ticks,
                x: self.x_of(ticks as f64),
                kind: GridKind::of(ticks),
            })
            .collect()
    }
}

/// How strong a grid line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridKind {
    /// Start of a bar.
    Bar,
    /// A beat inside a bar.
    Beat,
    /// A subdivision inside a beat.
    Sub,
}

impl GridKind {
    /// Classifies a tick position.
    pub fn of(ticks: i64) -> Self {
        if ticks.rem_euclid(BAR_TICKS) == 0 {
            GridKind::Bar
        } else if ticks.rem_euclid(TICKS_PER_BEAT) == 0 {
            GridKind::Beat
        } else {
            GridKind::Sub
        }
    }
}

/// One vertical grid line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLine {
    /// Position in ticks.
    pub ticks: i64,
    /// Screen x.
    pub x: f32,
    /// Bar, beat or subdivision.
    pub kind: GridKind,
}

/// Formats a tick position as `bar.beat.sixteenth`, counting from 1.
pub fn bar_beat_label(ticks: i64) -> String {
    let ticks = ticks.max(0);
    let bar = ticks / BAR_TICKS + 1;
    let beat = (ticks % BAR_TICKS) / TICKS_PER_BEAT + 1;
    let sixteenth = (ticks % TICKS_PER_BEAT) / SIXTEENTH_TICKS + 1;
    format!("{bar}.{beat}.{sixteenth}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_and_pixels_round_trip() {
        let map = TimeMap::new(100.0, 960.0, 48.0);
        assert_eq!(map.x_of(960.0), 100.0);
        assert_eq!(map.x_of(1920.0), 148.0);
        for ticks in [0.0, 240.0, 960.0, 12_345.0, 384_000.0] {
            let back = map.ticks_at(map.x_of(ticks));
            assert!((back - ticks).abs() < 1e-3, "{ticks} -> {back}");
        }
        assert_eq!(map.width_of(BAR_TICKS), 192.0);
    }

    #[test]
    fn snapping_rounds_to_the_nearest_grid_line() {
        assert_eq!(Snap::Beat.round(1439), 960);
        assert_eq!(Snap::Beat.round(1441), 1920);
        assert_eq!(Snap::Sixteenth.round(130), 240);
        assert_eq!(Snap::Bar.round(1900), 0);
        assert_eq!(Snap::Bar.round(1921), BAR_TICKS);
        assert_eq!(Snap::Off.round(1234), 1234);
        assert_eq!(Snap::Beat.floor(1919), 960);
        assert_eq!(Snap::Beat.floor(-1), -960);
        let map = TimeMap::new(0.0, 0.0, 96.0);
        // 96 px per beat: x = 130 is 1.354 beats, nearest beat 1.
        assert_eq!(map.snapped_at(130.0, Snap::Beat), 960);
        assert_eq!(map.snapped_at(-50.0, Snap::Beat), 0, "never before 0");
        assert_eq!(map.snapped_delta(-100.0, Snap::Beat), -960);
    }

    #[test]
    fn zoom_keeps_the_anchor_still_and_clamps() {
        let map = TimeMap::new(50.0, 3840.0, 24.0);
        let anchor_x = 300.0;
        let before = map.ticks_at(anchor_x);
        let zoomed = map.zoomed_around(anchor_x, 2.0);
        assert_eq!(zoomed.px_per_beat, 48.0);
        assert!((zoomed.ticks_at(anchor_x) - before).abs() < 1e-6);
        let far = map.zoomed_around(anchor_x, 1e9);
        assert_eq!(far.px_per_beat, MAX_PX_PER_BEAT);
        let near = map.zoomed_around(anchor_x, 1e-9);
        assert_eq!(near.px_per_beat, MIN_PX_PER_BEAT);
        assert!(near.scroll >= 0.0);
    }

    #[test]
    fn scrolling_never_goes_before_zero() {
        let map = TimeMap::new(0.0, 960.0, 24.0);
        assert_eq!(map.scrolled_by(24.0).scroll, 1920.0);
        assert_eq!(map.scrolled_by(-1000.0).scroll, 0.0);
    }

    #[test]
    fn grid_gets_coarser_as_you_zoom_out() {
        let close = TimeMap::new(0.0, 0.0, 200.0);
        assert_eq!(close.grid_step(12.0), SIXTEENTH_TICKS);
        let mid = TimeMap::new(0.0, 0.0, 24.0);
        assert_eq!(mid.grid_step(12.0), TICKS_PER_BEAT);
        let far = TimeMap::new(0.0, 0.0, 2.0);
        assert_eq!(far.grid_step(12.0), BAR_TICKS * 2);
        let lines = mid.grid_lines(0.0, 96.0 * 2.0, TICKS_PER_BEAT);
        assert_eq!(lines.len(), 9);
        assert_eq!(lines[0].kind, GridKind::Bar);
        assert_eq!(lines[1].kind, GridKind::Beat);
        assert_eq!(lines[4].kind, GridKind::Bar);
        assert_eq!(GridKind::of(240), GridKind::Sub);
    }

    #[test]
    fn positions_print_as_bars_beats_and_sixteenths() {
        assert_eq!(bar_beat_label(0), "1.1.1");
        assert_eq!(bar_beat_label(BAR_TICKS + 960 + 480), "2.2.3");
        assert_eq!(bar_beat_label(-5), "1.1.1");
    }
}
