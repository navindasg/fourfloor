//! Moving parameters, heard: automation lanes, LFO and envelope
//! modulation, and macros.

mod common;

use common::*;
use fourfloor_core::{AudioMessage, DeviceId, SampleRate, Ticks, TrackId};
use fourfloor_engine::devices::{Gain, KIND_GAIN};
use fourfloor_model::command::{
    AddMacro, AddModulator, SetMacro, SetMacroTarget, SetModulation, WriteAutomation,
    WriteTrackAutomation,
};
use fourfloor_model::{
    AutomationPoint, Command, CommandBus, LfoRate, LfoShape, MixerParam, ModSource,
};
use fourfloor_testkit::{
    assert_null, dominant_frequency, render_graph, rms, rms_window, rms_window_seconds,
    AllocDisabler, StereoBuffer,
};

// Every render in this file runs under the engine's allocation guard.
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const FULL: f32 = 0.25 * std::f32::consts::FRAC_1_SQRT_2;

fn point(time: i64, value: f32) -> AutomationPoint {
    AutomationPoint {
        time: Ticks::new(time),
        value,
    }
}

/// A held note through a gain device set to `gain`.
fn gained(beats: i64, gain: f32) -> (CommandBus, TrackId, DeviceId, DeviceId) {
    let mut bus = new_bus();
    let (track, tone) = held_track(&mut bus, "Pad", beats, 69);
    let device = add_device(
        &mut bus,
        track,
        KIND_GAIN,
        vec![param(Gain::GAIN, "Gain", gain, 0.0, 4.0)],
    );
    (bus, track, tone, device)
}

#[test]
fn an_automation_lane_ramps_a_gain() {
    let (mut bus, _, _, gain) = gained(4, 1.0);
    bus.execute(WriteAutomation {
        device: gain,
        param: Gain::GAIN,
        points: vec![
            point(0, 0.0),
            point(1_920, 1.0),
            point(2_880, 1.0),
            point(2_880, 0.25),
        ],
    })
    .unwrap();
    let out = render(&bus);
    // Linear from 0 to 1 over the first two beats (one second).
    for (at, expected) in [(0.25, 0.25), (0.5, 0.5), (0.75, 0.75)] {
        let level = rms_window_seconds(&out.left, RATE, at - 0.025, 0.05) / FULL;
        assert!((level - expected).abs() < 0.03, "at {at} s: {level}");
    }
    // Held at 1, then a step down to 0.25 on beat 3.
    let held = rms_window_seconds(&out.left, RATE, 1.2, 0.2) / FULL;
    assert!((held - 1.0).abs() < 0.01, "{held}");
    let after = rms_window_seconds(&out.left, RATE, 1.55, 0.4) / FULL;
    assert!((after - 0.25).abs() < 0.01, "{after}");
    // The ramp keeps rising: every 40 ms window is louder than the last.
    let steps: Vec<f32> = (0..24)
        .map(|n| rms_window(&out.left, n * 1_920, 1_920))
        .collect();
    assert!(steps.windows(2).all(|pair| pair[1] > pair[0]), "{steps:?}");
}

#[test]
fn track_automation_moves_the_fader_and_the_pan() {
    let (mut bus, track, _, _) = gained(4, 1.0);
    bus.execute(WriteTrackAutomation {
        track,
        param: MixerParam::Pan,
        points: vec![point(0, -1.0), point(3_840, 1.0)],
    })
    .unwrap();
    let out = render(&bus);
    let side = |signal: &[f32], at: f64| rms_window_seconds(signal, RATE, at, 0.05) / FULL;
    assert!(side(&out.left, 0.05) > 0.99 && side(&out.right, 0.05) < 0.15);
    assert!(side(&out.right, 1.9) > 0.99 && side(&out.left, 1.9) < 0.15);
    let (left, right) = (side(&out.left, 0.975), side(&out.right, 0.975));
    assert!(
        left > 0.97 && right > 0.97,
        "centre is unity: {left} {right}"
    );

    bus.execute(WriteTrackAutomation {
        track,
        param: MixerParam::Gain,
        points: vec![point(0, 0.0), point(1_920, 0.0), point(1_920, -12.0)],
    })
    .unwrap();
    let out = render(&bus);
    let mid = |at: f64| rms_window_seconds(&out.mono(), RATE, at, 0.05);
    let drop = 20.0 * (mid(1.5) / mid(0.5)).log10();
    assert!(
        (drop + 12.0).abs() < 0.6,
        "pan aside, the fader fell 12 dB: {drop}"
    );
}

/// The level of `signal` in 10 ms windows, as a 100 Hz signal.
fn level_curve(signal: &[f32]) -> Vec<f32> {
    signal.chunks_exact(480).map(rms).collect()
}

fn curve_rate() -> SampleRate {
    SampleRate::new(100).unwrap()
}

fn lfo(bus: &mut CommandBus, device: DeviceId, shape: LfoShape, rate: LfoRate, depth: f32) {
    let modulator = bus
        .execute(AddModulator {
            name: "LFO".to_owned(),
            source: ModSource::Lfo {
                shape,
                rate,
                phase: 0.0,
            },
        })
        .unwrap()
        .modulator()
        .unwrap();
    bus.execute(SetModulation {
        modulator,
        device,
        param: Gain::GAIN,
        depth,
    })
    .unwrap();
}

#[test]
fn an_lfo_modulates_a_gain_at_its_rate_and_depth() {
    // Gain centred on 0.5, swung by 0.1 of its 0..4 range: 0.1 to 0.9.
    let (mut bus, _, _, gain) = gained(16, 0.5);
    lfo(&mut bus, gain, LfoShape::Sine, LfoRate::Hertz(3.0), 0.1);
    let out = render(&bus);
    let curve = level_curve(&out.left);
    let rate = dominant_frequency(&curve, curve_rate());
    assert!((rate - 3.0).abs() < 0.15, "tremolo rate: {rate} Hz");
    let (low, high) = curve[10..]
        .iter()
        .fold((f32::MAX, 0.0f32), |(low, high), level| {
            (low.min(*level), high.max(*level))
        });
    assert!(
        (high / FULL - 0.9).abs() < 0.03,
        "peak gain: {}",
        high / FULL
    );
    assert!(
        (low / FULL - 0.1).abs() < 0.03,
        "trough gain: {}",
        low / FULL
    );

    // A tempo-synced LFO: one cycle per beat is 2 Hz at 120 BPM.
    let (mut bus, _, _, gain) = gained(16, 0.5);
    lfo(&mut bus, gain, LfoShape::Triangle, LfoRate::Beats(1.0), 0.1);
    let curve = level_curve(&render(&bus).left);
    let rate = dominant_frequency(&curve, curve_rate());
    assert!((rate - 2.0).abs() < 0.15, "synced rate: {rate} Hz");

    // A square LFO at 1 Hz flips the gain between its two ends.
    let (mut bus, _, _, gain) = gained(8, 0.5);
    lfo(&mut bus, gain, LfoShape::Square, LfoRate::Hertz(1.0), 0.1);
    let out = render(&bus);
    let at = |seconds: f64| rms_window_seconds(&out.left, RATE, seconds, 0.2) / FULL;
    assert!((at(0.2) - 0.9).abs() < 0.02 && (at(0.7) - 0.1).abs() < 0.02);
    assert!((at(1.2) - 0.9).abs() < 0.02 && (at(1.7) - 0.1).abs() < 0.02);
}

#[test]
fn an_envelope_follows_the_notes_of_its_trigger_track() {
    // The pad's gain is zero until a kick note opens the envelope.
    let (mut bus, pad, _, gain) = gained(4, 0.0);
    let kicks = (0..4).map(|beat| note(beat * 960, 240, 36)).collect();
    let (kick, _) = tone_track(&mut bus, "Kick", 4, kicks);
    let modulator = bus
        .execute(AddModulator {
            name: "Env".to_owned(),
            source: ModSource::Envelope {
                trigger: kick,
                attack_ms: 0.0,
                decay_ms: 0.0,
                sustain: 1.0,
                release_ms: 50.0,
            },
        })
        .unwrap()
        .modulator()
        .unwrap();
    bus.execute(SetModulation {
        modulator,
        device: gain,
        param: Gain::GAIN,
        depth: 0.25,
    })
    .unwrap();
    let pad = stem(&stems(&bus), pad);
    for beat in 0..4 {
        // A kick note lasts 125 ms; the release is over 50 ms later.
        let open = rms_window(&pad.left, beat * BEAT + 1_200, 4_000) / FULL;
        let shut = rms_window(&pad.left, beat * BEAT + 12_000, 10_000) / FULL;
        assert!((open - 1.0).abs() < 0.02, "beat {beat} open: {open}");
        assert!(shut < 1e-3, "beat {beat} shut: {shut}");
    }
}

#[test]
fn a_macro_drives_several_parameters_at_once() {
    let (mut bus, _, tone, gain) = gained(2, 1.0);
    let control = bus
        .execute(AddMacro {
            name: "Intensity".to_owned(),
            value: 0.0,
        })
        .unwrap()
        .macro_id()
        .unwrap();
    // Up goes the tone's level (0.1 to 0.5), down goes the gain (1 to 0.5).
    bus.execute(SetMacroTarget {
        macro_id: control,
        device: tone,
        param: TONE_LEVEL,
        min: 0.1,
        max: 0.5,
    })
    .unwrap();
    bus.execute(SetMacroTarget {
        macro_id: control,
        device: gain,
        param: Gain::GAIN,
        min: 1.0,
        max: 0.5,
    })
    .unwrap();
    let low = render(&bus);
    let amplitude = |out: &StereoBuffer| rms(&out.left) * std::f32::consts::SQRT_2;
    assert!((amplitude(&low) - 0.1).abs() < 1e-3, "{}", amplitude(&low));

    // Live: the command's messages move both parameters of a running graph.
    let graph_before = compile(&bus).graph;
    let command = Command::from(SetMacro {
        macro_id: control,
        value: 1.0,
    });
    bus.execute(command.clone()).unwrap();
    let messages = command.audio_messages(bus.project());
    assert_eq!(messages.len(), 2);
    let mut live = graph_before;
    for message in messages {
        let AudioMessage::SetParam {
            device,
            param,
            value,
        } = message
        else {
            panic!("a macro only sets parameters");
        };
        assert!(live.set_param(device, param, value));
    }
    let live = render_graph(live, bus.project().tempo, 1.0).unwrap();

    // Compiled afresh, the project sounds the same.
    let high = render(&bus);
    assert!(
        (amplitude(&high) - 0.25).abs() < 1e-3,
        "{}",
        amplitude(&high)
    );
    assert_null(&live.left, &high.left, -100.0);

    bus.undo().unwrap();
    assert_null(&render(&bus).left, &low.left, -120.0);
}

#[test]
fn lanes_and_modulation_aimed_at_missing_devices_are_reported() {
    let (mut bus, _, _, gain) = gained(2, 1.0);
    lfo(&mut bus, gain, LfoShape::Sine, LfoRate::Hertz(2.0), 0.1);
    bus.execute(WriteAutomation {
        device: gain,
        param: Gain::GAIN,
        points: vec![point(0, 1.0)],
    })
    .unwrap();
    assert!(compile(&bus).graph.has_control());
    bus.execute(fourfloor_model::command::SetDeviceBypass {
        device: gain,
        bypassed: true,
    })
    .unwrap();
    let compiled = compile(&bus);
    assert!(!compiled.graph.has_control());
    assert_eq!(compiled.warnings.len(), 2, "{:?}", compiled.warnings);
}

#[test]
fn a_learned_controller_moves_its_parameter_through_the_router() {
    use fourfloor_compile::{learned_command, midi_router};
    use fourfloor_engine::midi::{parse, MidiLearn};

    let (mut bus, track, _, gain) = gained(2, 1.0);
    // Learn: arm the gain, wiggle controller 74 on channel 2.
    let mut learn = MidiLearn::new(false);
    learn.arm(gain, Gain::GAIN, 0.0, 2.0);
    let binding = learn.feed(&parse(&[0xB2, 74, 9]).unwrap()).unwrap();
    bus.execute(learned_command(&binding)).unwrap();
    assert_eq!(bus.project().controller_map.len(), 1);

    let compiled = compile(&bus);
    let instrument = compiled.track(track).unwrap().instrument;
    let router = midi_router(bus.project(), instrument);
    // The controller at half way sets the gain to about 1.0; at a quarter, 0.5.
    let mut graph = compiled.graph;
    let message = router.route_bytes(&[0xB0, 74, 32]).unwrap();
    let AudioMessage::SetParam {
        device,
        param,
        value,
    } = message
    else {
        panic!("a mapped controller sets a parameter");
    };
    assert_eq!((device, param), (gain, Gain::GAIN));
    assert!(graph.set_param(device, param, value));
    let out = render_graph(graph, bus.project().tempo, 1.0).unwrap();
    let level = rms(&out.left) / FULL;
    assert!((level - 2.0 * 32.0 / 127.0).abs() < 0.01, "{level}");
    // Notes go to the instrument.
    assert!(matches!(
        router.route_bytes(&[0x90, 60, 100]),
        Some(AudioMessage::SendEvent { device, .. }) if Some(device) == instrument
    ));

    bus.undo().unwrap();
    assert!(midi_router(bus.project(), instrument)
        .route_bytes(&[0xB0, 74, 32])
        .is_some());
    assert!(fourfloor_compile::controller_map(bus.project())
        .bindings()
        .is_empty());
}
