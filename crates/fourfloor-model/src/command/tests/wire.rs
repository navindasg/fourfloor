//! Wire format, schema and audio-message tests.

use super::*;

/// One example of every command.
fn samples() -> Vec<Command> {
    let track = TrackId::new(1);
    let device = DeviceId::new(4);
    let clip = ClipId::new(5);
    let scene = SceneId::new(6);
    let modulator = ModulatorId::new(7);
    let macro_id = MacroId::new(8);
    vec![
        create("Kick", TrackKind::Midi).into(),
        DeleteTrack { track }.into(),
        RenameTrack {
            track,
            name: "Kick".to_owned(),
        }
        .into(),
        SetTrackGain {
            track,
            gain_db: -3.0,
        }
        .into(),
        SetTrackPan { track, pan: 0.5 }.into(),
        SetTrackMute { track, mute: true }.into(),
        SetTrackSolo { track, solo: true }.into(),
        SetTrackOutput {
            track,
            output: Some(TrackId::new(3)),
        }
        .into(),
        AddDevice {
            track,
            kind: "builtin.filter".to_owned(),
            name: None,
            index: Some(0),
            params: vec![param(CUTOFF, "Cutoff", 1_000.0, 20.0, 20_000.0)],
        }
        .into(),
        RemoveDevice { device }.into(),
        SetParam {
            device,
            param: CUTOFF,
            value: 0.5,
        }
        .into(),
        SetDeviceBypass {
            device,
            bypassed: true,
        }
        .into(),
        SetDeviceState {
            device,
            state_blob: Some(ContentHash::of_bytes(b"preset")),
        }
        .into(),
        WriteAutomation {
            device,
            param: CUTOFF,
            points: vec![AutomationPoint {
                time: Ticks::new(960),
                value: 0.25,
            }],
        }
        .into(),
        WriteMidiClip {
            track,
            clip: Some(clip),
            name: "Clip".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(3_840),
            notes: vec![note(0, 36)],
        }
        .into(),
        AddAudioClip {
            track,
            name: "Loop".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(3_840),
            source: audio_ref(),
            source_offset_frames: 0,
        }
        .into(),
        MoveClip {
            clip,
            start: Ticks::new(960),
            track: None,
        }
        .into(),
        DeleteClip { clip }.into(),
        SetTempo { bpm: 124.0 }.into(),
        SetSwing {
            percent: 56.0,
            grid: Some(SwingGrid::Sixteenth),
        }
        .into(),
        RenameProject {
            name: "Warehouse".to_owned(),
        }
        .into(),
        SetSend {
            track,
            target: TrackId::new(3),
            gain_db: -6.0,
            pre_fader: false,
        }
        .into(),
        RemoveSend {
            track,
            target: TrackId::new(3),
        }
        .into(),
        SetDeviceSidechain {
            device,
            source: Some(TrackId::new(2)),
            pre_fader: true,
        }
        .into(),
        WriteTrackAutomation {
            track,
            param: MixerParam::Gain,
            points: vec![AutomationPoint {
                time: Ticks::ZERO,
                value: -12.0,
            }],
        }
        .into(),
        SetClipGain {
            clip,
            gain_db: -3.0,
        }
        .into(),
        SetClipFades {
            clip,
            fade_in: Ticks::new(120),
            fade_out: Ticks::new(240),
        }
        .into(),
        SetClipWarp {
            clip,
            warp: Some(Warp {
                source_bpm: 120.0,
                key_lock: true,
            }),
        }
        .into(),
        SplitClip {
            clip,
            at: Ticks::new(960),
        }
        .into(),
        DuplicateClip {
            clip,
            start: None,
            track: None,
        }
        .into(),
        SetClipLoop {
            clip,
            loop_length: Some(Ticks::new(960)),
        }
        .into(),
        ResizeClip {
            clip,
            length: Ticks::new(7_680),
        }
        .into(),
        AddScene {
            name: "Drop".to_owned(),
            index: None,
        }
        .into(),
        RemoveScene { scene }.into(),
        RenameScene {
            scene,
            name: "Break".to_owned(),
        }
        .into(),
        WriteMidiSlot {
            track,
            scene,
            name: "Riff".to_owned(),
            length: Ticks::new(3_840),
            notes: vec![note(0, 36)],
        }
        .into(),
        ClipToSlot { clip, scene }.into(),
        ClearSlot { track, scene }.into(),
        SetLaunchQuantize {
            ticks: Ticks::new(960),
        }
        .into(),
        WriteStepPattern {
            track,
            clip: None,
            name: "Hats".to_owned(),
            start: Ticks::ZERO,
            bars: 1,
            lanes: vec![StepLane {
                pitch: 42,
                velocity: 90,
                note_length: Ticks::new(120),
                steps: vec![false, true],
                velocities: vec![0, 110],
            }],
            swing: None,
            groove: Some(GrooveSource::Builtin("swing_16_58".to_owned())),
            groove_amount: 1.0,
        }
        .into(),
        QuantizeClip {
            clip,
            grid: Ticks::new(240),
            strength: 1.0,
            lengths: false,
        }
        .into(),
        TransposeClip {
            clip,
            semitones: -12,
        }
        .into(),
        SnapClipToScale {
            clip,
            root: 9,
            scale: Scale::Minor,
        }
        .into(),
        StampChord {
            clip,
            start: Ticks::ZERO,
            length: Ticks::new(960),
            root: 57,
            chord: ChordKind::Minor7,
            inversion: 0,
            velocity: 100,
        }
        .into(),
        ApplyGroove {
            clip,
            groove: GrooveSource::Custom(GrooveTemplate::swing("Mine", 240, 60.0)),
            amount: 0.5,
        }
        .into(),
        AddModulator {
            name: "Wobble".to_owned(),
            source: ModSource::Lfo {
                shape: LfoShape::Sine,
                rate: LfoRate::Beats(1.0),
                phase: 0.0,
            },
        }
        .into(),
        RemoveModulator { modulator }.into(),
        SetModulatorSource {
            modulator,
            source: ModSource::Envelope {
                trigger: track,
                attack_ms: 5.0,
                decay_ms: 100.0,
                sustain: 0.5,
                release_ms: 200.0,
            },
        }
        .into(),
        SetModulation {
            modulator,
            device,
            param: CUTOFF,
            depth: 0.25,
        }
        .into(),
        RemoveModulation {
            modulator,
            device,
            param: CUTOFF,
        }
        .into(),
        AddMacro {
            name: "Filter".to_owned(),
            value: 0.0,
        }
        .into(),
        RemoveMacro { macro_id }.into(),
        SetMacroTarget {
            macro_id,
            device,
            param: CUTOFF,
            min: 200.0,
            max: 8_000.0,
        }
        .into(),
        RemoveMacroTarget {
            macro_id,
            device,
            param: CUTOFF,
        }
        .into(),
        SetMacro {
            macro_id,
            value: 0.5,
        }
        .into(),
        MapController {
            channel: None,
            controller: 74,
            device,
            param: CUTOFF,
            min: None,
            max: Some(8_000.0),
        }
        .into(),
        UnmapController {
            channel: None,
            controller: 74,
        }
        .into(),
    ]
}

#[test]
fn command_names_are_stable() {
    // Renaming or removing an entry breaks saved agent tool calls: add new
    // names instead.
    assert_eq!(
        Command::NAMES,
        [
            "create_track",
            "delete_track",
            "rename_track",
            "set_track_gain",
            "set_track_pan",
            "set_track_mute",
            "set_track_solo",
            "set_track_output",
            "add_device",
            "remove_device",
            "set_param",
            "set_device_bypass",
            "set_device_state",
            "write_automation",
            "write_midi_clip",
            "add_audio_clip",
            "move_clip",
            "delete_clip",
            "set_tempo",
            "set_swing",
            "rename_project",
            "set_send",
            "remove_send",
            "set_device_sidechain",
            "write_track_automation",
            "set_clip_gain",
            "set_clip_fades",
            "set_clip_warp",
            "split_clip",
            "duplicate_clip",
            "set_clip_loop",
            "resize_clip",
            "add_scene",
            "remove_scene",
            "rename_scene",
            "write_midi_slot",
            "clip_to_slot",
            "clear_slot",
            "set_launch_quantize",
            "write_step_pattern",
            "quantize_clip",
            "transpose_clip",
            "snap_clip_to_scale",
            "stamp_chord",
            "apply_groove",
            "add_modulator",
            "remove_modulator",
            "set_modulator_source",
            "set_modulation",
            "remove_modulation",
            "add_macro",
            "remove_macro",
            "set_macro_target",
            "remove_macro_target",
            "set_macro",
            "map_controller",
            "unmap_controller",
        ]
    );
}

#[test]
fn every_command_round_trips_through_json_under_its_name() {
    let samples = samples();
    let names: Vec<&str> = samples.iter().map(Command::name).collect();
    assert_eq!(
        names,
        Command::NAMES,
        "samples() must cover every command once"
    );
    for command in samples {
        let wire = serde_json::to_value(&command).unwrap();
        assert_eq!(wire["name"], command.name());
        assert!(wire["args"].is_object());
        let decoded: Command = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded, command);
        let rebuilt = Command::from_name_and_args(command.name(), wire["args"].clone()).unwrap();
        assert_eq!(rebuilt, command);
    }
}

#[test]
fn wire_format_is_plain_json() {
    let command = Command::from(WriteMidiClip {
        track: TrackId::new(1),
        clip: None,
        name: "Kick".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::new(3_840),
        notes: vec![note(0, 36)],
    });
    assert_eq!(
        serde_json::to_value(&command).unwrap(),
        json!({
            "name": "write_midi_clip",
            "args": {
                "track": 1,
                "clip": null,
                "name": "Kick",
                "start": 0,
                "length": 3840,
                "notes": [{ "start": 0, "length": 240, "pitch": 36, "velocity": 100 }]
            }
        })
    );
}

#[test]
fn optional_arguments_may_be_omitted() {
    let command =
        Command::from_name_and_args("create_track", json!({ "name": "Kick", "kind": "midi" }))
            .unwrap();
    assert_eq!(command, create("Kick", TrackKind::Midi).into());
    let command = Command::from_name_and_args("set_swing", json!({ "percent": 56 })).unwrap();
    assert_eq!(
        command,
        SetSwing {
            percent: 56.0,
            grid: None
        }
        .into()
    );
}

#[test]
fn decoding_rejects_unknown_names_fields_and_types() {
    let cases = [
        ("explode_track", json!({})),
        ("set_tempo", json!({ "bpm": 124, "extra": true })),
        ("set_tempo", json!({ "bpm": "fast" })),
        ("set_tempo", json!({})),
        ("create_track", json!({ "name": "Kick", "kind": "video" })),
        (
            "set_device_state",
            json!({ "device": 1, "state_blob": "../../etc/passwd" }),
        ),
    ];
    for (name, args) in cases {
        let error = Command::from_name_and_args(name, args).unwrap_err();
        assert!(matches!(error, ModelError::Decode { name: ref n, .. } if n == name));
    }
}

#[test]
fn schemas_list_every_command_with_an_object_schema() {
    let schemas = command_schemas();
    let names: Vec<&str> = schemas.iter().map(|schema| schema.name).collect();
    assert_eq!(names, Command::NAMES);
    for schema in &schemas {
        assert_eq!(schema.args_schema["type"], "object", "{}", schema.name);
        assert!(
            schema.args_schema["properties"].is_object(),
            "{}",
            schema.name
        );
        assert!(!schema.description.is_empty(), "{}", schema.name);
        assert_eq!(
            schema.args_schema["additionalProperties"], false,
            "{}",
            schema.name
        );
    }
}

#[test]
fn schemas_describe_ids_as_integers_and_mark_required_fields() {
    let schemas = command_schemas();
    let schema = |name: &str| {
        schemas
            .iter()
            .find(|schema| schema.name == name)
            .map(|schema| schema.args_schema.clone())
            .unwrap()
    };
    let set_param = schema("set_param");
    assert_eq!(set_param["properties"]["device"]["type"], "integer");
    assert_eq!(set_param["properties"]["param"]["type"], "integer");
    assert_eq!(set_param["properties"]["value"]["type"], "number");
    assert_eq!(set_param["required"], json!(["device", "param", "value"]));

    let create_track = schema("create_track");
    assert_eq!(create_track["required"], json!(["name", "kind"]));

    let write_midi_clip = schema("write_midi_clip");
    assert_eq!(write_midi_clip["properties"]["start"]["type"], "integer");
    assert_eq!(
        write_midi_clip["required"],
        json!(["track", "name", "start", "length", "notes"])
    );
}

#[test]
fn sample_commands_validate_against_their_own_required_fields() {
    let schemas = command_schemas();
    for command in samples() {
        let wire = serde_json::to_value(&command).unwrap();
        let schema = schemas.iter().find(|s| s.name == command.name()).unwrap();
        let properties = schema.args_schema["properties"].as_object().unwrap();
        for key in wire["args"].as_object().unwrap().keys() {
            assert!(properties.contains_key(key), "{}: {key}", command.name());
        }
    }
}

#[test]
fn only_param_and_tempo_commands_have_an_audio_message() {
    let messages: Vec<(&str, AudioMessage)> = samples()
        .iter()
        .filter_map(|command| {
            command
                .audio_message()
                .map(|message| (command.name(), message))
        })
        .collect();
    assert_eq!(
        messages,
        [
            (
                "set_param",
                AudioMessage::SetParam {
                    device: DeviceId::new(4),
                    param: CUTOFF,
                    value: 0.5
                }
            ),
            (
                "set_tempo",
                AudioMessage::SetTempo {
                    tempo: Tempo::new(124.0).unwrap()
                }
            ),
        ]
    );
    assert_eq!(Command::from(SetTempo { bpm: 1.0 }).audio_message(), None);
}

#[test]
fn outcomes_serialize_with_a_kind_tag() {
    assert_eq!(
        serde_json::to_value(Outcome::Done).unwrap(),
        json!({ "kind": "done" })
    );
    assert_eq!(
        serde_json::to_value(Outcome::ClipWritten {
            clip: ClipId::new(5),
            created: true
        })
        .unwrap(),
        json!({ "kind": "clip_written", "clip": 5, "created": true })
    );
    assert_eq!(Outcome::Done.track(), None);
    assert_eq!(Outcome::Done.device(), None);
    assert_eq!(Outcome::Done.clip(), None);
    assert_eq!(
        serde_json::to_value(Outcome::MacroAdded {
            macro_id: MacroId::new(9)
        })
        .unwrap(),
        json!({ "kind": "macro_added", "macro": 9 })
    );
    assert_eq!(Outcome::Done.scene(), None);
    assert_eq!(Outcome::Done.modulator(), None);
    assert_eq!(Outcome::Done.macro_id(), None);
}

#[test]
fn macro_commands_use_the_word_macro_on_the_wire() {
    let command =
        Command::from_name_and_args("set_macro", json!({ "macro": 8, "value": 0.25 })).unwrap();
    assert_eq!(
        command,
        SetMacro {
            macro_id: MacroId::new(8),
            value: 0.25
        }
        .into()
    );
    let lfo = Command::from_name_and_args(
        "add_modulator",
        json!({
            "name": "Wobble",
            "source": { "type": "lfo", "shape": "triangle", "rate": { "unit": "hertz", "value": 4 } }
        }),
    )
    .unwrap();
    assert_eq!(lfo.name(), "add_modulator");
    let groove = Command::from_name_and_args(
        "apply_groove",
        json!({ "clip": 5, "groove": { "builtin": "swing_16_58" } }),
    )
    .unwrap();
    assert_eq!(
        groove,
        ApplyGroove {
            clip: ClipId::new(5),
            groove: GrooveSource::Builtin("swing_16_58".to_owned()),
            amount: 1.0
        }
        .into()
    );
}
