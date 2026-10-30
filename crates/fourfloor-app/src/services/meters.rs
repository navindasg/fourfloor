//! Lock-free meter state shared between the audio callback and the UI.
//!
//! The callback writes peaks with `fetch_max` (non-negative finite `f32`
//! bit patterns order the same way as the numbers); the UI takes and clears
//! them once per frame. Load and dropout counters live here too.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use fourfloor_core::DeviceId;

/// The most tracks whose post-fader level is metered.
pub const MAX_TAPS: usize = 128;

/// Which graph nodes to meter, in slot order. Fixed size so it can cross
/// the audio-thread queue without allocating.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapPlan {
    nodes: [DeviceId; MAX_TAPS],
    len: usize,
}

impl Default for TapPlan {
    fn default() -> Self {
        Self {
            nodes: [DeviceId::new(0); MAX_TAPS],
            len: 0,
        }
    }
}

impl TapPlan {
    /// A plan metering `nodes` (anything past [`MAX_TAPS`] is ignored).
    pub fn new(nodes: &[DeviceId]) -> Self {
        let mut plan = Self::default();
        for (slot, node) in plan.nodes.iter_mut().zip(nodes) {
            *slot = *node;
        }
        plan.len = nodes.len().min(MAX_TAPS);
        plan
    }

    /// The metered nodes.
    pub fn nodes(&self) -> &[DeviceId] {
        &self.nodes[..self.len]
    }
}

/// Peak of a block of samples, with NaN and infinities read as 0.
pub fn block_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .map(|sample| {
            if sample.is_finite() {
                sample.abs()
            } else {
                0.0
            }
        })
        .fold(0.0, f32::max)
}

/// Atomics written by the audio callback and read by the UI.
#[derive(Debug)]
pub struct MeterBank {
    taps: Vec<[AtomicU32; 2]>,
    master: [AtomicU32; 2],
    load: AtomicU32,
    peak_load: AtomicU32,
    xruns: AtomicU32,
    stream_errors: AtomicU32,
    callbacks: AtomicU64,
    dropped_analysis: AtomicU32,
}

impl Default for MeterBank {
    fn default() -> Self {
        Self::new()
    }
}

fn store_max(cell: &AtomicU32, value: f32) {
    if value.is_finite() && value > 0.0 {
        cell.fetch_max(value.to_bits(), Ordering::Relaxed);
    }
}

fn take(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.swap(0, Ordering::Relaxed))
}

impl MeterBank {
    /// A bank with [`MAX_TAPS`] stereo slots. Allocates once, here.
    pub fn new() -> Self {
        Self {
            taps: (0..MAX_TAPS)
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
            master: [AtomicU32::new(0), AtomicU32::new(0)],
            load: AtomicU32::new(0),
            peak_load: AtomicU32::new(0),
            xruns: AtomicU32::new(0),
            stream_errors: AtomicU32::new(0),
            callbacks: AtomicU64::new(0),
            dropped_analysis: AtomicU32::new(0),
        }
    }

    /// Raises the held peak of tap `slot`, channel `channel` (0 or 1).
    pub fn record_tap(&self, slot: usize, channel: usize, peak: f32) {
        if let Some(cell) = self.taps.get(slot).and_then(|tap| tap.get(channel)) {
            store_max(cell, peak);
        }
    }

    /// Raises the held master peak of `channel`.
    pub fn record_master(&self, channel: usize, peak: f32) {
        if let Some(cell) = self.master.get(channel) {
            store_max(cell, peak);
        }
    }

    /// Takes and clears the held peaks of tap `slot` as `[left, right]`.
    pub fn take_tap(&self, slot: usize) -> [f32; 2] {
        self.taps
            .get(slot)
            .map_or([0.0; 2], |tap| [take(&tap[0]), take(&tap[1])])
    }

    /// Takes and clears the held master peaks.
    pub fn take_master(&self) -> [f32; 2] {
        [take(&self.master[0]), take(&self.master[1])]
    }

    /// Stores the callback's load: time spent rendering over the time the
    /// buffer lasts (1.0 means it only just kept up).
    pub fn record_load(&self, load: f32) {
        if load.is_finite() && load >= 0.0 {
            self.load.store(load.to_bits(), Ordering::Relaxed);
            store_max(&self.peak_load, load);
        }
    }

    /// The most recent load.
    pub fn load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }

    /// Takes and clears the highest load since the last call.
    pub fn take_peak_load(&self) -> f32 {
        take(&self.peak_load)
    }

    /// Counts a dropout (a callback that ran late or overran its buffer).
    pub fn record_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Dropouts counted so far.
    pub fn xruns(&self) -> u32 {
        self.xruns.load(Ordering::Relaxed)
    }

    /// Counts an error reported by the audio backend.
    pub fn record_stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Backend errors counted so far.
    pub fn stream_errors(&self) -> u32 {
        self.stream_errors.load(Ordering::Relaxed)
    }

    /// Counts one audio callback.
    pub fn record_callback(&self) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
    }

    /// Callbacks run so far.
    pub fn callbacks(&self) -> u64 {
        self.callbacks.load(Ordering::Relaxed)
    }

    /// Counts master samples the analysis queue had no room for.
    pub fn record_dropped_analysis(&self, samples: u32) {
        self.dropped_analysis.fetch_add(samples, Ordering::Relaxed);
    }

    /// Master samples dropped before analysis so far.
    pub fn dropped_analysis(&self) -> u32 {
        self.dropped_analysis.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_hold_the_maximum_until_taken() {
        let bank = MeterBank::new();
        bank.record_tap(3, 0, 0.25);
        bank.record_tap(3, 0, 0.75);
        bank.record_tap(3, 0, 0.5);
        bank.record_tap(3, 1, f32::NAN);
        bank.record_tap(MAX_TAPS + 5, 0, 1.0);
        assert_eq!(bank.take_tap(3), [0.75, 0.0]);
        assert_eq!(bank.take_tap(3), [0.0, 0.0], "taking clears");
        bank.record_master(1, 0.9);
        assert_eq!(bank.take_master(), [0.0, 0.9]);
    }

    #[test]
    fn load_and_counters() {
        let bank = MeterBank::new();
        bank.record_load(0.2);
        bank.record_load(0.6);
        bank.record_load(0.1);
        assert_eq!(bank.load(), 0.1);
        assert_eq!(bank.take_peak_load(), 0.6);
        bank.record_xrun();
        bank.record_xrun();
        bank.record_stream_error();
        assert_eq!((bank.xruns(), bank.stream_errors()), (2, 1));
    }

    #[test]
    fn tap_plans_cap_at_the_slot_count() {
        let ids: Vec<DeviceId> = (0..200).map(DeviceId::new).collect();
        assert_eq!(TapPlan::new(&ids).nodes().len(), MAX_TAPS);
        assert_eq!(TapPlan::new(&ids[..3]).nodes(), &ids[..3]);
        assert_eq!(block_peak(&[0.1, -0.8, f32::INFINITY, 0.3]), 0.8);
    }
}
