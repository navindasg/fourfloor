//! Proves the allocation guard is live and that a full render passes it.
//!
//! This binary installs `AllocDisabler`, as a real fourfloor binary would.
//! In a debug build any allocation inside
//! `AudioProcessor::process_interleaved` then aborts the process.

use std::process::Command;

use fourfloor_engine::core::{
    AudioMessage, Beats, Device, DeviceId, EventKind, PrepareConfig, ProcessIo, SampleRate, Tempo,
    Ticks, TransportContext,
};
use fourfloor_engine::devices::{
    Fader, Gain, LatentDelay, Metronome, Mixer, NotePlayer, PlaceholderSynth, PlayerNote, SineOsc,
};
use fourfloor_engine::offline::render;
use fourfloor_engine::{
    engine, AllocDisabler, AudioProcessor, CompiledGraph, EngineConfig, EngineController,
    GraphBuilder, LoopRegion,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const CHILD_ENV: &str = "FOURFLOOR_NO_ALLOC_CHILD";

/// Every built-in device, with a latent branch so compensation delays run.
fn full_graph(config: &EngineConfig, extra: Option<Box<dyn Device>>) -> CompiledGraph {
    let mut b = GraphBuilder::new(config.channels);
    b.add_node(DeviceId(1), Box::new(SineOsc::new(110.0, 0.2)), 0)
        .unwrap();
    b.add_node(DeviceId(2), Box::new(LatentDelay::new(300)), 1)
        .unwrap();
    b.add_node(DeviceId(3), Box::new(Gain::new(0.5)), 1)
        .unwrap();
    b.add_node(DeviceId(4), Box::new(Metronome::new(0.8)), 0)
        .unwrap();
    b.add_node(DeviceId(5), Box::new(Mixer::new(4)), 4).unwrap();
    // More notes than voices, across every voice kind, so stealing runs too.
    let notes: Vec<PlayerNote> = (0..64)
        .map(|i| {
            PlayerNote::new(
                Ticks::new(i * 60),
                Ticks::new(240),
                [36, 38, 42, 46, 60][i as usize % 5],
                100,
            )
        })
        .collect();
    // Note player -> instrument -> fader (panned, so its smoothing runs),
    // with the sine as a sidechain edge so that bus is exercised too.
    b.add_node(DeviceId(8), Box::new(NotePlayer::new(&notes)), 0)
        .unwrap();
    b.add_node(DeviceId(7), Box::new(PlaceholderSynth::new(0.2)), 0)
        .unwrap();
    b.add_node(DeviceId(9), Box::new(Fader::new(-3.0, 0.4)), 1)
        .unwrap();
    b.connect_events(DeviceId(8), DeviceId(7)).unwrap();
    b.connect(DeviceId(7), DeviceId(9), 0).unwrap();
    b.connect_sidechain(DeviceId(1), DeviceId(9)).unwrap();
    b.connect(DeviceId(9), DeviceId(5), 3).unwrap();
    b.connect(DeviceId(1), DeviceId(2), 0).unwrap();
    b.connect(DeviceId(1), DeviceId(3), 0).unwrap();
    b.connect(DeviceId(2), DeviceId(5), 0).unwrap();
    b.connect(DeviceId(3), DeviceId(5), 1).unwrap();
    b.connect(DeviceId(4), DeviceId(5), 2).unwrap();
    if let Some(device) = extra {
        b.add_node(DeviceId(6), device, 0).unwrap();
    }
    b.set_output(DeviceId(5)).unwrap();
    b.compile(config.sample_rate, config.max_block).unwrap()
}

fn start(extra: Option<Box<dyn Device>>) -> (EngineConfig, EngineController, AudioProcessor) {
    let config = EngineConfig {
        max_block: 128,
        ..EngineConfig::new(SampleRate::HZ_48000)
    };
    let (controller, processor) = engine(config, full_graph(&config, extra)).unwrap();
    (config, controller, processor)
}

#[test]
fn full_render_with_commands_and_a_graph_swap_never_allocates() {
    let (config, mut controller, mut processor) = start(None);
    controller.play().unwrap();
    controller
        .set_loop(Some(LoopRegion::new(Beats(0.0), Beats(2.0))))
        .unwrap();
    let mut out = render(&mut processor, 200, 300);

    controller.set_tempo(Tempo::new(128.0).unwrap()).unwrap();
    controller.set_param(DeviceId(3), Gain::GAIN, 0.25).unwrap();
    controller.set_param(DeviceId(9), Fader::PAN, -0.7).unwrap();
    controller
        .send_event(DeviceId(7), EventKind::note_on(64, 100))
        .unwrap();
    controller.seek(Beats(1.0)).unwrap();
    controller.swap_graph(full_graph(&config, None)).unwrap();
    controller.send(AudioMessage::Stop).unwrap();
    controller.play().unwrap();
    out.extend(render(&mut processor, 200, 300));

    assert_eq!(out.len(), 2 * 200 * 300 * 2);
    assert!(out.iter().any(|s| *s != 0.0));
    assert!(out.iter().all(|s| s.is_finite()));
    assert_eq!(controller.collect_garbage(), 1);
    assert_eq!(controller.leaked_graphs(), 0);
    assert!(controller.peak(0) > 0.0);
}

/// A device that breaks the rule on purpose.
struct Allocating;

impl Device for Allocating {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let scratch = vec![0.0f32; std::hint::black_box(64)];
        for output in io.outputs.iter_mut() {
            output.fill(scratch[0]);
        }
    }
}

/// Run only as a child of the test below.
#[test]
#[ignore = "aborts on purpose; run by guard_aborts_when_a_device_allocates"]
fn child_renders_an_allocating_device() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let (_config, _controller, mut processor) = start(Some(Box::new(Allocating)));
    render(&mut processor, 1, 64);
}

#[test]
fn guard_aborts_when_a_device_allocates() {
    if !cfg!(debug_assertions) {
        return; // The engine only arms the guard in debug builds.
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "child_renders_an_allocating_device",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the child should have aborted; stderr: {stderr}"
    );
    assert!(
        stderr.contains("memory allocation of"),
        "expected an allocation abort, got: {stderr}"
    );
}
