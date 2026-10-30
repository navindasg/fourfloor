//! Hosts Apple's stock Audio Units, which ship with every macOS, and checks
//! what comes out on rendered audio.
//!
//! This binary does not install the allocation guard: what an Apple unit
//! does inside its own render call is not ours to assert on. The host side
//! of the same code path (`fill_port`, `drain_port`, the event translation)
//! runs under the guard in `clap_fixtures.rs`.

#![cfg(target_os = "macos")]

mod common;

use std::sync::Arc;

use common::{prepare, process_block, tempo, RATE};
use fourfloor_core::{Device, DeviceCategory, DeviceId, DeviceInit, DeviceRegistry, ParamId};
use fourfloor_engine::devices::{Gain, Mixer, SineOsc};
use fourfloor_engine::{CompiledGraph, GraphBuilder};
use fourfloor_host::au::{describe, list_components, AuDevice};
use fourfloor_host::thread::HostThread;
use fourfloor_host::{HostError, PluginDatabase, PluginFormat, PluginHost, ScanOptions};
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, click_train, dominant_frequency, is_silent, note,
    null_residual_db, onset_times, peak, render_effect, render_graph, render_instrument, rms, sine,
    spectrum_band_energy_db, times_match, white_noise, RenderConfig, StereoBuffer,
};

const DELAY: &str = "aufx.dely.appl";
const LOWPASS: &str = "aufx.lpas.appl";
const LIMITER: &str = "aufx.lmtr.appl";
const DLS_SYNTH: &str = "aumu.dls .appl";

const LOWPASS_CUTOFF: ParamId = ParamId(0);
const DELAY_MIX: ParamId = ParamId(0);
const DELAY_TIME: ParamId = ParamId(1);
const DELAY_FEEDBACK: ParamId = ParamId(2);
const DELAY_LOWPASS: ParamId = ParamId(3);

fn thread() -> Arc<HostThread> {
    HostThread::spawn().unwrap()
}

fn load(id: &str, state: Option<&[u8]>) -> AuDevice {
    AuDevice::load(&thread(), id, state).unwrap()
}

fn noise(seconds: f64) -> StereoBuffer {
    StereoBuffer::new(
        RATE,
        white_noise(1, 0.25, seconds, RATE),
        white_noise(2, 0.25, seconds, RATE),
    )
}

#[test]
fn the_stock_units_are_listed_and_described() {
    let components = list_components();
    let find = |id: &str| components.iter().find(|c| c.id == id);
    for id in [DELAY, LOWPASS, LIMITER, DLS_SYNTH] {
        assert!(find(id).is_some(), "{id} is not listed");
    }
    assert!(
        components.len() >= 20,
        "only {} units listed",
        components.len()
    );
    assert!(
        components.windows(2).all(|pair| pair[0].id < pair[1].id),
        "not sorted"
    );
    let delay = find(DELAY).unwrap();
    assert_eq!(
        (delay.name.as_str(), delay.vendor.as_str()),
        ("AUDelay", "Apple")
    );

    let thread = thread();
    let info = describe(&thread, delay).unwrap();
    assert_eq!(info.format, PluginFormat::Au);
    assert_eq!(info.kind(), "plugin.au:aufx.dely.appl");
    assert_eq!(info.category, DeviceCategory::Effect);
    assert_eq!((info.audio_inputs, info.audio_outputs), (2, 2));
    assert!(info.loadable && !info.note_input);
    let names: Vec<&str> = info.params.iter().map(|p| p.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "Dry/Wet Mix",
            "Delay Time",
            "Feedback",
            "Lowpass Cutoff Frequency"
        ]
    );
    let time = &info.params[1];
    assert_eq!((time.id, time.max, time.default), (DELAY_TIME, 2.0, 1.0));

    let synth = describe(&thread, find(DLS_SYNTH).unwrap()).unwrap();
    assert_eq!(synth.category, DeviceCategory::Instrument);
    assert_eq!((synth.audio_inputs, synth.audio_outputs), (0, 2));
    assert!(synth.note_input);
    assert_eq!(synth.kind(), "plugin.au:aumu.dls%20.appl");
}

#[test]
fn loading_a_unit_that_does_not_exist_is_an_error() {
    let thread = thread();
    assert!(matches!(
        AuDevice::load(&thread, "aufx.zzzz.zzzz", None),
        Err(HostError::NotFound { .. })
    ));
    assert!(matches!(
        AuDevice::load(&thread, "nonsense", None),
        Err(HostError::InvalidId(_))
    ));
    assert!(matches!(
        AuDevice::load(&thread, "aumx.smxr.appl", None),
        Err(HostError::Unsupported(_))
    ));
}

#[test]
fn the_lowpass_removes_highs_and_keeps_lows() {
    let input = noise(1.0);
    let mut device = load(LOWPASS, None);
    assert!(!device.is_instrument());
    assert_eq!(device.layout(), None, "no layout before prepare");
    device.set_param(LOWPASS_CUTOFF, 500.0);
    let out = render_effect(&mut device, &input, &RenderConfig::default());
    assert_buffer_ok(&out);
    assert_eq!(device.layout(), Some((2, 2, 0)));
    assert_eq!(device.latency_samples(), 0);

    let change = |low: f32, high: f32| {
        spectrum_band_energy_db(&out.left, RATE, low, high)
            - spectrum_band_energy_db(&input.left, RATE, low, high)
    };
    let (lows, highs) = (change(40.0, 200.0), change(4000.0, 12_000.0));
    assert!(lows.abs() < 1.0, "lows changed by {lows} dB");
    assert!(highs < -30.0, "highs only changed by {highs} dB");
    // Both channels are processed, each from its own input.
    let right = spectrum_band_energy_db(&out.right, RATE, 4000.0, 12_000.0)
        - spectrum_band_energy_db(&input.right, RATE, 4000.0, 12_000.0);
    assert!(right < -30.0, "right highs only changed by {right} dB");
    assert!(
        null_residual_db(&out.left, &out.right) > -20.0,
        "channels were merged"
    );

    let controller = device.controller();
    assert_eq!(controller.kind(), "plugin.au:aufx.lpas.appl");
    assert_eq!(controller.param_value(LOWPASS_CUTOFF), Some(500.0));
    assert_eq!(
        controller.param_text(LOWPASS_CUTOFF, 500.0).as_deref(),
        Some("500.00 Hz")
    );
    assert_eq!(controller.param_value(ParamId(999)), None);
    assert_eq!(controller.failed_blocks(), 0);
    assert_eq!(controller.last_error(), None);
}

#[test]
fn a_parameter_change_while_running_is_heard() {
    let mut device = load(LOWPASS, None);
    prepare(&mut device, 4800);
    let tone = sine(8000.0, 0.5, 0.5, RATE);
    let block = |device: &mut AuDevice, index: usize| {
        let slice = &tone[index * 4800..(index + 1) * 4800];
        process_block(device, Some((slice, slice)), &[], 4800).0
    };
    device.set_param(LOWPASS_CUTOFF, 20_000.0);
    let _settle = block(&mut device, 0);
    let open = rms(&block(&mut device, 1));
    device.set_param(LOWPASS_CUTOFF, 200.0);
    let _settle = block(&mut device, 2);
    let closed = rms(&block(&mut device, 3));
    assert!(open > 0.3, "open filter: rms {open}");
    assert!(
        closed < open * 0.01,
        "closed filter: rms {closed} against {open}"
    );
}

#[test]
fn the_delay_echoes_after_its_delay_time() {
    let clicks = click_train(&[0.1], 0.8, 1.0, RATE);
    let input = StereoBuffer::from_mono(RATE, &clicks);

    // Fully wet, no feedback: only the echo, a quarter of a second late.
    let mut wet = load(DELAY, None);
    wet.set_param(DELAY_MIX, 100.0);
    wet.set_param(DELAY_TIME, 0.25);
    wet.set_param(DELAY_FEEDBACK, 0.0);
    wet.set_param(DELAY_LOWPASS, 22_050.0);
    let out = render_effect(&mut wet, &input, &RenderConfig::default().with_block(441));
    assert_buffer_ok(&out);
    let onsets = onset_times(&out.left, RATE);
    assert!(times_match(&onsets, &[0.35], 0.002), "{onsets:?}");
    assert!(is_silent(&out.slice_seconds(0.0, 0.34).left, -80.0));

    // Fully dry: the input, untouched.
    let mut dry = load(DELAY, None);
    dry.set_param(DELAY_MIX, 0.0);
    let out = render_effect(&mut dry, &input, &RenderConfig::default());
    assert_null(&out.left, &input.left, -100.0);
    assert_null(&out.right, &input.right, -100.0);
}

#[test]
fn state_round_trips_through_a_blob_into_a_new_instance() {
    let input = noise(0.5);
    let thread = thread();
    let mut first = AuDevice::load(&thread, LOWPASS, None).unwrap();
    first.set_param(LOWPASS_CUTOFF, 432.0);
    let reference = render_effect(&mut first, &input, &RenderConfig::default());
    let blob = first.controller().save_state().unwrap();
    assert!(blob.starts_with(b"bplist"), "not a binary property list");
    drop(first);

    let mut second = AuDevice::load(&thread, LOWPASS, Some(&blob)).unwrap();
    let controller = second.controller();
    assert_eq!(controller.last_error(), None);
    assert_eq!(controller.param_value(LOWPASS_CUTOFF), Some(432.0));
    let restored = render_effect(&mut second, &input, &RenderConfig::default());
    assert_null(&restored.left, &reference.left, -100.0);

    // A default instance sounds different, so the null above means something.
    let mut plain = AuDevice::load(&thread, LOWPASS, None).unwrap();
    let default = render_effect(&mut plain, &input, &RenderConfig::default());
    assert!(null_residual_db(&default.left, &reference.left) > -40.0);

    // Restoring into a live instance, and refusing junk.
    plain.controller().load_state(&blob).unwrap();
    assert_eq!(plain.controller().param_value(LOWPASS_CUTOFF), Some(432.0));
    assert!(matches!(
        plain.controller().load_state(b"not a property list"),
        Err(HostError::State(_))
    ));
    let bad = AuDevice::load(&thread, LOWPASS, Some(b"junk")).unwrap();
    assert!(bad.controller().last_error().is_some());
}

/// source -> AUPeakLimiter (well below its threshold) -> mixer port 0
/// source -> gain(-1)                                 -> mixer port 1
fn null_graph(compensate: bool) -> CompiledGraph {
    let id = DeviceId;
    let mut builder = GraphBuilder::stereo();
    builder.set_delay_compensation(compensate);
    builder
        .add_node(id(1), Box::new(SineOsc::new(441.0, 0.05)), 0)
        .unwrap();
    builder
        .add_node(id(2), Box::new(load(LIMITER, None)), 1)
        .unwrap();
    builder
        .add_node(id(3), Box::new(Gain::new(-1.0)), 1)
        .unwrap();
    builder.add_node(id(4), Box::new(Mixer::new(2)), 2).unwrap();
    builder.connect(id(1), id(2), 0).unwrap();
    builder.connect(id(1), id(3), 0).unwrap();
    builder.connect(id(2), id(4), 0).unwrap();
    builder.connect(id(3), id(4), 1).unwrap();
    builder.set_output(id(4)).unwrap();
    builder.compile(RATE, 256).unwrap()
}

#[test]
fn the_limiters_reported_latency_makes_the_delay_compensation_null_test_pass() {
    let compensated = null_graph(true);
    let latency = compensated.node_latency(DeviceId(2)).unwrap();
    assert!(latency > 0, "the limiter reported no latency");
    assert_eq!(compensated.latency_samples(), latency);
    let out = render_graph(compensated, tempo(), 0.5).unwrap();
    println!(
        "limiter latency {latency}, compensated residual peak {}",
        peak(&out.left)
    );
    assert!(
        peak(&out.left) < 0.05 * 0.01,
        "residual peak {}",
        peak(&out.left)
    );

    let raw = render_graph(null_graph(false), tempo(), 0.5).unwrap();
    println!("uncompensated residual peak {}", peak(&raw.left));
    assert!(peak(&raw.left) > 0.05 * 0.2, "peak {}", peak(&raw.left));
}

#[test]
fn the_dls_synth_plays_the_pitch_of_each_note() {
    let mut device = load(DLS_SYNTH, None);
    assert!(device.is_instrument());
    for (pitch, hz) in [(69u8, 440.0f32), (57, 220.0), (81, 880.0)] {
        let events = note(0.25, 0.5, pitch, 100, RATE);
        let out = render_instrument(&mut device, &events, 1.5, &RenderConfig::default());
        assert_buffer_ok(&out);
        assert!(
            is_silent(&out.slice_seconds(0.0, 0.24).left, -80.0),
            "sound before the note"
        );
        let onsets = onset_times(&out.mono(), RATE);
        assert!(
            onsets.first().is_some_and(|t| (t - 0.25).abs() < 0.02),
            "pitch {pitch}: onsets {onsets:?}"
        );
        let held = out.slice_seconds(0.3, 0.7).mono();
        assert!(rms(&held) > 0.005, "pitch {pitch}: rms {}", rms(&held));
        let found = dominant_frequency(&held, RATE);
        assert!(
            (found - hz).abs() < hz * 0.02,
            "pitch {pitch}: {found} Hz, expected {hz}"
        );
    }
    assert_eq!(device.controller().failed_blocks(), 0);
}

#[test]
fn an_unprepared_effect_passes_audio_through_and_an_instrument_is_silent() {
    let ramp: Vec<f32> = (0..32).map(|i| i as f32 / 64.0).collect();
    let mut lowpass = load(LOWPASS, None);
    let (left, right) = process_block(&mut lowpass, Some((&ramp, &ramp)), &[], 32);
    assert_eq!((left, right), (ramp.clone(), ramp.clone()));
    // Prepared for 16 frames, asked for 32: passed through, not overrun.
    prepare(&mut lowpass, 16);
    let (left, _) = process_block(&mut lowpass, Some((&ramp, &ramp)), &[], 32);
    assert_eq!(left, ramp);

    let mut synth = load(DLS_SYNTH, None);
    let (left, _) = process_block(&mut synth, None, &[], 32);
    assert_eq!(left, vec![0.0; 32]);
}

#[test]
fn a_scanned_unit_is_built_through_the_registry() {
    let host = PluginHost::with_thread(thread());
    let mut database = PluginDatabase::new();
    let report = database.rescan(host.thread(), &ScanOptions::none().with_audio_units());
    assert!(report.plugins >= 20, "{report:?}");
    assert!(database.failures.is_empty(), "{:?}", database.failures);
    let lowpass = database.find("plugin.au:aufx.lpas.appl").unwrap();
    assert_eq!(lowpass.name, "AULowpass");

    // A second scan reuses every description.
    let again = database
        .clone()
        .rescan(host.thread(), &ScanOptions::none().with_audio_units());
    assert_eq!(again.plugins, report.plugins);

    let mut registry = DeviceRegistry::new();
    let added = host.register_database(&mut registry, &database).unwrap();
    assert_eq!(added, report.plugins);
    let descriptor = registry.descriptor("plugin.au:aufx.lpas.appl").unwrap();
    assert_eq!(
        descriptor.param_by_name("Cutoff Frequency").map(|p| p.id),
        Some(LOWPASS_CUTOFF)
    );

    let params = [(LOWPASS_CUTOFF, 500.0)];
    let mut device = registry
        .create(
            "plugin.au:aufx.lpas.appl",
            &DeviceInit::new().with_params(&params),
        )
        .unwrap();
    let input = noise(0.5);
    let out = render_effect(device.as_mut(), &input, &RenderConfig::default());
    let highs = spectrum_band_energy_db(&out.left, RATE, 4000.0, 12_000.0)
        - spectrum_band_energy_db(&input.left, RATE, 4000.0, 12_000.0);
    assert!(highs < -30.0, "highs only changed by {highs} dB");

    let controllers = host.controllers();
    assert_eq!(controllers.len(), 1);
    assert_eq!(controllers[0].param_value(LOWPASS_CUTOFF), Some(500.0));
    drop((device, controllers));
    assert!(host.controllers().is_empty());
}
