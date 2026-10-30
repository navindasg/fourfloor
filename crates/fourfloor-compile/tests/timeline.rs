//! The timeline, heard: audio clips, arrangement edits, the clip launcher,
//! the step sequencer with swing and grooves, and note effects.

mod common;

use std::path::Path;

use common::*;
use fourfloor_audio::{encode_wav, WavSpec};
use fourfloor_compile::{render_project, CompileOptions, RenderOptions};
use fourfloor_core::{pitch_to_hz, ClipId, SampleRate, Ticks, TrackId};
use fourfloor_engine::devices::{ArpMode, Arpeggiator, KIND_ARP, KIND_CHORD};
use fourfloor_model::command::{
    AddAudioClip, AddScene, ApplyGroove, DuplicateClip, GrooveSource, MoveClip, ResizeClip,
    SetClipFades, SetClipGain, SetClipLoop, SetClipWarp, SetLaunchQuantize, SetSwing, SetTempo,
    SplitClip, WriteMidiClip, WriteMidiSlot, WriteStepPattern,
};
use fourfloor_model::sequencer::StepLane;
use fourfloor_model::{
    AudioPool, AudioRef, CommandBus, Groove, GrooveStep, GrooveTemplate, SwingGrid, TrackKind, Warp,
};
use fourfloor_testkit::{
    assert_null, dominant_frequency, is_silent, onset_times, rms_window_seconds,
    spectrum_band_energy, times_match, AllocDisabler, StereoBuffer,
};

// Every render in this file runs under the engine's allocation guard.
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const FULL: f32 = 0.25 * std::f32::consts::FRAC_1_SQRT_2;

/// Puts a mono-in-stereo WAV made of sine segments `(hz, seconds)` at
/// amplitude 0.5 into the project's audio pool.
fn pool_file(dir: &Path, rate: SampleRate, segments: &[(f64, f64)]) -> AudioRef {
    let mut interleaved = Vec::new();
    for &(hz, seconds) in segments {
        let frames = (seconds * rate.as_f64()).round() as usize;
        for n in 0..frames {
            let sample =
                (0.5 * (std::f64::consts::TAU * hz * n as f64 / rate.as_f64()).sin()) as f32;
            interleaved.extend([sample, sample]);
        }
    }
    let bytes = encode_wav(WavSpec::float_stereo(rate), &interleaved).unwrap();
    AudioPool::new(dir).import_bytes(&bytes, "wav").unwrap()
}

fn audio_clip(
    bus: &mut CommandBus,
    source: AudioRef,
    start: i64,
    length: i64,
    skip: u64,
) -> ClipId {
    let track = add_track(bus, "Loop", TrackKind::Audio);
    bus.execute(AddAudioClip {
        track,
        name: "Loop".to_owned(),
        start: Ticks::new(start),
        length: Ticks::new(length),
        source,
        source_offset_frames: skip,
    })
    .unwrap()
    .clip()
    .unwrap()
}

fn in_folder(dir: &Path) -> RenderOptions {
    RenderOptions {
        compile: CompileOptions::new(RATE, 512).with_project_dir(dir),
        ..options()
    }
    .with_tail(0.25)
}

#[test]
fn an_audio_clip_plays_at_its_position_with_gain_and_fades() {
    let dir = tempfile::tempdir().unwrap();
    // A 44.1 kHz file in a 48 kHz render: half a second of A4, then A5.
    let source = pool_file(
        dir.path(),
        SampleRate::HZ_44100,
        &[(440.0, 0.5), (880.0, 0.5)],
    );
    let mut bus = new_bus();
    let clip = audio_clip(&mut bus, source.clone(), 1_920, 960, 0);
    let out = render_with(&bus, &in_folder(dir.path()));
    assert_eq!(out.frames(), 72_000 + 12_000, "clip end plus the tail");
    assert!(
        is_silent(&out.left[..47_900], -100.0),
        "silent before beat 2"
    );
    assert!(times_match(&onset_times(&out.left, RATE), &[1.0], 0.002));
    let body = out.slice_seconds(1.0, 1.5);
    assert!((dominant_frequency(&body.left, RATE) - 440.0).abs() < 3.0);
    assert!(
        (rms_window_seconds(&out.left, RATE, 1.05, 0.4) - 0.5 * std::f32::consts::FRAC_1_SQRT_2)
            .abs()
            < 0.01
    );
    assert_eq!(out.left, out.right);
    assert!(
        is_silent(&out.left[72_100..], -100.0),
        "silent after the clip"
    );

    // Gain and fades.
    bus.execute(SetClipGain {
        clip,
        gain_db: -6.0,
    })
    .unwrap();
    bus.execute(SetClipFades {
        clip,
        fade_in: Ticks::new(240),
        fade_out: Ticks::new(240),
    })
    .unwrap();
    let shaped = render_with(&bus, &in_folder(dir.path()));
    let level = |at: f64| rms_window_seconds(&shaped.left, RATE, at, 0.01);
    let plateau = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * 0.5012;
    assert!((level(1.2) - plateau).abs() < 0.01, "{}", level(1.2));
    assert!(level(1.005) < 0.15 * plateau, "start of the fade-in");
    assert!((level(1.0575) - 0.5 * plateau).abs() < 0.03, "half way up");
    assert!(
        (level(1.4325) - 0.5 * plateau).abs() < 0.03,
        "half way down"
    );
    assert!(level(1.485) < 0.15 * plateau, "end of the fade-out");

    // The source offset counts frames of the file, whatever its rate.
    let mut bus = new_bus();
    audio_clip(&mut bus, source, 0, 960, 22_050);
    let late = render_with(&bus, &in_folder(dir.path()));
    let pitch = dominant_frequency(&late.slice_seconds(0.0, 0.5).left, RATE);
    assert!((pitch - 880.0).abs() < 3.0, "{pitch}");

    // Without the project folder the clip is left out, with a warning.
    let rendered = render_project(bus.project(), &registry(), &options()).unwrap();
    assert_eq!(rendered.warnings.len(), 1, "{:?}", rendered.warnings);
    assert!(rendered.warnings[0].contains("project folder"));
    assert!(is_silent(&rendered.samples, -200.0));
}

/// A 2.4 s A4 file (four beats at 100 BPM) in an eight-beat clip of a
/// 125 BPM project. Returns where the sound ends and its pitch.
fn warped(warp: Option<Warp>) -> (f64, f32, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let source = pool_file(dir.path(), RATE, &[(440.0, 2.4)]);
    let mut bus = new_bus();
    bus.execute(SetTempo { bpm: 125.0 }).unwrap();
    let clip = audio_clip(&mut bus, source, 0, 8 * 960, 0);
    bus.execute(SetClipWarp { clip, warp }).unwrap_or_else(|_| {
        assert!(warp.is_none(), "clearing an unset warp changes nothing");
        fourfloor_model::Outcome::Done
    });
    let rendered = render_project(bus.project(), &registry(), &in_folder(dir.path())).unwrap();
    let out = StereoBuffer::from_interleaved(RATE, &rendered.samples);
    let last = out
        .left
        .iter()
        .rposition(|sample| sample.abs() > 0.05)
        .expect("the clip sounds");
    let pitch = dominant_frequency(&out.slice_seconds(0.3, 1.5).left, RATE);
    (last as f64 / RATE.as_f64(), pitch, rendered.warnings)
}

#[test]
fn an_unwarped_clip_keeps_its_speed_and_a_varispeed_warp_follows_the_tempo() {
    let (end, pitch, warnings) = warped(None);
    assert!(
        (end - 2.4).abs() < 0.01 && (pitch - 440.0).abs() < 2.0,
        "{end} {pitch}"
    );
    assert!(warnings.is_empty());

    // 100 BPM material at 125 BPM: 0.8 times as long, a major third up.
    let (end, pitch, warnings) = warped(Some(Warp {
        source_bpm: 100.0,
        key_lock: false,
    }));
    assert!((end - 1.92).abs() < 0.01, "four beats at 125 BPM: {end}");
    assert!((pitch - 550.0).abs() < 2.0, "{pitch}");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[cfg(feature = "stretch")]
#[test]
fn a_key_locked_warp_changes_the_length_and_keeps_the_pitch() {
    let (end, pitch, warnings) = warped(Some(Warp {
        source_bpm: 100.0,
        key_lock: true,
    }));
    assert!((end - 1.92).abs() < 0.03, "four beats at 125 BPM: {end}");
    assert!((pitch - 440.0).abs() < 3.0, "the pitch stays: {pitch}");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[cfg(not(feature = "stretch"))]
#[test]
fn without_the_stretch_feature_key_lock_falls_back_to_varispeed_and_says_so() {
    let (end, pitch, warnings) = warped(Some(Warp {
        source_bpm: 100.0,
        key_lock: true,
    }));
    assert!(
        (end - 1.92).abs() < 0.01 && (pitch - 550.0).abs() < 2.0,
        "{end} {pitch}"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("key lock"));
}

/// One bar of four short notes, each a different pitch.
fn riff(bus: &mut CommandBus) -> (TrackId, ClipId) {
    let notes = [69, 72, 76, 81]
        .into_iter()
        .enumerate()
        .map(|(beat, pitch)| note(beat as i64 * 960, 240, pitch))
        .collect();
    let (track, _) = tone_track(bus, "Riff", 4, notes);
    let clip = bus.project().track(track).unwrap().clips[0].id;
    (track, clip)
}

#[test]
fn split_move_duplicate_and_loop_change_what_plays() {
    let mut bus = new_bus();
    let (_, clip) = riff(&mut bus);
    let reference = render(&bus);
    let beats = [0.0, 0.5, 1.0, 1.5];
    assert!(times_match(
        &onset_times(&reference.left, RATE),
        &beats,
        0.002
    ));

    // Splitting changes nothing you can hear.
    let right = bus
        .execute(SplitClip {
            clip,
            at: Ticks::new(1_920),
        })
        .unwrap()
        .clip()
        .unwrap();
    assert_null(&render(&bus).left, &reference.left, -120.0);

    // Moving the right half a bar later leaves a two-beat gap.
    bus.execute(MoveClip {
        clip: right,
        start: Ticks::new(1_920 + 3_840),
        track: None,
    })
    .unwrap();
    let moved = render(&bus);
    assert!(times_match(
        &onset_times(&moved.left, RATE),
        &[0.0, 0.5, 3.0, 3.5],
        0.002
    ));
    assert_null(&moved.left[144_000..], &reference.left[48_000..], -120.0);

    // A duplicate plays the same audio at its own position.
    bus.execute(DuplicateClip {
        clip,
        start: Some(Ticks::new(1_920)),
        track: None,
    })
    .unwrap();
    let doubled = render(&bus);
    assert_null(
        &doubled.left[48_000..96_000],
        &reference.left[..48_000],
        -120.0,
    );

    // Looping the first beat of a clip over two bars repeats its note.
    let mut bus = new_bus();
    let (_, clip) = riff(&mut bus);
    bus.execute(ResizeClip {
        clip,
        length: Ticks::new(2 * 3_840),
    })
    .unwrap();
    bus.execute(SetClipLoop {
        clip,
        loop_length: Some(Ticks::new(960)),
    })
    .unwrap();
    let looped = render(&bus);
    let every_beat: Vec<f64> = (0..8).map(|beat| beat as f64 * 0.5).collect();
    assert!(times_match(
        &onset_times(&looped.left, RATE),
        &every_beat,
        0.002
    ));
    let pitch = dominant_frequency(&looped.slice_seconds(3.5, 3.6).left, RATE);
    assert!(
        (pitch - 440.0).abs() < 12.0,
        "still the first note: {pitch}"
    );
}

fn hats(bus: &mut CommandBus, swing: Option<Groove>, groove: Option<GrooveSource>) -> ClipId {
    let track = match bus.project().tracks.first() {
        Some(track) => track.id,
        None => {
            let track = add_track(bus, "Hats", TrackKind::Midi);
            add_device(bus, track, KIND_TONE, Vec::new());
            track
        }
    };
    let clip = bus
        .project()
        .track(track)
        .unwrap()
        .clips
        .first()
        .map(|clip| clip.id);
    bus.execute(WriteStepPattern {
        track,
        clip,
        name: "Hats".to_owned(),
        start: Ticks::ZERO,
        bars: 1,
        lanes: vec![StepLane {
            pitch: 93,
            velocity: 100,
            note_length: Ticks::new(60),
            steps: vec![true; 16],
            velocities: Vec::new(),
        }],
        swing,
        groove,
        groove_amount: 1.0,
    })
    .unwrap()
    .clip()
    .unwrap()
}

/// Onset times for sixteenths whose off steps are `late` seconds late.
fn sixteenths(late: f64) -> Vec<f64> {
    (0..16)
        .map(|step| step as f64 * 0.125 + if step % 2 == 1 { late } else { 0.0 })
        .collect()
}

fn onsets(bus: &CommandBus) -> Vec<f64> {
    onset_times(&render(bus).left, RATE)
}

#[test]
fn swing_moves_the_off_sixteenths_by_the_right_amount() {
    let mut bus = new_bus();
    hats(&mut bus, None, None);
    assert!(
        times_match(&onsets(&bus), &sixteenths(0.0), 0.001),
        "straight"
    );

    // 62.5 % swing: the off step falls 62.5 % through its eighth-note
    // pair, 31.25 ms after the straight position at 120 BPM.
    let swing = Groove {
        swing_percent: 62.5,
        grid: SwingGrid::Sixteenth,
    };
    hats(&mut bus, Some(swing), None);
    let found = onsets(&bus);
    assert!(
        times_match(&found, &sixteenths(0.031_25), 0.001),
        "{found:?}"
    );

    // The project's swing setting is the default for new patterns.
    bus.execute(SetSwing {
        percent: 66.0,
        grid: None,
    })
    .unwrap();
    hats(&mut bus, None, None);
    let found = onsets(&bus);
    assert!(
        times_match(&found, &sixteenths(0.25 * 0.16), 0.001),
        "{found:?}"
    );
}

#[test]
fn a_groove_template_shifts_steps_by_its_per_step_offsets() {
    let template = GrooveTemplate {
        name: "Late twos".to_owned(),
        step_ticks: 240,
        steps: vec![
            GrooveStep::NEUTRAL,
            GrooveStep {
                timing: 0.2,
                velocity: 1.0,
            },
        ],
    };
    let straight = Groove::STRAIGHT;
    let mut bus = new_bus();
    let clip = hats(
        &mut bus,
        Some(straight),
        Some(GrooveSource::Custom(template.clone())),
    );
    // 0.2 of a sixteenth is 48 ticks: 25 ms at 120 BPM.
    let found = onsets(&bus);
    assert!(times_match(&found, &sixteenths(0.025), 0.001), "{found:?}");

    // Applying a groove to an existing straight clip does the same.
    hats(&mut bus, Some(straight), None);
    bus.execute(ApplyGroove {
        clip,
        groove: GrooveSource::Custom(template),
        amount: 0.5,
    })
    .unwrap();
    let found = onsets(&bus);
    assert!(
        times_match(&found, &sixteenths(0.0125), 0.001),
        "half strength: {found:?}"
    );
    bus.execute(ApplyGroove {
        clip,
        groove: GrooveSource::Builtin("swing_16_62".to_owned()),
        amount: 1.0,
    })
    .unwrap();
    assert!(
        onsets(&bus)[1] > 0.14,
        "a built-in groove moves the off step further"
    );
}

/// A C major triad held for a bar, through `kind` and then a tone.
fn chord_track(kind: &str, params: Vec<fourfloor_model::Param>, pitches: &[u8]) -> CommandBus {
    let mut bus = new_bus();
    let notes = pitches.iter().map(|pitch| note(0, 3_840, *pitch)).collect();
    let (track, _) = tone_track(&mut bus, "Keys", 4, notes);
    // Added after the instrument: its category still puts it before.
    add_device(&mut bus, track, kind, params);
    bus
}

fn step_pitches(out: &StereoBuffer, steps: usize) -> Vec<u8> {
    (0..steps)
        .map(|step| {
            let start = step * 6_000 + 300;
            let hz = dominant_frequency(&out.left[start..start + 2_400], RATE);
            (69.0 + 12.0 * (hz / 440.0).log2()).round() as u8
        })
        .collect()
}

#[test]
fn the_arpeggiator_plays_the_held_chord_one_note_per_step() {
    let bus = chord_track(KIND_ARP, Vec::new(), &[64, 60, 67]);
    let out = render(&bus);
    assert_eq!(
        step_pitches(&out, 16),
        [60, 64, 67, 60, 64, 67, 60, 64, 67, 60, 64, 67, 60, 64, 67, 60]
    );
    assert!(times_match(
        &onset_times(&out.left, RATE),
        &sixteenths(0.0),
        0.001
    ));
    for step in 0..16 {
        // Half gate: the second half of every step is silent.
        let gap = &out.left[step * 6_000 + 3_100..step * 6_000 + 5_900];
        assert!(is_silent(gap, -100.0), "step {step}");
    }
    let hz = dominant_frequency(&out.left[300..2_700], RATE);
    assert!((hz - pitch_to_hz(60.0)).abs() < 10.0, "{hz}");

    let down = vec![
        param(
            Arpeggiator::MODE,
            "Mode",
            ArpMode::Down.param_value(),
            0.0,
            3.0,
        ),
        param(Arpeggiator::RATE, "Rate", 0.5, 0.0625, 4.0),
    ];
    let out = render(&chord_track(KIND_ARP, down, &[64, 60, 67]));
    let eighths: Vec<u8> = step_pitches(&out, 16).into_iter().step_by(2).collect();
    assert_eq!(
        eighths,
        [67, 64, 60, 67, 64, 60, 67, 64],
        "downwards in eighths"
    );
}

#[test]
fn the_chord_device_adds_its_intervals_to_every_note() {
    let out = render(&chord_track(KIND_CHORD, Vec::new(), &[60]));
    let band = |pitch: f32| {
        let hz = pitch_to_hz(pitch);
        spectrum_band_energy(&out.left, RATE, hz - 4.0, hz + 4.0)
    };
    let (root, third, fifth) = (band(60.0), band(64.0), band(67.0));
    for (name, energy) in [("root", root), ("third", third), ("fifth", fifth)] {
        assert!(energy > 0.02, "{name}: {energy}");
    }
    assert!(band(62.0) < 1e-4 && band(65.5) < 1e-4, "nothing in between");
}

/// A track with an arrangement drone and two launcher scenes: A repeats
/// A5 every beat, B repeats A6 every half beat.
fn session() -> (CommandBus, [fourfloor_model::SceneId; 2]) {
    let mut bus = new_bus();
    let (track, _) = tone_track(&mut bus, "Lead", 16, vec![note(0, 16 * 960, 45)]);
    let scene = |bus: &mut CommandBus, name: &str, length: i64, pitch: u8| {
        let scene = bus
            .execute(AddScene {
                name: name.to_owned(),
                index: None,
            })
            .unwrap()
            .scene()
            .unwrap();
        bus.execute(WriteMidiSlot {
            track,
            scene,
            name: name.to_owned(),
            length: Ticks::new(length),
            notes: vec![note(0, 120, pitch)],
        })
        .unwrap();
        scene
    };
    let a = scene(&mut bus, "A", 960, 81);
    let b = scene(&mut bus, "B", 480, 93);
    (bus, [a, b])
}

/// Checks that a fresh tone voice at `hz` starts exactly on `frame`.
fn assert_starts_at(out: &StereoBuffer, frame: usize, hz: f32) {
    let expected: Vec<f32> = (0..2_000)
        .map(|n| 0.25 * (std::f32::consts::TAU * hz * n as f32 / 48_000.0).sin())
        .collect();
    assert_null(&out.left[frame..frame + 2_000], &expected, -60.0);
}

#[test]
fn a_scene_launch_lands_on_the_quantize_boundary() {
    let (mut bus, [a, b]) = session();
    let bar = 3_840;
    let launched = options()
        .with_length(Ticks::new(4 * bar))
        // Pressed a little after the start: waits for bar 2 (2.0 s).
        .with_launch(Ticks::new(1_000), Some(a))
        // Pressed just after bar 2 starts: waits for bar 3 (4.0 s).
        .with_launch(Ticks::new(bar + 1), Some(b))
        // Back to the arrangement at bar 4 (6.0 s).
        .with_launch(Ticks::new(2 * bar + 100), None);
    let out = render_with(&bus, &launched);
    assert_eq!(out.frames(), 384_000);

    // Until bar 2 the arrangement's drone plays; it stops on the boundary.
    let drone = dominant_frequency(&out.left[48_000..96_000], RATE);
    assert!((drone - 110.0).abs() < 1.0, "{drone}");
    assert_starts_at(&out, 96_000, 880.0);
    let scene_a = onset_times(&out.left[96_000..192_000], RATE);
    assert!(
        times_match(&scene_a, &[0.0, 0.5, 1.0, 1.5], 0.001),
        "{scene_a:?}"
    );
    assert_starts_at(&out, 192_000, 1_760.0);
    let scene_b = onset_times(&out.left[192_000..288_000], RATE);
    let halves: Vec<f64> = (0..8).map(|n| n as f64 * 0.25).collect();
    assert!(times_match(&scene_b, &halves, 0.001), "{scene_b:?}");
    // The drone began long ago and is not re-triggered.
    assert!(is_silent(&out.left[288_000 + 3_100..], -100.0));

    // With no launch the arrangement plays throughout.
    let plain = render_with(&bus, &options().with_length(Ticks::new(4 * bar)));
    let drone = dominant_frequency(&plain.left[200_000..300_000], RATE);
    assert!((drone - 110.0).abs() < 1.0, "{drone}");

    // A finer grid: the same press lands on the next beat (1.0 s).
    bus.execute(SetLaunchQuantize {
        ticks: Ticks::new(960),
    })
    .unwrap();
    let out = render_with(
        &bus,
        &options()
            .with_length(Ticks::new(bar))
            .with_launch(Ticks::new(1_000), Some(a)),
    );
    assert_starts_at(&out, 48_000, 880.0);
    let before = dominant_frequency(&out.left[24_000..48_000], RATE);
    assert!((before - 110.0).abs() < 2.0, "{before}");

    let compiled = compile(&bus);
    assert_eq!(compiled.scenes, [a, b]);
    assert_eq!(compiled.launch_messages(Some(b)).len(), 1);
    assert!(compiled
        .launch_messages(Some(fourfloor_model::SceneId::new(9_999)))
        .is_empty());
}

#[test]
fn a_looped_midi_clip_and_a_slot_only_track_compile() {
    // A track with launcher clips and no arrangement still gets a strip.
    let mut bus = new_bus();
    let track = add_track(&mut bus, "Lead", TrackKind::Midi);
    add_device(&mut bus, track, KIND_TONE, Vec::new());
    let scene = bus
        .execute(AddScene {
            name: "A".to_owned(),
            index: None,
        })
        .unwrap()
        .scene()
        .unwrap();
    bus.execute(WriteMidiSlot {
        track,
        scene,
        name: "A".to_owned(),
        length: Ticks::new(960),
        notes: vec![note(0, 120, 81)],
    })
    .unwrap();
    let compiled = compile(&bus);
    let strip = compiled.track(track).unwrap();
    assert!(strip.launcher.is_some() && strip.player.is_none());
    let out = render_with(
        &bus,
        &options()
            .with_length(Ticks::new(3_840))
            .with_launch(Ticks::ZERO, Some(scene)),
    );
    let found = onset_times(&out.left, RATE);
    assert!(
        times_match(&found, &[0.0, 0.5, 1.0, 1.5], 0.001),
        "{found:?}"
    );

    // An empty MIDI clip written over the riff silences it.
    let mut bus = new_bus();
    let (track, clip) = riff(&mut bus);
    bus.execute(WriteMidiClip {
        track,
        clip: Some(clip),
        name: "Empty".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::new(3_840),
        notes: Vec::new(),
    })
    .unwrap();
    assert!(is_silent(&render(&bus).left, -120.0));
    assert!((FULL - 0.1768).abs() < 1e-3);
}
