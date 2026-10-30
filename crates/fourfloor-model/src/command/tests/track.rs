//! Track command tests.

use super::*;

#[test]
fn create_track_appends_or_inserts() {
    let mut f = fixture();
    let outcome = round_trip(&mut f.bus, create("Pad", TrackKind::Midi));
    let pad = outcome.track().unwrap();
    assert_eq!(f.bus.project().tracks.last().unwrap().id, pad);

    let first = round_trip(
        &mut f.bus,
        CreateTrack {
            name: "Kick".to_owned(),
            kind: TrackKind::Audio,
            index: Some(0),
        },
    )
    .track()
    .unwrap();
    let project = f.bus.project();
    assert_eq!(project.tracks[0].id, first);
    assert_eq!(project.tracks[0].kind, TrackKind::Audio);
    assert_eq!(project.tracks.len(), 5);
}

#[test]
fn create_track_rejects_bad_names_and_indexes() {
    let mut f = fixture();
    for name in ["", "   ", "tab\there", &"x".repeat(129)] {
        let error = rejected(&mut f.bus, create(name, TrackKind::Midi));
        assert!(matches!(
            error,
            ModelError::InvalidName {
                field: "track name",
                ..
            }
        ));
    }
    let error = rejected(
        &mut f.bus,
        CreateTrack {
            name: "Late".to_owned(),
            kind: TrackKind::Midi,
            index: Some(4),
        },
    );
    assert_eq!(error, ModelError::IndexOutOfRange { index: 4, len: 3 });
}

#[test]
fn ids_are_unique_across_tracks_devices_and_clips() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    let ids = [
        f.midi.get(),
        f.audio.get(),
        f.group.get(),
        f.filter.get(),
        clip.get(),
    ];
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert_eq!(f.bus.project().next_id, 6);
}

#[test]
fn delete_track_removes_it_and_reroutes_its_feeders() {
    let mut f = fixture();
    f.bus
        .execute(SetTrackOutput {
            track: f.midi,
            output: Some(f.group),
        })
        .unwrap();
    round_trip(&mut f.bus, DeleteTrack { track: f.group });
    let project = f.bus.project();
    assert!(project.track(f.group).is_none());
    assert_eq!(project.track(f.midi).unwrap().output, None);

    let error = rejected(&mut f.bus, DeleteTrack { track: f.group });
    assert_eq!(error, ModelError::TrackNotFound(f.group));
}

#[test]
fn delete_track_takes_its_clips_and_devices_and_undo_brings_them_back() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    round_trip(&mut f.bus, DeleteTrack { track: f.midi });
    assert!(f.bus.project().find_clip(clip).is_none());
    assert!(f.bus.project().find_device(f.filter).is_none());
    f.bus.undo().unwrap();
    assert!(f.bus.project().find_clip(clip).is_some());
    assert!(f.bus.project().find_device(f.filter).is_some());
}

#[test]
fn rename_track_changes_the_name() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        RenameTrack {
            track: f.midi,
            name: "Sub bass".to_owned(),
        },
    );
    assert_eq!(f.bus.project().track(f.midi).unwrap().name, "Sub bass");

    let missing = TrackId::new(99);
    let error = rejected(
        &mut f.bus,
        RenameTrack {
            track: missing,
            name: "Ghost".to_owned(),
        },
    );
    assert_eq!(error, ModelError::TrackNotFound(missing));
    let error = rejected(
        &mut f.bus,
        RenameTrack {
            track: f.midi,
            name: String::new(),
        },
    );
    assert!(matches!(error, ModelError::InvalidName { .. }));
}

#[test]
fn set_track_gain_and_pan_validate_their_ranges() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        SetTrackGain {
            track: f.midi,
            gain_db: -6.0,
        },
    );
    round_trip(
        &mut f.bus,
        SetTrackPan {
            track: f.midi,
            pan: -0.25,
        },
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!((track.gain_db, track.pan), (-6.0, -0.25));

    for gain_db in [24.5, -97.0, f32::NAN, f32::INFINITY] {
        let error = rejected(
            &mut f.bus,
            SetTrackGain {
                track: f.midi,
                gain_db,
            },
        );
        assert!(matches!(
            error,
            ModelError::InvalidValue {
                field: "gain_db",
                ..
            }
        ));
    }
    for pan in [1.01, -2.0, f32::NAN] {
        let error = rejected(&mut f.bus, SetTrackPan { track: f.midi, pan });
        assert!(matches!(
            error,
            ModelError::InvalidValue { field: "pan", .. }
        ));
    }
    let missing = TrackId::new(99);
    let error = rejected(
        &mut f.bus,
        SetTrackGain {
            track: missing,
            gain_db: 0.0,
        },
    );
    assert_eq!(error, ModelError::TrackNotFound(missing));
}

#[test]
fn set_track_mute_and_solo_toggle_flags() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        SetTrackMute {
            track: f.audio,
            mute: true,
        },
    );
    round_trip(
        &mut f.bus,
        SetTrackSolo {
            track: f.audio,
            solo: true,
        },
    );
    let track = f.bus.project().track(f.audio).unwrap();
    assert!(track.mute && track.solo);

    let missing = TrackId::new(99);
    let error = rejected(
        &mut f.bus,
        SetTrackMute {
            track: missing,
            mute: true,
        },
    );
    assert_eq!(error, ModelError::TrackNotFound(missing));
    let error = rejected(
        &mut f.bus,
        SetTrackSolo {
            track: missing,
            solo: true,
        },
    );
    assert_eq!(error, ModelError::TrackNotFound(missing));
}

#[test]
fn set_track_output_routes_to_a_bus_and_back_to_master() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        SetTrackOutput {
            track: f.midi,
            output: Some(f.group),
        },
    );
    assert_eq!(f.bus.project().track(f.midi).unwrap().output, Some(f.group));
    round_trip(
        &mut f.bus,
        SetTrackOutput {
            track: f.midi,
            output: None,
        },
    );
    assert_eq!(f.bus.project().track(f.midi).unwrap().output, None);
}

#[test]
fn set_track_output_rejects_bad_routes() {
    let mut f = fixture();
    let second = f
        .bus
        .execute(create("Master bus", TrackKind::Bus))
        .unwrap()
        .track()
        .unwrap();
    f.bus
        .execute(SetTrackOutput {
            track: f.group,
            output: Some(second),
        })
        .unwrap();

    let route = |track, output| SetTrackOutput {
        track,
        output: Some(output),
    };
    let not_a_bus = rejected(&mut f.bus, route(f.midi, f.audio));
    assert!(matches!(not_a_bus, ModelError::InvalidRouting { .. }));
    let to_itself = rejected(&mut f.bus, route(f.group, f.group));
    assert!(matches!(to_itself, ModelError::InvalidRouting { .. }));
    let looped = rejected(&mut f.bus, route(second, f.group));
    assert!(matches!(looped, ModelError::InvalidRouting { track, .. } if track == second));

    let missing = TrackId::new(99);
    assert_eq!(
        rejected(&mut f.bus, route(f.midi, missing)),
        ModelError::TrackNotFound(missing)
    );
    assert_eq!(
        rejected(&mut f.bus, route(missing, f.group)),
        ModelError::TrackNotFound(missing)
    );
}
