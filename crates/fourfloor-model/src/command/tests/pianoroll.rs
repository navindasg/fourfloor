//! Step pattern and piano-roll command tests.

use super::*;

fn lane(pitch: u8, steps: &[u8]) -> StepLane {
    StepLane {
        pitch,
        velocity: 100,
        note_length: Ticks::new(120),
        steps: steps.iter().map(|step| *step != 0).collect(),
        velocities: Vec::new(),
    }
}

fn pattern(track: TrackId, lanes: Vec<StepLane>) -> WriteStepPattern {
    WriteStepPattern {
        track,
        clip: None,
        name: "Pattern".to_owned(),
        start: Ticks::ZERO,
        bars: 1,
        lanes,
        swing: None,
        groove: None,
        groove_amount: 1.0,
    }
}

fn clip_with(f: &mut Fixture, notes: Vec<MidiNote>) -> ClipId {
    let track = f.midi;
    f.bus
        .execute(WriteMidiClip {
            track,
            clip: None,
            name: "Notes".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(3_840),
            notes,
        })
        .unwrap()
        .clip()
        .unwrap()
}

fn stored(f: &Fixture, clip: ClipId) -> Vec<(i64, u8)> {
    let (_, clip) = f.bus.project().find_any_clip(clip).unwrap();
    clip.notes()
        .iter()
        .map(|note| (note.start.get(), note.pitch))
        .collect()
}

#[test]
fn write_step_pattern_writes_one_note_per_active_step_and_repeats_short_lanes() {
    let mut f = fixture();
    let command = WriteStepPattern {
        bars: 2,
        ..pattern(
            f.midi,
            vec![lane(36, &[1, 0, 0, 0]), lane(42, &[0, 0, 1, 0, 0, 0, 1, 0])],
        )
    };
    let clip = round_trip(&mut f.bus, command).clip().unwrap();
    let (_, written) = f.bus.project().find_clip(clip).unwrap();
    assert_eq!(written.length, Ticks::new(7_680));
    let kicks: Vec<i64> = stored(&f, clip)
        .iter()
        .filter(|(_, pitch)| *pitch == 36)
        .map(|(start, _)| *start)
        .collect();
    assert_eq!(kicks, [0, 960, 1_920, 2_880, 3_840, 4_800, 5_760, 6_720]);
    let hats = stored(&f, clip)
        .iter()
        .filter(|(_, pitch)| *pitch == 42)
        .count();
    assert_eq!(hats, 8);
}

#[test]
fn write_step_pattern_bakes_in_swing_and_a_groove_template() {
    let mut f = fixture();
    // Off sixteenths only: steps 1 and 3.
    let off = || vec![lane(42, &[0, 1, 0, 1])];

    // 60% swing on sixteenths puts the off step at 288 instead of 240.
    let swung = WriteStepPattern {
        swing: Some(Groove {
            swing_percent: 60.0,
            grid: SwingGrid::Sixteenth,
        }),
        ..pattern(f.midi, off())
    };
    let clip = round_trip(&mut f.bus, swung).clip().unwrap();
    assert_eq!(&stored(&f, clip)[..2], [(288, 42), (768, 42)]);

    // The project swing is used when the command names none.
    f.bus
        .execute(SetSwing {
            percent: 75.0,
            grid: None,
        })
        .unwrap();
    let clip = f
        .bus
        .execute(pattern(f.midi, off()))
        .unwrap()
        .clip()
        .unwrap();
    assert_eq!(stored(&f, clip)[0], (360, 42));
    f.bus
        .execute(SetSwing {
            percent: 50.0,
            grid: None,
        })
        .unwrap();

    // A groove template: 58% swing delays the off step by 38 ticks; at half
    // strength by 19.
    let grooved = |amount| WriteStepPattern {
        groove: Some(GrooveSource::Builtin("swing_16_58".to_owned())),
        groove_amount: amount,
        ..pattern(f.midi, off())
    };
    let clip = f.bus.execute(grooved(1.0)).unwrap().clip().unwrap();
    assert_eq!(&stored(&f, clip)[..2], [(278, 42), (758, 42)]);
    let clip = f.bus.execute(grooved(0.5)).unwrap().clip().unwrap();
    assert_eq!(stored(&f, clip)[0], (259, 42));
}

#[test]
fn write_step_pattern_uses_per_step_velocities_and_replaces_a_clip() {
    let mut f = fixture();
    let accented = StepLane {
        velocities: vec![127, 0, 60],
        ..lane(42, &[1, 1, 1, 1])
    };
    let first = f
        .bus
        .execute(pattern(f.midi, vec![accented.clone()]))
        .unwrap()
        .clip()
        .unwrap();
    let (_, clip) = f.bus.project().find_clip(first).unwrap();
    let velocities: Vec<u8> = clip.notes()[..4].iter().map(|n| n.velocity).collect();
    assert_eq!(velocities, [127, 100, 60, 100]);

    let outcome = round_trip(
        &mut f.bus,
        WriteStepPattern {
            clip: Some(first),
            ..pattern(f.midi, vec![lane(36, &[1, 0, 0, 0])])
        },
    );
    assert_eq!(
        outcome,
        Outcome::ClipWritten {
            clip: first,
            created: false
        }
    );
    assert_eq!(stored(&f, first).len(), 4);
}

#[test]
fn write_step_pattern_validates_its_arguments() {
    let mut f = fixture();
    let base = || pattern(f.midi, vec![lane(36, &[1])]);
    let cases: Vec<(WriteStepPattern, &str)> = vec![
        (WriteStepPattern { bars: 0, ..base() }, "bars"),
        (WriteStepPattern { bars: 65, ..base() }, "bars"),
        (
            WriteStepPattern {
                groove_amount: 2.0,
                ..base()
            },
            "groove_amount",
        ),
        (
            WriteStepPattern {
                groove: Some(GrooveSource::Builtin("nope".to_owned())),
                ..base()
            },
            "groove",
        ),
        (
            WriteStepPattern {
                lanes: vec![lane(36, &[1]); MAX_STEP_LANES + 1],
                ..base()
            },
            "lanes",
        ),
    ];
    for (command, field) in cases {
        let error = rejected(&mut f.bus, command);
        assert!(
            matches!(error, ModelError::InvalidValue { field: found, .. } if found == field),
            "{field}: {error}"
        );
    }
    assert!(matches!(
        rejected(
            &mut f.bus,
            WriteStepPattern {
                swing: Some(Groove {
                    swing_percent: 90.0,
                    grid: SwingGrid::Sixteenth
                }),
                ..base()
            }
        ),
        ModelError::InvalidSwing { .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, pattern(f.midi, vec![lane(200, &[1])])),
        ModelError::InvalidNote { .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, pattern(f.audio, vec![lane(36, &[1])])),
        ModelError::WrongTrackKind { .. }
    ));
}

#[test]
fn quantize_clip_moves_starts_and_optionally_lengths() {
    let mut f = fixture();
    let loose = |start: i64, length: i64| MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch: 60,
        velocity: 100,
    };
    let clip = clip_with(
        &mut f,
        vec![loose(10, 200), loose(250, 30), loose(700, 500)],
    );
    round_trip(
        &mut f.bus,
        QuantizeClip {
            clip,
            grid: Ticks::new(240),
            strength: 0.5,
            lengths: false,
        },
    );
    assert_eq!(stored(&f, clip), [(5, 60), (245, 60), (710, 60)]);

    round_trip(
        &mut f.bus,
        QuantizeClip {
            clip,
            grid: Ticks::new(240),
            strength: 1.0,
            lengths: true,
        },
    );
    let (_, quantized) = f.bus.project().find_clip(clip).unwrap();
    let notes: Vec<(i64, i64)> = quantized
        .notes()
        .iter()
        .map(|n| (n.start.get(), n.length.get()))
        .collect();
    assert_eq!(notes, [(0, 240), (240, 240), (720, 480)]);

    for (grid, strength) in [(0, 1.0), (5_000, 1.0), (240, 1.5), (240, f64::NAN)] {
        assert!(matches!(
            rejected(
                &mut f.bus,
                QuantizeClip {
                    clip,
                    grid: Ticks::new(grid),
                    strength,
                    lengths: false
                }
            ),
            ModelError::InvalidValue { .. }
        ));
    }
}

#[test]
fn transpose_clip_shifts_every_note_or_none() {
    let mut f = fixture();
    let clip = clip_with(&mut f, vec![note(0, 36), note(960, 48)]);
    round_trip(&mut f.bus, TransposeClip { clip, semitones: 7 });
    assert_eq!(stored(&f, clip), [(0, 43), (960, 55)]);
    round_trip(
        &mut f.bus,
        TransposeClip {
            clip,
            semitones: -43,
        },
    );
    assert_eq!(stored(&f, clip), [(0, 0), (960, 12)]);
    assert!(matches!(
        rejected(
            &mut f.bus,
            TransposeClip {
                clip,
                semitones: -1
            }
        ),
        ModelError::InvalidNote { index: 0, .. }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            TransposeClip {
                clip,
                semitones: 120
            }
        ),
        ModelError::InvalidNote { index: 1, .. }
    ));
    let audio = audio_clip(&mut f, 0, 960);
    assert!(matches!(
        rejected(
            &mut f.bus,
            TransposeClip {
                clip: audio,
                semitones: 1
            }
        ),
        ModelError::InvalidValue { field: "clip", .. }
    ));
}

#[test]
fn snap_clip_to_scale_moves_notes_into_the_scale_and_merges_collisions() {
    let mut f = fixture();
    // C, C#, D#, F# against C major: C# and D# both fall (C# to C, D# to D).
    let clip = clip_with(
        &mut f,
        vec![note(0, 60), note(0, 61), note(960, 63), note(1_920, 66)],
    );
    round_trip(
        &mut f.bus,
        SnapClipToScale {
            clip,
            root: 0,
            scale: Scale::Major,
        },
    );
    assert_eq!(
        stored(&f, clip),
        [(0, 60), (960, 62), (1_920, 65)],
        "C# landed on the C already there and was merged"
    );
    assert!(matches!(
        rejected(
            &mut f.bus,
            SnapClipToScale {
                clip,
                root: 12,
                scale: Scale::Major
            }
        ),
        ModelError::InvalidValue { field: "root", .. }
    ));
}

#[test]
fn stamp_chord_adds_the_chord_and_replaces_notes_it_lands_on() {
    let mut f = fixture();
    let clip = clip_with(&mut f, vec![note(0, 57), note(960, 50)]);
    let stamp = StampChord {
        clip,
        start: Ticks::ZERO,
        length: Ticks::new(1_920),
        root: 57,
        chord: ChordKind::Minor7,
        inversion: 0,
        velocity: 90,
    };
    round_trip(&mut f.bus, stamp);
    assert_eq!(
        stored(&f, clip),
        [(0, 57), (0, 60), (0, 64), (0, 67), (960, 50)]
    );
    let (_, written) = f.bus.project().find_clip(clip).unwrap();
    assert_eq!(written.notes()[0].length, Ticks::new(1_920), "replaced");
    assert_eq!(written.notes()[0].velocity, 90);

    round_trip(
        &mut f.bus,
        StampChord {
            start: Ticks::new(1_920),
            inversion: 1,
            ..stamp
        },
    );
    assert_eq!(
        &stored(&f, clip)[5..],
        [(1_920, 60), (1_920, 64), (1_920, 67), (1_920, 69)]
    );

    for bad in [
        StampChord {
            velocity: 0,
            ..stamp
        },
        StampChord {
            length: Ticks::ZERO,
            ..stamp
        },
        StampChord {
            start: Ticks::new(-1),
            ..stamp
        },
    ] {
        assert!(matches!(
            rejected(&mut f.bus, bad),
            ModelError::InvalidNote { .. }
        ));
    }
    assert!(matches!(
        rejected(&mut f.bus, StampChord { root: 200, ..stamp }),
        ModelError::InvalidValue { field: "root", .. }
    ));
}

#[test]
fn apply_groove_shifts_off_steps_and_scales_velocity() {
    let mut f = fixture();
    let clip = clip_with(&mut f, vec![note(0, 42), note(240, 42), note(480, 42)]);
    round_trip(
        &mut f.bus,
        ApplyGroove {
            clip,
            groove: GrooveSource::Builtin("swing_16_62".to_owned()),
            amount: 1.0,
        },
    );
    // 62% swing: 0.24 of a 240-tick step is 57.6, rounded to 58.
    assert_eq!(stored(&f, clip), [(0, 42), (298, 42), (480, 42)]);

    let custom = GrooveTemplate {
        name: "Accent".to_owned(),
        step_ticks: 480,
        steps: vec![crate::groove::GrooveStep {
            timing: 0.0,
            velocity: 0.5,
        }],
    };
    round_trip(
        &mut f.bus,
        ApplyGroove {
            clip,
            groove: GrooveSource::Custom(custom.clone()),
            amount: 1.0,
        },
    );
    let (_, grooved) = f.bus.project().find_clip(clip).unwrap();
    assert!(grooved.notes().iter().all(|note| note.velocity == 50));

    let broken = GrooveTemplate {
        steps: Vec::new(),
        ..custom
    };
    assert!(matches!(
        rejected(
            &mut f.bus,
            ApplyGroove {
                clip,
                groove: GrooveSource::Custom(broken),
                amount: 1.0
            }
        ),
        ModelError::InvalidValue {
            field: "groove",
            ..
        }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            ApplyGroove {
                clip,
                groove: GrooveSource::Builtin("straight".to_owned()),
                amount: -0.5
            }
        ),
        ModelError::InvalidValue {
            field: "amount",
            ..
        }
    ));
}
