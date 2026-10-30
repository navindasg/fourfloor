//! The running engine: an [`AudioProcessor`] for the audio thread and an
//! [`EngineController`] for everything else.
//!
//! Traffic between the two:
//!
//! - commands in: an `rtrb` queue of core's [`AudioMessage`], drained at the
//!   start of each
//!   [`process_interleaved`](AudioProcessor::process_interleaved) call;
//! - graphs in: an `rtrb` queue of boxed [`CompiledGraph`]s, installed when
//!   the matching `AudioMessage::SwapGraph` arrives, so a swap is ordered
//!   with the other commands and always happens at a block boundary;
//! - graphs out: an `rtrb` queue that carries replaced graphs back so they
//!   are dropped off the audio thread;
//! - state out: atomics for the playhead, tempo, play state and peak meters.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use assert_no_alloc::assert_no_alloc;
use fourfloor_core::{
    AudioMessage, Beats, DeviceId, EventKind, GraphHandle, ParamId, SamplePos, SampleRate, Tempo,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::graph::{CompiledGraph, GraphError, MAX_CHANNELS};
use crate::transport::{LoopRegion, Transport};

/// Sizes and rates fixed for the life of an engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    /// Sample rate of the output.
    pub sample_rate: SampleRate,
    /// Largest block the graph is asked to process. Longer callbacks are
    /// split.
    pub max_block: usize,
    /// Channels the graph produces.
    pub channels: usize,
    /// Tempo at start.
    pub tempo: Tempo,
    /// Capacity of the command queue.
    pub message_capacity: usize,
    /// How many compiled graphs may wait to be swapped in.
    pub graph_capacity: usize,
}

impl EngineConfig {
    /// Stereo, 512-frame blocks, 120 BPM, room for 1024 commands and 8
    /// pending graphs.
    pub fn new(sample_rate: SampleRate) -> Self {
        Self {
            sample_rate,
            max_block: 512,
            channels: 2,
            tempo: Tempo::DEFAULT,
            message_capacity: 1024,
            graph_capacity: 8,
        }
    }
}

/// Why an engine call failed.
#[derive(Debug)]
pub enum EngineError {
    /// The command queue is full; the audio thread is not draining it.
    QueueFull,
    /// The pending-graph queue is full. The graph is handed back.
    GraphQueueFull(Box<CompiledGraph>),
    /// The graph was compiled for a different rate, block size or channel
    /// count than the engine runs at. The graph is handed back.
    GraphMismatch(Box<CompiledGraph>),
    /// The configuration could not produce a graph.
    Graph(GraphError),
    /// A queue capacity in the configuration is zero.
    InvalidCapacity,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => write!(f, "the engine command queue is full"),
            Self::GraphQueueFull(_) => write!(f, "too many graphs are waiting to be swapped in"),
            Self::GraphMismatch(_) => write!(
                f,
                "the graph's sample rate, block size or channel count does not match the engine"
            ),
            Self::Graph(error) => write!(f, "graph error: {error}"),
            Self::InvalidCapacity => write!(f, "queue capacities must be at least 1"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<GraphError> for EngineError {
    fn from(error: GraphError) -> Self {
        Self::Graph(error)
    }
}

/// State the audio thread publishes for other threads to read.
#[derive(Debug)]
struct Shared {
    playhead: AtomicU64,
    playing: AtomicBool,
    tempo_bits: AtomicU64,
    /// Peak since the last read, per channel, as `f32` bits. Non-negative
    /// floats order the same as their bits, so `fetch_max` works.
    peaks: [AtomicU32; MAX_CHANNELS],
    graph_handle: AtomicU32,
    leaked_graphs: AtomicU32,
}

type PendingGraph = (GraphHandle, Box<CompiledGraph>);

/// Creates an engine running `initial` (use [`CompiledGraph::empty`] for
/// silence). Returns the control half and the audio-thread half.
pub fn engine(
    config: EngineConfig,
    initial: CompiledGraph,
) -> Result<(EngineController, AudioProcessor), EngineError> {
    if config.message_capacity == 0 || config.graph_capacity == 0 {
        return Err(EngineError::InvalidCapacity);
    }
    if !matches(&config, &initial) {
        return Err(EngineError::GraphMismatch(Box::new(initial)));
    }
    let (message_tx, message_rx) = RingBuffer::new(config.message_capacity);
    let (graph_tx, graph_rx) = RingBuffer::<PendingGraph>::new(config.graph_capacity);
    // One more than the graphs that can be pending, so that with the
    // controller draining before every push this queue can never be full.
    let (retired_tx, retired_rx) = RingBuffer::new(config.graph_capacity + 1);

    let shared = Arc::new(Shared {
        playhead: AtomicU64::new(0),
        playing: AtomicBool::new(false),
        tempo_bits: AtomicU64::new(config.tempo.bpm().to_bits()),
        peaks: std::array::from_fn(|_| AtomicU32::new(0)),
        graph_handle: AtomicU32::new(0),
        leaked_graphs: AtomicU32::new(0),
    });

    let controller = EngineController {
        config,
        message_tx,
        graph_tx,
        retired_rx,
        shared: Arc::clone(&shared),
        next_handle: 1,
    };
    let processor = AudioProcessor {
        config,
        graph: Box::new(initial),
        transport: Transport::new(config.sample_rate, config.tempo),
        message_rx,
        graph_rx,
        retired_tx,
        shared,
    };
    Ok((controller, processor))
}

fn matches(config: &EngineConfig, graph: &CompiledGraph) -> bool {
    graph.sample_rate() == config.sample_rate
        && graph.max_block() == config.max_block
        && graph.channels() == config.channels
}

/// The control half of the engine. Lives off the audio thread; may allocate.
#[derive(Debug)]
pub struct EngineController {
    config: EngineConfig,
    message_tx: Producer<AudioMessage>,
    graph_tx: Producer<PendingGraph>,
    retired_rx: Consumer<Box<CompiledGraph>>,
    shared: Arc<Shared>,
    next_handle: u32,
}

impl EngineController {
    /// The configuration the engine was created with.
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Queues a command. It takes effect at the start of the next block.
    pub fn send(&mut self, message: AudioMessage) -> Result<(), EngineError> {
        self.message_tx
            .push(message)
            .map_err(|_| EngineError::QueueFull)
    }

    /// Starts the transport.
    pub fn play(&mut self) -> Result<(), EngineError> {
        self.send(AudioMessage::Play)
    }

    /// Stops the transport, keeping its position.
    pub fn stop(&mut self) -> Result<(), EngineError> {
        self.send(AudioMessage::Stop)
    }

    /// Moves the playhead. Devices are reset.
    pub fn seek(&mut self, position: Beats) -> Result<(), EngineError> {
        self.send(AudioMessage::Seek { position })
    }

    /// Changes the tempo, keeping the beat position.
    pub fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError> {
        self.send(AudioMessage::SetTempo { tempo })
    }

    /// Sets a device parameter, bypassing the graph compiler.
    pub fn set_param(
        &mut self,
        device: DeviceId,
        param: ParamId,
        value: f32,
    ) -> Result<(), EngineError> {
        self.send(AudioMessage::SetParam {
            device,
            param,
            value,
        })
    }

    /// Sends a note or controller event to a device (normally an
    /// instrument). It arrives at the first frame of the next block, whether
    /// or not the transport rolls.
    pub fn send_event(&mut self, device: DeviceId, event: EventKind) -> Result<(), EngineError> {
        self.send(AudioMessage::SendEvent { device, event })
    }

    /// Sets or clears the loop region.
    pub fn set_loop(&mut self, region: Option<LoopRegion>) -> Result<(), EngineError> {
        self.send(match region {
            Some(LoopRegion { start, end }) => AudioMessage::SetLoop { start, end },
            None => AudioMessage::ClearLoop,
        })
    }

    /// Hands a newly compiled graph to the audio thread, which installs it
    /// at its next block boundary, after any commands queued before this
    /// call. Returns the handle that
    /// [`current_graph`](EngineController::current_graph) reports once the
    /// swap has happened.
    ///
    /// Also drops any graphs the audio thread has retired.
    pub fn swap_graph(&mut self, graph: CompiledGraph) -> Result<GraphHandle, EngineError> {
        self.collect_garbage();
        if !matches(&self.config, &graph) {
            return Err(EngineError::GraphMismatch(Box::new(graph)));
        }
        // This is the only producer, so free slots cannot disappear between
        // these checks and the pushes below.
        if self.message_tx.slots() == 0 {
            return Err(EngineError::QueueFull);
        }
        let handle = GraphHandle(self.next_handle);
        match self.graph_tx.push((handle, Box::new(graph))) {
            Ok(()) => {}
            Err(rtrb::PushError::Full((_, graph))) => {
                return Err(EngineError::GraphQueueFull(graph));
            }
        }
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        self.send(AudioMessage::SwapGraph { graph: handle })?;
        Ok(handle)
    }

    /// Drops graphs the audio thread has swapped out. Returns how many.
    /// Call it now and then (it also runs on every
    /// [`swap_graph`](EngineController::swap_graph)).
    pub fn collect_garbage(&mut self) -> usize {
        let mut dropped = 0;
        while let Ok(graph) = self.retired_rx.pop() {
            drop(graph);
            dropped += 1;
        }
        dropped
    }

    /// Handle of the graph the audio thread is running. `GraphHandle(0)` is
    /// the graph the engine was created with.
    pub fn current_graph(&self) -> GraphHandle {
        GraphHandle(self.shared.graph_handle.load(Ordering::Relaxed))
    }

    /// Playhead position in samples, as of the end of the last block.
    pub fn playhead_samples(&self) -> SamplePos {
        self.shared.playhead.load(Ordering::Relaxed)
    }

    /// Playhead position in beats at the engine's current tempo.
    pub fn playhead_beats(&self) -> Beats {
        Beats::from_sample_pos(
            self.playhead_samples(),
            self.tempo(),
            self.config.sample_rate,
        )
    }

    /// Whether the transport was rolling at the end of the last block.
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// The engine's tempo as of the last block.
    pub fn tempo(&self) -> Tempo {
        let bpm = f64::from_bits(self.shared.tempo_bits.load(Ordering::Relaxed));
        Tempo::new(bpm).unwrap_or(self.config.tempo)
    }

    /// Highest absolute output sample on `channel` since the last
    /// [`take_peak`](EngineController::take_peak). Zero for channels the
    /// engine does not have.
    pub fn peak(&self, channel: usize) -> f32 {
        self.shared
            .peaks
            .get(channel)
            .map_or(0.0, |p| f32::from_bits(p.load(Ordering::Relaxed)))
    }

    /// Reads the peak for `channel` and resets it to zero.
    pub fn take_peak(&self, channel: usize) -> f32 {
        self.shared
            .peaks
            .get(channel)
            .map_or(0.0, |p| f32::from_bits(p.swap(0, Ordering::Relaxed)))
    }

    /// Graphs that could not be returned for dropping and were leaked
    /// instead. Stays zero unless the retire queue invariant is broken.
    pub fn leaked_graphs(&self) -> u32 {
        self.shared.leaked_graphs.load(Ordering::Relaxed)
    }
}

/// The audio-thread half of the engine. Owns the graph snapshot and the
/// transport. Nothing it does while processing allocates, locks or logs.
#[derive(Debug)]
pub struct AudioProcessor {
    config: EngineConfig,
    graph: Box<CompiledGraph>,
    transport: Transport,
    message_rx: Consumer<AudioMessage>,
    graph_rx: Consumer<PendingGraph>,
    retired_tx: Producer<Box<CompiledGraph>>,
    shared: Arc<Shared>,
}

impl AudioProcessor {
    /// The configuration the engine was created with.
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Channels the graph produces.
    pub fn channels(&self) -> usize {
        self.config.channels
    }

    /// The transport, for inspection.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// The graph currently installed, for inspection.
    pub fn graph(&self) -> &CompiledGraph {
        &self.graph
    }

    /// The audio callback. Applies queued commands, then fills `out`
    /// (interleaved, `out_channels` wide) with the next
    /// `out.len() / out_channels` frames. Graph channels beyond
    /// `out_channels` are dropped; output channels beyond the graph's are
    /// silent. Trailing samples that do not make a whole frame are zeroed.
    ///
    /// Runs under `assert_no_alloc` in debug builds: in a debug build of a binary that
    /// installs [`AllocDisabler`](crate::AllocDisabler), any allocation in
    /// here aborts.
    pub fn process_interleaved(&mut self, out: &mut [f32], out_channels: usize) {
        if cfg!(debug_assertions) {
            assert_no_alloc(|| self.process_unguarded(out, out_channels));
        } else {
            self.process_unguarded(out, out_channels);
        }
    }

    fn process_unguarded(&mut self, out: &mut [f32], out_channels: usize) {
        self.apply_messages();
        if out_channels == 0 {
            out.fill(0.0);
            return;
        }
        let whole = out.len() - out.len() % out_channels;
        let (frames_out, tail) = out.split_at_mut(whole);
        tail.fill(0.0);

        let mut rest = frames_out;
        while !rest.is_empty() {
            let wanted = (rest.len() / out_channels).min(self.config.max_block);
            // Never let a block cross the loop end: positions inside a
            // block stay contiguous, so devices stay sample-accurate.
            let frames = match self.transport.frames_until_jump() {
                Some(until) => wanted.min(usize::try_from(until).unwrap_or(usize::MAX)),
                None => wanted,
            }
            .max(1);
            let (segment, remaining) = rest.split_at_mut(frames * out_channels);
            self.process_segment(segment, out_channels, frames);
            rest = remaining;
        }

        self.shared
            .playhead
            .store(self.transport.position_samples(), Ordering::Relaxed);
        self.shared
            .playing
            .store(self.transport.is_playing(), Ordering::Relaxed);
        self.shared
            .tempo_bits
            .store(self.transport.tempo().bpm().to_bits(), Ordering::Relaxed);
    }

    fn process_segment(&mut self, segment: &mut [f32], out_channels: usize, frames: usize) {
        let ctx = self.transport.context();
        self.graph.process(&ctx, frames);
        self.transport.advance(frames);

        for channel in 0..out_channels {
            let target = segment.iter_mut().skip(channel).step_by(out_channels);
            match self.graph.output(channel) {
                Some(source) => {
                    let mut peak = 0.0f32;
                    for (t, s) in target.zip(source.iter()) {
                        *t = *s;
                        peak = peak.max(s.abs());
                    }
                    if let (Some(meter), true) = (self.shared.peaks.get(channel), peak.is_finite())
                    {
                        meter.fetch_max(peak.to_bits(), Ordering::Relaxed);
                    }
                }
                None => target.for_each(|t| *t = 0.0),
            }
        }
    }

    fn apply_messages(&mut self) {
        while let Ok(message) = self.message_rx.pop() {
            self.apply_audio_message(message);
        }
    }

    fn apply_audio_message(&mut self, message: AudioMessage) {
        match message {
            AudioMessage::SetParam {
                device,
                param,
                value,
            } => {
                self.graph.set_param(device, param, value);
            }
            AudioMessage::SendEvent { device, event } => {
                self.graph.send_event(device, event);
            }
            AudioMessage::Play => self.transport.play(),
            AudioMessage::Stop => self.transport.stop(),
            AudioMessage::Seek { position } => {
                self.transport.seek(position);
                self.graph.reset();
            }
            AudioMessage::SetTempo { tempo } => self.transport.set_tempo(tempo),
            AudioMessage::SetLoop { start, end } => {
                self.transport.set_loop(LoopRegion::new(start, end));
            }
            AudioMessage::ClearLoop => self.transport.clear_loop(),
            AudioMessage::SwapGraph { graph } => self.swap_graph(graph),
        }
    }

    /// Installs the pending graph with this handle. Graphs queued before it
    /// that were never asked for are retired unused.
    fn swap_graph(&mut self, handle: GraphHandle) {
        while let Ok((pending_handle, pending)) = self.graph_rx.pop() {
            if pending_handle == handle {
                let old = std::mem::replace(&mut self.graph, pending);
                self.retire(old);
                self.shared.graph_handle.store(handle.0, Ordering::Relaxed);
                return;
            }
            self.retire(pending);
        }
    }

    /// Sends a graph back to be dropped off the audio thread.
    fn retire(&mut self, graph: Box<CompiledGraph>) {
        if let Err(rtrb::PushError::Full(graph)) = self.retired_tx.push(graph) {
            // Dropping here would free memory on the audio thread. The queue
            // is sized so this cannot happen; if it somehow does, leaking is
            // the real-time-safe choice.
            std::mem::forget(graph);
            self.shared.leaked_graphs.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
