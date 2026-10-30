//! Modulator, macro and controller mapping command tests.

use super::*;

fn lfo(beats: f64) -> ModSource {
    ModSource::Lfo {
        shape: LfoShape::Sine,
        rate: LfoRate::Beats(beats),
        phase: 0.0,
    }
}

fn add_lfo(f: &mut Fixture) -> ModulatorId {
    f.bus
        .execute(AddModulator {
            name: "Wobble".to_owned(),
            source: lfo(1.0),
        })
        .unwrap()
        .modulator()
        .unwrap()
}

fn add_macro(f: &mut Fixture, value: f32) -> MacroId {
    f.bus
        .execute(AddMacro {
            name: "Sweep".to_owned(),
            value,
        })
        .unwrap()
        .macro_id()
        .unwrap()
}

fn cutoff(f: &Fixture) -> f32 {
    let (_, device) = f.bus.project().find_device(f.filter).unwrap();
    device.param(CUTOFF).unwrap().value
}

#[test]
fn add_modulator_stores_an_lfo_or_an_envelope() {
    let mut f = fixture();
    let outcome = round_trip(
        &mut f.bus,
        AddModulator {
            name: "Wobble".to_owned(),
            source: lfo(4.0),
        },
    );
    let id = outcome.modulator().unwrap();
    let stored = f.bus.project().modulator(id).unwrap();
    assert_eq!(stored.name, "Wobble");
    assert_eq!(stored.source, lfo(4.0));
    assert!(stored.targets.is_empty());

    let envelope = ModSource::Envelope {
        trigger: f.midi,
        attack_ms: 5.0,
        decay_ms: 100.0,
        sustain: 0.5,
        release_ms: 200.0,
    };
    let second = round_trip(
        &mut f.bus,
        AddModulator {
            name: "Pluck".to_owned(),
            source: envelope,
        },
    )
    .modulator()
    .unwrap();
    assert_ne!(second, id);
    assert_eq!(f.bus.project().modulators.len(), 2);
}

#[test]
fn add_modulator_rejects_bad_names_rates_and_triggers() {
    let mut f = fixture();
    assert!(matches!(
        rejected(
            &mut f.bus,
            AddModulator {
                name: String::new(),
                source: lfo(1.0),
            }
        ),
        ModelError::InvalidName { .. }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            AddModulator {
                name: "Fast".to_owned(),
                source: ModSource::Lfo {
                    shape: LfoShape::Square,
                    rate: LfoRate::Hertz(5_000.0),
                    phase: 0.0,
                },
            }
        ),
        ModelError::InvalidValue { field: "rate", .. }
    ));
    let envelope = |trigger| ModSource::Envelope {
        trigger,
        attack_ms: 1.0,
        decay_ms: 1.0,
        sustain: 1.0,
        release_ms: 1.0,
    };
    assert!(matches!(
        rejected(
            &mut f.bus,
            AddModulator {
                name: "Env".to_owned(),
                source: envelope(f.audio),
            }
        ),
        ModelError::WrongTrackKind { .. }
    ));
    assert_eq!(
        rejected(
            &mut f.bus,
            AddModulator {
                name: "Env".to_owned(),
                source: envelope(TrackId::new(99)),
            }
        ),
        ModelError::TrackNotFound(TrackId::new(99))
    );
}

#[test]
fn set_modulator_source_keeps_the_assignments() {
    let mut f = fixture();
    let id = add_lfo(&mut f);
    f.bus
        .execute(SetModulation {
            modulator: id,
            device: f.filter,
            param: CUTOFF,
            depth: 0.5,
        })
        .unwrap();
    round_trip(
        &mut f.bus,
        SetModulatorSource {
            modulator: id,
            source: lfo(8.0),
        },
    );
    let stored = f.bus.project().modulator(id).unwrap();
    assert_eq!(stored.source, lfo(8.0));
    assert_eq!(stored.targets.len(), 1);
    assert_eq!(
        rejected(
            &mut f.bus,
            SetModulatorSource {
                modulator: ModulatorId::new(99),
                source: lfo(1.0),
            }
        ),
        ModelError::ModulatorNotFound(ModulatorId::new(99))
    );
}

#[test]
fn set_modulation_assigns_then_updates_and_remove_modulation_releases() {
    let mut f = fixture();
    let id = add_lfo(&mut f);
    let assign = |depth| SetModulation {
        modulator: id,
        device: f.filter,
        param: CUTOFF,
        depth,
    };
    round_trip(&mut f.bus, assign(0.25));
    round_trip(&mut f.bus, assign(-0.5));
    let stored = f.bus.project().modulator(id).unwrap();
    assert_eq!(stored.targets.len(), 1, "updated in place");
    assert_eq!(stored.target(f.filter, CUTOFF).unwrap().depth, -0.5);

    assert!(matches!(
        rejected(&mut f.bus, assign(2.0)),
        ModelError::InvalidValue { field: "depth", .. }
    ));
    assert_eq!(
        rejected(
            &mut f.bus,
            SetModulation {
                param: ParamId::new(77),
                ..assign(0.1)
            }
        ),
        ModelError::ParamNotFound {
            device: f.filter,
            param: ParamId::new(77)
        }
    );

    let release = RemoveModulation {
        modulator: id,
        device: f.filter,
        param: CUTOFF,
    };
    round_trip(&mut f.bus, release);
    assert!(f.bus.project().modulator(id).unwrap().targets.is_empty());
    assert!(matches!(
        rejected(&mut f.bus, release),
        ModelError::NotFound { .. }
    ));
}

#[test]
fn remove_modulator_removes_it_with_its_assignments() {
    let mut f = fixture();
    let id = add_lfo(&mut f);
    f.bus
        .execute(SetModulation {
            modulator: id,
            device: f.filter,
            param: RESONANCE,
            depth: 1.0,
        })
        .unwrap();
    round_trip(&mut f.bus, RemoveModulator { modulator: id });
    assert!(f.bus.project().modulators.is_empty());
    assert_eq!(
        rejected(&mut f.bus, RemoveModulator { modulator: id }),
        ModelError::ModulatorNotFound(id)
    );
}

#[test]
fn removing_a_device_drops_what_pointed_at_it() {
    let mut f = fixture();
    let id = add_lfo(&mut f);
    let knob = add_macro(&mut f, 0.0);
    f.bus
        .execute(SetModulation {
            modulator: id,
            device: f.filter,
            param: CUTOFF,
            depth: 0.5,
        })
        .unwrap();
    f.bus
        .execute(SetMacroTarget {
            macro_id: knob,
            device: f.filter,
            param: CUTOFF,
            min: 100.0,
            max: 5_000.0,
        })
        .unwrap();
    f.bus
        .execute(MapController {
            channel: None,
            controller: 74,
            device: f.filter,
            param: CUTOFF,
            min: None,
            max: None,
        })
        .unwrap();
    round_trip(&mut f.bus, RemoveDevice { device: f.filter });
    let project = f.bus.project();
    assert!(project.modulator(id).unwrap().targets.is_empty());
    assert!(project.macro_control(knob).unwrap().targets.is_empty());
    assert!(project.controller_map.is_empty());
}

#[test]
fn add_macro_and_remove_macro() {
    let mut f = fixture();
    let id = round_trip(
        &mut f.bus,
        AddMacro {
            name: "Sweep".to_owned(),
            value: 0.25,
        },
    )
    .macro_id()
    .unwrap();
    assert_eq!(f.bus.project().macro_control(id).unwrap().value, 0.25);
    assert!(matches!(
        rejected(
            &mut f.bus,
            AddMacro {
                name: "Bad".to_owned(),
                value: 1.5,
            }
        ),
        ModelError::InvalidValue { field: "value", .. }
    ));
    round_trip(&mut f.bus, RemoveMacro { macro_id: id });
    assert!(f.bus.project().macros.is_empty());
    assert_eq!(
        rejected(&mut f.bus, RemoveMacro { macro_id: id }),
        ModelError::MacroNotFound(id)
    );
}

#[test]
fn set_macro_target_sets_the_parameter_for_the_current_position() {
    let mut f = fixture();
    let id = add_macro(&mut f, 0.5);
    let target = SetMacroTarget {
        macro_id: id,
        device: f.filter,
        param: CUTOFF,
        min: 200.0,
        max: 2_200.0,
    };
    round_trip(&mut f.bus, target);
    assert_eq!(cutoff(&f), 1_200.0);
    assert_eq!(f.bus.project().macro_control(id).unwrap().targets.len(), 1);

    assert!(matches!(
        rejected(
            &mut f.bus,
            SetMacroTarget {
                max: 99_999.0,
                ..target
            }
        ),
        ModelError::ParamOutOfRange { .. }
    ));
    assert_eq!(
        rejected(
            &mut f.bus,
            SetMacroTarget {
                macro_id: MacroId::new(99),
                ..target
            }
        ),
        ModelError::MacroNotFound(MacroId::new(99))
    );

    let release = RemoveMacroTarget {
        macro_id: id,
        device: f.filter,
        param: CUTOFF,
    };
    round_trip(&mut f.bus, release);
    assert_eq!(cutoff(&f), 1_200.0, "the parameter keeps its value");
    assert!(matches!(
        rejected(&mut f.bus, release),
        ModelError::NotFound { .. }
    ));
}

#[test]
fn set_macro_moves_every_parameter_it_drives() {
    let mut f = fixture();
    let id = add_macro(&mut f, 0.0);
    for (param, min, max) in [(CUTOFF, 100.0, 1_100.0), (RESONANCE, 0.8, 0.2)] {
        f.bus
            .execute(SetMacroTarget {
                macro_id: id,
                device: f.filter,
                param,
                min,
                max,
            })
            .unwrap();
    }
    let command = SetMacro {
        macro_id: id,
        value: 0.5,
    };
    round_trip(&mut f.bus, command);
    assert_eq!(cutoff(&f), 600.0);
    let (_, device) = f.bus.project().find_device(f.filter).unwrap();
    assert!((device.param(RESONANCE).unwrap().value - 0.5).abs() < 1e-6);

    let messages = Command::from(command).audio_messages(f.bus.project());
    assert_eq!(messages.len(), 2);
    assert!(messages.contains(&AudioMessage::SetParam {
        device: f.filter,
        param: CUTOFF,
        value: 600.0
    }));
    assert!(matches!(
        rejected(
            &mut f.bus,
            SetMacro {
                macro_id: id,
                value: -0.1,
            }
        ),
        ModelError::InvalidValue { field: "value", .. }
    ));
}

#[test]
fn map_controller_binds_replaces_and_unmap_removes() {
    let mut f = fixture();
    let bind = MapController {
        channel: None,
        controller: 74,
        device: f.filter,
        param: CUTOFF,
        min: None,
        max: None,
    };
    round_trip(&mut f.bus, bind);
    let stored = *f.bus.project().controller_mapping(3, 74).unwrap();
    assert_eq!((stored.min, stored.max), (20.0, 20_000.0), "the full range");

    round_trip(
        &mut f.bus,
        MapController {
            param: RESONANCE,
            min: Some(0.1),
            max: Some(0.9),
            ..bind
        },
    );
    let project = f.bus.project();
    assert_eq!(project.controller_map.len(), 1, "the binding was replaced");
    assert_eq!(project.controller_mapping(0, 74).unwrap().param, RESONANCE);

    assert!(matches!(
        rejected(
            &mut f.bus,
            MapController {
                channel: Some(16),
                ..bind
            }
        ),
        ModelError::InvalidValue {
            field: "channel",
            ..
        }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            MapController {
                controller: 200,
                ..bind
            }
        ),
        ModelError::InvalidValue {
            field: "controller",
            ..
        }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            MapController {
                max: Some(1e9),
                ..bind
            }
        ),
        ModelError::ParamOutOfRange { .. }
    ));

    let unbind = UnmapController {
        channel: None,
        controller: 74,
    };
    round_trip(&mut f.bus, unbind);
    assert!(f.bus.project().controller_map.is_empty());
    assert!(matches!(
        rejected(&mut f.bus, unbind),
        ModelError::NotFound { .. }
    ));
}
