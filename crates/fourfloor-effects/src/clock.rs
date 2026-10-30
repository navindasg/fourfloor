//! Phase for tempo-synced modulation.

use fourfloor_core::TransportContext;

/// The phase of a cycle across one block: `at(frame)` is in `0.0..1.0`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockPhase {
    start: f64,
    increment: f64,
}

impl BlockPhase {
    #[inline]
    pub(crate) fn at(&self, frame: usize, offset: f64) -> f64 {
        (self.start + frame as f64 * self.increment + offset).rem_euclid(1.0)
    }
}

/// Produces a cycle phase per block.
///
/// While the transport rolls, a synced phase is read from the timeline
/// position, so it is the same after a seek or a loop and identical in
/// every render: beat 0 is phase 0. While the transport is stopped the
/// phase keeps running freely at the same speed, so the effect can still be
/// heard on live input.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PhaseClock {
    free: f64,
}

impl PhaseClock {
    /// A cycle that lasts `period_beats` beats.
    pub(crate) fn synced(
        &mut self,
        ctx: &TransportContext,
        period_beats: f64,
        frames: usize,
    ) -> BlockPhase {
        let period = period_beats.max(1.0e-3);
        let increment = 1.0 / (period * ctx.tempo.samples_per_beat(ctx.sample_rate));
        let start = if ctx.playing {
            (ctx.position_beats.0 / period).rem_euclid(1.0)
        } else {
            self.free
        };
        self.advance(start, increment, frames)
    }

    /// A cycle that repeats `hz` times a second, whatever the transport
    /// does.
    pub(crate) fn free(&mut self, hz: f64, sample_rate: f64, frames: usize) -> BlockPhase {
        self.advance(self.free, hz / sample_rate, frames)
    }

    fn advance(&mut self, start: f64, increment: f64, frames: usize) -> BlockPhase {
        self.free = (start + frames as f64 * increment).rem_euclid(1.0);
        BlockPhase { start, increment }
    }

    pub(crate) fn reset(&mut self) {
        self.free = 0.0;
    }
}
