//! Beat loops, loop rolls, hot cues, cue and beat jump, checked sample by
//! sample on rendered audio.

mod common;

use common::{kick_track, noise_track, renderer, run, seconds_to_frames, RATE};
use fourfloor_dj::{DjCommand, DjRenderer};
use fourfloor_testkit::onset_times;

fn track_left(track: &fourfloor_dj::DeckTrack) -> Vec<f32> {
    track.left().to_vec()
}

/// A renderer with a 120 BPM noise track on deck 0, playing from 0 for
/// `seconds`. Returns it and the track's left channel.
fn playing(seconds: f64, quantize: bool) -> (DjRenderer, Vec<f32>) {
    let track = noise_track(120.0, 60.0);
    let left = track_left(&track);
    let mut dj = renderer(2, &[]);
    dj.load(0, track).unwrap();
    run(
        &mut dj,
        DjCommand::SetQuantize {
            deck: 0,
            on: quantize,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 0 });
    dj.render_seconds(seconds).unwrap();
    (dj, left)
}

#[test]
fn a_four_beat_loop_repeats_exactly() {
    let (mut dj, track) = playing(1.3, true);
    let start = seconds_to_frames(1.3);
    run(
        &mut dj,
        DjCommand::BeatLoop {
            deck: 0,
            beats: 4.0,
        },
    );
    let out = dj.render(3 * 96_000).unwrap();
    let audio = &out.decks[0][0];
    // Quantized: the loop starts on beat 2 (48000) and lasts 4 beats.
    let state = dj.session().state();
    let region = state.decks[0].loop_state.unwrap();
    assert_eq!(region.start_seconds, 1.0);
    assert_eq!(region.end_seconds, 3.0);
    assert_eq!(&audio[..144_000 - start], &track[start..144_000]);
    assert_eq!(
        &audio[144_000 - start..144_000 - start + 96_000],
        &track[48_000..144_000]
    );
    // Every pass is identical.
    assert_eq!(&audio[..96_000], &audio[96_000..192_000]);
    assert_eq!(&audio[96_000..192_000], &audio[192_000..288_000]);
}

#[test]
fn a_loop_on_a_fractional_grid_keeps_the_beat() {
    // At 126 BPM a beat is 22857.14 samples: the loop end is fractional.
    let mut dj = renderer(2, &[]);
    dj.load(0, kick_track(126.0, 60.0)).unwrap();
    run(&mut dj, DjCommand::Play { deck: 0 });
    dj.render_seconds(2.1).unwrap();
    run(
        &mut dj,
        DjCommand::BeatLoop {
            deck: 0,
            beats: 4.0,
        },
    );
    let out = dj.render_seconds(20.0).unwrap();
    let onsets = onset_times(&out.decks[0][0], RATE);
    let beat = 60.0 / 126.0;
    assert!(onsets.len() > 35, "{} onsets", onsets.len());
    for gap in onsets.windows(2).map(|w| w[1] - w[0]) {
        assert!((gap - beat).abs() < 0.0005, "gap {gap} vs {beat}");
    }
}

#[test]
fn loop_exit_and_reloop() {
    let (mut dj, track) = playing(0.5, true);
    run(
        &mut dj,
        DjCommand::BeatLoop {
            deck: 0,
            beats: 1.0,
        },
    );
    dj.render(48_000).unwrap();
    // A beat loop of 24000..48000 (from beat 1) played twice: at 24000.
    run(&mut dj, DjCommand::LoopExit { deck: 0 });
    let out = dj.render(28_800).unwrap();
    assert_eq!(&out.decks[0][0][..], &track[24_000..52_800]);
    // Past the old loop, reloop jumps back to its start.
    run(&mut dj, DjCommand::Reloop { deck: 0 });
    let out = dj.render(28_800).unwrap();
    assert_eq!(&out.decks[0][0][..24_000], &track[24_000..48_000]);
    assert_eq!(&out.decks[0][0][24_000..], &track[24_000..28_800]);
    assert!(dj.session().state().decks[0].loop_state.is_some());
    // Reloop again leaves it.
    run(&mut dj, DjCommand::Reloop { deck: 0 });
    let out = dj.render(24_000).unwrap();
    assert_eq!(&out.decks[0][0][..], &track[28_800..52_800]);
}

#[test]
fn loop_roll_continues_where_the_track_would_have_been() {
    let (mut dj, track) = playing(1.1, false);
    let start = seconds_to_frames(1.1);
    run(
        &mut dj,
        DjCommand::LoopRoll {
            deck: 0,
            beats: 0.25,
        },
    );
    let rolled = dj.render(48_000).unwrap();
    // A quarter-beat roll from the playhead (quantize off): 6000 samples
    // repeated.
    assert_eq!(&rolled.decks[0][0][..6_000], &track[start..start + 6_000]);
    assert_eq!(
        &rolled.decks[0][0][6_000..12_000],
        &track[start..start + 6_000]
    );
    assert!(dj.session().state().decks[0].loop_state.unwrap().roll);
    run(&mut dj, DjCommand::LoopExit { deck: 0 });
    let out = dj.render(9_600).unwrap();
    let resume = start + 48_000;
    assert_eq!(&out.decks[0][0][..], &track[resume..resume + 9_600]);
}

#[test]
fn hot_cues_land_on_their_sample() {
    let (mut dj, track) = playing(0.7, false);
    let cue = seconds_to_frames(0.7);
    run(&mut dj, DjCommand::HotCue { deck: 0, slot: 3 });
    dj.render_seconds(2.0).unwrap();
    let state = dj.session().state();
    assert_eq!(state.decks[0].hot_cues[3], Some(0.7));
    run(&mut dj, DjCommand::HotCue { deck: 0, slot: 3 });
    let out = dj.render(4_800).unwrap();
    assert_eq!(&out.decks[0][0][..], &track[cue..cue + 4_800]);
    // Quantized, a hot cue snaps to the nearest beat (0.7 s -> beat 1).
    run(&mut dj, DjCommand::SetQuantize { deck: 0, on: true });
    run(&mut dj, DjCommand::HotCue { deck: 0, slot: 0 });
    dj.render(512).unwrap();
    run(&mut dj, DjCommand::HotCue { deck: 0, slot: 0 });
    let out = dj.render(4_800).unwrap();
    let expected = if dj.session().state().decks[0].hot_cues[0] == Some(1.0) {
        48_000
    } else {
        24_000
    };
    assert_eq!(&out.decks[0][0][..], &track[expected..expected + 4_800]);
    run(&mut dj, DjCommand::ClearHotCue { deck: 0, slot: 3 });
    dj.render(256).unwrap();
    assert_eq!(dj.session().state().decks[0].hot_cues[3], None);
}

#[test]
fn cdj_cue_sets_returns_and_stops() {
    let track = noise_track(120.0, 30.0);
    let left = track_left(&track);
    let mut dj = renderer(2, &[]);
    dj.load(0, track).unwrap();
    run(
        &mut dj,
        DjCommand::Seek {
            deck: 0,
            seconds: 1.01,
        },
    );
    // Stopped: cue sets the cue point (snapped to beat 2 at 1.0 s).
    run(&mut dj, DjCommand::Cue { deck: 0 });
    run(&mut dj, DjCommand::Play { deck: 0 });
    let out = dj.render(4_800).unwrap();
    assert_eq!(&out.decks[0][0][..], &left[48_000..52_800]);
    dj.render_seconds(1.0).unwrap();
    // Playing: cue returns to the cue point and stops.
    run(&mut dj, DjCommand::Cue { deck: 0 });
    let out = dj.render(4_800).unwrap();
    assert!(out.decks[0][0].iter().all(|&s| s == 0.0));
    let state = dj.session().state();
    assert!(!state.decks[0].playing);
    assert_eq!(state.decks[0].position_seconds, 1.0);
    assert_eq!(state.decks[0].cue_seconds, 1.0);
}

#[test]
fn beat_jump_moves_by_whole_beats_with_the_loop() {
    let (mut dj, track) = playing(0.25, false);
    let at = seconds_to_frames(0.25);
    run(
        &mut dj,
        DjCommand::BeatJump {
            deck: 0,
            beats: 8.0,
        },
    );
    let out = dj.render(4_800).unwrap();
    let target = at + 8 * 24_000;
    assert_eq!(&out.decks[0][0][..], &track[target..target + 4_800]);
    run(
        &mut dj,
        DjCommand::BeatJump {
            deck: 0,
            beats: -2.0,
        },
    );
    let out = dj.render(4_800).unwrap();
    let target = target + 4_800 - 2 * 24_000;
    assert_eq!(&out.decks[0][0][..], &track[target..target + 4_800]);
    // With a loop active, the loop moves along.
    run(
        &mut dj,
        DjCommand::BeatLoop {
            deck: 0,
            beats: 2.0,
        },
    );
    dj.render(256).unwrap();
    let before = dj.session().state().decks[0].loop_state.unwrap();
    run(
        &mut dj,
        DjCommand::BeatJump {
            deck: 0,
            beats: 4.0,
        },
    );
    dj.render(256).unwrap();
    let after = dj.session().state().decks[0].loop_state.unwrap();
    assert!((after.start_seconds - before.start_seconds - 2.0).abs() < 1e-9);
}

#[test]
fn playback_stops_at_the_end_of_the_track() {
    let track = noise_track(120.0, 1.0);
    let mut dj = renderer(2, &[]);
    dj.load(0, track).unwrap();
    run(&mut dj, DjCommand::Play { deck: 0 });
    let out = dj.render(72_000).unwrap();
    assert!(out.decks[0][0][48_000..].iter().all(|&s| s == 0.0));
    let state = dj.session().state();
    assert!(state.decks[0].at_end && !state.decks[0].playing);
}
