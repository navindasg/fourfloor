//! Save, load and content-store tests. Everything runs in a temp directory.

use std::fs;

use fourfloor_core::{ParamId, Ticks};
use serde_json::{json, Value};

use super::*;
use crate::bus::CommandBus;
use crate::clip::{AudioRef, MidiNote};
use crate::command::{
    AddAudioClip, AddDevice, CreateTrack, SetDeviceState, SetSwing, SetTempo, SetTrackOutput,
    WriteAutomation, WriteMidiClip,
};
use crate::device::{AutomationPoint, Param};
use crate::project::SwingGrid;
use crate::track::TrackKind;

fn track(bus: &mut CommandBus, name: &str, kind: TrackKind) -> fourfloor_core::TrackId {
    bus.execute(CreateTrack {
        name: name.to_owned(),
        kind,
        index: None,
    })
    .unwrap()
    .track()
    .unwrap()
}

/// A project that uses every part of the model.
fn full_project(source: AudioRef, state_blob: ContentHash) -> Project {
    let mut bus = CommandBus::new(Project::new("Warehouse"));
    bus.execute(SetTempo { bpm: 124.5 }).unwrap();
    bus.execute(SetSwing {
        percent: 57.5,
        grid: Some(SwingGrid::Eighth),
    })
    .unwrap();
    let drums = track(&mut bus, "Drums", TrackKind::Midi);
    let top_loop = track(&mut bus, "Top loop", TrackKind::Audio);
    let group = track(&mut bus, "Drum bus", TrackKind::Bus);
    bus.execute(SetTrackOutput {
        track: drums,
        output: Some(group),
    })
    .unwrap();
    let cutoff = ParamId::new(1);
    let filter = bus
        .execute(AddDevice {
            track: drums,
            kind: "builtin.filter".to_owned(),
            name: None,
            index: None,
            params: vec![Param {
                id: cutoff,
                name: "Cutoff".to_owned(),
                value: 1_000.0,
                min: 20.0,
                max: 20_000.0,
            }],
        })
        .unwrap()
        .device()
        .unwrap();
    bus.execute(SetDeviceState {
        device: filter,
        state_blob: Some(state_blob),
    })
    .unwrap();
    bus.execute(WriteAutomation {
        device: filter,
        param: cutoff,
        points: vec![
            AutomationPoint {
                time: Ticks::ZERO,
                value: 200.0,
            },
            AutomationPoint {
                time: Ticks::new(15_360),
                value: 12_000.0,
            },
        ],
    })
    .unwrap();
    bus.execute(WriteMidiClip {
        track: drums,
        clip: None,
        name: "Kick".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::new(3_840),
        notes: (0..4)
            .map(|beat| MidiNote {
                start: Ticks::new(beat * 960),
                length: Ticks::new(240),
                pitch: 36,
                velocity: 120,
            })
            .collect(),
    })
    .unwrap();
    bus.execute(AddAudioClip {
        track: top_loop,
        name: "Top loop".to_owned(),
        start: Ticks::new(3_840),
        length: Ticks::new(15_360),
        source,
        source_offset_frames: 128,
    })
    .unwrap();
    bus.project().clone()
}

fn document(project: &Project) -> Value {
    serde_json::from_str(&project_to_json(project).unwrap()).unwrap()
}

fn load_document(document: &Value) -> Result<Project, StorageError> {
    project_from_json(&document.to_string())
}

#[test]
fn save_then_load_round_trips_a_full_project() {
    let dir = tempfile::tempdir().unwrap();
    let pool = AudioPool::new(dir.path());
    let source = pool.import_bytes(b"RIFF fake wav bytes", "wav").unwrap();
    let blob = PluginStateStore::new(dir.path())
        .put(b"preset bytes")
        .unwrap();
    let project = full_project(source.clone(), blob.clone());

    save_project(dir.path(), &project).unwrap();
    let loaded = load_project(dir.path()).unwrap();

    assert_eq!(loaded, project);
    assert!(dir.path().join(PROJECT_FILE).is_file());
    assert!(dir.path().join(AUDIO_DIR).is_dir());
    assert!(dir.path().join(PLUGIN_STATE_DIR).is_dir());
    assert!(!dir.path().join("project.json.tmp").exists());
    assert_eq!(loaded.audio_refs(), [source]);
    assert!(pool.missing(&loaded).is_empty());
    assert_eq!(
        PluginStateStore::new(dir.path()).get(&blob).unwrap(),
        b"preset bytes"
    );
}

#[test]
fn the_saved_file_carries_the_schema_version_and_plain_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut bus = CommandBus::new(Project::new("Tiny"));
    track(&mut bus, "Kick", TrackKind::Midi);
    save_project(dir.path(), bus.project()).unwrap();

    let text = fs::read_to_string(dir.path().join(PROJECT_FILE)).unwrap();
    let saved: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        saved,
        json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "app": "fourfloor",
            "project": {
                "name": "Tiny",
                "tempo": 120.0,
                "groove": { "swing_percent": 50.0, "grid": "sixteenth" },
                "tracks": [{
                    "id": 1,
                    "name": "Kick",
                    "kind": "midi",
                    "gain_db": 0.0,
                    "pan": 0.0,
                    "mute": false,
                    "solo": false,
                    "output": null,
                    "clips": [],
                    "devices": [],
                    "automation": []
                }],
                "next_id": 2
            }
        })
    );
}

#[test]
fn saving_again_replaces_the_previous_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut bus = CommandBus::new(Project::new("First"));
    save_project(dir.path(), bus.project()).unwrap();
    bus.execute(SetTempo { bpm: 128.0 }).unwrap();
    save_project(dir.path(), bus.project()).unwrap();
    assert_eq!(load_project(dir.path()).unwrap().tempo.bpm(), 128.0);
}

#[test]
fn a_loaded_project_keeps_handing_out_fresh_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut bus = CommandBus::new(Project::new("Ids"));
    let first = track(&mut bus, "Kick", TrackKind::Midi);
    save_project(dir.path(), bus.project()).unwrap();

    let mut reloaded = CommandBus::new(load_project(dir.path()).unwrap());
    let second = track(&mut reloaded, "Hats", TrackKind::Midi);
    assert_ne!(first, second);
    assert_eq!(reloaded.project().validate(), Ok(()));
}

#[test]
fn saving_refuses_an_inconsistent_project() {
    let dir = tempfile::tempdir().unwrap();
    let error = save_project(dir.path(), &Project::new("")).unwrap_err();
    assert!(matches!(
        error,
        StorageError::InvalidProject(ModelError::InvalidName { .. })
    ));
    assert!(!dir.path().join(PROJECT_FILE).exists());
}

#[test]
fn loading_reports_a_missing_or_malformed_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        load_project(dir.path()),
        Err(StorageError::Io { action: "read", .. })
    ));
    fs::write(dir.path().join(PROJECT_FILE), "{ not json").unwrap();
    assert!(matches!(
        load_project(dir.path()),
        Err(StorageError::Json(_))
    ));
    fs::write(dir.path().join(PROJECT_FILE), "{}").unwrap();
    assert!(matches!(
        load_project(dir.path()),
        Err(StorageError::MissingSchemaVersion)
    ));
}

#[test]
fn loading_refuses_a_newer_schema_version() {
    let mut newer = document(&Project::new("Future"));
    newer["schema_version"] = json!(CURRENT_SCHEMA_VERSION + 1);
    assert!(matches!(
        load_document(&newer),
        Err(StorageError::UnsupportedVersion { found, supported })
            if found == CURRENT_SCHEMA_VERSION + 1 && supported == CURRENT_SCHEMA_VERSION
    ));
}

#[test]
fn loading_refuses_an_older_version_with_no_registered_migration() {
    let mut older = document(&Project::new("Past"));
    older["schema_version"] = json!(0);
    assert!(matches!(
        load_document(&older),
        Err(StorageError::MissingMigration { from: 0 })
    ));
}

#[test]
fn loading_rejects_values_the_model_types_forbid() {
    let mut bad_tempo = document(&Project::new("Bad"));
    bad_tempo["project"]["tempo"] = json!(5.0);
    assert!(matches!(
        load_document(&bad_tempo),
        Err(StorageError::Json(_))
    ));

    let mut missing_field = document(&Project::new("Bad"));
    missing_field["project"]
        .as_object_mut()
        .unwrap()
        .remove("groove");
    assert!(matches!(
        load_document(&missing_field),
        Err(StorageError::Json(_))
    ));
}

#[test]
fn loading_rejects_hand_edits_that_break_invariants() {
    let hash = ContentHash::of_bytes(b"x");
    let source = AudioRef {
        hash: hash.clone(),
        extension: "wav".to_owned(),
    };
    let good = document(&full_project(source, hash));
    assert!(load_document(&good).is_ok());

    type Edit = fn(&mut Value);
    let edits: [(&str, Edit); 9] = [
        ("duplicate track id", |d| {
            d["project"]["tracks"][1]["id"] = json!(1);
        }),
        ("id counter too low", |d| {
            d["project"]["next_id"] = json!(2);
        }),
        ("MIDI clip on an audio track", |d| {
            let clip = d["project"]["tracks"][0]["clips"][0].clone();
            d["project"]["tracks"][1]["clips"] = json!([clip]);
            d["project"]["tracks"][0]["clips"] = json!([]);
        }),
        ("unsorted notes", |d| {
            let notes = d["project"]["tracks"][0]["clips"][0]["content"]["notes"]
                .as_array_mut()
                .unwrap();
            notes.reverse();
        }),
        ("note velocity out of range", |d| {
            d["project"]["tracks"][0]["clips"][0]["content"]["notes"][0]["velocity"] = json!(0);
        }),
        ("automation for a missing device", |d| {
            d["project"]["tracks"][0]["automation"][0]["device"] = json!(77);
        }),
        ("output that is not a bus", |d| {
            d["project"]["tracks"][0]["output"] = json!(2);
        }),
        ("swing out of range", |d| {
            d["project"]["groove"]["swing_percent"] = json!(90.0);
        }),
        ("path-like audio extension", |d| {
            d["project"]["tracks"][1]["clips"][0]["content"]["source"]["extension"] = json!("../x");
        }),
    ];
    for (what, edit) in edits {
        let mut broken = good.clone();
        edit(&mut broken);
        assert!(
            matches!(load_document(&broken), Err(StorageError::InvalidProject(_))),
            "{what} should be rejected"
        );
    }
}

#[test]
fn loading_rejects_a_malformed_content_hash() {
    let hash = ContentHash::of_bytes(b"x");
    let source = AudioRef {
        hash: hash.clone(),
        extension: "wav".to_owned(),
    };
    let mut broken = document(&full_project(source, hash));
    broken["project"]["tracks"][1]["clips"][0]["content"]["source"]["hash"] = json!("../../secret");
    assert!(matches!(load_document(&broken), Err(StorageError::Json(_))));
}

// ---- audio pool and plugin state ----

#[test]
fn the_pool_names_files_by_content_hash_and_stores_each_content_once() {
    let dir = tempfile::tempdir().unwrap();
    let pool = AudioPool::new(dir.path());
    let first = pool.import_bytes(b"kick sample", "wav").unwrap();
    let again = pool.import_bytes(b"kick sample", "WAV").unwrap();
    let other = pool.import_bytes(b"hat sample", "flac").unwrap();

    assert_eq!(first, again);
    assert_ne!(first.hash, other.hash);
    assert_eq!(first.hash, ContentHash::of_bytes(b"kick sample"));
    assert_eq!(first.extension, "wav");
    let path = pool.path_for(&first).unwrap();
    assert_eq!(
        path,
        dir.path().join("audio").join(format!("{}.wav", first.hash))
    );
    assert_eq!(fs::read(&path).unwrap(), b"kick sample");
    let names: Vec<String> = fs::read_dir(pool.dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "no duplicates or temp files: {names:?}");
    assert!(pool.contains(&first) && pool.contains(&other));
}

#[test]
fn the_pool_imports_files_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source_path = outside.path().join("Clap 01.AIFF");
    let bytes: Vec<u8> = (0..200_000_u32).map(|i| (i % 251) as u8).collect();
    fs::write(&source_path, &bytes).unwrap();

    let pool = AudioPool::new(dir.path());
    let imported = pool.import_file(&source_path).unwrap();
    assert_eq!(imported.extension, "aiff");
    assert_eq!(imported.hash, ContentHash::of_bytes(&bytes));
    assert_eq!(fs::read(pool.path_for(&imported).unwrap()).unwrap(), bytes);
    assert!(pool.verify(&imported).is_ok());
    assert!(source_path.is_file(), "import copies; the original stays");
}

#[test]
fn the_pool_rejects_bad_extensions_and_missing_sources() {
    let dir = tempfile::tempdir().unwrap();
    let pool = AudioPool::new(dir.path());
    for extension in ["", "../x", "wav.exe", "toolongext", "w/v"] {
        assert!(matches!(
            pool.import_bytes(b"x", extension),
            Err(StorageError::InvalidExtension(_))
        ));
    }
    assert!(matches!(
        pool.import_file(&dir.path().join("no-extension")),
        Err(StorageError::InvalidExtension(_))
    ));
    assert!(matches!(
        pool.import_file(&dir.path().join("missing.wav")),
        Err(StorageError::Io { action: "open", .. })
    ));
    let escaping = AudioRef {
        hash: ContentHash::of_bytes(b"x"),
        extension: "../../x".to_owned(),
    };
    assert!(matches!(
        pool.path_for(&escaping),
        Err(StorageError::InvalidExtension(_))
    ));
    assert!(!pool.contains(&escaping));
}

#[test]
fn the_pool_detects_corrupt_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let pool = AudioPool::new(dir.path());
    let stored = pool.import_bytes(b"clean", "wav").unwrap();
    assert!(pool.verify(&stored).is_ok());

    fs::write(pool.path_for(&stored).unwrap(), b"tampered").unwrap();
    match pool.verify(&stored).unwrap_err() {
        StorageError::HashMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, stored.hash);
            assert_eq!(actual, ContentHash::of_bytes(b"tampered"));
        }
        other => panic!("unexpected error: {other}"),
    }

    let absent = AudioRef {
        hash: ContentHash::of_bytes(b"never imported"),
        extension: "wav".to_owned(),
    };
    assert!(!pool.contains(&absent));
    assert!(matches!(
        pool.verify(&absent),
        Err(StorageError::Io { action: "open", .. })
    ));
    let project = full_project(absent.clone(), ContentHash::of_bytes(b"s"));
    assert_eq!(pool.missing(&project), [absent]);
}

#[test]
fn plugin_state_blobs_round_trip_and_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginStateStore::new(dir.path());
    let hash = store.put(b"\x00\x01state").unwrap();
    assert_eq!(hash, ContentHash::of_bytes(b"\x00\x01state"));
    assert_eq!(store.put(b"\x00\x01state").unwrap(), hash);
    assert_eq!(store.get(&hash).unwrap(), b"\x00\x01state");
    assert_eq!(
        store.path_for(&hash),
        dir.path().join("plugin-state").join(format!("{hash}.bin"))
    );

    fs::write(store.path_for(&hash), b"garbage").unwrap();
    assert!(matches!(
        store.get(&hash),
        Err(StorageError::HashMismatch { .. })
    ));
    assert!(matches!(
        store.get(&ContentHash::of_bytes(b"absent")),
        Err(StorageError::Io { action: "read", .. })
    ));
}

#[test]
fn storage_errors_read_as_plain_sentences() {
    let error = StorageError::UnsupportedVersion {
        found: 7,
        supported: 1,
    };
    assert_eq!(
        error.to_string(),
        "project uses schema version 7; this build reads up to 1"
    );
    let dir = tempfile::tempdir().unwrap();
    let message = load_project(dir.path()).unwrap_err().to_string();
    assert!(message.starts_with("cannot read "), "{message}");
    assert!(message.contains("project.json"), "{message}");
}
