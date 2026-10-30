//! The mixer, heard: pan law, sends and returns, sidechain routing, delay
//! compensation across both, and stems that add up to the master.

mod common;

use common::*;
use fourfloor_compile::{normalize_loudness, render_stems, SampleFormat, CHANNELS};
use fourfloor_engine::devices::{Gain, KIND_GAIN};
use fourfloor_model::command::{
    RemoveSend, SetDeviceBypass, SetDeviceSidechain, SetSend, SetTrackGain, SetTrackMute,
    SetTrackOutput, SetTrackPan,
};
use fourfloor_model::{CommandBus, TrackKind};
use fourfloor_testkit::{
    assert_null, is_silent, lufs, null_residual_db, rms, rms_window, AllocDisabler, StereoBuffer,
};

// Every render in this file runs under the engine's allocation guard.
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

/// Four quarter-note kicks (short low notes) over one bar.
fn kick_track(bus: &mut CommandBus) -> fourfloor_core::TrackId {
    let notes = (0..4).map(|beat| note(beat * 960, 120, 36)).collect();
    tone_track(bus, "Kick", 4, notes).0
}

#[test]
fn pan_follows_the_balance_law() {
    let mut bus = new_bus();
    let (track, _) = held_track(&mut bus, "Lead", 2, 69);
    let centre = render(&bus);
    assert_eq!(centre.left, centre.right, "centre is unity on both sides");
    let unity = rms(&centre.left);
    assert!((unity - 0.25 / 2f32.sqrt()).abs() < 1e-3, "{unity}");

    let at = |bus: &mut CommandBus, pan: f32| {
        bus.execute(SetTrackPan { track, pan }).unwrap();
        let out = render(bus);
        (rms(&out.left) / unity, rms(&out.right) / unity)
    };
    let (left, right) = at(&mut bus, 0.5);
    assert!(
        (left - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
        "{left}"
    );
    assert!(
        (right - 1.0).abs() < 1e-4,
        "the near side stays at unity: {right}"
    );
    let (left, right) = at(&mut bus, 1.0);
    assert!(left < 1e-4 && (right - 1.0).abs() < 1e-4, "{left} {right}");
    let (left, right) = at(&mut bus, -1.0);
    assert!(right < 1e-4 && (left - 1.0).abs() < 1e-4, "{left} {right}");
    let (left, right) = at(&mut bus, -0.25);
    let expected = (0.25 * std::f32::consts::FRAC_PI_2).cos();
    assert!((left - 1.0).abs() < 1e-4 && (right - expected).abs() < 1e-3);
}

#[test]
fn a_send_reaches_its_return_at_the_send_level() {
    let mut bus = new_bus();
    let (track, _) = held_track(&mut bus, "Lead", 2, 69);
    let dry = render(&bus);
    let ret = add_track(&mut bus, "Return", TrackKind::Bus);
    let half = fourfloor_compile::db_to_linear(-6.0);

    // Post-fader: the return adds a copy at the send level.
    let send = SetSend {
        track,
        target: ret,
        gain_db: -6.0,
        pre_fader: false,
    };
    bus.execute(send).unwrap();
    let compiled = compile(&bus);
    let strip = compiled.track(track).unwrap();
    assert_eq!(strip.sends.len(), 1);
    assert_eq!(strip.sends[0].0, ret);
    assert!(compiled.track(ret).unwrap().to_master);
    let wet = render(&bus);
    assert_null(&wet.left, &scaled(&dry.left, 1.0 + half), -100.0);

    // The return's own fader scales only the sent copy.
    bus.execute(SetTrackGain {
        track: ret,
        gain_db: -6.0,
    })
    .unwrap();
    let wet = render(&bus);
    assert_null(&wet.left, &scaled(&dry.left, 1.0 + half * half), -100.0);
    bus.undo().unwrap();

    // Post-fader follows the track fader; pre-fader does not.
    bus.execute(SetTrackMute { track, mute: true }).unwrap();
    assert!(
        is_silent(&render(&bus).left, -120.0),
        "a muted track sends nothing post-fader"
    );
    bus.execute(SetSend {
        pre_fader: true,
        ..send
    })
    .unwrap();
    let pre = render(&bus);
    assert_null(&pre.left, &scaled(&dry.left, half), -100.0);
    assert_null(&pre.right, &scaled(&dry.right, half), -100.0);

    // A live level change goes to the send's gain node.
    let compiled = compile(&bus);
    let node = compiled.track(track).unwrap().sends[0].1;
    let mut graph = compiled.graph;
    assert!(graph.set_param(node, Gain::GAIN, 1.0));
    let out = fourfloor_testkit::render_graph(graph, bus.project().tempo, 1.0).unwrap();
    assert_null(&out.left, &dry.left, -100.0);

    bus.execute(RemoveSend { track, target: ret }).unwrap();
    assert!(is_silent(&render(&bus).left, -120.0));
}

/// Kick plus a pad whose ducker is keyed by the kick. Returns the pad
/// track and the ducker.
fn ducked() -> (
    CommandBus,
    fourfloor_core::TrackId,
    fourfloor_core::TrackId,
    fourfloor_core::DeviceId,
) {
    let mut bus = new_bus();
    let kick = kick_track(&mut bus);
    let (pad, _) = held_track(&mut bus, "Pad", 4, 69);
    let ducker = add_device(&mut bus, pad, KIND_DUCKER, Vec::new());
    bus.execute(SetDeviceSidechain {
        device: ducker,
        source: Some(kick),
        pre_fader: false,
    })
    .unwrap();
    (bus, kick, pad, ducker)
}

/// RMS of the pad stem just after each kick and between kicks.
fn pad_levels(bus: &CommandBus, pad: fourfloor_core::TrackId) -> (f32, f32) {
    let pad = stem(&stems(bus), pad);
    let window = |offset_ms: usize, length_ms: usize| {
        let levels: Vec<f32> = (0..4)
            .map(|beat| rms_window(&pad.left, beat * BEAT + offset_ms * 48, length_ms * 48))
            .collect();
        levels.iter().sum::<f32>() / 4.0
    };
    (window(10, 40), window(300, 100))
}

#[test]
fn a_sidechain_ducks_the_pad_when_the_kick_hits() {
    let (mut bus, kick, pad, ducker) = ducked();
    let full = 0.25 / 2f32.sqrt();
    let (on_kick, between) = pad_levels(&bus, pad);
    assert!(
        (between - full).abs() < 0.01,
        "free between kicks: {between}"
    );
    assert!(
        on_kick < 0.35 * between,
        "ducked on the kick: {on_kick} vs {between}"
    );

    // Without the routing the pad is steady.
    bus.execute(SetDeviceSidechain {
        device: ducker,
        source: None,
        pre_fader: false,
    })
    .unwrap();
    let (on_kick, between) = pad_levels(&bus, pad);
    assert!((on_kick - between).abs() < 0.01, "{on_kick} vs {between}");
    bus.undo().unwrap();

    // A muted kick keys nothing after its fader, but still keys before it.
    bus.execute(SetTrackMute {
        track: kick,
        mute: true,
    })
    .unwrap();
    let (on_kick, between) = pad_levels(&bus, pad);
    assert!((on_kick - between).abs() < 0.01, "post-fader key is silent");
    bus.execute(SetDeviceSidechain {
        device: ducker,
        source: Some(kick),
        pre_fader: true,
    })
    .unwrap();
    let (on_kick, between) = pad_levels(&bus, pad);
    assert!(
        on_kick < 0.35 * between,
        "pre-fader key: {on_kick} vs {between}"
    );
    assert!(
        is_silent(&stem(&stems(&bus), kick).left, -120.0),
        "the kick itself is muted"
    );
}

#[test]
fn sidechain_routing_that_cannot_work_is_reported() {
    let (mut bus, kick, pad, ducker) = ducked();
    // The model refuses a device keyed by its own track.
    let own = bus.execute(SetDeviceSidechain {
        device: ducker,
        source: Some(pad),
        pre_fader: false,
    });
    assert!(own.is_err());
    // A device with no sidechain input ignores the routing, with a warning.
    let gain = add_device(&mut bus, pad, KIND_GAIN, Vec::new());
    bus.execute(SetDeviceSidechain {
        device: gain,
        source: Some(kick),
        pre_fader: false,
    })
    .unwrap();
    let warnings = compile(&bus).warnings;
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("no sidechain input"));
}

/// Checks that `late` is `reference` delayed by exactly [`LATENCY`] frames.
fn assert_shifted(late: &StereoBuffer, reference: &StereoBuffer) {
    for (late, reference) in late.channels().into_iter().zip(reference.channels()) {
        assert!(
            is_silent(&late[..LATENCY], -120.0),
            "nothing before the latency"
        );
        let frames = reference.len() - LATENCY;
        assert_null(&late[LATENCY..], &reference[..frames], -100.0);
    }
}

#[test]
fn delay_compensation_keeps_sends_aligned() {
    let mut bus = new_bus();
    let (lead, _) = held_track(&mut bus, "Lead", 2, 69);
    let (other, _) = tone_track(&mut bus, "Other", 2, vec![note(480, 960, 76)]);
    let ret = add_track(&mut bus, "Return", TrackKind::Bus);
    add_device(
        &mut bus,
        ret,
        KIND_GAIN,
        vec![param(Gain::GAIN, "Gain", 0.5, 0.0, 4.0)],
    );
    for (track, pre_fader) in [(lead, false), (other, true)] {
        bus.execute(SetSend {
            track,
            target: ret,
            gain_db: -3.0,
            pre_fader,
        })
        .unwrap();
    }
    let latent = add_device(&mut bus, other, KIND_LATENT, Vec::new());
    let compiled = compile(&bus);
    assert_eq!(compiled.graph.latency_samples() as usize, LATENCY);
    let late = render(&bus);

    bus.execute(SetDeviceBypass {
        device: latent,
        bypassed: true,
    })
    .unwrap();
    assert_eq!(compile(&bus).graph.latency_samples(), 0);
    let reference = render(&bus);
    // Without compensation the lead and its send would arrive 480 frames
    // before the other track and the sum would not be a plain shift.
    assert_shifted(&late, &reference);
}

#[test]
fn delay_compensation_keeps_a_sidechain_aligned_with_what_it_ducks() {
    let (mut bus, kick, _pad, _) = ducked();
    let latent = add_device(&mut bus, kick, KIND_LATENT, Vec::new());
    let late = render(&bus);
    bus.execute(SetDeviceBypass {
        device: latent,
        bypassed: true,
    })
    .unwrap();
    let reference = render(&bus);
    assert!(
        null_residual_db(&late.left, &reference.left) > -40.0,
        "the render did move"
    );
    // The key arrives 480 frames late, so the pad is held back to meet it.
    assert_shifted(&late, &reference);
}

/// Kick and hat on a drum bus, a ducked pad with a send to a return, and
/// a latent effect on the hat.
fn full_mix() -> (CommandBus, [fourfloor_core::TrackId; 5]) {
    let (mut bus, kick, pad, _) = ducked();
    let hats = (0..8).map(|step| note(step * 480 + 240, 100, 90)).collect();
    let (hat, _) = tone_track(&mut bus, "Hat", 4, hats);
    bus.execute(SetTrackPan {
        track: hat,
        pan: 0.4,
    })
    .unwrap();
    add_device(&mut bus, hat, KIND_LATENT, Vec::new());
    let drums = add_track(&mut bus, "Drums", TrackKind::Bus);
    for track in [kick, hat] {
        bus.execute(SetTrackOutput {
            track,
            output: Some(drums),
        })
        .unwrap();
    }
    bus.execute(SetTrackGain {
        track: drums,
        gain_db: -3.0,
    })
    .unwrap();
    let ret = add_track(&mut bus, "Space / FX", TrackKind::Bus);
    add_device(
        &mut bus,
        ret,
        KIND_GAIN,
        vec![param(Gain::GAIN, "Gain", 0.5, 0.0, 4.0)],
    );
    bus.execute(SetSend {
        track: pad,
        target: ret,
        gain_db: -6.0,
        pre_fader: false,
    })
    .unwrap();
    (bus, [kick, hat, pad, drums, ret])
}

fn stem_sum(stems: &fourfloor_compile::RenderedStems) -> Vec<f32> {
    let mut sum = vec![0.0f32; stems.master.len()];
    for stem in &stems.stems {
        assert_eq!(stem.samples.len(), sum.len());
        sum = added(&sum, &stem.samples);
    }
    sum
}

#[test]
fn stems_sum_to_the_master() {
    let (bus, [_, _, pad, drums, ret]) = full_mix();
    let mut stems = stems(&bus);
    assert_eq!(stems.channels, CHANNELS);
    let tracks: Vec<_> = stems.stems.iter().map(|stem| stem.track).collect();
    assert_eq!(
        tracks,
        [pad, drums, ret],
        "one stem per strip that feeds the master"
    );
    assert_eq!(stems.stems[1].name, "Drums");
    for stem in &stems.stems {
        assert!(rms(&stem.samples) > 1e-3, "{} carries sound", stem.name);
    }
    assert!(null_residual_db(&stem_sum(&stems), &stems.master) < -120.0);

    // The master is the plain render, and stems survive normalisation.
    let plain = render(&bus);
    assert_eq!(plain.interleaved(), stems.master);
    let before = lufs(&plain);
    let normalized = stems.normalize(-20.0, -1.0).unwrap();
    assert!((normalized.measured_lufs - before).abs() < 0.01);
    assert!(!normalized.peak_limited);
    let after = lufs(&StereoBuffer::from_interleaved(RATE, &stems.master));
    assert!(
        (after + 20.0).abs() < 0.1,
        "normalised to -20 LUFS: {after}"
    );
    assert!(null_residual_db(&stem_sum(&stems), &stems.master) < -120.0);
}

#[test]
fn normalisation_respects_the_true_peak_ceiling() {
    let (bus, _) = full_mix();
    let rendered = render(&bus);
    let mut samples = rendered.interleaved();
    // Asking for a level the peaks cannot reach stops at the ceiling.
    let result = normalize_loudness(&mut samples, 2, RATE, -3.0, -1.0).unwrap();
    assert!(result.peak_limited);
    let out = StereoBuffer::from_interleaved(RATE, &samples);
    let peak = fourfloor_testkit::true_peak_dbtp(&out);
    assert!(
        (peak + 1.0).abs() < 0.05,
        "true peak sits on the ceiling: {peak}"
    );
    assert!(lufs(&out) < -3.0);
}

#[test]
fn stems_are_written_as_wav_files_that_sum_to_the_master_file() {
    let (bus, _) = full_mix();
    let stems = render_stems(bus.project(), &registry(), &options()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("export");
    let paths = stems.write_wav(&out, SampleFormat::Float32).unwrap();
    let names: Vec<String> = paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "master.wav",
            "01-Pad.wav",
            "02-Drums.wav",
            "03-Space___FX.wav"
        ]
    );
    let master = StereoBuffer::read_wav(&paths[0]).unwrap();
    assert_eq!(master.interleaved(), stems.master, "float WAV is exact");
    let mut sum = vec![0.0f32; master.frames()];
    for path in &paths[1..] {
        sum = added(&sum, &StereoBuffer::read_wav(path).unwrap().left);
    }
    assert!(null_residual_db(&sum, &master.left) < -120.0);
}
