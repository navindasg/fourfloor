//! Where a deck is in its track: position, loop and slip.

/// A loop in track samples. Positions may be fractional, so a beat loop
/// keeps the track's tempo exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopRegion {
    /// First sample of the loop.
    pub start: f64,
    /// Where the loop jumps back (exclusive).
    pub end: f64,
}

impl LoopRegion {
    /// Shortest loop a deck accepts, in samples.
    pub const MIN_LENGTH: f64 = 32.0;

    /// A loop, or `None` when it is shorter than [`Self::MIN_LENGTH`] or
    /// not finite.
    pub fn new(start: f64, end: f64) -> Option<Self> {
        (start.is_finite() && end.is_finite() && end - start >= Self::MIN_LENGTH)
            .then_some(Self { start, end })
    }

    /// Length in samples.
    pub fn length(&self) -> f64 {
        self.end - self.start
    }
}

/// Position state of a deck. Plain data, no allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Playhead {
    /// Position in track samples.
    pub pos: f64,
    /// Length of the track in samples.
    pub end: f64,
    /// The loop being played, if any.
    pub active: Option<LoopRegion>,
    /// The last loop set, for reloop.
    pub saved: Option<LoopRegion>,
    /// Where the track would be without the loop roll in progress.
    pub slip: Option<f64>,
}

impl Playhead {
    pub fn new() -> Self {
        Self {
            pos: 0.0,
            end: 0.0,
            active: None,
            saved: None,
            slip: None,
        }
    }

    /// Moves forward by `step` samples, wrapping at the loop end. Returns
    /// true when the end of the track was reached.
    #[inline]
    pub fn advance(&mut self, step: f64) -> bool {
        self.pos += step;
        if let Some(slip) = &mut self.slip {
            *slip += step;
        }
        if let Some(region) = self.active {
            if self.pos >= region.end {
                self.pos = region.start + (self.pos - region.end) % region.length();
            }
        }
        self.pos >= self.end
    }

    /// Where a read at `position` lands once the active loop is applied:
    /// audio past the loop end comes from the loop start.
    #[inline]
    pub fn map(&self, position: i64) -> i64 {
        match self.active {
            Some(region) if position as f64 >= region.end => {
                let past = (position as f64 - region.end) % region.length();
                (region.start + past).floor() as i64
            }
            _ => position,
        }
    }

    /// [`Self::map`] for a fractional position.
    #[inline]
    pub fn map_fractional(&self, position: f64) -> f64 {
        match self.active {
            Some(region) if position >= region.end => {
                region.start + (position - region.end) % region.length()
            }
            _ => position,
        }
    }

    pub fn clamp(&self, position: f64) -> f64 {
        if position.is_finite() {
            position.clamp(0.0, self.end)
        } else {
            self.pos
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_exactly_at_an_integer_loop() {
        let mut head = Playhead::new();
        head.end = 10_000.0;
        head.active = LoopRegion::new(100.0, 200.0);
        head.pos = 198.0;
        assert!(!head.advance(1.0));
        assert_eq!(head.pos, 199.0);
        head.advance(1.0);
        assert_eq!(head.pos, 100.0);
        head.pos = 199.5;
        head.advance(1.0);
        assert_eq!(head.pos, 100.5);
    }

    #[test]
    fn slip_keeps_running_through_a_loop() {
        let mut head = Playhead::new();
        head.end = 10_000.0;
        head.pos = 150.0;
        head.slip = Some(150.0);
        head.active = LoopRegion::new(100.0, 200.0);
        for _ in 0..500 {
            head.advance(1.0);
        }
        assert_eq!(head.slip, Some(650.0));
        assert_eq!(head.pos, 150.0);
    }

    #[test]
    fn reports_the_end_and_rejects_tiny_loops() {
        let mut head = Playhead::new();
        head.end = 4.0;
        head.pos = 2.0;
        assert!(!head.advance(1.0));
        assert!(head.advance(1.0));
        assert_eq!(LoopRegion::new(0.0, 8.0), None);
        assert_eq!(LoopRegion::new(0.0, f64::NAN), None);
        assert_eq!(head.map(1_000), 1_000);
        head.active = LoopRegion::new(100.0, 200.0);
        assert_eq!(head.map(150), 150);
        assert_eq!(head.map(205), 105);
        assert_eq!(head.map(305), 105);
    }
}
