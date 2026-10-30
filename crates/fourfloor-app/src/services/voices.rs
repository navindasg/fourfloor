//! Extra sources the audio callback mixes over the project: the DJ decks'
//! graph (master on top of the mix, cue to outputs 3 and 4) and a one-shot
//! audition voice for the sample browser.
//!
//! Both are real-time safe: buffers are preallocated, replaced graphs and
//! audio leave the audio thread through queues and are freed on the UI
//! thread.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use fourfloor_core::{DeviceId, SampleRate, Tempo, TransportContext};
use fourfloor_engine::CompiledGraph;
use rtrb::Producer;

use super::callback::ReferenceAudio;
use super::meters::block_peak;

/// Decks a DJ graph can have metered.
pub const MAX_DECK_METERS: usize = 4;

/// Which DJ graph nodes to meter: each deck's strip output. Copy and fixed
/// size so it crosses the queue without allocating.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeckTaps {
    /// Strip output of each deck, in deck order.
    pub strips: [Option<DeviceId>; MAX_DECK_METERS],
}

/// Peaks of each deck strip, written by the callback, taken by the UI.
#[derive(Debug, Default)]
pub struct DeckMeters {
    peaks: [[AtomicU32; 2]; MAX_DECK_METERS],
}

impl DeckMeters {
    fn record(&self, deck: usize, channel: usize, peak: f32) {
        if let Some(slot) = self.peaks.get(deck).and_then(|d| d.get(channel)) {
            slot.fetch_max(peak.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Takes and clears the held peaks of every deck.
    pub fn take(&self) -> [[f32; 2]; MAX_DECK_METERS] {
        let mut out = [[0.0; 2]; MAX_DECK_METERS];
        for (deck, channels) in self.peaks.iter().enumerate() {
            for (channel, slot) in channels.iter().enumerate() {
                out[deck][channel] = f32::from_bits(slot.swap(0, Ordering::Relaxed));
            }
        }
        out
    }
}

/// The DJ graph as the callback runs it.
pub struct DjSlot {
    graph: Option<Box<CompiledGraph>>,
    taps: DeckTaps,
    tempo: Tempo,
    position: u64,
    pending_retire: Option<Box<CompiledGraph>>,
    retired_tx: Producer<Box<CompiledGraph>>,
    meters: Arc<DeckMeters>,
    sample_rate: SampleRate,
}

impl std::fmt::Debug for DjSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DjSlot")
            .field("installed", &self.graph.is_some())
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl DjSlot {
    /// An empty slot.
    pub fn new(
        sample_rate: SampleRate,
        retired_tx: Producer<Box<CompiledGraph>>,
        meters: Arc<DeckMeters>,
    ) -> Self {
        Self {
            graph: None,
            taps: DeckTaps::default(),
            tempo: Tempo::DEFAULT,
            position: 0,
            pending_retire: None,
            retired_tx,
            meters,
            sample_rate,
        }
    }

    /// `true` while a DJ graph runs.
    pub fn is_installed(&self) -> bool {
        self.graph.is_some()
    }

    /// Installs `graph` (or removes the current one with `None`).
    pub fn install(&mut self, graph: Option<Box<CompiledGraph>>, taps: DeckTaps) {
        self.taps = taps;
        self.position = 0;
        if let Some(old) = std::mem::replace(&mut self.graph, graph) {
            self.retire(old);
        }
    }

    /// Follows the tempo master (for tempo-synced deck effects).
    pub fn set_tempo(&mut self, tempo: Tempo) {
        self.tempo = tempo;
    }

    /// Sets a parameter of a DJ graph node.
    pub fn set_param(&mut self, device: DeviceId, param: fourfloor_core::ParamId, value: f32) {
        if let Some(graph) = self.graph.as_deref_mut() {
            graph.set_param(device, param, value);
        }
    }

    /// Hands a retired graph that did not fit the queue to the UI thread.
    pub fn flush_retired(&mut self) {
        if let Some(waiting) = self.pending_retire.take() {
            if let Err(rtrb::PushError::Full(back)) = self.retired_tx.push(waiting) {
                self.pending_retire = Some(back);
            }
        }
    }

    fn retire(&mut self, old: Box<CompiledGraph>) {
        self.flush_retired();
        if self.pending_retire.is_some() {
            // The UI drains the queue every frame; only a stalled UI gets
            // here. Leak rather than free on the audio thread.
            std::mem::forget(old);
            return;
        }
        if let Err(rtrb::PushError::Full(back)) = self.retired_tx.push(old) {
            self.pending_retire = Some(back);
        }
    }

    /// Renders `frames` of the DJ graph, adds its master to `master`
    /// (interleaved stereo) and writes its cue mix to `cue` (interleaved
    /// stereo, overwritten). Does nothing without a graph.
    pub fn render(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize) {
        let Some(graph) = self.graph.as_deref_mut() else {
            return;
        };
        let ctx = TransportContext::playing_at(self.sample_rate, self.tempo, self.position);
        graph.process(&ctx, frames);
        self.position += frames as u64;
        for channel in 0..2 {
            if let Some(out) = graph.output(channel) {
                for (pair, sample) in master.chunks_exact_mut(2).zip(out) {
                    if sample.is_finite() {
                        pair[channel] += sample;
                    }
                }
            }
            match graph.output(channel + 2) {
                Some(out) => {
                    for (pair, sample) in cue.chunks_exact_mut(2).zip(out) {
                        pair[channel] = if sample.is_finite() { *sample } else { 0.0 };
                    }
                }
                None => {
                    for pair in cue.chunks_exact_mut(2) {
                        pair[channel] = 0.0;
                    }
                }
            }
        }
        for (deck, strip) in self.taps.strips.iter().enumerate() {
            let Some(strip) = strip else { continue };
            for channel in 0..2 {
                if let Some(out) = graph.node_output(*strip, channel) {
                    self.meters.record(deck, channel, block_peak(out));
                }
            }
        }
    }
}

/// A sound auditioned once from its start, over everything else.
#[derive(Debug, Default)]
pub struct PreviewVoice {
    audio: Option<Arc<ReferenceAudio>>,
    position: usize,
}

impl PreviewVoice {
    /// Starts `audio` from the top (or stops with `None`); returns what it
    /// replaced, for the caller to retire.
    pub fn replace(&mut self, audio: Option<Arc<ReferenceAudio>>) -> Option<Arc<ReferenceAudio>> {
        self.position = 0;
        std::mem::replace(&mut self.audio, audio)
    }

    /// `true` while sounding.
    pub fn is_playing(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| self.position < audio.frames())
    }

    /// Adds the next `master.len() / 2` frames to `master` (stereo).
    pub fn render(&mut self, master: &mut [f32]) {
        let Some(audio) = self.audio.as_deref() else {
            return;
        };
        let frames = audio.frames();
        for pair in master.chunks_exact_mut(2) {
            if self.position >= frames {
                break;
            }
            let at = self.position * 2;
            pair[0] += audio.samples[at];
            pair[1] += audio.samples[at + 1];
            self.position += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_plays_once_from_the_top() {
        let mut voice = PreviewVoice::default();
        let audio = Arc::new(ReferenceAudio {
            samples: vec![0.5, -0.5, 0.25, -0.25, 0.125, -0.125],
        });
        assert!(voice.replace(Some(audio)).is_none());
        let mut block = [0.0f32; 4];
        voice.render(&mut block);
        assert_eq!(block, [0.5, -0.5, 0.25, -0.25]);
        let mut block = [1.0f32; 4];
        voice.render(&mut block);
        assert_eq!(block, [1.125, 0.875, 1.0, 1.0]);
        assert!(!voice.is_playing());
    }

    #[test]
    fn deck_meters_hold_the_peak_until_taken() {
        let meters = DeckMeters::default();
        meters.record(1, 0, 0.25);
        meters.record(1, 0, 0.5);
        meters.record(1, 0, 0.1);
        meters.record(9, 0, 1.0);
        let taken = meters.take();
        assert_eq!(taken[1][0], 0.5);
        assert_eq!(meters.take()[1][0], 0.0);
    }
}
