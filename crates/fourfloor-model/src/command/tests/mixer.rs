//! Send, sidechain and mixer automation command tests.

use super::*;

fn send(track: TrackId, target: TrackId, gain_db: f32) -> SetSend {
    SetSend {
        track,
        target,
        gain_db,
        pre_fader: false,
    }
}

#[test]
fn set_send_adds_a_send_and_then_updates_it_in_place() {
    let mut f = fixture();
    round_trip(&mut f.bus, send(f.midi, f.group, -6.0));
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.sends.len(), 1);
    assert_eq!(track.send(f.group).unwrap().gain_db, -6.0);

    round_trip(
        &mut f.bus,
        SetSend {
            pre_fader: true,
            ..send(f.midi, f.group, -12.0)
        },
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(
        track.sends.len(),
        1,
        "the same target is updated, not doubled"
    );
    let stored = track.send(f.group).unwrap();
    assert_eq!((stored.gain_db, stored.pre_fader), (-12.0, true));
}

#[test]
fn set_send_rejects_bad_targets_levels_and_loops() {
    let mut f = fixture();
    assert_eq!(
        rejected(&mut f.bus, send(TrackId::new(99), f.group, 0.0)),
        ModelError::TrackNotFound(TrackId::new(99))
    );
    assert_eq!(
        rejected(&mut f.bus, send(f.midi, TrackId::new(99), 0.0)),
        ModelError::TrackNotFound(TrackId::new(99))
    );
    assert!(matches!(
        rejected(&mut f.bus, send(f.midi, f.audio, 0.0)),
        ModelError::InvalidRouting { .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, send(f.group, f.group, 0.0)),
        ModelError::InvalidRouting { .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, send(f.midi, f.group, 40.0)),
        ModelError::InvalidValue {
            field: "gain_db",
            ..
        }
    ));

    // Group -> Return by output, then Return -> Group by send would loop.
    let returns = f
        .bus
        .execute(create("Return", TrackKind::Bus))
        .unwrap()
        .track()
        .unwrap();
    f.bus
        .execute(SetTrackOutput {
            track: f.group,
            output: Some(returns),
        })
        .unwrap();
    assert!(matches!(
        rejected(&mut f.bus, send(returns, f.group, 0.0)),
        ModelError::InvalidRouting { .. }
    ));
}

#[test]
fn an_output_that_would_loop_through_a_send_is_rejected() {
    let mut f = fixture();
    let returns = f
        .bus
        .execute(create("Return", TrackKind::Bus))
        .unwrap()
        .track()
        .unwrap();
    f.bus.execute(send(f.group, returns, 0.0)).unwrap();
    let error = rejected(
        &mut f.bus,
        SetTrackOutput {
            track: returns,
            output: Some(f.group),
        },
    );
    assert!(
        matches!(error, ModelError::InvalidRouting { .. }),
        "{error}"
    );
}

#[test]
fn remove_send_removes_only_that_send() {
    let mut f = fixture();
    let returns = f
        .bus
        .execute(create("Return", TrackKind::Bus))
        .unwrap()
        .track()
        .unwrap();
    f.bus.execute(send(f.midi, f.group, 0.0)).unwrap();
    f.bus.execute(send(f.midi, returns, -3.0)).unwrap();
    round_trip(
        &mut f.bus,
        RemoveSend {
            track: f.midi,
            target: f.group,
        },
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.sends.len(), 1);
    assert!(track.send(returns).is_some());
    assert!(matches!(
        rejected(
            &mut f.bus,
            RemoveSend {
                track: f.midi,
                target: f.group
            }
        ),
        ModelError::NotFound { .. }
    ));
}

#[test]
fn set_device_sidechain_connects_and_disconnects_a_source() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        SetDeviceSidechain {
            device: f.filter,
            source: Some(f.audio),
            pre_fader: true,
        },
    );
    let (_, device) = f.bus.project().find_device(f.filter).unwrap();
    assert_eq!(
        device.sidechain,
        Some(SidechainSource {
            track: f.audio,
            pre_fader: true
        })
    );
    round_trip(
        &mut f.bus,
        SetDeviceSidechain {
            device: f.filter,
            source: None,
            pre_fader: false,
        },
    );
    let (_, device) = f.bus.project().find_device(f.filter).unwrap();
    assert_eq!(device.sidechain, None);
}

#[test]
fn set_device_sidechain_rejects_missing_things_itself_and_loops() {
    let mut f = fixture();
    let key = |device, source| SetDeviceSidechain {
        device,
        source: Some(source),
        pre_fader: false,
    };
    assert_eq!(
        rejected(&mut f.bus, key(DeviceId::new(99), f.audio)),
        ModelError::DeviceNotFound(DeviceId::new(99))
    );
    assert_eq!(
        rejected(&mut f.bus, key(f.filter, TrackId::new(99))),
        ModelError::TrackNotFound(TrackId::new(99))
    );
    assert!(matches!(
        rejected(&mut f.bus, key(f.filter, f.midi)),
        ModelError::InvalidRouting { .. }
    ));
    // The filter's track feeds the group, so the group cannot key it.
    f.bus
        .execute(SetTrackOutput {
            track: f.midi,
            output: Some(f.group),
        })
        .unwrap();
    assert!(matches!(
        rejected(&mut f.bus, key(f.filter, f.group)),
        ModelError::InvalidRouting { .. }
    ));
}

#[test]
fn deleting_a_track_drops_sends_sidechains_and_outputs_that_named_it() {
    let mut f = fixture();
    f.bus.execute(send(f.midi, f.group, 0.0)).unwrap();
    f.bus
        .execute(SetDeviceSidechain {
            device: f.filter,
            source: Some(f.audio),
            pre_fader: false,
        })
        .unwrap();
    f.bus
        .execute(SetTrackOutput {
            track: f.audio,
            output: Some(f.group),
        })
        .unwrap();
    round_trip(&mut f.bus, DeleteTrack { track: f.group });
    let project = f.bus.project();
    assert!(project.track(f.midi).unwrap().sends.is_empty());
    assert_eq!(project.track(f.audio).unwrap().output, None);
    assert!(project.find_device(f.filter).unwrap().1.sidechain.is_some());

    round_trip(&mut f.bus, DeleteTrack { track: f.audio });
    let project = f.bus.project();
    assert_eq!(project.find_device(f.filter).unwrap().1.sidechain, None);
    assert_eq!(project.validate(), Ok(()));
}

#[test]
fn write_track_automation_stores_sorted_points_and_clears() {
    let mut f = fixture();
    let point = |time: i64, value: f32| AutomationPoint {
        time: Ticks::new(time),
        value,
    };
    round_trip(
        &mut f.bus,
        WriteTrackAutomation {
            track: f.midi,
            param: MixerParam::Gain,
            points: vec![point(1_920, 0.0), point(0, -60.0)],
        },
    );
    round_trip(
        &mut f.bus,
        WriteTrackAutomation {
            track: f.midi,
            param: MixerParam::Pan,
            points: vec![point(0, -1.0), point(3_840, 1.0)],
        },
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.mixer_automation.len(), 2);
    let gain = track.mixer_lane(MixerParam::Gain).unwrap();
    assert_eq!(gain.points, [point(0, -60.0), point(1_920, 0.0)]);

    round_trip(
        &mut f.bus,
        WriteTrackAutomation {
            track: f.midi,
            param: MixerParam::Gain,
            points: Vec::new(),
        },
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert!(track.mixer_lane(MixerParam::Gain).is_none());
    assert!(track.mixer_lane(MixerParam::Pan).is_some());
}

#[test]
fn write_track_automation_rejects_values_outside_the_control() {
    let mut f = fixture();
    let lane = |track, param, value| WriteTrackAutomation {
        track,
        param,
        points: vec![AutomationPoint {
            time: Ticks::ZERO,
            value,
        }],
    };
    assert!(matches!(
        rejected(&mut f.bus, lane(f.midi, MixerParam::Pan, 2.0)),
        ModelError::InvalidValue { field: "pan", .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, lane(f.midi, MixerParam::Gain, 99.0)),
        ModelError::InvalidValue {
            field: "gain_db",
            ..
        }
    ));
    assert_eq!(
        rejected(&mut f.bus, lane(TrackId::new(99), MixerParam::Gain, 0.0)),
        ModelError::TrackNotFound(TrackId::new(99))
    );
}
