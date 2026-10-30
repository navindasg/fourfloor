//! Scene, slot and launch quantization command tests.

use super::*;

fn scene(f: &mut Fixture, name: &str) -> SceneId {
    f.bus
        .execute(AddScene {
            name: name.to_owned(),
            index: None,
        })
        .unwrap()
        .scene()
        .unwrap()
}

fn slot(track: TrackId, scene: SceneId) -> WriteMidiSlot {
    WriteMidiSlot {
        track,
        scene,
        name: "Riff".to_owned(),
        length: Ticks::new(3_840),
        notes: vec![note(960, 40), note(0, 36)],
    }
}

#[test]
fn add_scene_appends_or_inserts_and_rename_scene_renames() {
    let mut f = fixture();
    let intro = round_trip(
        &mut f.bus,
        AddScene {
            name: "Intro".to_owned(),
            index: None,
        },
    )
    .scene()
    .unwrap();
    let drop = round_trip(
        &mut f.bus,
        AddScene {
            name: "Drop".to_owned(),
            index: Some(0),
        },
    )
    .scene()
    .unwrap();
    let project = f.bus.project();
    assert_eq!(project.scene_index(drop), Some(0));
    assert_eq!(project.scene_index(intro), Some(1));

    round_trip(
        &mut f.bus,
        RenameScene {
            scene: intro,
            name: "Build".to_owned(),
        },
    );
    assert_eq!(f.bus.project().scene(intro).unwrap().name, "Build");
}

#[test]
fn scene_commands_reject_bad_names_positions_and_ids() {
    let mut f = fixture();
    assert!(matches!(
        rejected(
            &mut f.bus,
            AddScene {
                name: " ".to_owned(),
                index: None
            }
        ),
        ModelError::InvalidName { .. }
    ));
    assert_eq!(
        rejected(
            &mut f.bus,
            AddScene {
                name: "A".to_owned(),
                index: Some(3)
            }
        ),
        ModelError::IndexOutOfRange { index: 3, len: 0 }
    );
    let missing = SceneId::new(99);
    assert_eq!(
        rejected(&mut f.bus, RemoveScene { scene: missing }),
        ModelError::SceneNotFound(missing)
    );
    assert_eq!(
        rejected(
            &mut f.bus,
            RenameScene {
                scene: missing,
                name: "A".to_owned()
            }
        ),
        ModelError::SceneNotFound(missing)
    );
}

#[test]
fn write_midi_slot_fills_a_slot_and_replaces_what_was_there() {
    let mut f = fixture();
    let row = scene(&mut f, "Drop");
    let first = round_trip(&mut f.bus, slot(f.midi, row)).clip().unwrap();
    let project = f.bus.project();
    let (track, stored) = project.find_slot_clip(first).unwrap();
    assert_eq!((track.id, stored.scene), (f.midi, row));
    assert_eq!(stored.clip.notes(), [note(0, 36), note(960, 40)]);
    assert!(project.find_clip(first).is_none(), "not on the timeline");
    assert!(project.find_any_clip(first).is_some());

    let second = round_trip(&mut f.bus, slot(f.midi, row)).clip().unwrap();
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.slots.len(), 1);
    assert_eq!(track.slot(row).unwrap().clip.id, second);
}

#[test]
fn write_midi_slot_rejects_bad_tracks_scenes_and_notes() {
    let mut f = fixture();
    let row = scene(&mut f, "Drop");
    assert!(matches!(
        rejected(&mut f.bus, slot(f.audio, row)),
        ModelError::WrongTrackKind { .. }
    ));
    assert_eq!(
        rejected(&mut f.bus, slot(f.midi, SceneId::new(99))),
        ModelError::SceneNotFound(SceneId::new(99))
    );
    assert!(matches!(
        rejected(
            &mut f.bus,
            WriteMidiSlot {
                notes: vec![note(9_999, 36)],
                ..slot(f.midi, row)
            }
        ),
        ModelError::InvalidNote { index: 0, .. }
    ));
}

#[test]
fn clip_to_slot_copies_a_timeline_clip_of_either_kind() {
    let mut f = fixture();
    let row = scene(&mut f, "Drop");
    let midi = midi_clip(&mut f, 3_840);
    let audio = audio_clip(&mut f, 960, 1_920);
    let midi_copy = round_trip(
        &mut f.bus,
        ClipToSlot {
            clip: midi,
            scene: row,
        },
    )
    .clip()
    .unwrap();
    let audio_copy = round_trip(
        &mut f.bus,
        ClipToSlot {
            clip: audio,
            scene: row,
        },
    )
    .clip()
    .unwrap();
    let project = f.bus.project();
    assert!(project.find_clip(midi).is_some(), "the original stays");
    let (track, stored) = project.find_slot_clip(midi_copy).unwrap();
    assert_eq!(track.id, f.midi);
    assert_eq!(stored.clip.start, Ticks::ZERO);
    assert_eq!(stored.clip.length, Ticks::new(3_840));
    let (track, stored) = project.find_slot_clip(audio_copy).unwrap();
    assert_eq!(track.id, f.audio);
    assert_eq!(stored.clip.audio_ref(), Some(&audio_ref()));
    assert_eq!(project.audio_refs(), [audio_ref()]);
    assert_eq!(
        rejected(
            &mut f.bus,
            ClipToSlot {
                clip: midi_copy,
                scene: row
            }
        ),
        ModelError::ClipNotFound(midi_copy),
        "only timeline clips can be copied into a slot"
    );
}

#[test]
fn clear_slot_and_remove_scene_empty_slots() {
    let mut f = fixture();
    let first = scene(&mut f, "A");
    let second = scene(&mut f, "B");
    f.bus.execute(slot(f.midi, first)).unwrap();
    f.bus.execute(slot(f.midi, second)).unwrap();
    round_trip(
        &mut f.bus,
        ClearSlot {
            track: f.midi,
            scene: first,
        },
    );
    assert!(f.bus.project().track(f.midi).unwrap().slot(first).is_none());
    assert!(matches!(
        rejected(
            &mut f.bus,
            ClearSlot {
                track: f.midi,
                scene: first
            }
        ),
        ModelError::NotFound { .. }
    ));

    round_trip(&mut f.bus, RemoveScene { scene: second });
    let project = f.bus.project();
    assert!(project.scene(second).is_none());
    assert!(project.track(f.midi).unwrap().slots.is_empty());
    assert_eq!(project.scenes.len(), 1);
}

#[test]
fn clip_commands_reach_launcher_clips() {
    let mut f = fixture();
    let row = scene(&mut f, "Drop");
    let clip = f.bus.execute(slot(f.midi, row)).unwrap().clip().unwrap();
    round_trip(
        &mut f.bus,
        TransposeClip {
            clip,
            semitones: 12,
        },
    );
    let (_, stored) = f.bus.project().find_slot_clip(clip).unwrap();
    assert_eq!(stored.clip.notes()[0].pitch, 48);
    assert_eq!(
        rejected(
            &mut f.bus,
            MoveClip {
                clip,
                start: Ticks::new(960),
                track: None
            }
        ),
        ModelError::ClipNotFound(clip),
        "a launcher clip has no timeline position to move"
    );
}

#[test]
fn set_launch_quantize_accepts_zero_to_sixteen_bars() {
    let mut f = fixture();
    assert_eq!(f.bus.project().launch_quantize, Ticks::new(3_840));
    round_trip(
        &mut f.bus,
        SetLaunchQuantize {
            ticks: Ticks::new(960),
        },
    );
    assert_eq!(f.bus.project().launch_quantize, Ticks::new(960));
    round_trip(&mut f.bus, SetLaunchQuantize { ticks: Ticks::ZERO });
    for bad in [-1, 3_840 * 16 + 1] {
        assert!(matches!(
            rejected(
                &mut f.bus,
                SetLaunchQuantize {
                    ticks: Ticks::new(bad)
                }
            ),
            ModelError::InvalidValue {
                field: "launch_quantize",
                ..
            }
        ));
    }
}
