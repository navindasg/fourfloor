//! The audio graph: a builder for describing devices and connections, and
//! the compiled, preallocated execution plan the audio thread runs.
//!
//! # Model
//!
//! Every node produces one audio bus of `channels` channels (stereo unless
//! the graph is built otherwise) and one stream of events. A node has three
//! kinds of input, named by [`Input`]:
//!
//! - zero or more *audio ports*, each `channels` wide. A port receives the
//!   sum of every edge connected to it. The device sees its ports one after
//!   another: `inputs[port * channels + channel]`.
//! - an optional *sidechain* bus, `channels` wide: the sum of every
//!   sidechain edge. The device sees it as `ProcessIo::sidechain`, which is
//!   `None` when no sidechain edge exists.
//! - an *event* input: the merged events of every event edge, plus events
//!   sent to the node with [`CompiledGraph::send_event`].
//!
//! Compiling (off the audio thread) calls [`Device::prepare`], sorts the
//! nodes topologically across all three kinds of edge, computes delay
//! compensation from each device's [`Device::latency_samples`] and
//! allocates every buffer and delay line. [`CompiledGraph::process`] then
//! runs without allocating.
//!
//! Delay compensation covers audio and sidechain edges. Event edges are not
//! delayed: put event sources and the instruments they feed at the head of
//! a chain, before anything that reports latency.

use std::fmt;

use fourfloor_core::{
    Device, DeviceId, Event, EventBuffer, EventKind, ParamId, PrepareConfig, ProcessIo, SampleRate,
    TransportContext,
};

use crate::delay::DelayLine;

mod control;

pub use control::{
    lane_value, ControlPlan, ControlPoint, ControlTarget, LfoRate, LfoShape, ModulatorSpec,
};

/// Most channels a graph can carry.
pub const MAX_CHANNELS: usize = 8;
/// Most audio input ports one node can have.
pub const MAX_INPUT_PORTS: usize = 16;
/// Events one node can receive or emit in one block unless
/// [`GraphBuilder::set_event_capacity`] says otherwise.
pub const DEFAULT_EVENT_CAPACITY: usize = 1024;

const MAX_INPUT_SLICES: usize = MAX_CHANNELS * MAX_INPUT_PORTS;
/// Events [`CompiledGraph::send_event`] can queue for one node per block.
const LIVE_EVENT_CAPACITY: usize = 64;

/// Which input of a node an edge feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    /// Audio input port `n`.
    Port(usize),
    /// The sidechain bus.
    Sidechain,
    /// The event input.
    Events,
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Port(port) => write!(f, "port {port}"),
            Self::Sidechain => f.write_str("sidechain"),
            Self::Events => f.write_str("events"),
        }
    }
}

/// Why a graph could not be built or compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// Channel count is zero or above [`MAX_CHANNELS`].
    InvalidChannelCount(usize),
    /// Maximum block size is zero.
    InvalidBlockSize,
    /// A node asked for more than [`MAX_INPUT_PORTS`] input ports.
    TooManyPorts(DeviceId),
    /// Two nodes share an id.
    DuplicateNode(DeviceId),
    /// An edge or the output refers to a node that was never added.
    UnknownNode(DeviceId),
    /// An edge targets a port the destination node does not have.
    InvalidPort {
        /// Destination node.
        node: DeviceId,
        /// The port that does not exist.
        port: usize,
    },
    /// The same connection was added twice.
    DuplicateEdge {
        /// Source node.
        from: DeviceId,
        /// Destination node.
        to: DeviceId,
        /// Destination input.
        input: Input,
    },
    /// The connections form a cycle through this node.
    Cycle(DeviceId),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChannelCount(n) => {
                write!(f, "channel count {n} is not in 1..={MAX_CHANNELS}")
            }
            Self::InvalidBlockSize => write!(f, "maximum block size must be at least 1"),
            Self::TooManyPorts(id) => {
                write!(f, "node {id} has more than {MAX_INPUT_PORTS} input ports")
            }
            Self::DuplicateNode(id) => write!(f, "node {id} was added twice"),
            Self::UnknownNode(id) => write!(f, "node {id} does not exist"),
            Self::InvalidPort { node, port } => {
                write!(f, "node {node} has no input port {port}")
            }
            Self::DuplicateEdge { from, to, input } => {
                write!(f, "edge {from} -> {to} {input} was added twice")
            }
            Self::Cycle(id) => write!(f, "the graph has a cycle through node {id}"),
        }
    }
}

impl std::error::Error for GraphError {}

struct NodeSpec {
    id: DeviceId,
    device: Box<dyn Device>,
    ports: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct EdgeSpec {
    from: DeviceId,
    to: DeviceId,
    input: Input,
}

/// An editable description of a graph. Build it off the audio thread, then
/// [`compile`](GraphBuilder::compile) it.
pub struct GraphBuilder {
    channels: usize,
    nodes: Vec<NodeSpec>,
    edges: Vec<EdgeSpec>,
    output: Option<DeviceId>,
    compensate: bool,
    event_capacity: usize,
    control: ControlPlan,
}

impl GraphBuilder {
    /// An empty graph in which every bus carries `channels` channels.
    pub fn new(channels: usize) -> Self {
        Self {
            channels,
            nodes: Vec::new(),
            edges: Vec::new(),
            output: None,
            compensate: true,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            control: ControlPlan::default(),
        }
    }

    /// Sets the automation lanes and modulators the graph drives by itself
    /// (see [`ControlPlan`]). Every target must name a node of the graph,
    /// or [`compile`](GraphBuilder::compile) fails with
    /// [`GraphError::UnknownNode`].
    pub fn set_control(&mut self, plan: ControlPlan) {
        self.control = plan;
    }

    /// An empty stereo graph: the default.
    pub fn stereo() -> Self {
        Self::new(2)
    }

    /// Adds a node with `input_ports` audio input ports (zero for an
    /// instrument or other source).
    pub fn add_node(
        &mut self,
        id: DeviceId,
        device: Box<dyn Device>,
        input_ports: usize,
    ) -> Result<(), GraphError> {
        if self.nodes.iter().any(|n| n.id == id) {
            return Err(GraphError::DuplicateNode(id));
        }
        if input_ports > MAX_INPUT_PORTS {
            return Err(GraphError::TooManyPorts(id));
        }
        self.nodes.push(NodeSpec {
            id,
            device,
            ports: input_ports,
        });
        Ok(())
    }

    /// Connects the audio output of `from` to audio input port `port` of
    /// `to`. Several edges into one port are summed.
    pub fn connect(&mut self, from: DeviceId, to: DeviceId, port: usize) -> Result<(), GraphError> {
        self.add_edge(from, to, Input::Port(port))
    }

    /// Connects the audio output of `from` to the sidechain bus of `to`.
    /// Several sidechain edges are summed.
    pub fn connect_sidechain(&mut self, from: DeviceId, to: DeviceId) -> Result<(), GraphError> {
        self.add_edge(from, to, Input::Sidechain)
    }

    /// Connects the event output of `from` to the event input of `to`.
    /// Events from several sources are merged in time order.
    pub fn connect_events(&mut self, from: DeviceId, to: DeviceId) -> Result<(), GraphError> {
        self.add_edge(from, to, Input::Events)
    }

    fn add_edge(&mut self, from: DeviceId, to: DeviceId, input: Input) -> Result<(), GraphError> {
        let edge = EdgeSpec { from, to, input };
        if !self.nodes.iter().any(|n| n.id == from) {
            return Err(GraphError::UnknownNode(from));
        }
        let dest = self
            .nodes
            .iter()
            .find(|n| n.id == to)
            .ok_or(GraphError::UnknownNode(to))?;
        if let Input::Port(port) = input {
            if port >= dest.ports {
                return Err(GraphError::InvalidPort { node: to, port });
            }
        }
        if self.edges.contains(&edge) {
            return Err(GraphError::DuplicateEdge { from, to, input });
        }
        self.edges.push(edge);
        Ok(())
    }

    /// Chooses the node whose audio output is the graph's output. Without
    /// one the graph renders silence.
    pub fn set_output(&mut self, id: DeviceId) -> Result<(), GraphError> {
        if !self.nodes.iter().any(|n| n.id == id) {
            return Err(GraphError::UnknownNode(id));
        }
        self.output = Some(id);
        Ok(())
    }

    /// Turns delay compensation on or off (on by default). Off is meant for
    /// tests and diagnostics.
    pub fn set_delay_compensation(&mut self, enabled: bool) {
        self.compensate = enabled;
    }

    /// Sets how many events each node can receive, and emit, in one block
    /// (default [`DEFAULT_EVENT_CAPACITY`]). Events beyond that are dropped
    /// and counted in [`CompiledGraph::dropped_events`].
    pub fn set_event_capacity(&mut self, events: usize) {
        self.event_capacity = events;
    }

    /// Compiles the description into an execution plan for blocks of at
    /// most `max_block` frames. Prepares every device and allocates all
    /// buffers; call it off the audio thread.
    pub fn compile(
        self,
        sample_rate: SampleRate,
        max_block: usize,
    ) -> Result<CompiledGraph, GraphError> {
        let channels = self.channels;
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(GraphError::InvalidChannelCount(channels));
        }
        if max_block == 0 {
            return Err(GraphError::InvalidBlockSize);
        }
        let config = PrepareConfig::new(sample_rate, max_block, channels);

        let order = topological_order(&self.nodes, &self.edges)?;
        // rank[spec index] = position in the execution order.
        let mut rank = vec![0usize; self.nodes.len()];
        for (position, &spec) in order.iter().enumerate() {
            rank[spec] = position;
        }
        let spec_index = |id: DeviceId| self.nodes.iter().position(|n| n.id == id);
        let output = self.output.and_then(spec_index).map(|i| rank[i]);

        // Edges grouped by destination, as (source rank, input).
        let mut incoming: Vec<Vec<(usize, Input)>> = vec![Vec::new(); self.nodes.len()];
        for edge in &self.edges {
            let (Some(from), Some(to)) = (spec_index(edge.from), spec_index(edge.to)) else {
                continue; // `add_edge` already rejected unknown nodes.
            };
            incoming[rank[to]].push((rank[from], edge.input));
        }

        let mut specs: Vec<Option<NodeSpec>> = self.nodes.into_iter().map(Some).collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(specs.len());
        // Latency accumulated at each node's audio output, in execution order.
        let mut latency_out: Vec<u32> = Vec::with_capacity(specs.len());

        for (position, &spec) in order.iter().enumerate() {
            let Some(NodeSpec {
                id,
                mut device,
                ports,
            }) = specs[spec].take()
            else {
                continue; // Each index appears once in a topological order.
            };
            device.prepare(&config);
            let own_latency = device.latency_samples();

            let sources = &incoming[position];
            let audio_sources = || {
                sources
                    .iter()
                    .filter(|(_, input)| *input != Input::Events)
                    .copied()
            };
            let input_latency = audio_sources()
                .map(|(src, _)| latency_out[src])
                .max()
                .unwrap_or(0);
            let edges: Vec<Edge> = audio_sources()
                .map(|(src, input)| {
                    let delay = if self.compensate {
                        input_latency - latency_out[src]
                    } else {
                        0
                    };
                    Edge {
                        src,
                        input,
                        delay_samples: delay,
                        delay: (delay > 0).then(|| DelayLine::new(channels, delay as usize)),
                    }
                })
                .collect();
            let has_sidechain = edges.iter().any(|edge| edge.input == Input::Sidechain);
            let event_sources = sources
                .iter()
                .filter(|(_, input)| *input == Input::Events)
                .map(|&(src, _)| src)
                .collect();

            latency_out.push(input_latency.saturating_add(own_latency));
            nodes.push(Node {
                id,
                device,
                ports,
                edges,
                event_sources,
                in_buf: vec![0.0; ports * channels * max_block],
                sidechain_buf: vec![0.0; usize::from(has_sidechain) * channels * max_block],
                has_sidechain,
                events_in: EventBuffer::with_capacity(self.event_capacity),
                live_events: EventBuffer::with_capacity(LIVE_EVENT_CAPACITY),
                latency: own_latency,
            });
        }

        let mut lookup: Vec<(DeviceId, usize)> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        lookup.sort_unstable_by_key(|&(id, _)| id);
        let control = control::Control::new(self.control, &nodes)?;

        Ok(CompiledGraph {
            out_bufs: vec![vec![0.0; channels * max_block]; nodes.len()],
            // Built one by one: cloning an empty buffer would not keep its
            // preallocated capacity.
            event_bufs: (0..nodes.len())
                .map(|_| EventBuffer::with_capacity(self.event_capacity))
                .collect(),
            order: nodes.iter().map(|n| n.id).collect(),
            latency: output.map_or(0, |o| latency_out[o]),
            nodes,
            lookup,
            output,
            channels,
            max_block,
            sample_rate,
            frames: 0,
            dropped_events: 0,
            control,
        })
    }
}

/// Kahn's algorithm over every kind of edge. Among nodes that are ready,
/// the one added first runs first, so the order is deterministic. Returns
/// indices into `nodes`.
fn topological_order(nodes: &[NodeSpec], edges: &[EdgeSpec]) -> Result<Vec<usize>, GraphError> {
    let index = |id: DeviceId| nodes.iter().position(|n| n.id == id);
    let mut pairs = Vec::with_capacity(edges.len());
    let mut indegree = vec![0usize; nodes.len()];
    for edge in edges {
        let from = index(edge.from).ok_or(GraphError::UnknownNode(edge.from))?;
        let to = index(edge.to).ok_or(GraphError::UnknownNode(edge.to))?;
        indegree[to] += 1;
        pairs.push((from, to));
    }

    let mut done = vec![false; nodes.len()];
    let mut order = Vec::with_capacity(nodes.len());
    while order.len() < nodes.len() {
        let Some(next) = (0..nodes.len()).find(|&i| !done[i] && indegree[i] == 0) else {
            let stuck = (0..nodes.len()).find(|&i| !done[i]).unwrap_or(0);
            return Err(GraphError::Cycle(nodes[stuck].id));
        };
        done[next] = true;
        order.push(next);
        for &(from, to) in &pairs {
            if from == next {
                indegree[to] -= 1;
            }
        }
    }
    Ok(order)
}

/// An audio or sidechain edge.
struct Edge {
    /// Source node, as an index into the execution order.
    src: usize,
    input: Input,
    delay_samples: u32,
    delay: Option<DelayLine>,
}

struct Node {
    id: DeviceId,
    device: Box<dyn Device>,
    ports: usize,
    edges: Vec<Edge>,
    /// Nodes whose event output feeds this node, as execution-order indices.
    event_sources: Vec<usize>,
    /// Planar input: `ports * channels` regions of `max_block` frames.
    in_buf: Vec<f32>,
    /// Planar sidechain: `channels` regions of `max_block` frames, or empty
    /// when the node has no sidechain edge.
    sidechain_buf: Vec<f32>,
    has_sidechain: bool,
    /// Events for the current block, merged from every source.
    events_in: EventBuffer,
    /// Events sent with `send_event`, delivered at the next block's start.
    live_events: EventBuffer,
    latency: u32,
}

/// An immutable, preallocated execution plan. The topology never changes
/// after compiling; to change it, compile a new graph and swap it in.
pub struct CompiledGraph {
    /// Nodes in execution order.
    nodes: Vec<Node>,
    /// One planar output buffer per node: `channels` regions of `max_block`.
    out_bufs: Vec<Vec<f32>>,
    /// One event output buffer per node.
    event_bufs: Vec<EventBuffer>,
    order: Vec<DeviceId>,
    /// `(id, node index)` sorted by id, for parameter routing.
    lookup: Vec<(DeviceId, usize)>,
    output: Option<usize>,
    latency: u32,
    channels: usize,
    max_block: usize,
    sample_rate: SampleRate,
    /// Frame count of the most recent block.
    frames: usize,
    dropped_events: u32,
    /// Automation lanes and modulators.
    control: control::Control,
}

impl fmt::Debug for CompiledGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledGraph")
            .field("order", &self.order)
            .field("channels", &self.channels)
            .field("max_block", &self.max_block)
            .field("sample_rate", &self.sample_rate)
            .field("latency", &self.latency)
            .finish_non_exhaustive()
    }
}

impl CompiledGraph {
    /// A graph with no nodes; renders silence.
    pub fn empty(
        channels: usize,
        sample_rate: SampleRate,
        max_block: usize,
    ) -> Result<Self, GraphError> {
        GraphBuilder::new(channels).compile(sample_rate, max_block)
    }

    /// Node ids in the order they are processed.
    pub fn order(&self) -> &[DeviceId] {
        &self.order
    }

    /// Whether the graph has a node with this id.
    pub fn contains(&self, id: DeviceId) -> bool {
        self.node_index(id).is_some()
    }

    /// Channels every bus (and the graph output) carries.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Largest block [`process`](CompiledGraph::process) accepts.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Sample rate the devices were prepared for.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Total latency at the graph output, in samples, after compensation.
    pub fn latency_samples(&self) -> u32 {
        self.latency
    }

    /// Latency the given node's device reported when the graph was compiled.
    pub fn node_latency(&self, id: DeviceId) -> Option<u32> {
        self.node_index(id).map(|i| self.nodes[i].latency)
    }

    /// Compensation delay, in samples, inserted on the audio edge
    /// `from -> to` at `port`. `None` when there is no such edge.
    pub fn edge_delay(&self, from: DeviceId, to: DeviceId, port: usize) -> Option<u32> {
        self.input_delay(from, to, Input::Port(port))
    }

    /// Compensation delay, in samples, inserted on the sidechain edge
    /// `from -> to`. `None` when there is no such edge.
    pub fn sidechain_delay(&self, from: DeviceId, to: DeviceId) -> Option<u32> {
        self.input_delay(from, to, Input::Sidechain)
    }

    fn input_delay(&self, from: DeviceId, to: DeviceId, input: Input) -> Option<u32> {
        let node = &self.nodes[self.node_index(to)?];
        node.edges
            .iter()
            .find(|e| e.input == input && self.nodes[e.src].id == from)
            .map(|e| e.delay_samples)
    }

    fn node_index(&self, id: DeviceId) -> Option<usize> {
        self.lookup
            .binary_search_by_key(&id, |&(key, _)| key)
            .ok()
            .map(|i| self.lookup[i].1)
    }

    /// Forwards a parameter change to a device. Returns whether the device
    /// exists in this graph. Safe on the audio thread.
    ///
    /// For a parameter the graph's [`ControlPlan`] drives, the value becomes
    /// the centre that modulation swings around; an automation lane on the
    /// parameter still wins.
    pub fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> bool {
        match self.node_index(device) {
            Some(i) => {
                self.nodes[i].device.set_param(param, value);
                self.control.set_base(device, param, value);
                true
            }
            None => false,
        }
    }

    /// Whether the graph drives any parameter by automation or modulation.
    pub fn has_control(&self) -> bool {
        !self.control.is_empty()
    }

    /// The output of modulator `index` of the [`ControlPlan`] in the most
    /// recent block: -1 to 1 for an LFO, 0 to 1 for an envelope.
    pub fn modulator_value(&self, index: usize) -> Option<f32> {
        self.control.modulator_value(index)
    }

    /// Queues an event for a device; it arrives at frame 0 of the next
    /// block, ahead of events from the graph. Returns `false` when the
    /// device does not exist or its queue is full. Safe on the audio thread.
    pub fn send_event(&mut self, device: DeviceId, event: EventKind) -> bool {
        match self.node_index(device) {
            Some(i) => self.nodes[i].live_events.push(Event::new(0, event)),
            None => false,
        }
    }

    /// Events dropped since the graph was compiled because a node's event
    /// buffer was full. Stays zero unless a block carries more events than
    /// the capacity set with [`GraphBuilder::set_event_capacity`].
    pub fn dropped_events(&self) -> u32 {
        self.dropped_events
    }

    /// Audio thread. Resets every device and clears the compensation
    /// delays and pending graph events, e.g. after a seek.
    pub fn reset(&mut self) {
        for node in &mut self.nodes {
            node.device.reset();
            node.events_in.clear();
            for edge in &mut node.edges {
                if let Some(delay) = &mut edge.delay {
                    delay.clear();
                }
            }
        }
        for events in &mut self.event_bufs {
            events.clear();
        }
        self.control.reset();
    }

    /// Audio thread. Processes one block of `frames` frames (clamped to
    /// [`max_block`](CompiledGraph::max_block)). Read the result with
    /// [`output`](CompiledGraph::output).
    pub fn process(&mut self, ctx: &TransportContext, frames: usize) {
        let n = frames.min(self.max_block);
        let (channels, max_block) = (self.channels, self.max_block);
        self.frames = n;
        if n == 0 {
            return;
        }
        let last_frame = (n - 1) as u32;
        let mut dropped = 0u32;
        self.control.apply(ctx, n, &mut self.nodes);

        for i in 0..self.nodes.len() {
            let node = &mut self.nodes[i];

            // Sum every incoming edge into its bus, through its delay line.
            for region in node.in_buf.chunks_exact_mut(max_block) {
                region[..n].fill(0.0);
            }
            for region in node.sidechain_buf.chunks_exact_mut(max_block) {
                region[..n].fill(0.0);
            }
            for edge in &mut node.edges {
                let src = &self.out_bufs[edge.src];
                let (bus, first) = match edge.input {
                    Input::Port(port) => (&mut node.in_buf, port * channels),
                    _ => (&mut node.sidechain_buf, 0),
                };
                for c in 0..channels {
                    let from = &src[c * max_block..c * max_block + n];
                    let start = (first + c) * max_block;
                    let to = &mut bus[start..start + n];
                    match &mut edge.delay {
                        Some(delay) => delay.run(c, from, to, true),
                        None => {
                            for (s, d) in from.iter().zip(to.iter_mut()) {
                                *d += *s;
                            }
                        }
                    }
                }
                if let Some(delay) = &mut edge.delay {
                    delay.advance(n);
                }
            }

            // Gather events: live ones first, then every source's output.
            // Sources run earlier, so they sit before `i`.
            let (earlier, own) = self.event_bufs.split_at_mut(i);
            node.events_in.clear();
            node.events_in.extend_from(node.live_events.as_slice());
            node.live_events.clear();
            for &src in &node.event_sources {
                for event in earlier[src].as_slice() {
                    // A device must keep frames inside the block; hold it
                    // to that so the next device can index with them.
                    node.events_in
                        .push(Event::new(event.frame.min(last_frame), event.kind));
                }
            }
            let events_out = &mut own[0];
            events_out.clear();

            // Slice-of-slices views live on the stack: no allocation.
            let mut inputs: [&[f32]; MAX_INPUT_SLICES] = [&[]; MAX_INPUT_SLICES];
            let input_count = node.ports * channels;
            for (slot, region) in inputs.iter_mut().zip(node.in_buf.chunks_exact(max_block)) {
                *slot = &region[..n];
            }
            let mut sidechain: [&[f32]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
            for (slot, region) in sidechain
                .iter_mut()
                .zip(node.sidechain_buf.chunks_exact(max_block))
            {
                *slot = &region[..n];
            }
            let mut outputs: [&mut [f32]; MAX_CHANNELS] = Default::default();
            for (slot, region) in outputs
                .iter_mut()
                .zip(self.out_bufs[i].chunks_exact_mut(max_block))
            {
                *slot = &mut region[..n];
            }
            node.device.process(
                ctx,
                &mut ProcessIo {
                    inputs: &inputs[..input_count],
                    sidechain: node.has_sidechain.then_some(&sidechain[..channels]),
                    outputs: &mut outputs[..channels],
                    events: node.events_in.as_slice(),
                    events_out,
                },
            );
            dropped = dropped
                .saturating_add(node.events_in.take_dropped())
                .saturating_add(node.live_events.take_dropped())
                .saturating_add(own[0].take_dropped());
        }
        self.dropped_events = self.dropped_events.saturating_add(dropped);
    }

    /// One channel of the graph output for the most recent block, or `None`
    /// when the graph has no output node or no such channel.
    pub fn output(&self, channel: usize) -> Option<&[f32]> {
        let buf = &self.out_bufs[self.output?];
        if channel >= self.channels {
            return None;
        }
        let start = channel * self.max_block;
        Some(&buf[start..start + self.frames])
    }

    /// One channel of any node's audio output for the most recent block:
    /// a tap for meters and stem export. `None` when there is no such node
    /// or channel.
    pub fn node_output(&self, id: DeviceId, channel: usize) -> Option<&[f32]> {
        let buf = &self.out_bufs[self.node_index(id)?];
        if channel >= self.channels {
            return None;
        }
        let start = channel * self.max_block;
        Some(&buf[start..start + self.frames])
    }

    /// The events the given node emitted in the most recent block.
    pub fn node_events(&self, id: DeviceId) -> Option<&[Event]> {
        self.node_index(id).map(|i| self.event_bufs[i].as_slice())
    }
}

#[cfg(test)]
mod tests;
