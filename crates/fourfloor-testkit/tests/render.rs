//! Tests for the render helpers, using the engine's built-in devices and a
//! few tiny devices defined here.
//!
//! This binary installs the allocation guard, so every `process` call made
//! by `render_device` aborts the test if it allocates.

use std::process::Command;

use fourfloor_core::{
    Device, DeviceId, DeviceRegistry, EventKind, PrepareConfig, ProcessIo, SampleRate, Tempo,
    Ticks, TransportContext,
};
use fourfloor_engine::devices::{Fader, Gain, NotePlayer, PlaceholderSynth, PlayerNote, SineOsc};
use fourfloor_engine::GraphBuilder;
use fourfloor_model::command::SetTrackPan;
use fourfloor_model::sequencer::{write_house_pattern, HousePattern};
use fourfloor_model::{CommandBus, Project};
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, dominant_frequency, find_nan_or_clipping, is_silent, note,
    notes, null_test, onset_times, peak, render_device, render_effect,
    render_effect_with_sidechain, render_graph, render_instrument, render_project, rms, sine,
    spectrum_band_energy, stereo_correlation, times_match, AllocDisabler, DeviceInput, Fault,
    RenderConfig, StereoBuffer, TimedEvent,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const RATE: SampleRate = SampleRate::HZ_48000;
const CHILD_ENV: &str = "FOURFLOOR_TESTKIT_ALLOC_CHILD";

fn config() -> RenderConfig {
    RenderConfig::default()
}

#[test]
fn an_instrument_plays_notes_at_the_right_time_and_pitch() {
    let events = notes(&[(0.25, 0.25, 69, 127), (0.75, 0.1, 57, 127)], RATE);
    let out = render_instrument(&mut PlaceholderSynth::new(0.8), &events, 1.0, &config());
    assert_buffer_ok(&out);
    assert_eq!(out.frames(), 48_000);
    assert_eq!(out.sample_rate, RATE);

    assert!(is_silent(&out.slice_seconds(0.0, 0.25).left, -120.0));
    let first = out.slice_seconds(0.27, 0.48);
    assert!((dominant_frequency(&first.left, RATE) - 440.0).abs() < 1.0);
    assert!(is_silent(&out.slice_seconds(0.52, 0.75).left, -120.0));
    let second = out.slice_seconds(0.76, 0.84);
    assert!((dominant_frequency(&second.left, RATE) - 220.0).abs() < 2.0);
    assert!(is_silent(&out.slice_seconds(0.87, 1.0).left, -120.0));

    let found = onset_times(&out.mono(), RATE);
    assert!(times_match(&found, &[0.25, 0.75], 0.003), "{found:?}");
    assert!((stereo_correlation(&out.left, &out.right) - 1.0).abs() < 1e-6);
}

#[test]
fn rendering_does_not_depend_on_the_block_size() {
    let events = notes(
        &[
            (0.0, 0.1, 36, 120),
            (0.1, 0.05, 42, 100),
            (0.2, 0.2, 60, 90),
        ],
        RATE,
    );
    let reference = render_instrument(&mut PlaceholderSynth::new(0.5), &events, 0.5, &config());
    for block in [1, 7, 64, 480, 4_096, 100_000] {
        let out = render_instrument(
            &mut PlaceholderSynth::new(0.5),
            &events,
            0.5,
            &config().with_block(block),
        );
        assert_null(&out.left, &reference.left, -140.0);
        assert_null(&out.right, &reference.right, -140.0);
    }
}

#[test]
fn an_effect_processes_each_channel_of_its_input() {
    let input = StereoBuffer::new(
        RATE,
        sine(440.0, 0.5, 0.25, RATE),
        sine(880.0, 0.25, 0.25, RATE),
    );
    let out = render_effect(&mut Gain::new(0.5), &input, &config());
    assert_eq!(out.frames(), input.frames());
    assert!((peak(&out.left) - 0.25).abs() < 1e-4);
    assert!((peak(&out.right) - 0.125).abs() < 1e-4);
    assert!((dominant_frequency(&out.left, RATE) - 440.0).abs() < 1.0);
    assert!((dominant_frequency(&out.right, RATE) - 880.0).abs() < 1.0);

    // Unity gain is a perfect null against the input.
    let same = render_effect(&mut Gain::new(1.0), &input, &config().with_block(100));
    assert_null(&same.left, &input.left, -140.0);
    assert_null(&same.right, &input.right, -140.0);

    // Hard left on the fader: the right channel disappears.
    let panned = render_effect(&mut Fader::new(0.0, -1.0), &input, &config());
    assert_null(&panned.left, &input.left, -140.0);
    assert!(is_silent(&panned.right, -100.0));
}

/// Mutes its input wherever the sidechain is louder than 0.1; reports
/// through its output whether a sidechain bus was connected at all.
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
                *o = if k.abs() > 0.1 { 0.0 } else { *i };
            }
        }
    }
}

#[test]
fn a_sidechain_reaches_the_device_only_when_given() {
    let input = StereoBuffer::from_mono(RATE, &vec![0.5; 4_800]);
    // The key is silent for the first half and loud for the second.
    let mut key = vec![0.0f32; 2_400];
    key.extend(vec![1.0f32; 2_400]);
    let key = StereoBuffer::from_mono(RATE, &key);

    let ducked = render_effect_with_sidechain(&mut Ducker, &input, &key, &config().with_block(100));
    assert_eq!(rms(&ducked.left[..2_400]), 0.5);
    assert!(is_silent(&ducked.left[2_400..], -120.0));

    let open = render_effect(&mut Ducker, &input, &config());
    assert_null(&open.left, &input.left, -140.0);

    // A key shorter than the render is padded with silence.
    let short_key = StereoBuffer::from_mono(RATE, &[1.0; 100]);
    let tail_open = render_effect_with_sidechain(&mut Ducker, &input, &short_key, &config());
    assert!(is_silent(&tail_open.left[..100], -120.0));
    assert_eq!(rms(&tail_open.left[100..]), 0.5);
}

#[test]
fn emitted_events_come_back_at_absolute_frames() {
    // 120 BPM at 48 kHz: tick 480 is frame 12 000, tick 720 is 18 000.
    let player_notes = [PlayerNote::new(Ticks::new(480), Ticks::new(240), 60, 100)];
    let mut player = NotePlayer::new(&player_notes);
    let out = render_device(
        &mut player,
        &DeviceInput::default(),
        24_000,
        &config().with_block(500),
    );
    assert_eq!(
        out.events,
        [
            TimedEvent::new(12_000, EventKind::note_on(60, 100)),
            TimedEvent::new(18_000, EventKind::note_off(60)),
        ]
    );
    assert!(is_silent(&out.audio.left, -200.0));

    // Stopped: the player emits nothing.
    let mut stopped_player = NotePlayer::new(&player_notes);
    let idle = render_device(
        &mut stopped_player,
        &DeviceInput::default(),
        24_000,
        &config().stopped(),
    );
    assert!(idle.events.is_empty());

    // Tempo is passed through: at 60 BPM the note starts at frame 24 000.
    let slow = config().with_tempo(Tempo::new(60.0).unwrap());
    let mut slow_player = NotePlayer::new(&player_notes);
    let late = render_device(&mut slow_player, &DeviceInput::default(), 30_000, &slow);
    assert_eq!(late.events[0].frame, 24_000);
}

#[test]
fn note_helpers_build_sorted_events() {
    let [on, off] = note(0.5, 0.25, 60, 100, RATE);
    assert_eq!(on, TimedEvent::new(24_000, EventKind::note_on(60, 100)));
    assert_eq!(off, TimedEvent::new(36_000, EventKind::note_off(60)));
    let events = notes(&[(0.5, 0.5, 62, 90), (0.0, 0.5, 60, 100)], RATE);
    let frames: Vec<u64> = events.iter().map(|event| event.frame).collect();
    assert_eq!(frames, [0, 24_000, 24_000, 48_000]);
    assert!(
        events[1].kind.is_note_off(),
        "note-off first at equal times"
    );
    assert_eq!(
        RenderConfig::at_rate(SampleRate::HZ_44100).sample_rate.hz(),
        44_100
    );
    assert_eq!(config().with_block(0).block, 1);
}

/// Breaks the contract: writes only the first half of each block.
struct Lazy;

impl Device for Lazy {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        for channel in io.outputs.iter_mut() {
            let half = channel.len() / 2;
            channel[..half].fill(0.0);
        }
    }
}

#[test]
fn a_device_that_leaves_output_unwritten_is_caught() {
    let out = render_instrument(&mut Lazy, &[], 0.01, &config().with_block(64));
    assert_eq!(
        find_nan_or_clipping(&out.left),
        Some(Fault::NotFinite { index: 32 })
    );
}

#[test]
fn a_graph_renders_through_the_real_engine() {
    let mut builder = GraphBuilder::stereo();
    builder
        .add_node(DeviceId(1), Box::new(SineOsc::new(1_000.0, 0.5)), 0)
        .unwrap();
    builder
        .add_node(DeviceId(2), Box::new(Fader::new(-6.0, 1.0)), 1)
        .unwrap();
    builder.connect(DeviceId(1), DeviceId(2), 0).unwrap();
    builder.set_output(DeviceId(2)).unwrap();
    let graph = builder.compile(SampleRate::HZ_44100, 256).unwrap();

    let out = render_graph(graph, Tempo::DEFAULT, 0.5).unwrap();
    assert_eq!(out.sample_rate, SampleRate::HZ_44100);
    assert_eq!(out.frames(), 22_050);
    assert!(is_silent(&out.left, -100.0), "panned hard right");
    assert!((dominant_frequency(&out.right, out.sample_rate) - 1_000.0).abs() < 1.0);
    assert!((peak(&out.right) - 0.5 * 0.501_187).abs() < 1e-3);
}

fn registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    registry
}

#[test]
fn a_project_renders_to_true_stereo() {
    let mut bus = CommandBus::new(Project::new("Test"));
    let written = write_house_pattern(&mut bus, &HousePattern::default()).unwrap();
    let centred = render_project(bus.project(), &registry(), RATE, 0.25).unwrap();
    assert_buffer_ok(&centred);
    // One bar at 120 BPM plus the tail.
    assert_eq!(centred.frames(), 96_000 + 12_000);
    assert!(null_test(&centred.left, &centred.right, -120.0));

    // The house pattern has a hit on every sixteenth: kicks on the beats,
    // hats between them.
    let found = onset_times(&centred.mono(), RATE);
    let sixteenths: Vec<f64> = (0..16).map(|step| f64::from(step) * 0.125).collect();
    assert!(times_match(&found, &sixteenths, 0.005), "{found:?}");
    // Only the beats carry the kick's low end.
    let mono = centred.mono();
    for step in 0..16 {
        let start = step * 6_000;
        let low = spectrum_band_energy(&mono[start..start + 4_800], RATE, 30.0, 150.0);
        if step % 4 == 0 {
            assert!(low > 1e-3, "beat {}: low-band power {low}", step / 4);
        } else if step % 4 == 2 {
            // Half a beat on, the kick has rung out.
            assert!(low < 1e-4, "step {step}: low-band power {low}");
        }
    }

    bus.execute(SetTrackPan {
        track: written.hat_track,
        pan: 1.0,
    })
    .unwrap();
    let panned = render_project(bus.project(), &registry(), RATE, 0.25).unwrap();
    assert!(
        !null_test(&panned.left, &panned.right, -40.0),
        "no longer dual mono"
    );
    assert!(
        rms(&panned.right) > rms(&panned.left),
        "hats are on the right"
    );
    assert!(stereo_correlation(&panned.left, &panned.right) < 0.999);

    // A WAV written from the render reads back sample for sample.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("panned.wav");
    panned.write_wav(&path).unwrap();
    assert_eq!(StereoBuffer::read_wav(&path).unwrap(), panned);
}

/// Allocates in `process`, which the contract forbids.
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
#[ignore = "aborts on purpose; run by render_device_aborts_when_a_device_allocates"]
fn child_renders_an_allocating_device() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    render_instrument(&mut Allocating, &[], 0.01, &config());
}

#[test]
fn render_device_aborts_when_a_device_allocates() {
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
