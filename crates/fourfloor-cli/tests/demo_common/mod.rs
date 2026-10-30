//! Shared helpers for the end-to-end tests on the `fourfloor demo` track.

#![allow(dead_code)]

use std::sync::OnceLock;

use fourfloor_cli::args::DemoOptions;
use fourfloor_cli::commands::device_registry;
use fourfloor_cli::demo::{build_demo_project, parts, LIMITER_CEILING_DB};
use fourfloor_core::{Device, DeviceId, DeviceRegistry, PrepareConfig, SampleRate, TrackId};
use fourfloor_effects::Limiter;
use fourfloor_model::{Command, CommandBus, Project};
use fourfloor_testkit::{render_project, StereoBuffer};

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const BPM: f64 = 124.0;
/// Tail rendered after the last bar, in seconds.
pub const TAIL: f64 = 1.0;

pub fn registry() -> &'static DeviceRegistry {
    static REGISTRY: OnceLock<DeviceRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| device_registry().expect("device registry"))
}

/// The demo, built through commands exactly as `fourfloor demo` builds it.
pub fn demo() -> Project {
    build_demo_project(&DemoOptions {
        dir: "demo".into(),
        name: Some("Demo".to_owned()),
        bpm: BPM,
    })
    .expect("demo builds")
}

/// The demo rendered once per test binary.
pub fn demo_mix() -> &'static StereoBuffer {
    static MIX: OnceLock<StereoBuffer> = OnceLock::new();
    MIX.get_or_init(|| render(&demo()))
}

pub fn render(project: &Project) -> StereoBuffer {
    render_project(project, registry(), RATE, TAIL).expect("render")
}

/// Applies `commands` to `project` through a command bus.
pub fn edit(project: &Project, commands: Vec<Command>) -> Project {
    let mut bus = CommandBus::new(project.clone());
    for command in commands {
        bus.execute(command).expect("command applies");
    }
    bus.project().clone()
}

pub fn track(project: &Project, name: &str) -> TrackId {
    project
        .tracks
        .iter()
        .find(|track| track.name == name)
        .unwrap_or_else(|| panic!("no track {name}"))
        .id
}

/// The first device of `kind` on track `name`.
pub fn device(project: &Project, name: &str, kind: &str) -> DeviceId {
    let id = track(project, name);
    project
        .track(id)
        .and_then(|track| track.devices.iter().find(|d| d.kind == kind))
        .unwrap_or_else(|| panic!("no {kind} on {name}"))
        .id
}

/// Samples the master limiter delays everything by: the master output is
/// that late against the timeline.
pub fn latency() -> usize {
    let mut limiter = Limiter::new();
    limiter.set_param(Limiter::CEILING, LIMITER_CEILING_DB);
    limiter.set_param(Limiter::TRUE_PEAK, 1.0);
    limiter.prepare(&PrepareConfig::stereo(RATE, 512));
    limiter.latency_samples() as usize
}

pub fn beat_seconds() -> f64 {
    60.0 / BPM
}

/// Sample of beat `beat` (from the start of the song) in the master
/// output, including the limiter's latency.
pub fn beat_sample(beat: u32) -> usize {
    (f64::from(beat) * beat_seconds() * RATE.as_f64()).round() as usize + latency()
}

/// Seconds at the start of bar `bar`, without latency.
pub fn bar_seconds(bar: u32) -> f64 {
    f64::from(bar) * 4.0 * beat_seconds()
}

pub fn section_bars(section: parts::Section) -> std::ops::Range<u32> {
    section.start_bar..section.end_bar()
}

pub fn rms(signal: &[f32]) -> f64 {
    let sum: f64 = signal.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / signal.len().max(1) as f64).sqrt()
}

/// A one-pole low-pass at `hz`, run forwards: enough to isolate a kick
/// from hats and pads for level comparisons.
pub fn low_pass(signal: &[f32], hz: f64) -> Vec<f32> {
    let a = (-std::f64::consts::TAU * hz / RATE.as_f64()).exp();
    let mut state = 0.0f64;
    signal
        .iter()
        .map(|&x| {
            state = (1.0 - a) * f64::from(x) + a * state;
            state as f32
        })
        .collect()
}
