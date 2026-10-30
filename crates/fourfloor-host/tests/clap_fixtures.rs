//! Hosts the in-repo fixture CLAP plugins (`test-plugins/`) and checks what
//! comes out on rendered audio.

mod common;

use std::sync::Arc;

use common::{
    fixture, prepare, process_block, tempo, DELAY_ID, DELAY_SAMPLES, GAIN_ID, PARAM, RATE, SINE_ID,
};
use fourfloor_core::{Device, DeviceCategory, DeviceId, Event, ParamId};
use fourfloor_engine::devices::{Gain, Mixer, SineOsc};
use fourfloor_engine::{CompiledGraph, GraphBuilder};
use fourfloor_host::thread::HostThread;
use fourfloor_host::{
    clap::scan_bundle, ClapDevice, HostError, PluginDatabase, PluginFormat, ScanOptions,
};
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, dominant_frequency, is_silent, note, null_residual_db,
    onset_times, peak, render_effect, render_graph, render_instrument, rms_window_seconds,
    times_match, white_noise, AllocDisabler, RenderConfig, StereoBuffer,
};

// Every render through the testkit or the engine runs under the allocation
// guard: a host-side allocation in `process` aborts the test binary.
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn thread() -> Arc<HostThread> {
    HostThread::spawn().unwrap()
}

fn load(id: &str, state: Option<&[u8]>) -> ClapDevice {
    ClapDevice::load(&thread(), &fixture().bundle, id, state).unwrap()
}

fn noise() -> StereoBuffer {
    StereoBuffer::new(
        RATE,
        white_noise(1, 0.5, 0.5, RATE),
        white_noise(2, 0.5, 0.5, RATE),
    )
}

fn scaled(signal: &[f32], gain: f32) -> Vec<f32> {
    signal.iter().map(|s| s * gain).collect()
}

#[test]
fn scanning_the_bundle_describes_all_three_plugins() {
    let scan = scan_bundle(&thread(), &fixture().bundle).unwrap();
    assert!(scan.failures.is_empty(), "{:?}", scan.failures);
    let find = |id: &str| scan.plugins.iter().find(|p| p.id == id).unwrap();

    let gain = find(GAIN_ID);
    assert_eq!(gain.name, "Fourfloor Test Gain");
    assert_eq!(gain.vendor, "fourfloor");
    assert_eq!(gain.category, DeviceCategory::Effect);
    assert_eq!((gain.audio_inputs, gain.audio_outputs), (2, 2));
    assert!(!gain.sidechain && !gain.note_input && gain.loadable);
    assert_eq!(gain.params.len(), 1);
    let param = &gain.params[0];
    assert_eq!((param.id, param.name.as_ref()), (PARAM, "Gain"));
    assert_eq!((param.min, param.max, param.default), (0.0, 4.0, 1.0));
    assert_eq!(gain.kind(), "plugin.clap:org.fourfloor.test.gain");

    let delay = find(DELAY_ID);
    assert_eq!(delay.category, DeviceCategory::Effect);
    assert!(delay.params.is_empty());

    let sine = find(SINE_ID);
    assert_eq!(sine.category, DeviceCategory::Instrument);
    assert_eq!((sine.audio_inputs, sine.audio_outputs), (0, 2));
    assert!(sine.note_input && !sine.note_output);
    assert_eq!(sine.params[0].name.as_ref(), "Level");
}

#[test]
fn an_unknown_plugin_id_in_a_real_bundle_is_not_found() {
    let result = ClapDevice::load(
        &thread(),
        &fixture().bundle,
        "org.fourfloor.test.nope",
        None,
    );
    assert!(
        matches!(&result, Err(HostError::NotFound { id }) if id == "org.fourfloor.test.nope"),
        "{:?}",
        result.err()
    );
}

#[test]
fn the_gain_plugin_applies_its_gain_exactly() {
    let input = noise();
    let mut device = load(GAIN_ID, None);
    assert_eq!(device.params().len(), 1);
    assert_eq!((device.audio_inputs(), device.audio_outputs()), (2, 2));

    // Default: unity.
    let unity = render_effect(&mut device, &input, &RenderConfig::default());
    assert_buffer_ok(&unity);
    assert_null(&unity.left, &input.left, -120.0);
    assert_null(&unity.right, &input.right, -120.0);

    // Set before `prepare`: sent to the plugin when it is activated.
    device.set_param(PARAM, 0.25);
    let quiet = render_effect(&mut device, &input, &RenderConfig::default());
    assert_null(&quiet.left, &scaled(&input.left, 0.25), -120.0);
    assert_null(&quiet.right, &scaled(&input.right, 0.25), -120.0);
    assert!(
        null_residual_db(&quiet.left, &input.left) > -20.0,
        "gain had no effect"
    );

    let controller = device.controller();
    assert_eq!(controller.param_value(PARAM), Some(0.25));
    assert_eq!(
        controller.param_text(PARAM, 0.25).as_deref(),
        Some("0.25 x")
    );
    assert_eq!(controller.param_value(ParamId(99)), None);
    assert_eq!(controller.failed_blocks(), 0);
    assert_eq!(controller.dropped_events(), 0);
    assert_eq!(controller.last_error(), None);
}

#[test]
fn a_parameter_change_while_running_lands_on_the_next_block() {
    let mut device = load(GAIN_ID, None);
    prepare(&mut device, 64);
    let ones = vec![1.0f32; 64];

    let (left, _) = process_block(&mut device, Some((&ones, &ones)), &[], 64);
    assert_eq!(left, vec![1.0; 64]);

    device.set_param(PARAM, 2.0);
    device.set_param(PARAM, 0.5); // the last value wins
    let (left, right) = process_block(&mut device, Some((&ones, &ones)), &[], 64);
    assert_eq!(left, vec![0.5; 64]);
    assert_eq!(right, vec![0.5; 64]);

    // Out-of-range values are clamped to the descriptor's range; unknown
    // parameters are ignored.
    device.set_param(PARAM, 100.0);
    device.set_param(ParamId(1234), 0.0);
    let (left, _) = process_block(&mut device, Some((&ones, &ones)), &[], 64);
    assert_eq!(left, vec![4.0; 64]);
}

#[test]
fn an_unprepared_effect_passes_audio_through_and_an_instrument_is_silent() {
    let ramp: Vec<f32> = (0..32).map(|i| i as f32 / 64.0).collect();
    let mut gain = load(GAIN_ID, None);
    gain.set_param(PARAM, 0.0);
    let (left, right) = process_block(&mut gain, Some((&ramp, &ramp)), &[], 32);
    assert_eq!((left, right), (ramp.clone(), ramp.clone()));

    // Prepared for 16 frames, asked for 32: the block is passed through
    // instead of overrunning the plugin's buffers.
    prepare(&mut gain, 16);
    let (left, _) = process_block(&mut gain, Some((&ramp, &ramp)), &[], 32);
    assert_eq!(left, ramp);

    let mut sine = load(SINE_ID, None);
    let (left, right) = process_block(&mut sine, None, &[Event::note_on(0, 69, 127)], 32);
    assert_eq!((left, right), (vec![0.0; 32], vec![0.0; 32]));
}

#[test]
fn the_sine_instrument_plays_the_pitch_of_each_note() {
    let mut device = load(SINE_ID, None);
    assert!(device.takes_notes() && !device.emits_notes());
    assert_eq!(device.audio_inputs(), 0);

    for (pitch, hz) in [(69u8, 440.0f32), (57, 220.0), (81, 880.0), (60, 261.626)] {
        let events = note(0.0, 0.5, pitch, 127, RATE);
        let out = render_instrument(&mut device, &events, 0.5, &RenderConfig::default());
        assert_buffer_ok(&out);
        let found = dominant_frequency(&out.left, RATE);
        assert!(
            (found - hz).abs() < 1.0,
            "pitch {pitch}: {found} Hz, expected {hz}"
        );
        assert_null(&out.left, &out.right, -120.0);
    }
}

#[test]
fn notes_start_and_stop_on_time_at_the_right_level() {
    let mut device = load(SINE_ID, None);
    // Two notes: 0.10-0.30 s and 0.50-0.70 s. The second is at half velocity.
    let mut events = note(0.1, 0.2, 69, 127, RATE).to_vec();
    events.extend(note(0.5, 0.2, 69, 64, RATE));
    // A block size that does not divide the note times.
    let config = RenderConfig::default().with_block(333);
    let out = render_instrument(&mut device, &events, 1.0, &config);
    assert_buffer_ok(&out);

    let onsets = onset_times(&out.left, RATE);
    assert!(times_match(&onsets, &[0.1, 0.5], 0.004), "{onsets:?}");

    // Level parameter 0.5: a full-velocity note peaks at 0.5.
    let loud = out.slice_seconds(0.15, 0.28);
    assert!(
        (peak(&loud.left) - 0.5).abs() < 0.005,
        "{}",
        peak(&loud.left)
    );
    let soft = out.slice_seconds(0.55, 0.68);
    let expected = 0.5 * 64.0 / 127.0;
    assert!(
        (peak(&soft.left) - expected).abs() < 0.005,
        "{}",
        peak(&soft.left)
    );

    // Silent before the first note, between notes and after the release.
    assert!(is_silent(&out.slice_seconds(0.0, 0.099).left, -100.0));
    assert!(is_silent(&out.slice_seconds(0.32, 0.499).left, -100.0));
    assert!(is_silent(&out.slice_seconds(0.72, 1.0).left, -100.0));
    assert!(rms_window_seconds(&out.left, RATE, 0.15, 0.1) > 0.3);
}

#[test]
fn all_notes_off_and_reset_silence_held_notes() {
    use fourfloor_core::EventKind;
    let mut device = load(SINE_ID, None);
    prepare(&mut device, 4800);
    let chord = [
        Event::note_on(0, 60, 100),
        Event::note_on(0, 64, 100),
        Event::note_on(0, 67, 100),
    ];
    let (held, _) = process_block(&mut device, None, &chord, 4800);
    assert!(peak(&held[2400..]) > 0.3);

    let off = [Event::new(0, EventKind::AllNotesOff)];
    let _release = process_block(&mut device, None, &off, 4800);
    let (after, _) = process_block(&mut device, None, &[], 4800);
    assert!(
        is_silent(&after, -100.0),
        "notes still sounding after all-notes-off"
    );

    let _ = process_block(&mut device, None, &chord, 4800);
    device.reset();
    let (after, _) = process_block(&mut device, None, &[], 4800);
    assert!(
        is_silent(&after, -100.0),
        "notes still sounding after reset"
    );
}

#[test]
fn state_round_trips_through_a_blob_into_a_new_instance() {
    let input = noise();
    let thread = thread();
    let mut first = ClapDevice::load(&thread, &fixture().bundle, GAIN_ID, None).unwrap();
    first.set_param(PARAM, 0.375);
    let reference = render_effect(&mut first, &input, &RenderConfig::default());
    let blob = first.controller().save_state().unwrap();
    assert!(!blob.is_empty());
    drop(first);

    // A fresh instance built from the blob sounds the same.
    let mut second = ClapDevice::load(&thread, &fixture().bundle, GAIN_ID, Some(&blob)).unwrap();
    let controller = second.controller();
    assert_eq!(controller.last_error(), None);
    assert_eq!(controller.param_value(PARAM), Some(0.375));
    let restored = render_effect(&mut second, &input, &RenderConfig::default());
    assert_null(&restored.left, &reference.left, -120.0);
    assert_null(&restored.left, &scaled(&input.left, 0.375), -120.0);
    assert_eq!(
        controller.save_state().unwrap(),
        blob,
        "saving again gives the same blob"
    );

    // Restoring into a running instance works too.
    let mut third = ClapDevice::load(&thread, &fixture().bundle, GAIN_ID, None).unwrap();
    prepare(&mut third, 64);
    third.controller().load_state(&blob).unwrap();
    let ones = vec![1.0f32; 64];
    let (left, _) = process_block(&mut third, Some((&ones, &ones)), &[], 64);
    assert_eq!(left, vec![0.375; 64]);
}

#[test]
fn a_foreign_state_blob_is_reported_and_the_plugin_keeps_its_defaults() {
    let mut device = load(GAIN_ID, Some(b"definitely not a gain state"));
    let controller = device.controller();
    let error = controller
        .last_error()
        .expect("the bad blob was not reported");
    assert!(error.contains("state"), "{error}");
    assert_eq!(controller.param_value(PARAM), Some(1.0));
    assert!(matches!(
        controller.load_state(b"junk"),
        Err(HostError::State(_))
    ));

    let input = noise();
    let out = render_effect(&mut device, &input, &RenderConfig::default());
    assert_null(&out.left, &input.left, -120.0);

    // The delay plugin has no state extension at all.
    let delay = load(DELAY_ID, None);
    assert!(matches!(
        delay.controller().save_state(),
        Err(HostError::Unsupported(_))
    ));
}

#[test]
fn the_delay_plugin_reports_its_latency_and_delays_by_exactly_that() {
    let mut device = load(DELAY_ID, None);
    assert_eq!(
        device.latency_samples(),
        0,
        "latency is unknown before activation"
    );
    let input = noise();
    let out = render_effect(&mut device, &input, &RenderConfig::default());
    assert_buffer_ok(&out);
    assert_eq!(device.latency_samples(), DELAY_SAMPLES);
    assert_eq!(device.controller().latency_samples(), DELAY_SAMPLES);

    let delay = DELAY_SAMPLES as usize;
    assert!(is_silent(&out.left[..delay], -120.0));
    let frames = input.frames();
    assert_null(&out.left[delay..], &input.left[..frames - delay], -120.0);
    assert_null(&out.right[delay..], &input.right[..frames - delay], -120.0);
}

/// source -> CLAP delay plugin -> mixer port 0
/// source -> gain(-1)          -> mixer port 1
fn null_graph(compensate: bool) -> CompiledGraph {
    let id = DeviceId;
    let mut builder = GraphBuilder::stereo();
    builder.set_delay_compensation(compensate);
    builder
        .add_node(id(1), Box::new(SineOsc::new(441.0, 0.8)), 0)
        .unwrap();
    builder
        .add_node(id(2), Box::new(load(DELAY_ID, None)), 1)
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
    builder.compile(RATE, 128).unwrap()
}

#[test]
fn the_reported_latency_makes_the_delay_compensation_null_test_pass() {
    let compensated = null_graph(true);
    assert_eq!(compensated.node_latency(DeviceId(2)), Some(DELAY_SAMPLES));
    assert_eq!(compensated.latency_samples(), DELAY_SAMPLES);
    assert_eq!(
        compensated.edge_delay(DeviceId(3), DeviceId(4), 1),
        Some(DELAY_SAMPLES)
    );
    let out = render_graph(compensated, tempo(), 0.5).unwrap();
    assert!(
        is_silent(&out.left, -120.0) && is_silent(&out.right, -120.0),
        "the delayed path did not cancel: peak {}",
        peak(&out.left)
    );

    // Without compensation the two paths are 441 samples apart and do not
    // cancel, which proves the null above is the compensation's doing.
    let raw = render_graph(null_graph(false), tempo(), 0.5).unwrap();
    assert!(peak(&raw.left) > 0.5, "peak {}", peak(&raw.left));
}

#[test]
fn a_database_scan_finds_the_fixture_and_round_trips_through_json() {
    let thread = thread();
    let options = ScanOptions::none().with_clap_dir(&fixture().dir);
    let mut database = PluginDatabase::new();
    let first = database.rescan(&thread, &options);
    assert_eq!((first.scanned_files, first.reused_files), (1, 0));
    assert_eq!((first.plugins, first.failures), (3, 0));
    let kinds: Vec<String> = database.plugins.iter().map(|p| p.kind()).collect();
    assert_eq!(
        kinds,
        [
            "plugin.clap:org.fourfloor.test.delay",
            "plugin.clap:org.fourfloor.test.gain",
            "plugin.clap:org.fourfloor.test.sine",
        ]
    );
    assert_eq!(database.by_format(PluginFormat::Clap).count(), 3);

    // An unchanged file is not loaded again.
    let second = database.rescan(&thread, &options);
    assert_eq!(
        (second.scanned_files, second.reused_files, second.plugins),
        (0, 1, 3)
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache/plugins.json");
    database.save(&path).unwrap();
    let loaded = PluginDatabase::load(&path).unwrap();
    assert_eq!(loaded, database);
    let sine = loaded.find("plugin.clap:org.fourfloor.test.sine").unwrap();
    assert_eq!(sine.path, fixture().bundle);
}
