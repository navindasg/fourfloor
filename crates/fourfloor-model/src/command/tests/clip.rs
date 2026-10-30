//! Clip command tests.

use super::*;

#[test]
fn write_midi_clip_creates_a_clip_with_sorted_notes() {
    let mut f = fixture();
    let outcome = round_trip(
        &mut f.bus,
        WriteMidiClip {
            track: f.midi,
            clip: None,
            name: "Bassline".to_owned(),
            start: Ticks::new(3_840),
            length: Ticks::new(3_840),
            notes: vec![note(960, 40), note(0, 43), note(0, 36)],
        },
    );
    let Outcome::ClipWritten { clip, created } = outcome else {
        panic!("unexpected outcome {outcome:?}");
    };
    assert!(created);
    let (track, stored) = f.bus.project().find_clip(clip).unwrap();
    assert_eq!(track.id, f.midi);
    assert_eq!(stored.notes(), [note(0, 36), note(0, 43), note(960, 40)]);
    assert_eq!(stored.end(), Ticks::new(7_680));
    assert!(stored.is_midi());
}

#[test]
fn write_midi_clip_replaces_an_existing_clip_in_place() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    let outcome = round_trip(
        &mut f.bus,
        WriteMidiClip {
            track: f.midi,
            clip: Some(clip),
            name: "Bassline B".to_owned(),
            start: Ticks::new(960),
            length: Ticks::new(1_920),
            notes: vec![note(0, 38), note(480, 38)],
        },
    );
    assert_eq!(
        outcome,
        Outcome::ClipWritten {
            clip,
            created: false
        }
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.clips.len(), 1);
    assert_eq!(track.clips[0].name, "Bassline B");
    assert_eq!(track.clips[0].notes().len(), 2);
}

#[test]
fn write_midi_clip_rejects_bad_targets() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    let other = f
        .bus
        .execute(create("Lead", TrackKind::Midi))
        .unwrap()
        .track()
        .unwrap();
    let write = |track, clip| WriteMidiClip {
        track,
        clip,
        name: "Clip".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::new(960),
        notes: Vec::new(),
    };
    let missing = TrackId::new(99);
    assert_eq!(
        rejected(&mut f.bus, write(missing, None)),
        ModelError::TrackNotFound(missing)
    );
    for (track, actual) in [(f.audio, TrackKind::Audio), (f.group, TrackKind::Bus)] {
        assert_eq!(
            rejected(&mut f.bus, write(track, None)),
            ModelError::WrongTrackKind {
                track,
                expected: TrackKind::Midi,
                actual
            }
        );
    }
    let no_clip = ClipId::new(99);
    assert_eq!(
        rejected(&mut f.bus, write(f.midi, Some(no_clip))),
        ModelError::ClipNotFound(no_clip)
    );
    assert_eq!(
        rejected(&mut f.bus, write(other, Some(clip))),
        ModelError::ClipNotOnTrack { clip, track: other }
    );
}

#[test]
fn write_midi_clip_rejects_bad_spans_and_notes() {
    let mut f = fixture();
    let write = |start: i64, length: i64, notes: Vec<MidiNote>| WriteMidiClip {
        track: f.midi,
        clip: None,
        name: "Clip".to_owned(),
        start: Ticks::new(start),
        length: Ticks::new(length),
        notes,
    };
    let error = rejected(&mut f.bus, write(-1, 960, Vec::new()));
    assert!(matches!(
        error,
        ModelError::InvalidValue { field: "start", .. }
    ));
    for length in [0, -960, i64::MAX] {
        let error = rejected(&mut f.bus, write(0, length, Vec::new()));
        assert!(matches!(
            error,
            ModelError::InvalidValue {
                field: "length",
                ..
            }
        ));
    }
    let bad_notes = [
        MidiNote {
            pitch: 128,
            ..note(0, 60)
        },
        MidiNote {
            velocity: 0,
            ..note(0, 60)
        },
        MidiNote {
            velocity: 128,
            ..note(0, 60)
        },
        MidiNote {
            length: Ticks::ZERO,
            ..note(0, 60)
        },
        note(-1, 60),
        note(960, 60),
    ];
    for bad in bad_notes {
        let error = rejected(&mut f.bus, write(0, 960, vec![note(0, 60), bad]));
        assert!(
            matches!(error, ModelError::InvalidNote { index: 1, .. }),
            "{bad:?}"
        );
    }
    let mut bad_name = write(0, 960, Vec::new());
    bad_name.name = String::new();
    let error = rejected(&mut f.bus, bad_name);
    assert!(matches!(
        error,
        ModelError::InvalidName {
            field: "clip name",
            ..
        }
    ));
}

#[test]
fn add_audio_clip_references_the_pool_by_hash() {
    let mut f = fixture();
    let clip = round_trip(
        &mut f.bus,
        AddAudioClip {
            track: f.audio,
            name: "Top loop".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(15_360),
            source: audio_ref(),
            source_offset_frames: 512,
        },
    )
    .clip()
    .unwrap();
    let (_, stored) = f.bus.project().find_clip(clip).unwrap();
    assert_eq!(stored.audio_ref(), Some(&audio_ref()));
    assert!(!stored.is_midi());
    assert!(stored.notes().is_empty());
    assert_eq!(f.bus.project().audio_refs(), [audio_ref()]);
}

#[test]
fn add_audio_clip_rejects_bad_input() {
    let mut f = fixture();
    let add = |track, extension: &str| AddAudioClip {
        track,
        name: "Loop".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::new(960),
        source: AudioRef {
            extension: extension.to_owned(),
            ..audio_ref()
        },
        source_offset_frames: 0,
    };
    assert_eq!(
        rejected(&mut f.bus, add(f.midi, "wav")),
        ModelError::WrongTrackKind {
            track: f.midi,
            expected: TrackKind::Audio,
            actual: TrackKind::Midi
        }
    );
    let missing = TrackId::new(99);
    assert_eq!(
        rejected(&mut f.bus, add(missing, "wav")),
        ModelError::TrackNotFound(missing)
    );
    for extension in ["", "WAV", "wav/../x", "toolongext", "w v"] {
        let error = rejected(&mut f.bus, add(f.audio, extension));
        assert!(matches!(
            error,
            ModelError::InvalidValue {
                field: "extension",
                ..
            }
        ));
    }
}

#[test]
fn move_clip_changes_start_and_optionally_track() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    let other = f
        .bus
        .execute(create("Lead", TrackKind::Midi))
        .unwrap()
        .track()
        .unwrap();

    round_trip(
        &mut f.bus,
        MoveClip {
            clip,
            start: Ticks::new(7_680),
            track: None,
        },
    );
    let (track, moved) = f.bus.project().find_clip(clip).unwrap();
    assert_eq!((track.id, moved.start), (f.midi, Ticks::new(7_680)));

    round_trip(
        &mut f.bus,
        MoveClip {
            clip,
            start: Ticks::new(960),
            track: Some(other),
        },
    );
    let project = f.bus.project();
    let (track, moved) = project.find_clip(clip).unwrap();
    assert_eq!((track.id, moved.start), (other, Ticks::new(960)));
    assert_eq!(moved.notes(), [note(0, 36)]);
    assert!(project.track(f.midi).unwrap().clips.is_empty());
}

#[test]
fn move_clip_rejects_bad_targets() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    let to = |start: i64, track| MoveClip {
        clip,
        start: Ticks::new(start),
        track,
    };
    let error = rejected(&mut f.bus, to(-960, None));
    assert!(matches!(
        error,
        ModelError::InvalidValue { field: "start", .. }
    ));
    assert_eq!(
        rejected(&mut f.bus, to(0, Some(f.audio))),
        ModelError::WrongTrackKind {
            track: f.audio,
            expected: TrackKind::Midi,
            actual: TrackKind::Audio
        }
    );
    let missing = TrackId::new(99);
    assert_eq!(
        rejected(&mut f.bus, to(0, Some(missing))),
        ModelError::TrackNotFound(missing)
    );
    let no_clip = ClipId::new(99);
    assert_eq!(
        rejected(
            &mut f.bus,
            MoveClip {
                clip: no_clip,
                start: Ticks::ZERO,
                track: None
            }
        ),
        ModelError::ClipNotFound(no_clip)
    );
}

#[test]
fn delete_clip_removes_only_that_clip() {
    let mut f = fixture();
    let first = midi_clip(&mut f, 0);
    let second = midi_clip(&mut f, 3_840);
    round_trip(&mut f.bus, DeleteClip { clip: first });
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.clips.len(), 1);
    assert_eq!(track.clips[0].id, second);

    let error = rejected(&mut f.bus, DeleteClip { clip: first });
    assert_eq!(error, ModelError::ClipNotFound(first));
}
