//! The DJ view's service path, measured on rendered audio: decks load and
//! analyse files on a worker, play through the app's audio callback, and
//! the pitch fader, key lock, sync, crossfader, EQ, filter, loops, beat
//! jump and recording do what they say.

mod dj_common;

use dj_common::{deck, dj_app, load, render, send, temp_dir, write_track, RATE};
use fourfloor_dj::{CrossfaderSide, DjCommand, EqBand, SyncMode};
use fourfloor_testkit::{dominant_frequency, rms_db, spectrum_band_energy_db, StereoBuffer};

fn within(found: f64, expected: f64, tolerance: f64) -> bool {
    (found - expected).abs() <= expected * tolerance
}

fn pitch_of(buffer: &StereoBuffer) -> f64 {
    f64::from(dominant_frequency(&buffer.mono(), RATE))
}

/// Deck A on side A, deck B on side B, crossfader at `position`.
fn sides(position: f32) -> Vec<DjCommand> {
    vec![
        DjCommand::SetCrossfaderSide {
            deck: 0,
            side: CrossfaderSide::A,
        },
        DjCommand::SetCrossfaderSide {
            deck: 1,
            side: CrossfaderSide::B,
        },
        DjCommand::SetCrossfader { position },
    ]
}

#[test]
fn a_loaded_deck_is_analysed_and_plays_its_pitch_with_and_without_key_lock() {
    let (_guard, dir) = temp_dir();
    let path = write_track(&dir, "tone-124", 124.0, 1000.0, 20.0);
    let mut app = dj_app(&dir.join("features"));
    load(&mut app, 0, &path);
    let state = deck(&app, 0);
    let bpm = state.bpm.expect("tempo found");
    assert!(within(bpm, 124.0, 0.002), "analysed {bpm} BPM");
    assert!((state.track.expect("track").duration_seconds - 20.0).abs() < 0.01);

    // Silent until played.
    let quiet = render(&mut app, 0.3);
    assert!(rms_db(&quiet.mono()) < -90.0, "nothing plays before Play");
    send(&mut app, &sides(-1.0));
    send(&mut app, &[DjCommand::Play { deck: 0 }]);
    let plain = render(&mut app, 1.5).slice_seconds(0.5, 1.5);
    assert!(
        within(pitch_of(&plain), 1000.0, 0.01),
        "{}",
        pitch_of(&plain)
    );
    assert!(deck(&app, 0).playing);

    // +8 % without key lock raises the tone by 8 %.
    send(
        &mut app,
        &[DjCommand::SetPitch {
            deck: 0,
            percent: 8.0,
        }],
    );
    let fast = render(&mut app, 1.5).slice_seconds(0.5, 1.5);
    assert!(within(pitch_of(&fast), 1080.0, 0.01), "{}", pitch_of(&fast));
    let state = deck(&app, 0);
    assert!(within(state.bpm.expect("bpm"), 124.0 * 1.08, 0.002));

    // Key lock keeps the pitch at the new tempo.
    send(&mut app, &[DjCommand::SetKeyLock { deck: 0, on: true }]);
    let locked = render(&mut app, 2.0).slice_seconds(0.8, 2.0);
    assert!(
        within(pitch_of(&locked), 1000.0, 0.015),
        "{}",
        pitch_of(&locked)
    );

    // Pause stops the sound and the playhead.
    send(&mut app, &[DjCommand::Pause { deck: 0 }]);
    let at = deck(&app, 0).position_seconds;
    let paused = render(&mut app, 0.5).slice_seconds(0.2, 0.5);
    assert!(rms_db(&paused.mono()) < -80.0);
    assert!((deck(&app, 0).position_seconds - at).abs() < 0.03);
}

#[test]
fn beat_sync_brings_the_second_deck_to_the_masters_tempo() {
    let (_guard, dir) = temp_dir();
    let a = write_track(&dir, "a-120", 120.0, 500.0, 20.0);
    let b = write_track(&dir, "b-126", 126.0, 1000.0, 20.0);
    let mut app = dj_app(&dir.join("features"));
    load(&mut app, 0, &a);
    load(&mut app, 1, &b);
    let bpm_a = deck(&app, 0).bpm.expect("A tempo");
    let bpm_b = deck(&app, 1).bpm.expect("B tempo");
    send(&mut app, &sides(1.0));
    send(
        &mut app,
        &[
            DjCommand::SetMaster { deck: Some(0) },
            DjCommand::Play { deck: 0 },
            DjCommand::SetSync {
                deck: 1,
                mode: SyncMode::Beat,
            },
            DjCommand::Play { deck: 1 },
        ],
    );
    let heard = render(&mut app, 2.0).slice_seconds(1.0, 2.0);
    let synced = deck(&app, 1).bpm.expect("B tempo");
    assert!(
        (synced - bpm_a).abs() < 0.05,
        "B at {synced}, master {bpm_a}"
    );
    // Only B is heard (crossfader on B), slowed by the tempo ratio.
    let expected = 1000.0 * bpm_a / bpm_b;
    assert!(
        within(pitch_of(&heard), expected, 0.01),
        "{}",
        pitch_of(&heard)
    );
    let tone_a = spectrum_band_energy_db(&heard.mono(), RATE, 450.0, 550.0);
    let tone_b = spectrum_band_energy_db(&heard.mono(), RATE, 900.0, 1000.0);
    assert!(
        tone_b - tone_a > 30.0,
        "crossfader keeps A out: {tone_a} vs {tone_b}"
    );

    // At the centre both are heard.
    send(&mut app, &[DjCommand::SetCrossfader { position: 0.0 }]);
    let both = render(&mut app, 1.0).slice_seconds(0.3, 1.0);
    let tone_a = spectrum_band_energy_db(&both.mono(), RATE, 450.0, 550.0);
    let tone_b = spectrum_band_energy_db(&both.mono(), RATE, 900.0, 1000.0);
    assert!((tone_a - tone_b).abs() < 3.0, "{tone_a} vs {tone_b}");
}

#[test]
fn eq_kill_filter_and_fader_cut_what_they_should() {
    let (_guard, dir) = temp_dir();
    let path = write_track(&dir, "tone", 124.0, 1000.0, 20.0);
    let mut app = dj_app(&dir.join("features"));
    load(&mut app, 0, &path);
    send(&mut app, &sides(-1.0));
    send(&mut app, &[DjCommand::Play { deck: 0 }]);
    let band = |b: &StereoBuffer| spectrum_band_energy_db(&b.mono(), RATE, 950.0, 1050.0);
    let open = band(&render(&mut app, 1.0).slice_seconds(0.4, 1.0));
    let kill = DjCommand::SetEqKill {
        deck: 0,
        band: EqBand::Mid,
        on: true,
    };
    send(&mut app, &[kill]);
    let killed = band(&render(&mut app, 1.0).slice_seconds(0.4, 1.0));
    assert!(open - killed > 25.0, "mid kill: {open} -> {killed} dB");
    let restore = DjCommand::SetEqKill {
        deck: 0,
        band: EqBand::Mid,
        on: false,
    };
    send(&mut app, &[restore]);
    let back = band(&render(&mut app, 1.0).slice_seconds(0.4, 1.0));
    assert!((open - back).abs() < 1.0, "{open} vs {back}");
    send(
        &mut app,
        &[DjCommand::SetFilter {
            deck: 0,
            amount: -1.0,
        }],
    );
    let low_passed = band(&render(&mut app, 1.0).slice_seconds(0.5, 1.0));
    assert!(
        open - low_passed > 20.0,
        "filter: {open} -> {low_passed} dB"
    );
    send(
        &mut app,
        &[
            DjCommand::SetFilter {
                deck: 0,
                amount: 0.0,
            },
            DjCommand::SetFader {
                deck: 0,
                value: 0.0,
            },
        ],
    );
    let closed = render(&mut app, 1.0).slice_seconds(0.5, 1.0);
    assert!(rms_db(&closed.mono()) < -80.0, "fader closed");
}

#[test]
fn loops_hot_cues_and_beat_jumps_move_the_playhead() {
    let (_guard, dir) = temp_dir();
    let path = write_track(&dir, "loop", 120.0, 1000.0, 30.0);
    let mut app = dj_app(&dir.join("features"));
    load(&mut app, 0, &path);
    send(&mut app, &[DjCommand::Play { deck: 0 }]);
    render(&mut app, 1.0);
    send(
        &mut app,
        &[DjCommand::BeatLoop {
            deck: 0,
            beats: 2.0,
        }],
    );
    // Deck commands take effect in the next audio block.
    render(&mut app, 0.05);
    let active = deck(&app, 0).loop_state.expect("loop on");
    assert!((active.end_seconds - active.start_seconds - 1.0).abs() < 0.01);
    render(&mut app, 3.3);
    let at = deck(&app, 0).position_seconds;
    assert!(
        at >= active.start_seconds - 0.01 && at <= active.end_seconds + 0.03,
        "{at} inside {active:?}"
    );
    send(&mut app, &[DjCommand::LoopExit { deck: 0 }]);
    render(&mut app, 0.05);
    assert!(deck(&app, 0).loop_state.is_none());
    send(&mut app, &[DjCommand::HotCue { deck: 0, slot: 0 }]);
    render(&mut app, 0.05);
    let cue = deck(&app, 0).hot_cues[0].expect("cue set");
    render(&mut app, 1.0);
    let before = deck(&app, 0).position_seconds;
    send(
        &mut app,
        &[DjCommand::BeatJump {
            deck: 0,
            beats: 16.0,
        }],
    );
    render(&mut app, 0.05);
    let jumped = deck(&app, 0).position_seconds - before;
    assert!(
        (jumped - 8.0).abs() < 0.1,
        "16 beats at 120 BPM: jumped {jumped}"
    );
    send(&mut app, &[DjCommand::HotCue { deck: 0, slot: 0 }]);
    render(&mut app, 0.02);
    assert!((deck(&app, 0).position_seconds - cue).abs() < 0.05);
}

#[test]
fn the_record_button_writes_the_mix_to_a_wav() {
    let (_guard, dir) = temp_dir();
    let path = write_track(&dir, "rec", 124.0, 1000.0, 20.0);
    let mut app = dj_app(&dir.join("features"));
    load(&mut app, 0, &path);
    let out = dir.join("mix.wav");
    app.push_intent(fourfloor_app::state::DjIntent::Record(out.clone()).into());
    app.flush();
    assert_eq!(
        app.features
            .dj
            .as_ref()
            .and_then(|dj| dj.state().recording.clone()),
        Some(out.clone())
    );
    send(&mut app, &sides(-1.0));
    send(&mut app, &[DjCommand::Play { deck: 0 }]);
    render(&mut app, 2.0);
    send(&mut app, &[DjCommand::StopRecording]);
    render(&mut app, 0.2);
    let dj = app.features.dj.as_ref().expect("dj");
    assert_eq!(dj.recordings.len(), 1, "one recording closed");
    let written = StereoBuffer::read_wav(&out).expect("the WAV reads back");
    assert!(
        (written.seconds() - 2.0).abs() < 0.25,
        "{} s",
        written.seconds()
    );
    let tail = written.slice_seconds(0.5, written.seconds() - 0.1);
    assert!(within(pitch_of(&tail), 1000.0, 0.01));
    assert!(rms_db(&tail.mono()) > -30.0);
}
