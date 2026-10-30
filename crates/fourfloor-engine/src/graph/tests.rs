//! Tests for the graph builder and the compiled graph.

use super::*;
use crate::devices::{Gain, LatentDelay, Mixer, NotePlayer, PlaceholderSynth, PlayerNote, SineOsc};
use fourfloor_core::{Tempo, Ticks};

const RATE: SampleRate = SampleRate::HZ_48000;

fn id(n: u64) -> DeviceId {
    DeviceId(n)
}

fn ctx() -> TransportContext {
    TransportContext::stopped(RATE, Tempo::DEFAULT)
}

fn render(graph: &mut CompiledGraph, blocks: usize, frames: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        graph.process(&ctx(), frames);
        out.extend_from_slice(graph.output(0).unwrap());
    }
    out
}

/// Writes a different constant to each channel: `base + channel`.
struct ChannelDc {
    base: f32,
}

impl Device for ChannelDc {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        for (channel, output) in io.outputs.iter_mut().enumerate() {
            output.fill(self.base + channel as f32);
        }
    }
}

/// Passes its input where the sidechain is silent and mutes it elsewhere.
/// Without a sidechain it passes everything.
struct Ducker;

impl Device for Ducker {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let Some(key) = io.sidechain else {
            io.pass_through();
            return;
        };
        for ((out, inp), key) in io.outputs.iter_mut().zip(io.inputs).zip(key) {
            for ((o, i), k) in out.iter_mut().zip(inp.iter()).zip(key.iter()) {
                *o = if k.abs() < 0.5 { *i } else { 0.0 };
            }
        }
    }
}

/// Emits a fixed list of events every block and records the channel count
/// it was prepared with as its audio output.
struct Emitter {
    events: Vec<Event>,
    channels: f32,
}

impl Device for Emitter {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.channels = config.channels as f32;
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        for channel in io.outputs.iter_mut() {
            channel.fill(self.channels);
        }
        io.events_out.extend_from(&self.events);
    }
}

/// Transposes note events up an octave: a minimal note effect.
struct Octave;

impl Device for Octave {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        for event in io.events {
            let kind = match event.kind {
                EventKind::NoteOn {
                    pitch,
                    velocity,
                    channel,
                } => EventKind::NoteOn {
                    pitch: pitch + 12,
                    velocity,
                    channel,
                },
                other => other,
            };
            io.events_out.push(Event::new(event.frame, kind));
        }
    }
}

#[test]
fn chain_added_backwards_runs_source_first() {
    let mut b = GraphBuilder::new(1);
    b.add_node(id(3), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(2), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(1), Box::new(SineOsc::new(440.0, 1.0)), 0)
        .unwrap();
    b.connect(id(2), id(3), 0).unwrap();
    b.connect(id(1), id(2), 0).unwrap();
    let g = b.compile(RATE, 64).unwrap();
    assert_eq!(g.order(), [id(1), id(2), id(3)]);
    assert!(g.contains(id(2)) && !g.contains(id(9)));
}

#[test]
fn diamond_orders_every_source_before_its_destination() {
    let mut b = GraphBuilder::stereo();
    b.add_node(id(4), Box::new(Mixer::new(2)), 2).unwrap();
    b.add_node(id(3), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(2), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(1), Box::new(SineOsc::new(440.0, 1.0)), 0)
        .unwrap();
    let edges = [(1, 2, 0), (1, 3, 0), (2, 4, 0), (3, 4, 1)];
    for (from, to, port) in edges {
        b.connect(id(from), id(to), port).unwrap();
    }
    let g = b.compile(RATE, 64).unwrap();
    let pos = |n| g.order().iter().position(|&d| d == id(n)).unwrap();
    for (from, to, _) in edges {
        assert!(pos(from) < pos(to), "{from} must run before {to}");
    }
    // Ties break by insertion order: 3 was added before 2.
    assert_eq!(g.order(), [id(1), id(3), id(2), id(4)]);
}

#[test]
fn cycle_is_rejected_across_every_kind_of_edge() {
    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(2), Box::new(Gain::new(1.0)), 1).unwrap();
    b.connect(id(1), id(2), 0).unwrap();
    b.connect(id(2), id(1), 0).unwrap();
    assert_eq!(b.compile(RATE, 64).unwrap_err(), GraphError::Cycle(id(1)));

    let mut b = GraphBuilder::new(1);
    b.add_node(id(7), Box::new(Gain::new(1.0)), 1).unwrap();
    b.connect(id(7), id(7), 0).unwrap();
    assert_eq!(b.compile(RATE, 64).unwrap_err(), GraphError::Cycle(id(7)));

    // Audio one way, sidechain back: still a cycle.
    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(2), Box::new(Ducker), 1).unwrap();
    b.connect(id(1), id(2), 0).unwrap();
    b.connect_sidechain(id(2), id(1)).unwrap();
    assert_eq!(b.compile(RATE, 64).unwrap_err(), GraphError::Cycle(id(1)));

    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(Octave), 0).unwrap();
    b.add_node(id(2), Box::new(Octave), 0).unwrap();
    b.connect_events(id(1), id(2)).unwrap();
    b.connect_events(id(2), id(1)).unwrap();
    assert_eq!(b.compile(RATE, 64).unwrap_err(), GraphError::Cycle(id(1)));
}

#[test]
fn builder_rejects_bad_input() {
    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(Gain::new(1.0)), 1).unwrap();
    b.add_node(id(2), Box::new(Gain::new(1.0)), 1).unwrap();
    assert_eq!(
        b.add_node(id(1), Box::new(Gain::new(1.0)), 1),
        Err(GraphError::DuplicateNode(id(1)))
    );
    assert_eq!(
        b.add_node(id(5), Box::new(Gain::new(1.0)), MAX_INPUT_PORTS + 1),
        Err(GraphError::TooManyPorts(id(5)))
    );
    assert_eq!(
        b.connect(id(9), id(1), 0),
        Err(GraphError::UnknownNode(id(9)))
    );
    assert_eq!(
        b.connect_sidechain(id(1), id(9)),
        Err(GraphError::UnknownNode(id(9)))
    );
    assert_eq!(
        b.connect(id(1), id(2), 1),
        Err(GraphError::InvalidPort {
            node: id(2),
            port: 1
        })
    );
    b.connect(id(1), id(2), 0).unwrap();
    assert_eq!(
        b.connect(id(1), id(2), 0),
        Err(GraphError::DuplicateEdge {
            from: id(1),
            to: id(2),
            input: Input::Port(0)
        })
    );
    b.connect_events(id(1), id(2)).unwrap();
    let again = b.connect_events(id(1), id(2)).unwrap_err();
    assert_eq!(again.to_string(), "edge 1 -> 2 events was added twice");
    assert_eq!(b.set_output(id(9)), Err(GraphError::UnknownNode(id(9))));
    assert_eq!(Input::Sidechain.to_string(), "sidechain");
}

#[test]
fn compile_rejects_bad_sizes() {
    let err = |channels, block| {
        GraphBuilder::new(channels)
            .compile(RATE, block)
            .unwrap_err()
    };
    assert_eq!(err(0, 64), GraphError::InvalidChannelCount(0));
    assert_eq!(
        err(MAX_CHANNELS + 1, 64),
        GraphError::InvalidChannelCount(MAX_CHANNELS + 1)
    );
    assert_eq!(err(2, 0), GraphError::InvalidBlockSize);
}

#[test]
fn graph_without_output_is_silent() {
    let mut g = CompiledGraph::empty(2, RATE, 32).unwrap();
    g.process(&ctx(), 32);
    assert!(g.output(0).is_none());
    assert_eq!(g.latency_samples(), 0);
    g.process(&ctx(), 0);
}

#[test]
fn devices_are_prepared_with_the_graph_channel_count() {
    for channels in [1usize, 2, 6] {
        let mut b = GraphBuilder::new(channels);
        let emitter = Emitter {
            events: Vec::new(),
            channels: 0.0,
        };
        b.add_node(id(1), Box::new(emitter), 0).unwrap();
        b.set_output(id(1)).unwrap();
        let mut g = b.compile(RATE, 8).unwrap();
        assert_eq!(g.channels(), channels);
        g.process(&ctx(), 8);
        for channel in 0..channels {
            assert_eq!(g.output(channel).unwrap(), [channels as f32; 8]);
        }
        assert!(g.output(channels).is_none());
    }
}

#[test]
fn stereo_channels_stay_separate_through_gain_and_mixer() {
    // Source A: L=1, R=2. Source B: L=10, R=11.
    let mut b = GraphBuilder::stereo();
    b.add_node(id(1), Box::new(ChannelDc { base: 1.0 }), 0)
        .unwrap();
    b.add_node(id(2), Box::new(ChannelDc { base: 10.0 }), 0)
        .unwrap();
    b.add_node(id(3), Box::new(Gain::new(2.0)), 1).unwrap();
    b.add_node(id(4), Box::new(Mixer::new(2)), 2).unwrap();
    b.connect(id(1), id(3), 0).unwrap();
    b.connect(id(3), id(4), 0).unwrap();
    b.connect(id(2), id(4), 1).unwrap();
    b.set_output(id(4)).unwrap();
    let mut g = b.compile(RATE, 4).unwrap();
    g.process(&ctx(), 4);
    assert_eq!(g.output(0).unwrap(), [12.0; 4], "left: 1*2 + 10");
    assert_eq!(g.output(1).unwrap(), [15.0; 4], "right: 2*2 + 11");
}

#[test]
fn edges_into_one_port_are_summed() {
    // Sines at a quarter of the rate: samples are 0, 1, 0, -1.
    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(SineOsc::new(12_000.0, 1.0)), 0)
        .unwrap();
    b.add_node(id(2), Box::new(SineOsc::new(12_000.0, 1.0)), 0)
        .unwrap();
    b.add_node(id(3), Box::new(Gain::new(1.0)), 1).unwrap();
    b.connect(id(1), id(3), 0).unwrap();
    b.connect(id(2), id(3), 0).unwrap();
    b.set_output(id(3)).unwrap();
    let mut g = b.compile(RATE, 8).unwrap();
    let out = render(&mut g, 1, 8);
    for (i, expected) in [0.0, 2.0, 0.0, -2.0].iter().enumerate() {
        assert!((out[i] - expected).abs() < 1e-5, "sample {i}: {}", out[i]);
    }
}

#[test]
fn short_blocks_only_touch_the_requested_frames() {
    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(SineOsc::new(1000.0, 0.5)), 0)
        .unwrap();
    b.set_output(id(1)).unwrap();
    let mut g = b.compile(RATE, 64).unwrap();
    g.process(&ctx(), 10);
    assert_eq!(g.output(0).unwrap().len(), 10);
    g.process(&ctx(), 1000);
    assert_eq!(g.output(0).unwrap().len(), 64, "clamped to max_block");
    assert!(g.output(1).is_none());
}

#[test]
fn set_param_reaches_the_right_device() {
    let mut b = GraphBuilder::new(1);
    b.add_node(id(10), Box::new(SineOsc::new(12_000.0, 1.0)), 0)
        .unwrap();
    b.add_node(id(20), Box::new(Gain::new(1.0)), 1).unwrap();
    b.connect(id(10), id(20), 0).unwrap();
    b.set_output(id(20)).unwrap();
    let mut g = b.compile(RATE, 4).unwrap();
    assert!(g.set_param(id(20), Gain::GAIN, 0.25));
    assert!(!g.set_param(id(99), Gain::GAIN, 0.25));
    let out = render(&mut g, 1, 4);
    assert!((out[1] - 0.25).abs() < 1e-6);
}

/// source -> latent(LAT) -> mixer port 0
/// source -> gain(-1)    -> mixer port 1   (the "bypassed twin", inverted)
fn null_test_graph(compensate: bool, latency: u32) -> CompiledGraph {
    let mut b = GraphBuilder::stereo();
    b.set_delay_compensation(compensate);
    b.add_node(id(1), Box::new(SineOsc::new(441.0, 0.8)), 0)
        .unwrap();
    b.add_node(id(2), Box::new(LatentDelay::new(latency)), 1)
        .unwrap();
    b.add_node(id(3), Box::new(Gain::new(-1.0)), 1).unwrap();
    b.add_node(id(4), Box::new(Mixer::new(2)), 2).unwrap();
    b.connect(id(1), id(2), 0).unwrap();
    b.connect(id(1), id(3), 0).unwrap();
    b.connect(id(2), id(4), 0).unwrap();
    b.connect(id(3), id(4), 1).unwrap();
    b.set_output(id(4)).unwrap();
    b.compile(RATE, 128).unwrap()
}

#[test]
fn pdc_null_test_latent_path_cancels_against_compensated_twin() {
    const LATENCY: u32 = 777; // deliberately not a multiple of the block
    let mut g = null_test_graph(true, LATENCY);
    assert_eq!(g.node_latency(id(2)), Some(LATENCY));
    assert_eq!(g.edge_delay(id(2), id(4), 0), Some(0));
    assert_eq!(g.edge_delay(id(3), id(4), 1), Some(LATENCY));
    assert_eq!(g.edge_delay(id(1), id(2), 0), Some(0));
    assert_eq!(g.edge_delay(id(1), id(9), 0), None);
    assert_eq!(g.latency_samples(), LATENCY);

    let mut peak = 0.0f32;
    for _ in 0..40 {
        g.process(&ctx(), 128);
        for channel in 0..2 {
            let out = g.output(channel).unwrap();
            peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
        }
    }
    assert_eq!(peak, 0.0, "compensated paths must cancel on both channels");
}

#[test]
fn without_compensation_the_same_graph_does_not_null() {
    let mut g = null_test_graph(false, 777);
    assert_eq!(g.edge_delay(id(3), id(4), 1), Some(0));
    let out = render(&mut g, 40, 128);
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.5, "control: uncompensated peak was {peak}");
}

#[test]
fn pdc_accumulates_latency_along_chains() {
    // 1 -> A(100) -> B(50) -> mix.0 ; 1 -> C(30) -> mix.1 ; 1 -> mix.2
    let mut b = GraphBuilder::new(1);
    b.add_node(id(1), Box::new(SineOsc::new(100.0, 1.0)), 0)
        .unwrap();
    b.add_node(id(2), Box::new(LatentDelay::new(100)), 1)
        .unwrap();
    b.add_node(id(3), Box::new(LatentDelay::new(50)), 1)
        .unwrap();
    b.add_node(id(4), Box::new(LatentDelay::new(30)), 1)
        .unwrap();
    b.add_node(id(5), Box::new(Mixer::new(3)), 3).unwrap();
    for (from, to, port) in [
        (1, 2, 0),
        (2, 3, 0),
        (1, 4, 0),
        (3, 5, 0),
        (4, 5, 1),
        (1, 5, 2),
    ] {
        b.connect(id(from), id(to), port).unwrap();
    }
    b.set_output(id(5)).unwrap();
    let g = b.compile(RATE, 32).unwrap();
    assert_eq!(g.edge_delay(id(3), id(5), 0), Some(0));
    assert_eq!(g.edge_delay(id(4), id(5), 1), Some(120));
    assert_eq!(g.edge_delay(id(1), id(5), 2), Some(150));
    assert_eq!(g.latency_samples(), 150);
}

#[test]
fn reset_clears_compensation_delays() {
    let mut g = null_test_graph(true, 64);
    render(&mut g, 4, 128);
    g.reset();
    // After a reset both paths restart from silence and still cancel.
    let out = render(&mut g, 4, 128);
    assert!(out.iter().all(|s| *s == 0.0));
}

/// carrier (DC 1 / 2) -> ducker <- sidechain from `key`, if given.
fn ducker_graph(key: Option<Box<dyn Device>>, key_latency: u32) -> CompiledGraph {
    let mut b = GraphBuilder::stereo();
    b.add_node(id(1), Box::new(ChannelDc { base: 1.0 }), 0)
        .unwrap();
    b.add_node(id(2), Box::new(Ducker), 1).unwrap();
    b.connect(id(1), id(2), 0).unwrap();
    if let Some(key) = key {
        b.add_node(id(3), key, 0).unwrap();
        b.add_node(id(4), Box::new(LatentDelay::new(key_latency)), 1)
            .unwrap();
        b.connect(id(3), id(4), 0).unwrap();
        b.connect_sidechain(id(4), id(2)).unwrap();
    }
    b.set_output(id(2)).unwrap();
    b.compile(RATE, 8).unwrap()
}

#[test]
fn sidechain_bus_is_absent_unless_connected_and_carries_the_key_signal() {
    let mut open = ducker_graph(None, 0);
    open.process(&ctx(), 8);
    assert_eq!(open.output(0).unwrap(), [1.0; 8], "no sidechain: pass");
    assert_eq!(open.output(1).unwrap(), [2.0; 8]);

    // Key is 0, 1, 0, -1...: the ducker mutes every odd frame.
    let mut keyed = ducker_graph(Some(Box::new(SineOsc::new(12_000.0, 1.0))), 0);
    keyed.process(&ctx(), 8);
    let left = keyed.output(0).unwrap();
    for (frame, sample) in left.iter().enumerate() {
        let expected = if frame % 2 == 0 { 1.0 } else { 0.0 };
        assert!((sample - expected).abs() < 1e-6, "frame {frame}: {sample}");
    }
    assert_eq!(keyed.sidechain_delay(id(4), id(2)), Some(0));
    assert_eq!(keyed.sidechain_delay(id(1), id(2)), None);
}

#[test]
fn sidechain_edges_take_part_in_delay_compensation() {
    // The key path is 5 samples late, so the main input is delayed by 5 to
    // stay aligned with it and the output reports 5 samples of latency.
    let g = ducker_graph(Some(Box::new(SineOsc::new(12_000.0, 1.0))), 5);
    assert_eq!(g.edge_delay(id(1), id(2), 0), Some(5));
    assert_eq!(g.sidechain_delay(id(4), id(2)), Some(0));
    assert_eq!(g.latency_samples(), 5);
}

fn note_ons(events: &[Event]) -> Vec<(u32, u8)> {
    events
        .iter()
        .filter_map(|event| match event.kind {
            EventKind::NoteOn { pitch, .. } => Some((event.frame, pitch)),
            _ => None,
        })
        .collect()
}

#[test]
fn events_flow_along_event_edges_through_a_note_effect_and_merge_in_order() {
    let emit = |events: Vec<Event>| {
        Box::new(Emitter {
            events,
            channels: 0.0,
        })
    };
    let mut b = GraphBuilder::stereo();
    b.add_node(
        id(1),
        emit(vec![Event::note_on(2, 40, 100), Event::note_on(6, 41, 100)]),
        0,
    )
    .unwrap();
    b.add_node(id(2), emit(vec![Event::note_on(4, 50, 100)]), 0)
        .unwrap();
    b.add_node(id(3), Box::new(Octave), 0).unwrap();
    b.connect_events(id(1), id(3)).unwrap();
    b.connect_events(id(2), id(3)).unwrap();
    let mut g = b.compile(RATE, 8).unwrap();
    g.process(&ctx(), 8);
    assert_eq!(
        note_ons(g.node_events(id(3)).unwrap()),
        [(2, 52), (4, 62), (6, 53)],
        "merged by frame and transposed"
    );
    assert!(g.node_events(id(9)).is_none());
    assert_eq!(g.dropped_events(), 0);

    // A block shorter than the event frames: frames are held inside it.
    g.process(&ctx(), 4);
    let frames: Vec<u32> = g
        .node_events(id(3))
        .unwrap()
        .iter()
        .map(|event| event.frame)
        .collect();
    assert_eq!(frames, [2, 3, 3]);
}

#[test]
fn send_event_reaches_an_instrument_at_the_start_of_the_next_block() {
    let mut b = GraphBuilder::stereo();
    b.add_node(id(1), Box::new(PlaceholderSynth::new(1.0)), 0)
        .unwrap();
    b.set_output(id(1)).unwrap();
    let mut g = b.compile(RATE, 256).unwrap();
    g.process(&ctx(), 256);
    assert!(g.output(0).unwrap().iter().all(|s| *s == 0.0));

    assert!(g.send_event(id(1), EventKind::note_on(69, 127)));
    assert!(!g.send_event(id(2), EventKind::note_on(69, 127)));
    g.process(&ctx(), 256);
    let (left, right) = (g.output(0).unwrap(), g.output(1).unwrap());
    assert!(left[1] != 0.0, "the note starts on frame 0 of the block");
    assert!(left.iter().any(|s| s.abs() > 0.5));
    assert_eq!(left, right);

    assert!(g.send_event(id(1), EventKind::AllNotesOff));
    for _ in 0..4 {
        g.process(&ctx(), 256);
    }
    assert!(g.output(0).unwrap().iter().all(|s| *s == 0.0), "released");
}

#[test]
fn note_player_drives_an_instrument_sample_accurately() {
    // 120 BPM at 48 kHz: tick 1440 is sample 36 000.
    let notes = [PlayerNote::new(Ticks::new(1440), Ticks::new(240), 69, 127)];
    let mut b = GraphBuilder::stereo();
    b.add_node(id(1), Box::new(NotePlayer::new(&notes)), 0)
        .unwrap();
    b.add_node(id(2), Box::new(PlaceholderSynth::new(1.0)), 0)
        .unwrap();
    b.connect_events(id(1), id(2)).unwrap();
    b.set_output(id(2)).unwrap();
    let mut g = b.compile(RATE, 500).unwrap();
    assert_eq!(g.order(), [id(1), id(2)]);

    let mut out = Vec::new();
    for block in 0..96u64 {
        let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, block * 500);
        g.process(&ctx, 500);
        out.extend_from_slice(g.output(0).unwrap());
    }
    // A sine starts at zero, so the first non-zero sample is one later.
    assert_eq!(out.iter().position(|s| *s != 0.0), Some(36_001));
    // The note-off at tick 1680 (sample 42 000) starts a 480-sample release.
    let last = out.iter().rposition(|s| *s != 0.0).unwrap();
    assert!((42_000..42_480).contains(&last), "last sound at {last}");
}

#[test]
fn a_full_event_buffer_drops_and_counts_instead_of_growing() {
    let events: Vec<Event> = (0..6).map(|i| Event::note_on(i, 60, 100)).collect();
    let mut b = GraphBuilder::new(1);
    b.set_event_capacity(4);
    b.add_node(
        id(1),
        Box::new(Emitter {
            events,
            channels: 0.0,
        }),
        0,
    )
    .unwrap();
    b.add_node(id(2), Box::new(Octave), 0).unwrap();
    b.connect_events(id(1), id(2)).unwrap();
    let mut g = b.compile(RATE, 8).unwrap();
    g.process(&ctx(), 8);
    assert_eq!(g.node_events(id(1)).unwrap().len(), 4);
    assert_eq!(g.node_events(id(2)).unwrap().len(), 4);
    assert_eq!(g.dropped_events(), 2);
}

#[test]
fn compiled_graph_is_send_and_debug() {
    fn assert_send<T: Send>() {}
    assert_send::<CompiledGraph>();
    let g = CompiledGraph::empty(1, RATE, 8).unwrap();
    assert!(format!("{g:?}").contains("CompiledGraph"));
}
