//! The app's audio path without a device: the callback that runs in the
//! cpal stream, and the engine host the UI talks to. Everything is checked
//! on rendered samples.

use std::sync::Arc;
use std::time::Duration;

use fourfloor_app::project::build_demo_project;
use fourfloor_app::services::callback::{AudioCallback, CallbackMessage, Monitor, ReferenceAudio};
use fourfloor_app::services::meters::{MeterBank, TapPlan};
use fourfloor_app::services::{build_registry, EngineHost, OutputMode, MAX_BLOCK};
use fourfloor_compile::{compile_project, CompileOptions};
use fourfloor_core::{DeviceRegistry, SampleRate, Tempo};
use fourfloor_engine::{engine, CompiledGraph, EngineConfig, EngineController};
use fourfloor_model::Project;
use fourfloor_testkit::{
    dominant_frequency, null_residual_db, onset_times_with, sine, OnsetOptions, RenderOptions,
    StereoBuffer,
};

const RATE: SampleRate = SampleRate::HZ_48000;

struct Rig {
    controller: EngineController,
    callback: AudioCallback,
    link: fourfloor_app::services::callback::CallbackLink,
    meters: Arc<MeterBank>,
    faders: Vec<(String, fourfloor_core::DeviceId)>,
}

fn rig(project: &Project, registry: &DeviceRegistry) -> Rig {
    let mut config = EngineConfig::new(RATE);
    config.max_block = MAX_BLOCK;
    config.tempo = project.tempo;
    let empty = CompiledGraph::empty(2, RATE, MAX_BLOCK).expect("empty graph");
    let (mut controller, processor) = engine(config, empty).expect("engine");
    let meters = Arc::new(MeterBank::new());
    let (callback, mut link) = AudioCallback::new(processor, Arc::clone(&meters));
    let compiled = compile_project(project, registry, &CompileOptions::new(RATE, MAX_BLOCK))
        .expect("compiles");
    let faders: Vec<_> = compiled
        .tracks
        .iter()
        .map(|t| {
            (
                project
                    .track(t.track)
                    .map(|x| x.name.clone())
                    .unwrap_or_default(),
                t.fader,
            )
        })
        .collect();
    let nodes: Vec<_> = faders.iter().map(|(_, id)| *id).collect();
    controller.swap_graph(compiled.graph).expect("swap");
    link.tx
        .push(CallbackMessage::Taps(TapPlan::new(&nodes)))
        .expect("taps");
    Rig {
        controller,
        callback,
        link,
        meters,
        faders,
    }
}

fn render(callback: &mut AudioCallback, seconds: f64) -> StereoBuffer {
    // Uneven device buffers, as a real callback sees them.
    render_in(callback, seconds, &[256, 480, 1024, 333])
}

fn render_in(callback: &mut AudioCallback, seconds: f64, sizes: &[usize]) -> StereoBuffer {
    let frames = (seconds * RATE.as_f64()) as usize;
    let mut out = vec![0.0; frames * 2];
    let mut done = 0;
    for size in sizes.iter().cycle() {
        if done >= frames {
            break;
        }
        let n = (*size).min(frames - done);
        callback.render(&mut out[done * 2..(done + n) * 2], 2);
        done += n;
    }
    StereoBuffer::from_interleaved(RATE, &out)
}

#[test]
fn the_callback_plays_exactly_what_the_offline_renderer_renders() {
    let registry = build_registry(false).expect("registry");
    let project = build_demo_project(&registry).expect("demo");
    let mut rig = rig(&project, &registry);
    rig.controller.play().expect("play");
    // Device buffers that are whole engine blocks: sample for sample the
    // same as the offline render.
    let live = render_in(&mut rig.callback, 4.0, &[MAX_BLOCK, 2 * MAX_BLOCK]);
    let offline =
        fourfloor_testkit::render_project_with(&project, &registry, &RenderOptions::new(RATE))
            .expect("offline")
            .slice_seconds(0.0, 4.0);
    let residual = null_residual_db(&live.mono(), &offline.mono());
    assert!(
        residual < -120.0,
        "live and offline differ by {residual} dB"
    );
    // Uneven buffers move the block boundaries, and automation and LFOs
    // update once per block, so the result is close but not identical.
    let mut uneven = crate::rig(&project, &registry);
    uneven.controller.play().expect("play");
    let live = render(&mut uneven.callback, 4.0);
    let residual = null_residual_db(&live.mono(), &offline.mono());
    assert!(residual < -40.0, "uneven buffers differ by {residual} dB");
}

#[test]
fn track_meters_follow_each_fader() {
    let registry = build_registry(false).expect("registry");
    let project = build_demo_project(&registry).expect("demo");
    let mut rig = rig(&project, &registry);
    rig.controller.play().expect("play");
    let _ = render(&mut rig.callback, 1.0);
    let slot = |name: &str| rig.faders.iter().position(|(n, _)| n == name).expect(name);
    let kick = rig.meters.take_tap(slot("Kick"));
    assert!(kick[0] > 0.2 && kick[1] > 0.2, "kick meter {kick:?}");
    let master = rig.meters.take_master();
    assert!(master[0] > 0.2, "master meter {master:?}");
    // Close the kick's fader live: its meter drops to (near) silence.
    let fader = rig.faders[slot("Kick")].1;
    rig.controller
        .set_param(fader, fourfloor_engine::devices::Fader::GAIN_DB, -96.0)
        .expect("param");
    let _ = render(&mut rig.callback, 0.5);
    let _ = rig.meters.take_tap(slot("Kick"));
    let _ = render(&mut rig.callback, 0.5);
    let closed = rig.meters.take_tap(slot("Kick"));
    assert!(
        closed[0] < 1e-3,
        "kick meter after closing its fader: {closed:?}"
    );
    assert!(rig.meters.callbacks() > 0);
}

#[test]
fn the_metronome_clicks_on_every_beat_with_an_accent() {
    let registry = build_registry(false).expect("registry");
    let mut project = Project::new("Click");
    project.tempo = Tempo::new(120.0).expect("tempo");
    let mut rig = rig(&project, &registry);
    rig.link
        .tx
        .push(CallbackMessage::Monitor(Monitor {
            metronome: true,
            ..Monitor::default()
        }))
        .expect("monitor");
    rig.controller.play().expect("play");
    let out = render(&mut rig.callback, 2.2);
    let mono = out.mono();
    let options = OnsetOptions {
        min_gap_ms: 200.0,
        ..OnsetOptions::default()
    };
    let onsets = onset_times_with(&mono, RATE, &options);
    let expected = [0.0, 0.5, 1.0, 1.5, 2.0];
    assert_eq!(onsets.len(), expected.len(), "{onsets:?}");
    for (found, want) in onsets.iter().zip(expected) {
        assert!((found - want).abs() < 0.002, "{found} vs {want}");
    }
    let accent = dominant_frequency(&out.slice_seconds(0.0, 0.03).mono(), RATE);
    let beat = dominant_frequency(&out.slice_seconds(0.5, 0.53).mono(), RATE);
    assert!((accent - 1760.0).abs() < 60.0, "downbeat {accent} Hz");
    assert!((beat - 880.0).abs() < 60.0, "other beats {beat} Hz");
    // Stopped: no clicks.
    rig.controller.stop().expect("stop");
    let quiet = render(&mut rig.callback, 1.0);
    assert!(fourfloor_testkit::is_silent(&quiet.mono(), -90.0));
}

#[test]
fn the_reference_plays_in_time_at_the_matched_gain() {
    let registry = build_registry(false).expect("registry");
    let project = build_demo_project(&registry).expect("demo");
    let mut rig = rig(&project, &registry);
    let tone = sine(330.0, 0.5, 3.0, RATE);
    let samples: Vec<f32> = tone.iter().flat_map(|s| [*s, -*s]).collect();
    let reference = Arc::new(ReferenceAudio {
        samples: samples.clone(),
    });
    rig.link
        .tx
        .push(CallbackMessage::Reference(Some(Arc::clone(&reference))))
        .expect("ref");
    rig.link
        .tx
        .push(CallbackMessage::Monitor(Monitor {
            reference: true,
            reference_gain: 0.5,
            metronome: false,
        }))
        .expect("monitor");
    rig.controller.play().expect("play");
    let out = render(&mut rig.callback, 1.0);
    let expected: Vec<f32> = samples[..out.frames() * 2]
        .iter()
        .map(|s| s * 0.5)
        .collect();
    let expected = StereoBuffer::from_interleaved(RATE, &expected);
    assert!(null_residual_db(&out.left, &expected.left) < -120.0);
    assert!(null_residual_db(&out.right, &expected.right) < -120.0);
    // Back to the mix: the demo plays again, and the old reference is
    // handed back to this thread to be freed.
    rig.link
        .tx
        .push(CallbackMessage::Monitor(Monitor::default()))
        .expect("monitor");
    rig.link
        .tx
        .push(CallbackMessage::Reference(None))
        .expect("clear");
    let mix = render(&mut rig.callback, 0.5);
    assert!(fourfloor_testkit::rms(&mix.mono()) > 0.01);
    assert!(
        rig.link.retired_rx.pop().is_ok(),
        "retired reference returned"
    );
    assert_eq!(Arc::strong_count(&reference), 1);
}

#[test]
fn the_host_compiles_plays_meters_and_queues_launches() {
    let registry = Arc::new(build_registry(false).expect("registry"));
    let project = build_demo_project(&registry).expect("demo");
    let mut host = EngineHost::new(
        Arc::clone(&registry),
        &OutputMode::Offline { sample_rate: RATE },
    )
    .expect("host");
    assert!(!host.is_live());
    host.request_compile(&project, 1, None);
    assert!(
        host.wait_for_graph(Duration::from_secs(30)),
        "{:?}",
        host.compile_error
    );
    assert_eq!(host.installed_revision(), Some(1));
    host.set_tempo(project.tempo.bpm());
    host.play();
    host.pump(RATE.hz() as usize);
    let readout = host.readout();
    assert!(readout.playing);
    let one_second = 124.0 / 60.0 * 960.0;
    assert!(
        (readout.position - one_second).abs() < 40.0,
        "{} ticks",
        readout.position
    );
    let kick = project
        .tracks
        .iter()
        .find(|t| t.name == "Kick")
        .expect("kick")
        .id;
    assert!(host.take_track_peak(kick)[0] > 0.1);
    let mut samples = 0;
    host.drain_master(|chunk| samples += chunk.len());
    assert_eq!(
        samples,
        RATE.hz() as usize * 2,
        "every master sample reached analysis"
    );
    let bass = project
        .tracks
        .iter()
        .find(|t| t.name == "Bass")
        .expect("bass")
        .id;
    let scene = project.scenes[0].id;
    host.launch(bass, Some(scene), &project);
    let state = host.launch_state(bass).expect("launched");
    assert_eq!(
        (state.scene, state.at),
        (Some(scene), 3840),
        "starts on the next bar"
    );
    // Seeking and looping go through to the transport.
    host.set_loop(Some((0, 3840)));
    host.seek(3000);
    host.pump(RATE.hz() as usize / 2);
    let position = host.readout().position;
    assert!(position < 3840.0, "looped back inside bar 1: {position}");
}
