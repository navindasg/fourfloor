//! High-level tools without the MCP layer: generated parts are in key, on
//! the grid and fixed by their seed; swing re-times written notes; a filter
//! riser really sweeps when rendered; saving and autosave write the folder.

mod common;

use std::sync::Arc;

use common::agent_in;
use fourfloor_agent::generate::Key;
use fourfloor_agent::{default_registry, Agent, AgentSettings};
use fourfloor_model::sequencer::{BAR_TICKS, STEP_TICKS};
use fourfloor_model::{load_project, MidiNote, Project};
use fourfloor_testkit::{spectrum_band_energy_db, StereoBuffer};
use serde_json::{json, Value};

fn run(agent: &mut Agent, tool: &str, args: Value) -> Value {
    agent
        .call_tool(tool, args)
        .unwrap_or_else(|error| panic!("{tool}: {error}"))
        .data
}

fn notes_of(project: &Project, track: u64) -> Vec<MidiNote> {
    project
        .tracks
        .iter()
        .find(|t| t.id.get() == track)
        .unwrap()
        .clips
        .iter()
        .flat_map(|clip| clip.notes().to_vec())
        .collect()
}

#[test]
fn generated_parts_are_in_key_on_grid_and_seeded() {
    let dir = tempfile::tempdir().unwrap();
    let build = |seed: u64| {
        let mut agent = agent_in(dir.path());
        let bass = run(
            &mut agent,
            "add_bassline",
            json!({ "bars": 8, "key": "F# minor", "style": "walking", "seed": seed }),
        );
        let chords = run(
            &mut agent,
            "add_chords",
            json!({ "bars": 8, "key": "F# minor", "style": "organ", "seed": seed }),
        );
        let project = agent.project().clone();
        (
            notes_of(&project, bass["track"].as_u64().unwrap()),
            notes_of(&project, chords["track"].as_u64().unwrap()),
            chords["progression"].clone(),
        )
    };
    let key: Key = "F# minor".parse().unwrap();
    let (bass, chords, progression) = build(11);
    assert!(!bass.is_empty() && !chords.is_empty());
    for note in bass.iter().chain(&chords) {
        assert!(
            key.contains(note.pitch),
            "pitch {} not in {key}",
            note.pitch
        );
        assert_eq!(note.start.get() % STEP_TICKS, 0, "on the sixteenth grid");
        assert!(note.start.get() < 8 * BAR_TICKS);
    }
    assert!(
        bass.iter()
            .all(|note| note.start.get() % (4 * STEP_TICKS) != 0),
        "bass leaves the beat to the kick"
    );
    assert_eq!(progression.as_array().unwrap().len(), 8);

    // Same seed, same parts; another seed, other parts.
    assert_eq!(
        build(11),
        (bass.clone(), chords.clone(), progression.clone())
    );
    assert_ne!(build(12).0, bass);
}

#[test]
fn drum_patterns_follow_the_house_rules_and_project_swing() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = agent_in(dir.path());
    run(&mut agent, "set_swing", json!({ "percent": 62.5 }));
    let written = run(
        &mut agent,
        "add_house_drum_pattern",
        json!({ "bars": 4, "seed": 5 }),
    );
    let project = agent.project().clone();
    let kick = notes_of(&project, written["kick_track"].as_u64().unwrap());
    let starts: Vec<i64> = kick.iter().map(|note| note.start.get()).collect();
    let beats: Vec<i64> = (0..16).map(|beat| beat * 960).collect();
    assert_eq!(starts, beats, "kick on every beat");
    let drums = notes_of(&project, written["drum_track"].as_u64().unwrap());
    // Closed hats on off-sixteenths sit at 62.5% of the pair: tick 300.
    let hats: Vec<i64> = drums
        .iter()
        .filter(|note| note.pitch == 42)
        .map(|note| note.start.get() % 480)
        .collect();
    assert!(hats.contains(&300), "swung hats: {hats:?}");
    assert!(hats.iter().all(|offset| *offset == 0 || *offset == 300));
    // Claps on beats two and four.
    let claps: Vec<i64> = drums
        .iter()
        .filter(|note| note.pitch == 39)
        .map(|note| note.start.get() % 3840)
        .collect();
    assert!(claps.iter().all(|tick| *tick == 960 || *tick == 2880));
}

#[test]
fn set_swing_retimes_one_track_and_can_go_back_to_straight() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = agent_in(dir.path());
    let written = run(
        &mut agent,
        "add_house_drum_pattern",
        json!({ "bars": 2, "seed": 2 }),
    );
    let drum_track = written["drum_track"].as_u64().unwrap();
    let straight = notes_of(agent.project(), drum_track);

    let swung = run(
        &mut agent,
        "set_swing",
        json!({ "percent": 58, "track": "drums" }),
    );
    assert_eq!(swung["clips_retimed"], json!(1));
    let moved = notes_of(agent.project(), drum_track);
    assert_ne!(moved, straight);
    assert!(moved.iter().any(|note| note.start.get() % 480 == 278));
    assert_eq!(
        agent.project().groove.swing_percent,
        50.0,
        "project swing unchanged"
    );

    // Back to straight: exactly the original notes again.
    run(
        &mut agent,
        "set_swing",
        json!({ "percent": 50, "track": "drums" }),
    );
    assert_eq!(notes_of(agent.project(), drum_track), straight);
    // Two undo steps back to the swung and then the straight pattern.
    agent.undo().unwrap();
    assert_eq!(notes_of(agent.project(), drum_track), moved);
    agent.undo().unwrap();
    assert_eq!(notes_of(agent.project(), drum_track), straight);
    assert!(agent
        .call_tool("set_swing", json!({ "percent": 80 }))
        .is_err());
}

#[test]
fn a_filter_riser_sweeps_the_low_end_away_and_resets_at_the_drop() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = agent_in(dir.path());
    run(&mut agent, "set_tempo", json!({ "bpm": 120 }));
    run(
        &mut agent,
        "add_bassline",
        json!({ "bars": 8, "style": "rolling", "seed": 4 }),
    );
    let riser = run(
        &mut agent,
        "add_filter_riser",
        json!({ "bars": 4, "end_bar": 6 }),
    );
    assert_eq!(riser["start_bar"], json!(2));
    assert_eq!(riser["tracks"].as_array().unwrap().len(), 1);
    let rendered = run(
        &mut agent,
        "render",
        json!({ "path": "riser.wav", "tail_seconds": 0 }),
    );
    let path = rendered["path"].as_str().unwrap().to_owned();
    let mix = StereoBuffer::read_wav(std::path::Path::new(&path)).unwrap();
    let rate = mix.sample_rate;
    // 120 BPM: a bar is 2 s.
    let low = |bar: f64| {
        let slice = mix.slice_seconds(bar * 2.0, bar * 2.0 + 2.0);
        spectrum_band_energy_db(&slice.mono(), rate, 30.0, 150.0)
    };
    let before = low(1.0);
    let end_of_sweep = low(5.0);
    let after_drop = low(6.0);
    assert!(
        end_of_sweep < before - 20.0,
        "the sweep removes the bass: {before:.1} dB before, {end_of_sweep:.1} dB at the end"
    );
    assert!(
        (after_drop - before).abs() < 3.0,
        "the filter resets at the drop: {after_drop:.1} vs {before:.1} dB"
    );

    // One undo removes the filter and its automation.
    agent.undo().unwrap();
    assert!(agent.project().tracks[0]
        .devices
        .iter()
        .all(|device| device.kind != "fx.filter"));
    assert!(agent.project().tracks[0].automation.is_empty());
}

#[test]
fn saving_and_autosave_write_the_project_folder() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(default_registry().unwrap());
    let folder = dir.path().join("song");
    let mut agent = Agent::open(&folder, registry.clone())
        .unwrap()
        .with_settings(AgentSettings {
            autosave: true,
            ..AgentSettings::default()
        });
    assert_eq!(agent.project().name, "song");
    run(&mut agent, "set_tempo", json!({ "bpm": 128 }));
    assert_eq!(load_project(&folder).unwrap().tempo.bpm(), 128.0);
    agent.undo().unwrap();
    assert_eq!(load_project(&folder).unwrap().tempo.bpm(), 120.0);

    // Re-opening reads the saved project.
    run(&mut agent, "add_house_drum_pattern", json!({ "bars": 1 }));
    let reopened = Agent::open(&folder, registry).unwrap();
    assert_eq!(reopened.project(), agent.project());

    // Without a folder there is nowhere to save or render.
    let mut loose = Agent::new(Project::new("Loose"), Arc::new(default_registry().unwrap()));
    assert!(loose.call_tool("save_project", json!({})).is_err());
    assert!(loose.call_tool("render", json!({})).is_err());
}
