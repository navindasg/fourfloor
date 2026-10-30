//! The deterministic phrase parser: phrases become the expected tool
//! calls, unknown phrases are refused, and `instruct` runs them through
//! the agent so they are undoable.

mod common;

use fourfloor_agent::nl::{instruct, parse, ChatOptions, ToolCall};
use fourfloor_model::command::CreateTrack;
use fourfloor_model::{CommandBus, Project, TrackKind};
use serde_json::{json, Value};

/// A project with Kick (1), Hats (2), Bass (3), Pad (4) and Clap (5).
fn studio() -> Project {
    let mut bus = CommandBus::new(Project::new("Parser"));
    for name in ["Kick", "Hats", "Bass", "Pad", "Clap"] {
        bus.execute(CreateTrack {
            name: name.to_owned(),
            kind: TrackKind::Midi,
            index: None,
        })
        .unwrap();
    }
    bus.project().clone()
}

fn id(project: &Project, name: &str) -> u64 {
    project
        .tracks
        .iter()
        .find(|track| track.name == name)
        .unwrap()
        .id
        .get()
}

fn one(text: &str, project: &Project) -> ToolCall {
    let calls = parse(text, project).unwrap_or_else(|error| panic!("{text}: {error}"));
    assert_eq!(calls.len(), 1, "{text}: {calls:?}");
    calls.into_iter().next().unwrap()
}

fn expect(text: &str, name: &str, args: Value) {
    let project = studio();
    let call = one(text, &project);
    assert_eq!(call.name, name, "{text}");
    assert_eq!(call.args, args, "{text}");
}

#[test]
fn tempo_phrases() {
    for text in [
        "set tempo to 124",
        "Set the tempo to 124 BPM.",
        "tempo 124",
        "change the bpm to 124",
        "please set tempo to 124bpm",
        "124 bpm",
    ] {
        expect(text, "set_tempo", json!({ "bpm": 124.0 }));
    }
    assert!(parse("set tempo to fast", &studio()).is_err());
}

#[test]
fn swing_phrases() {
    expect(
        "swing the hats to 56%",
        "set_swing",
        json!({ "percent": 56.0, "track": "hats" }),
    );
    expect("set swing to 58%", "set_swing", json!({ "percent": 58.0 }));
    expect("swing 60 percent", "set_swing", json!({ "percent": 60.0 }));
    expect(
        "add 55% swing to the drums",
        "set_swing",
        json!({ "percent": 55.0, "track": "drums" }),
    );
    expect(
        "swing the eighths to 62%",
        "set_swing",
        json!({ "percent": 62.0, "track": "eighths", "grid": "eighth" }),
    );
    // Out of the model's range: refused rather than clamped.
    assert!(parse("swing the hats to 90%", &studio()).is_err());
}

#[test]
fn sidechain_phrases() {
    let args = json!({ "track": "pad", "key_track": "kick" });
    for text in [
        "sidechain the pad to the kick",
        "Sidechain pad from kick",
        "side chain the pad to the kick",
        "duck the pad with the kick",
        "make the kick duck the pad",
    ] {
        expect(text, "sidechain", args.clone());
    }
    expect(
        "sidechain the bass to the kick hard",
        "sidechain",
        json!({ "track": "bass", "key_track": "kick", "depth": 0.95 }),
    );
}

#[test]
fn mixer_phrases_resolve_track_names() {
    let project = studio();
    let clap = id(&project, "Clap");
    let bass = id(&project, "Bass");
    let hats = id(&project, "Hats");
    let call = one("mute the clap", &project);
    assert_eq!(call.name, "set_track_mute");
    assert_eq!(call.args, json!({ "track": clap, "mute": true }));
    assert_eq!(
        one("unmute clap", &project).args,
        json!({ "track": clap, "mute": false })
    );
    assert_eq!(
        one("solo the bass", &project).args,
        json!({ "track": bass, "solo": true })
    );
    assert_eq!(
        one("turn the bass down 3 db", &project).args,
        json!({ "track": bass, "gain_db": -3.0 })
    );
    assert_eq!(
        one("turn up the hats by 2db", &project).args,
        json!({ "track": hats, "gain_db": 2.0 })
    );
    assert_eq!(
        one("set the bass volume to -6 db", &project).args,
        json!({ "track": bass, "gain_db": -6.0 })
    );
    assert_eq!(
        one("pan the hats left 30%", &project).args,
        json!({ "track": hats, "pan": -0.3 })
    );
    assert_eq!(
        one("pan hats hard right", &project).args,
        json!({ "track": hats, "pan": 1.0 })
    );
    // A singular name finds the plural track.
    assert_eq!(one("mute the hat", &project).args["track"], json!(hats));
    let delete = one("delete the clap track", &project);
    assert_eq!(delete.name, "delete_track");
    assert_eq!(delete.args, json!({ "track": clap }));
    let error = parse("mute the theremin", &project).unwrap_err();
    assert!(error.0.contains("no track matches"), "{error}");
}

#[test]
fn part_phrases() {
    expect(
        "make a 16 bar deep house beat",
        "add_house_drum_pattern",
        json!({ "bars": 16, "style": "deep" }),
    );
    expect(
        "add a busy tech house groove with seed 9",
        "add_house_drum_pattern",
        json!({ "style": "tech", "density": 0.85, "seed": 9 }),
    );
    expect(
        "add a rolling bassline in F minor",
        "add_bassline",
        json!({ "style": "rolling", "key": "f minor" }),
    );
    expect(
        "write an 8-bar bassline in c# dorian",
        "add_bassline",
        json!({ "bars": 8, "style": "offbeat", "key": "c# dorian" }),
    );
    expect(
        "add organ chords",
        "add_chords",
        json!({ "style": "organ" }),
    );
    expect(
        "add some lush pads in a minor",
        "add_chords",
        json!({ "style": "sustained", "key": "a minor" }),
    );
    expect(
        "add an 8 bar filter riser",
        "add_filter_riser",
        json!({ "bars": 8 }),
    );
    expect(
        "add a 4 bar low pass filter sweep on the pad",
        "add_filter_riser",
        json!({ "bars": 4, "mode": "low_pass", "track": "pad" }),
    );
}

#[test]
fn session_phrases() {
    expect("undo", "undo", json!({}));
    expect("undo that", "undo", json!({}));
    expect("redo", "redo", json!({}));
    expect("render", "render", json!({}));
    expect(
        "bounce 8 bars to loop.wav",
        "render",
        json!({ "path": "loop.wav", "bars": 8 }),
    );
    expect(
        "render normalized to -9 lufs",
        "render",
        json!({ "normalize_lufs": -9.0 }),
    );
    expect("export stems", "export_stems", json!({}));
    expect(
        "rename the project to Deep Dive",
        "rename_project",
        json!({ "name": "Deep Dive" }),
    );
    expect("save", "save_project", json!({}));
    expect(
        "add an audio track called Vox Chops",
        "create_track",
        json!({ "name": "Vox Chops", "kind": "audio" }),
    );
}

#[test]
fn clauses_split_into_several_calls() {
    let project = studio();
    let calls = parse(
        "set tempo to 124 and swing the hats to 56%, then sidechain the pad to the kick",
        &project,
    )
    .unwrap();
    let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
    assert_eq!(names, ["set_tempo", "set_swing", "sidechain"]);

    let calls = parse("add a bassline and drums", &project).unwrap();
    let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
    assert_eq!(names, ["add_bassline", "add_house_drum_pattern"]);

    // "at 124 bpm" inside a request becomes its own tempo call first.
    let calls = parse("make a house beat at 124 bpm", &project).unwrap();
    assert_eq!(
        calls[0],
        ToolCall {
            name: "set_tempo".into(),
            args: json!({ "bpm": 124.0 })
        }
    );
    assert_eq!(calls[1].name, "add_house_drum_pattern");
}

#[test]
fn nonsense_is_refused_with_a_hint() {
    let project = studio();
    for text in ["", "what is the meaning of life", "make it sound better"] {
        let error = parse(text, &project).unwrap_err();
        assert!(!error.0.is_empty());
    }
    let error = parse("what is the meaning of life", &project).unwrap_err();
    assert!(error.0.contains("set tempo to 124"));
}

#[tokio::test(flavor = "multi_thread")]
async fn instructions_run_through_the_bus_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let agent = common::shared_in(dir.path());
    let done = instruct(
        &agent,
        "make a 4 bar house beat at 125 bpm",
        None,
        &ChatOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(done.handled_by, "parser");
    assert!(done.succeeded());
    let state = common::project(&agent);
    assert_eq!(state.tempo.bpm(), 125.0);
    assert_eq!(state.tracks.len(), 2);

    instruct(
        &agent,
        "swing the drums to 60%",
        None,
        &ChatOptions::default(),
    )
    .await
    .unwrap();
    let swung = common::project(&agent);
    let drums = swung.tracks.iter().find(|t| t.name == "Drums").unwrap();
    // The off-sixteenths moved: 60% of a 480-tick pair is 288.
    assert!(drums.clips[0]
        .notes()
        .iter()
        .any(|note| note.start.get() % 480 == 288));
    // The kick track was not touched, and the project swing is unchanged.
    assert_eq!(swung.groove.swing_percent, 50.0);

    // Three steps: tempo, drums, swing. Undo all three.
    for _ in 0..3 {
        instruct(&agent, "undo", None, &ChatOptions::default())
            .await
            .unwrap();
    }
    let reverted = common::project(&agent);
    assert!(reverted.tracks.is_empty());
    assert_eq!(reverted.tempo.bpm(), 120.0);

    let refused = instruct(
        &agent,
        "make it sound better",
        None,
        &ChatOptions::default(),
    )
    .await;
    assert!(refused.is_err());
}
