//! What runs inside the audio callback: the engine's processor plus the
//! app's extras (per-track meters, a metronome, the master analysis feed and
//! reference A/B playback).
//!
//! Real-time rules hold here: every buffer is allocated in
//! [`AudioCallback::new`], and nothing in [`AudioCallback::render`]
//! allocates, frees, locks or logs. Replaced reference audio goes back to
//! the UI thread through a queue to be freed there.

use std::sync::Arc;

use fourfloor_core::{
    Device, EventBuffer, PrepareConfig, ProcessIo, SampleRate, Tempo, TransportContext,
};
use fourfloor_core::{DeviceId, ParamId};
use fourfloor_engine::devices::Metronome;
use fourfloor_engine::{AudioProcessor, CompiledGraph};
use rtrb::{Consumer, Producer, RingBuffer};

use super::meters::{block_peak, MeterBank, TapPlan};
use super::voices::{DeckMeters, DeckTaps, DjSlot, PreviewVoice};

/// Channels the engine renders (the compiler builds stereo graphs).
pub const ENGINE_CHANNELS: usize = 2;

/// Stereo reference audio at the engine's rate, interleaved.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceAudio {
    /// Interleaved left/right samples.
    pub samples: Vec<f32>,
}

impl ReferenceAudio {
    /// Frames of audio.
    pub fn frames(&self) -> usize {
        self.samples.len() / ENGINE_CHANNELS
    }
}

/// What the output plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    /// `true` plays the reference instead of the mix.
    pub reference: bool,
    /// Linear gain applied to the reference (loudness matching).
    pub reference_gain: f32,
    /// Metronome clicks on top of the output while playing.
    pub metronome: bool,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            reference: false,
            reference_gain: 1.0,
            metronome: false,
        }
    }
}

/// Messages from the UI thread to the callback.
///
/// The tap plan is inline on purpose: boxing it would free the box on the
/// audio thread.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum CallbackMessage {
    /// Meter these nodes from now on.
    Taps(TapPlan),
    /// Replace the reference audio (or clear it with `None`).
    Reference(Option<Arc<ReferenceAudio>>),
    /// Change what is monitored.
    Monitor(Monitor),
    /// Install the DJ decks' graph (or remove it with `None`). The old
    /// graph comes back through [`CallbackLink::retired_graphs`].
    DjGraph(Option<Box<CompiledGraph>>, DeckTaps),
    /// A parameter change for a DJ graph node.
    DjParam {
        /// The node.
        device: DeviceId,
        /// The parameter.
        param: ParamId,
        /// Plain value.
        value: f32,
    },
    /// The DJ tempo master's tempo.
    DjTempo(Tempo),
    /// Audition a sound once (or stop with `None`).
    Preview(Option<Arc<ReferenceAudio>>),
}

/// The UI side of the callback's queues.
#[derive(Debug)]
pub struct CallbackLink {
    /// Messages to the callback.
    pub tx: Producer<CallbackMessage>,
    /// Interleaved stereo master samples for analysis.
    pub master_rx: Consumer<f32>,
    /// Reference buffers the callback is done with, to free here.
    pub retired_rx: Consumer<Arc<ReferenceAudio>>,
    /// DJ graphs the callback is done with, to free here.
    pub retired_graphs: Consumer<Box<CompiledGraph>>,
    /// Peaks of the DJ deck strips.
    pub deck_meters: Arc<DeckMeters>,
}

impl CallbackLink {
    /// Frees retired reference buffers and graphs. Call from the UI thread.
    pub fn collect_garbage(&mut self) {
        while self.retired_rx.pop().is_ok() {}
        while self.retired_graphs.pop().is_ok() {}
    }
}

/// The audio-thread half. Move it into the output stream (or keep it for
/// offline pumping) and call [`render`](Self::render) per buffer.
pub struct AudioCallback {
    processor: AudioProcessor,
    meters: Arc<MeterBank>,
    taps: TapPlan,
    rx: Consumer<CallbackMessage>,
    master_tx: Producer<f32>,
    retired_tx: Producer<Arc<ReferenceAudio>>,
    pending_retire: Option<Arc<ReferenceAudio>>,
    reference: Option<Arc<ReferenceAudio>>,
    monitor: Monitor,
    scratch: Vec<f32>,
    click: Vec<f32>,
    click_events: EventBuffer,
    metronome: Metronome,
    dj: DjSlot,
    preview: PreviewVoice,
    cue: Vec<f32>,
    sample_rate: SampleRate,
    max_block: usize,
}

impl std::fmt::Debug for AudioCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioCallback")
            .field("sample_rate", &self.sample_rate)
            .field("max_block", &self.max_block)
            .finish_non_exhaustive()
    }
}

/// Seconds of master audio the analysis queue can hold.
const ANALYSIS_SECONDS: usize = 2;

impl AudioCallback {
    /// Wraps `processor`. Allocates every buffer the callback will use.
    pub fn new(processor: AudioProcessor, meters: Arc<MeterBank>) -> (Self, CallbackLink) {
        let config = processor.config();
        let max_block = config.max_block.max(1);
        let rate = config.sample_rate;
        // Room for a burst of DJ parameter changes between two callbacks.
        let (tx, rx) = RingBuffer::new(512);
        let (master_tx, master_rx) =
            RingBuffer::new(rate.hz() as usize * ENGINE_CHANNELS * ANALYSIS_SECONDS);
        let (retired_tx, retired_rx) = RingBuffer::new(8);
        let (graph_tx, retired_graphs) = RingBuffer::new(4);
        let deck_meters = Arc::new(DeckMeters::default());
        let mut metronome = Metronome::new(0.4);
        metronome.prepare(&PrepareConfig::new(rate, max_block, 1));
        let callback = Self {
            processor,
            meters,
            taps: TapPlan::default(),
            rx,
            master_tx,
            retired_tx,
            pending_retire: None,
            reference: None,
            monitor: Monitor::default(),
            scratch: vec![0.0; max_block * ENGINE_CHANNELS],
            click: vec![0.0; max_block],
            click_events: EventBuffer::with_capacity(4),
            metronome,
            dj: DjSlot::new(rate, graph_tx, Arc::clone(&deck_meters)),
            preview: PreviewVoice::default(),
            cue: vec![0.0; max_block * ENGINE_CHANNELS],
            sample_rate: rate,
            max_block,
        };
        let link = CallbackLink {
            tx,
            master_rx,
            retired_rx,
            retired_graphs,
            deck_meters,
        };
        (callback, link)
    }

    /// The engine's sample rate.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// The shared meters.
    pub fn meters(&self) -> &Arc<MeterBank> {
        &self.meters
    }

    fn flush_retired(&mut self) {
        if let Some(waiting) = self.pending_retire.take() {
            if let Err(rtrb::PushError::Full(back)) = self.retired_tx.push(waiting) {
                self.pending_retire = Some(back);
            }
        }
    }

    fn retire(&mut self, old: Arc<ReferenceAudio>) {
        self.flush_retired();
        if self.pending_retire.is_some() {
            // The UI drains this queue every frame, so this only happens
            // when it has stalled. Leak rather than free on this thread.
            std::mem::forget(old);
            return;
        }
        if let Err(rtrb::PushError::Full(back)) = self.retired_tx.push(old) {
            self.pending_retire = Some(back);
        }
    }

    fn apply_messages(&mut self) {
        while let Ok(message) = self.rx.pop() {
            match message {
                CallbackMessage::Taps(plan) => self.taps = plan,
                CallbackMessage::Monitor(monitor) => self.monitor = monitor,
                CallbackMessage::Reference(reference) => {
                    if let Some(old) = std::mem::replace(&mut self.reference, reference) {
                        self.retire(old);
                    }
                }
                CallbackMessage::DjGraph(graph, taps) => self.dj.install(graph, taps),
                CallbackMessage::DjParam {
                    device,
                    param,
                    value,
                } => self.dj.set_param(device, param, value),
                CallbackMessage::DjTempo(tempo) => self.dj.set_tempo(tempo),
                CallbackMessage::Preview(audio) => {
                    if let Some(old) = self.preview.replace(audio) {
                        self.retire(old);
                    }
                }
            }
        }
        self.flush_retired();
        self.dj.flush_retired();
    }

    /// Renders `out` (interleaved, `out_channels` wide). Real-time safe.
    pub fn render(&mut self, out: &mut [f32], out_channels: usize) {
        // Taps first: a graph swap queued before these taps is applied by
        // the processor inside this same call, so they always match.
        self.apply_messages();
        if out_channels == 0 {
            return;
        }
        let total = out.len() / out_channels;
        let mut done = 0;
        while done < total {
            let frames = (total - done).min(self.max_block);
            self.render_block(frames);
            let block = &self.scratch[..frames * ENGINE_CHANNELS];
            let target = &mut out[done * out_channels..(done + frames) * out_channels];
            write_out(block, target, out_channels);
            if out_channels >= 4 && self.dj.is_installed() {
                write_cue(&self.cue[..frames * ENGINE_CHANNELS], target, out_channels);
            }
            done += frames;
        }
        let tail = total * out_channels;
        out[tail..].fill(0.0);
        self.meters.record_callback();
    }

    fn render_block(&mut self, frames: usize) {
        let samples = frames * ENGINE_CHANNELS;
        self.processor
            .process_interleaved(&mut self.scratch[..samples], ENGINE_CHANNELS);
        self.meter_taps();
        let ctx = self.block_context(frames);
        if self.monitor.reference {
            self.play_reference(&ctx, frames);
        }
        self.dj.render(
            &mut self.scratch[..samples],
            &mut self.cue[..samples],
            frames,
        );
        self.preview.render(&mut self.scratch[..samples]);
        self.feed_analysis(samples);
        if self.monitor.metronome && ctx.playing {
            self.add_clicks(&ctx, frames);
        }
        let block = &self.scratch[..samples];
        for channel in 0..ENGINE_CHANNELS {
            let peak = block
                .iter()
                .skip(channel)
                .step_by(ENGINE_CHANNELS)
                .map(|sample| {
                    if sample.is_finite() {
                        sample.abs()
                    } else {
                        0.0
                    }
                })
                .fold(0.0, f32::max);
            self.meters.record_master(channel, peak);
        }
    }

    /// The transport context of the block just rendered: the processor has
    /// already advanced, so step back by the block length.
    fn block_context(&self, frames: usize) -> TransportContext {
        let after = self.processor.transport().context();
        let start = after.position_samples.saturating_sub(frames as u64);
        let tempo: Tempo = after.tempo;
        if after.playing {
            TransportContext::playing_at(self.sample_rate, tempo, start)
        } else {
            TransportContext::stopped(self.sample_rate, tempo)
        }
    }

    fn meter_taps(&mut self) {
        let graph = self.processor.graph();
        for (slot, node) in self.taps.nodes().iter().enumerate() {
            for channel in 0..ENGINE_CHANNELS {
                if let Some(output) = graph.node_output(*node, channel) {
                    self.meters.record_tap(slot, channel, block_peak(output));
                }
            }
        }
    }

    fn play_reference(&mut self, ctx: &TransportContext, frames: usize) {
        let gain = self.monitor.reference_gain;
        let block = &mut self.scratch[..frames * ENGINE_CHANNELS];
        let Some(reference) = self.reference.as_deref() else {
            block.fill(0.0);
            return;
        };
        let length = reference.frames();
        if length == 0 || !ctx.playing {
            block.fill(0.0);
            return;
        }
        let start = (ctx.position_samples % length as u64) as usize;
        for (frame, pair) in block.chunks_exact_mut(ENGINE_CHANNELS).enumerate() {
            let source = (start + frame) % length * ENGINE_CHANNELS;
            for (channel, sample) in pair.iter_mut().enumerate() {
                *sample = reference.samples[source + channel] * gain;
            }
        }
    }

    fn feed_analysis(&mut self, samples: usize) {
        if self.master_tx.slots() < samples {
            self.meters.record_dropped_analysis(samples as u32);
            return;
        }
        for sample in &self.scratch[..samples] {
            // Cannot fail: the free slots were checked above.
            let _ = self.master_tx.push(*sample);
        }
    }

    fn add_clicks(&mut self, ctx: &TransportContext, frames: usize) {
        self.click_events.clear();
        let click = &mut self.click[..frames];
        let mut outputs: [&mut [f32]; 1] = [click];
        let mut io = ProcessIo {
            inputs: &[],
            sidechain: None,
            outputs: &mut outputs,
            events: &[],
            events_out: &mut self.click_events,
        };
        self.metronome.process(ctx, &mut io);
        let click = &self.click[..frames];
        for (pair, value) in self.scratch.chunks_exact_mut(ENGINE_CHANNELS).zip(click) {
            for sample in pair {
                *sample += value;
            }
        }
    }
}

/// Copies an engine stereo block to a device buffer of any width: mono
/// gets the average, extra channels get silence.
fn write_out(block: &[f32], target: &mut [f32], out_channels: usize) {
    for (frame, out) in block
        .chunks_exact(ENGINE_CHANNELS)
        .zip(target.chunks_exact_mut(out_channels))
    {
        match out_channels {
            1 => out[0] = 0.5 * (frame[0] + frame[1]),
            _ => {
                out[0] = frame[0];
                out[1] = frame[1];
                for extra in &mut out[2..] {
                    *extra = 0.0;
                }
            }
        }
    }
}

/// Copies the DJ cue mix (stereo) to device channels 3 and 4.
fn write_cue(cue: &[f32], target: &mut [f32], out_channels: usize) {
    for (pair, out) in cue
        .chunks_exact(2)
        .zip(target.chunks_exact_mut(out_channels))
    {
        out[2] = pair[0];
        out[3] = pair[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mapping_handles_mono_and_wide_devices() {
        let block = [0.5, -0.5, 1.0, 0.0];
        let mut mono = [9.0; 2];
        write_out(&block, &mut mono, 1);
        assert_eq!(mono, [0.0, 0.5]);
        let mut wide = [9.0; 8];
        write_out(&block, &mut wide, 4);
        assert_eq!(wide, [0.5, -0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    }
}
