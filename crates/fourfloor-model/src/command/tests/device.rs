//! Device, parameter and automation command tests.

use super::*;

#[test]
fn add_device_appends_or_inserts_and_defaults_its_name() {
    let mut f = fixture();
    let reverb = round_trip(
        &mut f.bus,
        AddDevice {
            track: f.midi,
            kind: "builtin.reverb".to_owned(),
            name: None,
            index: Some(0),
            params: Vec::new(),
        },
    )
    .device()
    .unwrap();
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.devices[0].id, reverb);
    assert_eq!(track.devices[0].name, "builtin.reverb");
    assert_eq!(track.devices[1].id, f.filter);
    assert!(!track.devices[0].bypassed);
}

#[test]
fn add_device_rejects_bad_input() {
    let mut f = fixture();
    let device = |track, index, params| AddDevice {
        track,
        kind: "builtin.gain".to_owned(),
        name: None,
        index,
        params,
    };
    let missing = TrackId::new(99);
    assert_eq!(
        rejected(&mut f.bus, device(missing, None, Vec::new())),
        ModelError::TrackNotFound(missing)
    );
    assert_eq!(
        rejected(&mut f.bus, device(f.midi, Some(2), Vec::new())),
        ModelError::IndexOutOfRange { index: 2, len: 1 }
    );
    let bad_params = [
        vec![
            param(CUTOFF, "A", 0.5, 0.0, 1.0),
            param(CUTOFF, "B", 0.5, 0.0, 1.0),
        ],
        vec![param(CUTOFF, "Inverted", 0.5, 1.0, 0.0)],
        vec![param(CUTOFF, "Outside", 2.0, 0.0, 1.0)],
        vec![param(CUTOFF, "NaN", f32::NAN, 0.0, 1.0)],
        vec![param(CUTOFF, "", 0.5, 0.0, 1.0)],
    ];
    for params in bad_params {
        let error = rejected(&mut f.bus, device(f.midi, None, params));
        assert!(matches!(
            error,
            ModelError::InvalidValue {
                field: "params",
                ..
            } | ModelError::InvalidName { .. }
        ));
    }
    let error = rejected(
        &mut f.bus,
        AddDevice {
            track: f.midi,
            kind: String::new(),
            name: None,
            index: None,
            params: Vec::new(),
        },
    );
    assert!(matches!(
        error,
        ModelError::InvalidName {
            field: "device kind",
            ..
        }
    ));
    // A failed add must not consume an id.
    assert_eq!(f.bus.project().next_id, 5);
}

#[test]
fn remove_device_drops_the_device_and_its_automation() {
    let mut f = fixture();
    f.bus
        .execute(WriteAutomation {
            device: f.filter,
            param: CUTOFF,
            points: vec![AutomationPoint {
                time: Ticks::ZERO,
                value: 200.0,
            }],
        })
        .unwrap();
    round_trip(&mut f.bus, RemoveDevice { device: f.filter });
    let track = f.bus.project().track(f.midi).unwrap();
    assert!(track.devices.is_empty());
    assert!(track.automation.is_empty());

    let error = rejected(&mut f.bus, RemoveDevice { device: f.filter });
    assert_eq!(error, ModelError::DeviceNotFound(f.filter));
}

#[test]
fn set_param_changes_one_value() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        SetParam {
            device: f.filter,
            param: CUTOFF,
            value: 440.0,
        },
    );
    let (_, device) = f.bus.project().find_device(f.filter).unwrap();
    assert_eq!(device.param(CUTOFF).unwrap().value, 440.0);
    assert_eq!(device.param(RESONANCE).unwrap().value, 0.2);
}

#[test]
fn set_param_rejects_unknown_targets_and_out_of_range_values() {
    let mut f = fixture();
    let set = |device, param, value| SetParam {
        device,
        param,
        value,
    };
    let no_device = DeviceId::new(99);
    assert_eq!(
        rejected(&mut f.bus, set(no_device, CUTOFF, 1.0)),
        ModelError::DeviceNotFound(no_device)
    );
    let no_param = ParamId::new(7);
    assert_eq!(
        rejected(&mut f.bus, set(f.filter, no_param, 1.0)),
        ModelError::ParamNotFound {
            device: f.filter,
            param: no_param
        }
    );
    assert_eq!(
        rejected(&mut f.bus, set(f.filter, RESONANCE, 1.5)),
        ModelError::ParamOutOfRange {
            device: f.filter,
            param: RESONANCE,
            value: 1.5,
            min: 0.0,
            max: 1.0
        }
    );
    let error = rejected(&mut f.bus, set(f.filter, RESONANCE, f32::NAN));
    assert!(matches!(error, ModelError::ParamOutOfRange { .. }));
}

#[test]
fn set_device_bypass_and_state_update_the_device() {
    let mut f = fixture();
    let blob = ContentHash::of_bytes(b"preset");
    round_trip(
        &mut f.bus,
        SetDeviceBypass {
            device: f.filter,
            bypassed: true,
        },
    );
    round_trip(
        &mut f.bus,
        SetDeviceState {
            device: f.filter,
            state_blob: Some(blob.clone()),
        },
    );
    let (_, device) = f.bus.project().find_device(f.filter).unwrap();
    assert!(device.bypassed);
    assert_eq!(device.state_blob, Some(blob));
    round_trip(
        &mut f.bus,
        SetDeviceState {
            device: f.filter,
            state_blob: None,
        },
    );

    let missing = DeviceId::new(99);
    assert_eq!(
        rejected(
            &mut f.bus,
            SetDeviceBypass {
                device: missing,
                bypassed: true
            }
        ),
        ModelError::DeviceNotFound(missing)
    );
    assert_eq!(
        rejected(
            &mut f.bus,
            SetDeviceState {
                device: missing,
                state_blob: None
            }
        ),
        ModelError::DeviceNotFound(missing)
    );
}

#[test]
fn write_automation_sorts_replaces_and_clears_a_lane() {
    let mut f = fixture();
    let point = |time, value| AutomationPoint {
        time: Ticks::new(time),
        value,
    };
    round_trip(
        &mut f.bus,
        WriteAutomation {
            device: f.filter,
            param: CUTOFF,
            points: vec![point(960, 8_000.0), point(0, 200.0)],
        },
    );
    let lane = f
        .bus
        .project()
        .track(f.midi)
        .unwrap()
        .lane(f.filter, CUTOFF)
        .cloned()
        .unwrap();
    assert_eq!(lane.points, [point(0, 200.0), point(960, 8_000.0)]);

    round_trip(
        &mut f.bus,
        WriteAutomation {
            device: f.filter,
            param: CUTOFF,
            points: vec![point(0, 500.0)],
        },
    );
    let track = f.bus.project().track(f.midi).unwrap();
    assert_eq!(track.automation.len(), 1);
    assert_eq!(
        track.lane(f.filter, CUTOFF).unwrap().points,
        [point(0, 500.0)]
    );

    round_trip(
        &mut f.bus,
        WriteAutomation {
            device: f.filter,
            param: CUTOFF,
            points: Vec::new(),
        },
    );
    assert!(f.bus.project().track(f.midi).unwrap().automation.is_empty());
}

#[test]
fn write_automation_rejects_bad_points_and_targets() {
    let mut f = fixture();
    let write = |device, param, time, value| WriteAutomation {
        device,
        param,
        points: vec![AutomationPoint {
            time: Ticks::new(time),
            value,
        }],
    };
    let missing = DeviceId::new(99);
    assert_eq!(
        rejected(&mut f.bus, write(missing, CUTOFF, 0, 500.0)),
        ModelError::DeviceNotFound(missing)
    );
    let no_param = ParamId::new(7);
    assert_eq!(
        rejected(&mut f.bus, write(f.filter, no_param, 0, 500.0)),
        ModelError::ParamNotFound {
            device: f.filter,
            param: no_param
        }
    );
    let error = rejected(&mut f.bus, write(f.filter, CUTOFF, 0, 5.0));
    assert!(matches!(error, ModelError::ParamOutOfRange { .. }));
    let error = rejected(&mut f.bus, write(f.filter, CUTOFF, -1, 500.0));
    assert!(matches!(
        error,
        ModelError::InvalidValue {
            field: "points",
            ..
        }
    ));
}
