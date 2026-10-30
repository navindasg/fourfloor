//! Every audio-thread path of a DJ session under the allocation guard.
//!
//! This binary installs `AllocDisabler` as its global allocator and runs
//! each graph block inside `assert_no_alloc`, so any allocation or free in
//! a deck, the mixer, the strips, the effect slots or the recorder aborts
//! the test. Commands and pumping run between blocks, on the control side.

mod common;

use assert_no_alloc::assert_no_alloc;
use common::{kick_track, noise_track, tone_pulses, BLOCK, RATE};
use fourfloor_core::{Tempo, TransportContext};
use fourfloor_dj::{
    default_registry, CrossfaderCurve, DjCommand, DjSession, EqBand, SessionConfig, SyncMode,
};
use fourfloor_testkit::{is_silent, AllocDisabler};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

/// Commands applied in turn, one every few blocks, while the decks play.
fn script(path: std::path::PathBuf) -> Vec<DjCommand> {
    vec![
        DjCommand::SetMaster { deck: Some(0) },
        DjCommand::Play { deck: 0 },
        DjCommand::Play { deck: 1 },
        DjCommand::SetSync {
            deck: 1,
            mode: SyncMode::Beat,
        },
        DjCommand::SetKeyLock { deck: 1, on: true },
        DjCommand::StartRecording { path },
        DjCommand::SetPitch {
            deck: 0,
            percent: 3.0,
        },
        DjCommand::SetDeckEffect {
            deck: 0,
            slot: 0,
            on: true,
        },
        DjCommand::SetDeckEffect {
            deck: 1,
            slot: 1,
            on: true,
        },
        DjCommand::BeatLoop {
            deck: 0,
            beats: 4.0,
        },
        DjCommand::HotCue { deck: 1, slot: 0 },
        DjCommand::LoopRoll {
            deck: 1,
            beats: 0.25,
        },
        DjCommand::SetEqKill {
            deck: 0,
            band: EqBand::Low,
            on: true,
        },
        DjCommand::SetFilter {
            deck: 1,
            amount: -0.6,
        },
        DjCommand::SetCrossfaderCurve {
            curve: CrossfaderCurve::Sharp,
        },
        DjCommand::SetCrossfader { position: 0.3 },
        DjCommand::SetCueMonitor { deck: 1, on: true },
        DjCommand::LoopExit { deck: 1 },
        DjCommand::Nudge {
            deck: 1,
            milliseconds: 15.0,
        },
        DjCommand::Bend {
            deck: 1,
            percent: 4.0,
        },
        DjCommand::Bend {
            deck: 1,
            percent: 0.0,
        },
        DjCommand::BeatJump {
            deck: 0,
            beats: -8.0,
        },
        DjCommand::HotCue { deck: 1, slot: 0 },
        DjCommand::SetKeyLock { deck: 1, on: false },
        DjCommand::Cue { deck: 2 },
        DjCommand::Play { deck: 2 },
        DjCommand::SetSync {
            deck: 2,
            mode: SyncMode::Tempo,
        },
        DjCommand::Eject { deck: 0 },
        DjCommand::StopRecording,
    ]
}

#[test]
fn a_busy_session_never_allocates_on_the_audio_thread() {
    let dir = tempfile::tempdir().unwrap();
    let registry = default_registry().unwrap();
    let config = SessionConfig::new(3, RATE, BLOCK).with_deck_effects(&["fx.delay", "fx.reverb"]);
    let (mut session, mut graph) = DjSession::new(&config, &registry).unwrap();
    session.load_track(0, kick_track(120.0, 30.0)).unwrap();
    session
        .load_track(1, tone_pulses(330.0, 126.0, 30.0))
        .unwrap();
    session.load_track(2, noise_track(124.0, 30.0)).unwrap();
    let commands = script(dir.path().join("mix.wav"));
    let mut position = 0u64;
    let mut heard = false;
    let mut recordings = 0;
    for (block, command) in (0..2_000).map(|i| (i, commands.get(i / 40))) {
        if block % 40 == 0 {
            if let Some(command) = command {
                session.apply(command, &mut graph).unwrap();
            }
        }
        let tempo = session.tempo().unwrap_or(Tempo::DEFAULT);
        let ctx = TransportContext::playing_at(RATE, tempo, position);
        assert_no_alloc(|| graph.process(&ctx, BLOCK));
        heard |= graph.output(0).is_some_and(|out| !is_silent(out, -60.0));
        if session.pump().unwrap().is_some() {
            recordings += 1;
        }
        position += BLOCK as u64;
    }
    assert!(heard, "the session made no sound");
    assert_eq!(recordings, 1);
    assert!(dir.path().join("mix.wav").exists());
}
