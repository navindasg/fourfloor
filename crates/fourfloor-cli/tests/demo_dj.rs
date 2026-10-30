//! DJ mode on two tracks fourfloor rendered itself: the 124 BPM demo and a
//! 128 BPM house pattern from `fourfloor new`. Their grids come from the
//! analysis crate, deck B beat-syncs to deck A, and the kicks of the two
//! deck outputs are compared.

mod demo_common;

use demo_common::*;
use fourfloor_analysis::{estimate_tempo, TempoOptions};
use fourfloor_cli::args::NewOptions;
use fourfloor_cli::commands::build_house_project;
use fourfloor_dj::{default_registry, DeckTrack, DjCommand, DjRenderer, SessionConfig, SyncMode};
use fourfloor_testkit::{onset_times, StereoBuffer};

/// A deck track with the tempo and first beat the analysis finds.
fn analysed(audio: &StereoBuffer, name: &str) -> (DeckTrack, f64) {
    let estimate =
        estimate_tempo(&audio.mono(), RATE, &TempoOptions::default()).expect("tempo found");
    let track = DeckTrack::new(RATE, audio.left.clone(), audio.right.clone())
        .expect("deck track")
        .with_name(name)
        .with_bpm(estimate.bpm(), estimate.grid.first_beat_seconds)
        .expect("grid");
    eprintln!(
        "{name}: {:.3} BPM, first beat {:.4} s",
        estimate.bpm(),
        estimate.grid.first_beat_seconds
    );
    (track, estimate.bpm())
}

/// Kick times in a deck output: onsets of the band below 90 Hz.
fn kicks(deck: &[f32]) -> Vec<f64> {
    onset_times(&low_pass(deck, 90.0), RATE)
}

fn run(dj: &mut DjRenderer, command: DjCommand) {
    dj.apply(&command).expect("command applies");
}

#[test]
fn a_synced_128_bpm_track_lands_on_the_demos_beats() {
    let (demo_track, demo_bpm) = analysed(demo_mix(), "demo");
    let pattern = build_house_project(&NewOptions {
        dir: "pattern".into(),
        name: None,
        bpm: 128.0,
        bars: 16,
        swing: None,
    })
    .expect("pattern");
    let (pattern_track, pattern_bpm) = analysed(&render(&pattern), "pattern");
    eprintln!("analysed: demo {demo_bpm:.3} BPM, pattern {pattern_bpm:.3} BPM");
    assert!((demo_bpm - 124.0).abs() < 0.1 && (pattern_bpm - 128.0).abs() < 0.1);

    let registry = default_registry().expect("registry");
    let mut dj = DjRenderer::new(&SessionConfig::new(2, RATE, 256), &registry).expect("DJ session");
    dj.load(0, demo_track).expect("load A");
    dj.load(1, pattern_track).expect("load B");
    run(&mut dj, DjCommand::SetMaster { deck: Some(0) });
    run(&mut dj, DjCommand::Play { deck: 0 });
    // Deck B starts 1.37 s later, off the beat, then syncs.
    dj.render_seconds(1.37).expect("render");
    run(
        &mut dj,
        DjCommand::SetSync {
            deck: 1,
            mode: SyncMode::Beat,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 1 });
    // Stay inside the demo's intro, where the kick plays on every beat.
    let out = dj.render_seconds(12.0).expect("render");
    let follower_bpm = dj.session().state().decks[1].bpm.expect("deck B tempo");
    assert!(
        (follower_bpm - 124.0).abs() < 0.1,
        "deck B at {follower_bpm}"
    );

    let master = kicks(&out.decks[0][0]);
    let follower = kicks(&out.decks[1][0]);
    let settled: Vec<f64> = follower.into_iter().filter(|&t| t > 1.0).collect();
    assert!(
        settled.len() >= 20,
        "only {} kicks on deck B",
        settled.len()
    );
    let worst = settled
        .iter()
        .map(|&time| {
            master
                .iter()
                .map(|&m| (m - time).abs())
                .fold(f64::INFINITY, f64::min)
        })
        .fold(0.0f64, f64::max);
    eprintln!(
        "{} deck B kicks, worst distance to a deck A kick {:.2} ms",
        settled.len(),
        worst * 1e3
    );
    assert!(worst < 0.005, "kicks drift apart by {:.2} ms", worst * 1e3);
}
