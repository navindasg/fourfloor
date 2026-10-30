//! A circular delay line.

/// A delay line whose length is a power of two. Allocated by
/// [`DelayLine::prepare`]; before that it holds one sample, so processing an
/// unprepared device cannot index out of range.
#[derive(Clone, Debug)]
pub(crate) struct DelayLine {
    buffer: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    pub(crate) fn new() -> Self {
        Self {
            buffer: vec![0.0; 1],
            mask: 0,
            write: 0,
        }
    }

    /// Makes room for delays up to `max_samples`. Allocates; clears the
    /// line.
    pub(crate) fn prepare(&mut self, max_samples: usize) {
        let length = (max_samples + 2).next_power_of_two();
        if self.buffer.len() != length {
            self.buffer = vec![0.0; length];
        } else {
            self.buffer.fill(0.0);
        }
        self.mask = length - 1;
        self.write = 0;
    }

    /// The longest delay [`tap`](Self::tap) can read.
    pub(crate) fn max_delay(&self) -> usize {
        self.mask
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.fill(0.0);
    }

    /// The sample pushed `delay` pushes ago; `tap(1)` is the latest one.
    /// Read before pushing the current sample to get a delay of exactly
    /// `delay` samples.
    #[inline]
    pub(crate) fn tap(&self, delay: usize) -> f32 {
        let delay = delay.clamp(1, self.mask.max(1));
        self.buffer[self.write.wrapping_sub(delay) & self.mask]
    }

    /// [`tap`](Self::tap) at a fractional delay, by linear interpolation.
    /// Exact when `delay` is a whole number.
    #[inline]
    pub(crate) fn tap_fractional(&self, delay: f64) -> f32 {
        let limit = self.mask.saturating_sub(1).max(1) as f64;
        let delay = delay.clamp(1.0, limit);
        let whole = delay.floor();
        let fraction = (delay - whole) as f32;
        let whole = whole as usize;
        let near = self.tap(whole);
        if fraction == 0.0 {
            near
        } else {
            near + (self.tap(whole + 1) - near) * fraction
        }
    }

    #[inline]
    pub(crate) fn push(&mut self, sample: f32) {
        self.buffer[self.write & self.mask] = sample;
        self.write = (self.write + 1) & self.mask;
    }
}
