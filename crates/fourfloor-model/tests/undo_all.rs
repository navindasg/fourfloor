//! One session that runs every command the model has and checks, for each,
//! that undo restores the exact project before it and redo the exact
//! project after it. The test fails if a command is ever added without
//! being added here.

use std::collections::BTreeSet;

use fourfloor_model::command::*;
use fourfloor_model::core::{ParamId, Ticks};
use fourfloor_model::sequencer::StepLane;
use fourfloor_model::{
    AudioRef, AutomationPoint, ChordKind, Command, CommandBus, ContentHash, LfoRate, LfoShape,
    MidiNote, MixerParam, ModSource, Outcome, Param, Project, Scale, TrackKind, Warp,
};

const CUTOFF: ParamId = ParamId::new(1);

struct Session {
    bus: CommandBus,
    seen: BTreeSet<&'static str>,
}

impl Session {
    /// Runs a command; checks it changed the project, that the result is
    /// valid, and that undo and redo restore the exact values.
    fn run(&mut self, command: impl Into<Command>) -> Outcome {
        let command = command.into();
        let name = command.name();
        let before = self.bus.project().clone();
        let outcome = self
            .bus
            .execute(command)
            .unwrap_or_else(|error| panic!("{name} failed: {error}"));
        let after = self.bus.project().clone();
        assert!(before != after, "{name} should change the project");
        assert_eq!(after.validate(), Ok(()), "{name} left an invalid project");
        self.bus.undo().unwrap();
        assert!(
            self.bus.project() == &before,
            "undo of {name} did not restore the project"
        );
        self.bus.redo().unwrap();
        assert!(
            self.bus.project() == &after,
            "redo of {name} did not restore the project"
        );
        self.seen.insert(name);
        outcome
    }
}

fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch,
        velocity: 100,
    }
}

fn point(time: i64, value: f32) -> AutomationPoint {
    AutomationPoint {
        time: Ticks::new(time),
        value,
    }
}

#[test]
fn every_command_can_be_undone_and_redone() {
    let mut s = Session {
        bus: CommandBus::new(Project::new("Untitled")),
        seen: BTreeSet::new(),
    };
    let track = |s: &mut Session, name: &str, kind: TrackKind| {
        s.run(CreateTrack {
            name: name.to_owned(),
            kind,
            index: None,
        })
        .track()
        .unwrap()
    };

    // Tracks and the mixer.
    let midi = track(&mut s, "Bass", TrackKind::Midi);
    let audio = track(&mut s, "Loop", TrackKind::Audio);
    let group = track(&mut s, "Group", TrackKind::Bus);
    let ret = track(&mut s, "Return", TrackKind::Bus);
    let spare = track(&mut s, "Spare", TrackKind::Midi);
    s.run(RenameTrack {
        track: midi,
        name: "Sub".to_owned(),
    });
    s.run(SetTrackGain {
        track: midi,
        gain_db: -6.0,
    });
    s.run(SetTrackPan {
        track: midi,
        pan: 0.25,
    });
    s.run(SetTrackMute {
        track: audio,
        mute: true,
    });
    s.run(SetTrackSolo {
        track: midi,
        solo: true,
    });
    s.run(SetTrackOutput {
        track: midi,
        output: Some(group),
    });
    s.run(SetSend {
        track: midi,
        target: ret,
        gain_db: -9.0,
        pre_fader: true,
    });
    s.run(WriteTrackAutomation {
        track: midi,
        param: MixerParam::Gain,
        points: vec![point(0, -12.0), point(960, 0.0)],
    });

    // Devices.
    let filter = s
        .run(AddDevice {
            track: midi,
            kind: "fx.filter".to_owned(),
            name: Some("Filter".to_owned()),
            index: None,
            params: vec![Param {
                id: CUTOFF,
                name: "Cutoff".to_owned(),
                value: 1_000.0,
                min: 20.0,
                max: 20_000.0,
            }],
        })
        .device()
        .unwrap();
    s.run(SetParam {
        device: filter,
        param: CUTOFF,
        value: 500.0,
    });
    s.run(SetDeviceBypass {
        device: filter,
        bypassed: true,
    });
    s.run(SetDeviceState {
        device: filter,
        state_blob: Some(ContentHash::of_bytes(b"state")),
    });
    s.run(WriteAutomation {
        device: filter,
        param: CUTOFF,
        points: vec![point(0, 200.0), point(1_920, 2_000.0)],
    });
    s.run(SetDeviceSidechain {
        device: filter,
        source: Some(audio),
        pre_fader: true,
    });

    // Clips and the arrangement.
    let clip = s
        .run(WriteMidiClip {
            track: midi,
            clip: None,
            name: "Line".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(3_840),
            notes: vec![
                note(10, 230, 37),
                note(1_000, 200, 42),
                note(2_890, 240, 49),
            ],
        })
        .clip()
        .unwrap();
    let loop_clip = s
        .run(AddAudioClip {
            track: audio,
            name: "Break".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(3_840),
            source: AudioRef {
                hash: ContentHash::of_bytes(b"break"),
                extension: "wav".to_owned(),
            },
            source_offset_frames: 0,
        })
        .clip()
        .unwrap();
    s.run(MoveClip {
        clip: loop_clip,
        start: Ticks::new(3_840),
        track: None,
    });
    s.run(SetClipGain {
        clip: loop_clip,
        gain_db: -3.0,
    });
    s.run(SetClipFades {
        clip: loop_clip,
        fade_in: Ticks::new(120),
        fade_out: Ticks::new(240),
    });
    s.run(SetClipWarp {
        clip: loop_clip,
        warp: Some(Warp {
            source_bpm: 126.0,
            key_lock: true,
        }),
    });
    let right = s
        .run(SplitClip {
            clip: loop_clip,
            at: Ticks::new(3_840 + 1_920),
        })
        .clip()
        .unwrap();
    let copy = s
        .run(DuplicateClip {
            clip,
            start: Some(Ticks::new(7_680)),
            track: None,
        })
        .clip()
        .unwrap();
    s.run(ResizeClip {
        clip: copy,
        length: Ticks::new(7_680),
    });
    s.run(SetClipLoop {
        clip: copy,
        loop_length: Some(Ticks::new(1_920)),
    });

    // The piano roll and the step sequencer.
    s.run(QuantizeClip {
        clip,
        grid: Ticks::new(240),
        strength: 1.0,
        lengths: true,
    });
    s.run(TransposeClip { clip, semitones: 2 });
    s.run(SnapClipToScale {
        clip,
        root: 0,
        scale: Scale::Major,
    });
    s.run(StampChord {
        clip,
        start: Ticks::new(2_160),
        length: Ticks::new(480),
        root: 60,
        chord: ChordKind::Minor7,
        inversion: 1,
        velocity: 90,
    });
    s.run(ApplyGroove {
        clip,
        groove: GrooveSource::Builtin("swing_16_62".to_owned()),
        amount: 1.0,
    });
    s.run(WriteStepPattern {
        track: spare,
        clip: None,
        name: "Hats".to_owned(),
        start: Ticks::ZERO,
        bars: 1,
        lanes: vec![StepLane {
            pitch: 42,
            velocity: 100,
            note_length: Ticks::new(120),
            steps: vec![true, false],
            velocities: Vec::new(),
        }],
        swing: None,
        groove: None,
        groove_amount: 1.0,
    });

    // The clip launcher.
    let scene = |s: &mut Session, name: &str| {
        s.run(AddScene {
            name: name.to_owned(),
            index: None,
        })
        .scene()
        .unwrap()
    };
    let (verse, drop) = (scene(&mut s, "Verse"), scene(&mut s, "Drop"));
    s.run(RenameScene {
        scene: verse,
        name: "Intro".to_owned(),
    });
    s.run(WriteMidiSlot {
        track: midi,
        scene: verse,
        name: "Intro line".to_owned(),
        length: Ticks::new(960),
        notes: vec![note(0, 240, 36)],
    });
    s.run(ClipToSlot { clip, scene: drop });
    s.run(ClearSlot {
        track: midi,
        scene: verse,
    });
    s.run(SetLaunchQuantize {
        ticks: Ticks::new(960),
    });

    // Modulation, macros and controllers.
    let lfo = |rate: f64| ModSource::Lfo {
        shape: LfoShape::Sine,
        rate: LfoRate::Hertz(rate),
        phase: 0.0,
    };
    let modulator = s
        .run(AddModulator {
            name: "Wobble".to_owned(),
            source: lfo(2.0),
        })
        .modulator()
        .unwrap();
    s.run(SetModulatorSource {
        modulator,
        source: lfo(4.0),
    });
    s.run(SetModulation {
        modulator,
        device: filter,
        param: CUTOFF,
        depth: 0.3,
    });
    s.run(RemoveModulation {
        modulator,
        device: filter,
        param: CUTOFF,
    });
    s.run(RemoveModulator { modulator });
    let macro_id = s
        .run(AddMacro {
            name: "Energy".to_owned(),
            value: 0.0,
        })
        .macro_id()
        .unwrap();
    s.run(SetMacroTarget {
        macro_id,
        device: filter,
        param: CUTOFF,
        min: 200.0,
        max: 8_000.0,
    });
    s.run(SetMacro {
        macro_id,
        value: 0.5,
    });
    s.run(RemoveMacroTarget {
        macro_id,
        device: filter,
        param: CUTOFF,
    });
    s.run(RemoveMacro { macro_id });
    s.run(MapController {
        channel: None,
        controller: 74,
        device: filter,
        param: CUTOFF,
        min: None,
        max: None,
    });
    s.run(UnmapController {
        channel: None,
        controller: 74,
    });

    // Project settings.
    s.run(SetTempo { bpm: 124.0 });
    s.run(SetSwing {
        percent: 58.0,
        grid: None,
    });
    s.run(RenameProject {
        name: "Friday night".to_owned(),
    });

    // Taking things away again.
    s.run(RemoveSend {
        track: midi,
        target: ret,
    });
    s.run(RemoveScene { scene: verse });
    s.run(DeleteClip { clip: right });
    s.run(RemoveDevice { device: filter });
    s.run(DeleteTrack { track: group });

    let all: BTreeSet<&str> = Command::NAMES.iter().copied().collect();
    let missing: Vec<&&str> = all.difference(&s.seen).collect();
    assert!(
        missing.is_empty(),
        "commands with no undo check: {missing:?}"
    );
    assert_eq!(s.seen.len(), Command::NAMES.len());

    // The whole session unwinds to the empty project.
    while s.bus.can_undo() {
        s.bus.undo().unwrap();
    }
    assert_eq!(s.bus.project(), &Project::new("Untitled"));
}
