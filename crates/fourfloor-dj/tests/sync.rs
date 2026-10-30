//! Tempo master and beat sync, measured on rendered audio.

mod common;

use common::{kick_track, renderer, run, seconds_to_frames, RATE};
use fourfloor_dj::{DjCommand, SyncMode};
use fourfloor_testkit::onset_times;

/// Seconds of the 120 BPM master in `bars` bars.
fn bars(count: f64) -> f64 {
    count * 4.0 * 0.5
}

/// Largest distance from each onset in `follower` (after `from` seconds)
/// to the nearest onset in `master`, and how many were compared.
fn worst_alignment(master: &[f64], follower: &[f64], from: f64) -> (f64, usize) {
    let mut worst = 0.0f64;
    let mut count = 0;
    for &time in follower.iter().filter(|&&t| t >= from) {
        let nearest = master
            .iter()
            .map(|&m| (m - time).abs())
            .fold(f64::INFINITY, f64::min);
        worst = worst.max(nearest);
        count += 1;
    }
    (worst, count)
}

fn synced_pair(key_lock: bool) -> (Vec<f64>, Vec<f64>) {
    let mut dj = renderer(2, &[]);
    dj.load(0, kick_track(120.0, 90.0)).unwrap();
    dj.load(1, kick_track(126.0, 90.0)).unwrap();
    run(&mut dj, DjCommand::SetMaster { deck: Some(0) });
    run(&mut dj, DjCommand::Play { deck: 0 });
    // The follower starts late and off the beat: 1.37 s in.
    dj.render_seconds(1.37).unwrap();
    run(
        &mut dj,
        DjCommand::SetKeyLock {
            deck: 1,
            on: key_lock,
        },
    );
    run(
        &mut dj,
        DjCommand::SetSync {
            deck: 1,
            mode: SyncMode::Beat,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 1 });
    let out = dj.render_seconds(bars(36.0)).unwrap();
    let state = dj.session().state();
    let bpm = state.decks[1].bpm.unwrap();
    assert!((bpm - 120.0).abs() < 0.1, "follower sounds at {bpm} BPM");
    assert!((state.decks[1].rate - 120.0 / 126.0).abs() < 0.01);
    (
        onset_times(&out.decks[0][0], RATE),
        onset_times(&out.decks[1][0], RATE),
    )
}

#[test]
fn beat_sync_aligns_126_bpm_onto_120_bpm_for_32_bars() {
    let (master, follower) = synced_pair(false);
    // Skip the first bar after the follower starts, then 32 bars.
    let (worst, count) = worst_alignment(&master, &follower, 2.0 + bars(1.0));
    assert!(count >= 32 * 4, "only {count} follower onsets");
    assert!(worst < 0.002, "worst misalignment {:.2} ms", worst * 1e3);
}

#[test]
fn beat_sync_with_key_lock_stays_aligned() {
    let (master, follower) = synced_pair(true);
    let (worst, count) = worst_alignment(&master, &follower, 2.0 + bars(1.0));
    assert!(count >= 32 * 4, "only {count} follower onsets");
    // The stretcher moves grains by up to a few milliseconds.
    assert!(worst < 0.008, "worst misalignment {:.2} ms", worst * 1e3);
}

#[test]
fn tempo_sync_matches_tempo_but_keeps_the_offset() {
    let mut dj = renderer(2, &[]);
    dj.load(0, kick_track(120.0, 40.0)).unwrap();
    dj.load(1, kick_track(126.0, 40.0)).unwrap();
    run(&mut dj, DjCommand::SetMaster { deck: Some(0) });
    run(&mut dj, DjCommand::Play { deck: 0 });
    dj.render_seconds(0.1).unwrap();
    run(
        &mut dj,
        DjCommand::SetSync {
            deck: 1,
            mode: SyncMode::Tempo,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 1 });
    let out = dj.render_seconds(20.0).unwrap();
    let follower = onset_times(&out.decks[1][0], RATE);
    let gaps: Vec<f64> = follower.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.len() > 30);
    for gap in &gaps {
        assert!((gap - 0.5).abs() < 0.0015, "beat gap {gap}");
    }
    // Started 0.1 s late, it stays 0.1 s late.
    let master = onset_times(&out.decks[0][0], RATE);
    let offset = follower[10]
        - master
            .iter()
            .copied()
            .find(|m| *m > follower[10] - 0.4)
            .unwrap();
    assert!((offset - 0.1).abs() < 0.003, "offset {offset}");
}

#[test]
fn unsynced_deck_follows_its_pitch_fader() {
    let mut dj = renderer(2, &[]);
    dj.load(0, kick_track(120.0, 20.0)).unwrap();
    run(
        &mut dj,
        DjCommand::SetPitch {
            deck: 0,
            percent: 5.0,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 0 });
    let out = dj.render_seconds(10.0).unwrap();
    let onsets = onset_times(&out.decks[0][0], RATE);
    let gap = (onsets[onsets.len() - 1] - onsets[0]) / (onsets.len() - 1) as f64;
    assert!((gap - 0.5 / 1.05).abs() < 0.0005, "beat gap {gap}");
    let bpm = dj.session().state().decks[0].bpm.unwrap();
    assert!((bpm - 126.0).abs() < 1e-6);
}

#[test]
fn nudge_while_beat_synced_moves_the_phase_and_it_holds() {
    let mut dj = renderer(2, &[]);
    dj.load(0, kick_track(124.0, 40.0)).unwrap();
    dj.load(1, kick_track(124.0, 40.0)).unwrap();
    run(&mut dj, DjCommand::SetMaster { deck: Some(0) });
    for deck in [0, 1] {
        run(
            &mut dj,
            DjCommand::SetSync {
                deck,
                mode: SyncMode::Beat,
            },
        );
        run(&mut dj, DjCommand::Play { deck });
    }
    dj.render_seconds(2.0).unwrap();
    run(
        &mut dj,
        DjCommand::Nudge {
            deck: 1,
            milliseconds: 20.0,
        },
    );
    let out = dj.render_seconds(10.0).unwrap();
    let master = onset_times(&out.decks[0][0], RATE);
    let follower = onset_times(&out.decks[1][0], RATE);
    // Nudged 20 ms forward, it plays its beats 20 ms early, and the sync
    // does not pull it back.
    for &time in follower.iter().filter(|&&t| t > 1.0) {
        let next = master.iter().copied().find(|&m| m > time - 0.1).unwrap();
        assert!(
            ((next - time) - 0.020).abs() < 0.002,
            "ahead by {}",
            next - time
        );
    }
    assert!(follower.len() > 15);
    let _ = seconds_to_frames(0.0);
}
