//! Clip gain, fade, warp, split, duplicate, loop and resize command tests.

use super::*;

fn audio_settings(f: &Fixture, clip: ClipId) -> (f32, i64, i64, Option<Warp>) {
    let (_, stored) = f.bus.project().find_any_clip(clip).unwrap();
    match &stored.content {
        ClipContent::Audio {
            gain_db,
            fade_in,
            fade_out,
            warp,
            ..
        } => (*gain_db, fade_in.get(), fade_out.get(), *warp),
        ClipContent::Midi { .. } => panic!("expected an audio clip"),
    }
}

#[test]
fn set_clip_gain_fades_and_warp_change_an_audio_clip() {
    let mut f = fixture();
    let clip = audio_clip(&mut f, 0, 3_840);
    round_trip(
        &mut f.bus,
        SetClipGain {
            clip,
            gain_db: -6.0,
        },
    );
    round_trip(
        &mut f.bus,
        SetClipFades {
            clip,
            fade_in: Ticks::new(240),
            fade_out: Ticks::new(960),
        },
    );
    let warp = Warp {
        source_bpm: 126.0,
        key_lock: false,
    };
    round_trip(
        &mut f.bus,
        SetClipWarp {
            clip,
            warp: Some(warp),
        },
    );
    assert_eq!(audio_settings(&f, clip), (-6.0, 240, 960, Some(warp)));
    round_trip(&mut f.bus, SetClipWarp { clip, warp: None });
    assert_eq!(audio_settings(&f, clip).3, None);
}

#[test]
fn audio_settings_are_validated_and_refuse_midi_clips() {
    let mut f = fixture();
    let audio = audio_clip(&mut f, 0, 960);
    let midi = midi_clip(&mut f, 0);
    assert!(matches!(
        rejected(
            &mut f.bus,
            SetClipGain {
                clip: audio,
                gain_db: 99.0
            }
        ),
        ModelError::InvalidValue {
            field: "gain_db",
            ..
        }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            SetClipGain {
                clip: midi,
                gain_db: 0.0
            }
        ),
        ModelError::InvalidValue { field: "clip", .. }
    ));
    let fades = |clip, fade_in: i64, fade_out: i64| SetClipFades {
        clip,
        fade_in: Ticks::new(fade_in),
        fade_out: Ticks::new(fade_out),
    };
    assert!(matches!(
        rejected(&mut f.bus, fades(audio, 600, 600)),
        ModelError::InvalidValue { field: "fades", .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, fades(audio, -1, 0)),
        ModelError::InvalidValue { field: "fades", .. }
    ));
    assert!(matches!(
        rejected(&mut f.bus, fades(midi, 0, 10)),
        ModelError::InvalidValue { field: "clip", .. }
    ));
    let warp = |clip, source_bpm| SetClipWarp {
        clip,
        warp: Some(Warp {
            source_bpm,
            key_lock: true,
        }),
    };
    assert_eq!(
        rejected(&mut f.bus, warp(audio, 5.0)),
        ModelError::InvalidTempo { bpm: 5.0 }
    );
    assert!(matches!(
        rejected(&mut f.bus, warp(midi, 120.0)),
        ModelError::InvalidValue { field: "clip", .. }
    ));
    assert_eq!(
        rejected(&mut f.bus, warp(ClipId::new(99), 120.0)),
        ModelError::ClipNotFound(ClipId::new(99))
    );
}

#[test]
fn split_clip_cuts_a_midi_clip_into_two_windows_on_the_same_notes() {
    let mut f = fixture();
    let track = f.midi;
    let clip = f
        .bus
        .execute(WriteMidiClip {
            track,
            clip: None,
            name: "Riff".to_owned(),
            start: Ticks::new(3_840),
            length: Ticks::new(3_840),
            notes: vec![note(0, 36), note(960, 38), note(1_920, 40), note(2_880, 41)],
        })
        .unwrap()
        .clip()
        .unwrap();
    let outcome = round_trip(
        &mut f.bus,
        SplitClip {
            clip,
            at: Ticks::new(5_760),
        },
    );
    let right = outcome.clip().unwrap();
    assert_ne!(right, clip);
    let project = f.bus.project();
    let (_, left) = project.find_clip(clip).unwrap();
    let (_, right) = project.find_clip(right).unwrap();
    assert_eq!(
        (left.start.get(), left.length.get(), left.offset.get()),
        (3_840, 1_920, 0)
    );
    assert_eq!(
        (right.start.get(), right.length.get(), right.offset.get()),
        (5_760, 1_920, 1_920)
    );
    let pitches = |clip: &crate::clip::Clip| -> Vec<(i64, u8)> {
        clip.sounding_notes()
            .iter()
            .map(|n| (n.start.get(), n.pitch))
            .collect()
    };
    assert_eq!(pitches(left), [(0, 36), (960, 38)]);
    assert_eq!(pitches(right), [(0, 40), (960, 41)]);
}

#[test]
fn split_clip_gives_each_half_of_an_audio_clip_its_own_fade() {
    let mut f = fixture();
    let clip = audio_clip(&mut f, 0, 3_840);
    f.bus
        .execute(SetClipFades {
            clip,
            fade_in: Ticks::new(480),
            fade_out: Ticks::new(480),
        })
        .unwrap();
    let right = round_trip(
        &mut f.bus,
        SplitClip {
            clip,
            at: Ticks::new(960),
        },
    )
    .clip()
    .unwrap();
    assert_eq!(audio_settings(&f, clip), (0.0, 480, 0, None));
    assert_eq!(audio_settings(&f, right), (0.0, 0, 480, None));
    let (_, stored) = f.bus.project().find_clip(right).unwrap();
    assert_eq!(stored.offset, Ticks::new(960));
}

#[test]
fn split_clip_wraps_the_offset_of_a_looped_clip() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    f.bus
        .execute(SetClipLoop {
            clip,
            loop_length: Some(Ticks::new(960)),
        })
        .unwrap();
    let right = f
        .bus
        .execute(SplitClip {
            clip,
            at: Ticks::new(1_440),
        })
        .unwrap()
        .clip()
        .unwrap();
    let (_, stored) = f.bus.project().find_clip(right).unwrap();
    assert_eq!(stored.offset, Ticks::new(480));
    assert_eq!(stored.loop_length, Some(Ticks::new(960)));
}

#[test]
fn split_clip_rejects_cuts_outside_the_clip() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 960);
    for at in [0, 960, 4_800, 9_000] {
        assert!(matches!(
            rejected(
                &mut f.bus,
                SplitClip {
                    clip,
                    at: Ticks::new(at)
                }
            ),
            ModelError::InvalidValue { field: "at", .. }
        ));
    }
    assert_eq!(
        rejected(
            &mut f.bus,
            SplitClip {
                clip: ClipId::new(99),
                at: Ticks::new(10)
            }
        ),
        ModelError::ClipNotFound(ClipId::new(99))
    );
}

#[test]
fn duplicate_clip_copies_after_the_original_or_where_asked() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 960);
    let copy = round_trip(
        &mut f.bus,
        DuplicateClip {
            clip,
            start: None,
            track: None,
        },
    )
    .clip()
    .unwrap();
    let project = f.bus.project();
    let (track, stored) = project.find_clip(copy).unwrap();
    assert_eq!(track.id, f.midi);
    assert_eq!(stored.start, Ticks::new(4_800), "right after the original");
    assert_eq!(stored.notes(), project.find_clip(clip).unwrap().1.notes());

    let other = f
        .bus
        .execute(create("Lead", TrackKind::Midi))
        .unwrap()
        .track()
        .unwrap();
    let moved = round_trip(
        &mut f.bus,
        DuplicateClip {
            clip,
            start: Some(Ticks::ZERO),
            track: Some(other),
        },
    )
    .clip()
    .unwrap();
    let (track, stored) = f.bus.project().find_clip(moved).unwrap();
    assert_eq!((track.id, stored.start), (other, Ticks::ZERO));
}

#[test]
fn duplicate_clip_rejects_other_kinds_of_track_and_bad_positions() {
    let mut f = fixture();
    let clip = midi_clip(&mut f, 0);
    let to = |start, track| DuplicateClip { clip, start, track };
    assert!(matches!(
        rejected(&mut f.bus, to(None, Some(f.audio))),
        ModelError::WrongTrackKind { .. }
    ));
    assert_eq!(
        rejected(&mut f.bus, to(None, Some(TrackId::new(99)))),
        ModelError::TrackNotFound(TrackId::new(99))
    );
    assert!(matches!(
        rejected(&mut f.bus, to(Some(Ticks::new(-5)), None)),
        ModelError::InvalidValue { field: "start", .. }
    ));
}

#[test]
fn set_clip_loop_and_resize_clip_make_a_clip_repeat() {
    let mut f = fixture();
    let track = f.midi;
    let clip = f
        .bus
        .execute(WriteMidiClip {
            track,
            clip: None,
            name: "Riff".to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(1_920),
            notes: vec![note(0, 36), note(480, 38), note(1_440, 40)],
        })
        .unwrap()
        .clip()
        .unwrap();
    round_trip(
        &mut f.bus,
        SetClipLoop {
            clip,
            loop_length: Some(Ticks::new(960)),
        },
    );
    round_trip(
        &mut f.bus,
        ResizeClip {
            clip,
            length: Ticks::new(2_880),
        },
    );
    let (_, stored) = f.bus.project().find_clip(clip).unwrap();
    let sounding: Vec<(i64, u8)> = stored
        .sounding_notes()
        .iter()
        .map(|n| (n.start.get(), n.pitch))
        .collect();
    assert_eq!(
        sounding,
        [
            (0, 36),
            (480, 38),
            (960, 36),
            (1_440, 38),
            (1_920, 36),
            (2_400, 38)
        ]
    );
    assert_eq!(stored.notes().len(), 3, "the note past the loop is kept");

    round_trip(
        &mut f.bus,
        SetClipLoop {
            clip,
            loop_length: None,
        },
    );
    let (_, stored) = f.bus.project().find_clip(clip).unwrap();
    assert_eq!(stored.sounding_notes().len(), 3);
}

#[test]
fn loop_and_resize_are_validated_and_resize_shortens_fades() {
    let mut f = fixture();
    let clip = audio_clip(&mut f, 0, 1_920);
    f.bus
        .execute(SetClipFades {
            clip,
            fade_in: Ticks::new(960),
            fade_out: Ticks::new(960),
        })
        .unwrap();
    round_trip(
        &mut f.bus,
        ResizeClip {
            clip,
            length: Ticks::new(1_200),
        },
    );
    assert_eq!(audio_settings(&f, clip), (0.0, 960, 240, None));
    assert!(matches!(
        rejected(
            &mut f.bus,
            ResizeClip {
                clip,
                length: Ticks::ZERO
            }
        ),
        ModelError::InvalidValue {
            field: "length",
            ..
        }
    ));
    assert!(matches!(
        rejected(
            &mut f.bus,
            SetClipLoop {
                clip,
                loop_length: Some(Ticks::ZERO)
            }
        ),
        ModelError::InvalidValue {
            field: "loop_length",
            ..
        }
    ));
    assert_eq!(
        rejected(
            &mut f.bus,
            ResizeClip {
                clip: ClipId::new(99),
                length: Ticks::new(10)
            }
        ),
        ModelError::ClipNotFound(ClipId::new(99))
    );
}
