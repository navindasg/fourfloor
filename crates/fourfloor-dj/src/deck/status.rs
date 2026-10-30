//! What a deck reports back to the control side, through atomics.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::deck::HOT_CUES;

/// An `f64` stored in an `AtomicU64`.
#[derive(Debug)]
pub(crate) struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }

    #[inline]
    pub fn store(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Live deck state written by the audio thread after every block. Values
/// are in track samples; NaN means "not set".
#[derive(Debug)]
pub(crate) struct DeckStatus {
    pub position: AtomicF64,
    pub rate: AtomicF64,
    pub bpm: AtomicF64,
    pub cue_point: AtomicF64,
    pub loop_start: AtomicF64,
    pub loop_end: AtomicF64,
    pub hot_cues: [AtomicF64; HOT_CUES],
    pub playing: AtomicBool,
    pub roll: AtomicBool,
    pub at_end: AtomicBool,
    /// How many `Load` / `Eject` commands the deck has carried out.
    pub loads: AtomicU64,
    /// Tracks the deck could not hand back for freeing (leaked instead).
    pub leaked: AtomicU64,
}

impl DeckStatus {
    pub fn new() -> Self {
        Self {
            position: AtomicF64::new(0.0),
            rate: AtomicF64::new(1.0),
            bpm: AtomicF64::new(f64::NAN),
            cue_point: AtomicF64::new(0.0),
            loop_start: AtomicF64::new(f64::NAN),
            loop_end: AtomicF64::new(f64::NAN),
            hot_cues: std::array::from_fn(|_| AtomicF64::new(f64::NAN)),
            playing: AtomicBool::new(false),
            roll: AtomicBool::new(false),
            at_end: AtomicBool::new(false),
            loads: AtomicU64::new(0),
            leaked: AtomicU64::new(0),
        }
    }
}

/// A copy of a deck's live state, in track samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckSnapshot {
    /// Playhead position in track samples.
    pub position: f64,
    /// Playback rate in the last block (1 is normal speed), including
    /// sync, bend and phase correction.
    pub rate: f64,
    /// Sounding tempo, when the track has a grid.
    pub bpm: Option<f64>,
    /// The cue point.
    pub cue_point: f64,
    /// The active loop, as start and end samples.
    pub loop_region: Option<(f64, f64)>,
    /// Whether the active loop is a loop roll.
    pub roll: bool,
    /// Hot cue positions.
    pub hot_cues: [Option<f64>; HOT_CUES],
    /// Whether the deck is playing.
    pub playing: bool,
    /// Whether the deck stopped at the end of its track.
    pub at_end: bool,
    /// Load and eject commands carried out so far.
    pub loads: u64,
}

fn finite(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

impl DeckStatus {
    pub fn snapshot(&self) -> DeckSnapshot {
        let (start, end) = (self.loop_start.load(), self.loop_end.load());
        DeckSnapshot {
            position: self.position.load(),
            rate: self.rate.load(),
            bpm: finite(self.bpm.load()),
            cue_point: self.cue_point.load(),
            loop_region: (start.is_finite() && end.is_finite()).then_some((start, end)),
            roll: self.roll.load(Ordering::Relaxed),
            hot_cues: std::array::from_fn(|i| finite(self.hot_cues[i].load())),
            playing: self.playing.load(Ordering::Relaxed),
            at_end: self.at_end.load(Ordering::Relaxed),
            loads: self.loads.load(Ordering::Acquire),
        }
    }
}
