//! The audio callback must not allocate or free: this binary installs the
//! guard allocator, and any allocation inside `assert_no_alloc` aborts it.
//! Covered: the full demo graph, meter taps, metronome, a graph swap,
//! reference A/B on and off, and a reference replaced mid-play.

use std::sync::Arc;

use assert_no_alloc::assert_no_alloc;
use fourfloor_app::project::build_demo_project;
use fourfloor_app::services::callback::{AudioCallback, CallbackMessage, Monitor, ReferenceAudio};
use fourfloor_app::services::meters::{MeterBank, TapPlan};
use fourfloor_app::services::{build_registry, MAX_BLOCK};
use fourfloor_compile::{compile_project, CompileOptions};
use fourfloor_core::SampleRate;
use fourfloor_engine::{engine, CompiledGraph, EngineConfig};

#[global_allocator]
static ALLOCATOR: fourfloor_testkit::AllocDisabler = fourfloor_testkit::AllocDisabler;

const RATE: SampleRate = SampleRate::HZ_48000;

#[test]
fn the_callback_never_allocates() {
    let registry = build_registry(false).expect("registry");
    let project = build_demo_project(&registry).expect("demo");
    let mut config = EngineConfig::new(RATE);
    config.max_block = MAX_BLOCK;
    config.tempo = project.tempo;
    let (mut controller, processor) = engine(
        config,
        CompiledGraph::empty(2, RATE, MAX_BLOCK).expect("graph"),
    )
    .expect("engine");
    let meters = Arc::new(MeterBank::new());
    let (mut callback, mut link) = AudioCallback::new(processor, meters);
    let options = CompileOptions::new(RATE, MAX_BLOCK);
    let compiled = compile_project(&project, &registry, &options).expect("compiles");
    let faders: Vec<_> = compiled.tracks.iter().map(|t| t.fader).collect();
    controller.swap_graph(compiled.graph).expect("swap");
    link.tx
        .push(CallbackMessage::Taps(TapPlan::new(&faders)))
        .expect("taps");
    link.tx
        .push(CallbackMessage::Monitor(Monitor {
            metronome: true,
            ..Monitor::default()
        }))
        .expect("monitor");
    controller.play().expect("play");
    let first = Arc::new(ReferenceAudio {
        samples: vec![0.25; 48_000 * 2],
    });
    let second = Arc::new(ReferenceAudio {
        samples: vec![-0.25; 24_000 * 2],
    });
    let mut out = vec![0.0f32; 1024 * 2];
    let mut wide = vec![0.0f32; 333 * 6];
    for round in 0..200 {
        match round {
            40 => {
                link.tx
                    .push(CallbackMessage::Reference(Some(Arc::clone(&first))))
                    .expect("ref");
                link.tx
                    .push(CallbackMessage::Monitor(Monitor {
                        reference: true,
                        reference_gain: 0.7,
                        metronome: true,
                    }))
                    .expect("monitor");
            }
            80 => {
                // Replacing the reference retires the old one to the queue.
                link.tx
                    .push(CallbackMessage::Reference(Some(Arc::clone(&second))))
                    .expect("ref");
            }
            120 => {
                link.tx
                    .push(CallbackMessage::Monitor(Monitor::default()))
                    .expect("monitor");
                let again = compile_project(&project, &registry, &options).expect("compiles");
                controller.swap_graph(again.graph).expect("swap");
            }
            _ => {}
        }
        assert_no_alloc(|| {
            callback.render(&mut out, 2);
            callback.render(&mut wide, 6);
        });
        // Freed here, on the test's "UI" thread.
        link.collect_garbage();
        controller.collect_garbage();
        let mut drained = 0;
        while link.master_rx.pop().is_ok() {
            drained += 1;
        }
        assert!(drained > 0);
    }
    assert!(out.iter().all(|s| s.is_finite()));
}
