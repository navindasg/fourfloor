//! The session API: commands in JSON, state snapshots, loading files with
//! cached analysis, stems, suggestions and error handling.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::{kick_signal, noise_track, renderer, run, seconds_to_frames, RATE};
use fourfloor_analysis::Key;
use fourfloor_dj::{
    default_registry, AnalysisCache, CrossfaderCurve, CrossfaderSide, DeckTrack, DjCommand,
    DjError, DjSession, EqBand, LibraryTrack, MatchOptions, SessionConfig, SessionState, StemFile,
    SyncMode, TrackStem,
};
use fourfloor_testkit::{null_residual_db, sine, white_noise, StereoBuffer};

fn every_command() -> Vec<DjCommand> {
    let path = PathBuf::from("/tmp/x.wav");
    vec![
        DjCommand::LoadTrack {
            deck: 0,
            path: path.clone(),
            analyze: false,
            stems: vec![StemFile {
                name: "drums".into(),
                path: path.clone(),
            }],
        },
        DjCommand::Eject { deck: 0 },
        DjCommand::Play { deck: 0 },
        DjCommand::Pause { deck: 0 },
        DjCommand::TogglePlay { deck: 0 },
        DjCommand::Cue { deck: 0 },
        DjCommand::SetCuePoint { deck: 0 },
        DjCommand::Seek {
            deck: 0,
            seconds: 1.5,
        },
        DjCommand::SetPitch {
            deck: 0,
            percent: 2.5,
        },
        DjCommand::SetKeyLock { deck: 0, on: true },
        DjCommand::SetSync {
            deck: 0,
            mode: SyncMode::Beat,
        },
        DjCommand::SetMaster { deck: None },
        DjCommand::Bend {
            deck: 0,
            percent: 3.0,
        },
        DjCommand::Nudge {
            deck: 0,
            milliseconds: -4.0,
        },
        DjCommand::SetQuantize { deck: 0, on: false },
        DjCommand::BeatLoop {
            deck: 0,
            beats: 0.25,
        },
        DjCommand::LoopRoll {
            deck: 0,
            beats: 0.5,
        },
        DjCommand::LoopExit { deck: 0 },
        DjCommand::Reloop { deck: 0 },
        DjCommand::HotCue { deck: 0, slot: 7 },
        DjCommand::ClearHotCue { deck: 0, slot: 7 },
        DjCommand::BeatJump {
            deck: 0,
            beats: -16.0,
        },
        DjCommand::SetEq {
            deck: 0,
            band: EqBand::Low,
            db: -6.0,
        },
        DjCommand::SetEqKill {
            deck: 0,
            band: EqBand::High,
            on: true,
        },
        DjCommand::SetFilter {
            deck: 0,
            amount: -0.5,
        },
        DjCommand::SetFader {
            deck: 0,
            value: 0.8,
        },
        DjCommand::SetTrim { deck: 0, db: 2.0 },
        DjCommand::SetCueMonitor { deck: 0, on: true },
        DjCommand::SetCrossfaderSide {
            deck: 0,
            side: CrossfaderSide::B,
        },
        DjCommand::SetCrossfader { position: 0.25 },
        DjCommand::SetCrossfaderCurve {
            curve: CrossfaderCurve::Sharp,
        },
        DjCommand::SetMasterGain { db: -1.0 },
        DjCommand::SetCueMix { value: 0.5 },
        DjCommand::SetHeadphoneGain { db: -3.0 },
        DjCommand::SetDeckEffect {
            deck: 0,
            slot: 0,
            on: true,
        },
        DjCommand::SetDeckEffectParam {
            deck: 0,
            slot: 0,
            param: "Mix".into(),
            value: 0.4,
        },
        DjCommand::SetStemGain {
            deck: 0,
            stem: "bass".into(),
            gain: 0.5,
        },
        DjCommand::SetStemMute {
            deck: 0,
            stem: "bass".into(),
            muted: true,
        },
        DjCommand::StartRecording { path },
        DjCommand::StopRecording,
    ]
}

#[test]
fn every_command_round_trips_through_json_under_its_stable_name() {
    let commands = every_command();
    let names: BTreeSet<&str> = commands.iter().map(DjCommand::name).collect();
    let listed: BTreeSet<&str> = DjCommand::NAMES.iter().copied().collect();
    assert_eq!(names, listed, "every variant is covered once");
    assert_eq!(commands.len(), DjCommand::NAMES.len());
    for command in &commands {
        let json = command.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["command"], command.name(), "{json}");
        assert_eq!(&DjCommand::from_json(&json).unwrap(), command, "{json}");
    }
    // Hand-written JSON, as the agent would send it.
    let parsed = DjCommand::from_json(r#"{"command":"beat_loop","deck":1,"beats":4}"#).unwrap();
    assert_eq!(
        parsed,
        DjCommand::BeatLoop {
            deck: 1,
            beats: 4.0
        }
    );
    let parsed =
        DjCommand::from_json(r#"{"command":"load_track","deck":0,"path":"a.wav"}"#).unwrap();
    assert!(matches!(parsed, DjCommand::LoadTrack { analyze: true, .. }));
    let parsed = DjCommand::from_json(r#"{"command":"set_sync","deck":1,"mode":"tempo"}"#).unwrap();
    assert_eq!(
        parsed,
        DjCommand::SetSync {
            deck: 1,
            mode: SyncMode::Tempo
        }
    );
    assert!(DjCommand::from_json(r#"{"command":"warp_drive","deck":0}"#).is_err());
    assert!(DjCommand::from_json(r#"{"command":"play","deck":0,"extra":1}"#).is_err());
}

#[test]
fn invalid_commands_are_rejected_with_typed_errors() {
    let mut dj = renderer(2, &["fx.delay"]);
    let error = |dj: &mut fourfloor_dj::DjRenderer, command: DjCommand| {
        dj.apply(&command).expect_err(command.name())
    };
    assert!(matches!(
        error(&mut dj, DjCommand::Play { deck: 4 }),
        DjError::NoSuchDeck(4)
    ));
    assert!(matches!(
        error(&mut dj, DjCommand::Play { deck: 0 }),
        DjError::NoTrack(0)
    ));
    dj.load(0, DeckTrack::from_mono(RATE, &[0.1; 48_000]).unwrap())
        .unwrap();
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::BeatLoop {
                deck: 0,
                beats: 4.0
            }
        ),
        DjError::NoGrid(0)
    ));
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::SetPitch {
                deck: 0,
                percent: 400.0
            }
        ),
        DjError::InvalidValue { .. }
    ));
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::SetFader {
                deck: 0,
                value: f32::NAN
            }
        ),
        DjError::InvalidValue { .. }
    ));
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::Seek {
                deck: 0,
                seconds: 2.0
            }
        ),
        DjError::InvalidValue { .. }
    ));
    assert!(matches!(
        error(&mut dj, DjCommand::HotCue { deck: 0, slot: 8 }),
        DjError::OutOfRange { .. }
    ));
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::SetDeckEffect {
                deck: 0,
                slot: 1,
                on: true
            }
        ),
        DjError::OutOfRange { .. }
    ));
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::SetDeckEffectParam {
                deck: 0,
                slot: 0,
                param: "Wobble".into(),
                value: 1.0
            }
        ),
        DjError::NoSuchParam { .. }
    ));
    assert!(matches!(
        error(
            &mut dj,
            DjCommand::SetStemGain {
                deck: 0,
                stem: "vocals".into(),
                gain: 1.0
            }
        ),
        DjError::NoSuchStem { .. }
    ));
    assert!(matches!(
        error(&mut dj, DjCommand::StopRecording),
        DjError::Recording(_)
    ));
    let missing = DjCommand::LoadTrack {
        deck: 0,
        path: "/definitely/not/here.wav".into(),
        analyze: true,
        stems: Vec::new(),
    };
    assert!(matches!(error(&mut dj, missing), DjError::Track(_)));

    let registry = default_registry().unwrap();
    for decks in [1, 5] {
        let config = SessionConfig::new(decks, RATE, 256);
        assert!(matches!(
            DjSession::new(&config, &registry),
            Err(DjError::DeckCount(_))
        ));
    }
    let config = SessionConfig::new(2, RATE, 256).with_deck_effects(&["builtin.sine"]);
    assert!(matches!(
        DjSession::new(&config, &registry),
        Err(DjError::NotAnEffect(_))
    ));
    let config = SessionConfig::new(2, RATE, 256).with_deck_effects(&["fx.nothing"]);
    assert!(matches!(
        DjSession::new(&config, &registry),
        Err(DjError::Effect(_))
    ));
}

#[test]
fn state_serializes_and_settings_replay_onto_a_new_session() {
    let mut dj = renderer(4, &["fx.reverb"]);
    dj.load(1, noise_track(126.0, 30.0)).unwrap();
    let commands = [
        DjCommand::SetCrossfader { position: 0.3 },
        DjCommand::SetCrossfaderCurve {
            curve: CrossfaderCurve::Linear,
        },
        DjCommand::SetCueMix { value: 0.25 },
        DjCommand::SetPitch {
            deck: 1,
            percent: -3.0,
        },
        DjCommand::SetKeyLock { deck: 1, on: true },
        DjCommand::SetEq {
            deck: 2,
            band: EqBand::Mid,
            db: -4.0,
        },
        DjCommand::SetEqKill {
            deck: 3,
            band: EqBand::Low,
            on: true,
        },
        DjCommand::SetFader {
            deck: 3,
            value: 0.6,
        },
        DjCommand::SetCrossfaderSide {
            deck: 2,
            side: CrossfaderSide::A,
        },
        DjCommand::SetDeckEffectParam {
            deck: 2,
            slot: 0,
            param: "Mix".into(),
            value: 0.9,
        },
        DjCommand::SetDeckEffect {
            deck: 2,
            slot: 0,
            on: true,
        },
        DjCommand::SetMaster { deck: Some(1) },
        DjCommand::SetSync {
            deck: 0,
            mode: SyncMode::Beat,
        },
        DjCommand::HotCue { deck: 1, slot: 2 },
        DjCommand::Play { deck: 1 },
    ];
    dj.apply_all(&commands).unwrap();
    dj.render_seconds(0.5).unwrap();
    let state = dj.session().state();
    assert_eq!(state.decks.len(), 4);
    assert_eq!(state.master, Some(1));
    let bpm = state.master_bpm.unwrap();
    assert!((bpm - 126.0 * 0.97).abs() < 1e-6, "{bpm}");
    let deck = &state.decks[1];
    assert!(deck.playing);
    assert!((deck.position_seconds - 0.5 * 0.97).abs() < 0.01);
    assert_eq!(deck.hot_cues[2], Some(0.0));
    assert!(deck.track.as_ref().unwrap().bpm.is_some());

    let json = state.to_json();
    let back: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, state);
    assert!(json.contains("\"loop\""));

    let mut other = renderer(4, &["fx.reverb"]);
    other.apply_all(&state.settings_commands()).unwrap();
    let replayed = other.session().state();
    assert_eq!(replayed.mixer, state.mixer);
    assert_eq!(replayed.master, state.master);
    for (a, b) in replayed.decks.iter().zip(&state.decks) {
        assert_eq!(a.eq_db, b.eq_db);
        assert_eq!(a.eq_kill, b.eq_kill);
        assert_eq!(a.fader, b.fader);
        assert_eq!(a.crossfader_side, b.crossfader_side);
        assert_eq!(a.effects, b.effects);
        assert_eq!(a.pitch_percent, b.pitch_percent);
        assert_eq!(a.key_lock, b.key_lock);
        assert_eq!(a.sync, b.sync);
    }
}

/// A 124 BPM track in A minor: kicks plus an i-iv-v pad, a bar per chord.
fn house_loop(seconds: f64) -> Vec<f32> {
    let kicks = kick_signal(124.0, seconds, RATE);
    let bar = 4.0 * 60.0 / 124.0;
    let chords: [[f64; 3]; 3] = [
        [220.0, 261.63, 329.63], // A C E
        [293.66, 349.23, 440.0], // D F A
        [329.63, 392.0, 493.88], // E G B
    ];
    kicks
        .iter()
        .enumerate()
        .map(|(i, kick)| {
            let t = i as f64 / RATE.as_f64();
            let chord = chords[(t / bar) as usize % 3];
            let pad: f64 = chord
                .iter()
                .map(|f| (std::f64::consts::TAU * f * t).sin())
                .sum();
            kick + (0.08 * pad) as f32
        })
        .collect()
}

#[test]
fn load_track_decodes_analyses_and_caches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("house.wav");
    StereoBuffer::from_mono(RATE, &house_loop(40.0))
        .write_wav(&path)
        .unwrap();
    let cache_dir = dir.path().join("cache");
    let registry = default_registry().unwrap();
    let config = SessionConfig::new(2, RATE, 256).with_cache_dir(&cache_dir);
    let mut dj = fourfloor_dj::DjRenderer::new(&config, &registry).unwrap();
    run(
        &mut dj,
        DjCommand::LoadTrack {
            deck: 0,
            path: path.clone(),
            analyze: true,
            stems: Vec::new(),
        },
    );
    let state = dj.session().state();
    let track = state.decks[0].track.clone().unwrap();
    assert_eq!(track.name, "house");
    assert_eq!(track.path.as_deref(), Some(path.as_path()));
    let bpm = track.bpm.expect("tempo found");
    assert!((bpm - 124.0).abs() < 0.1, "{bpm}");
    let key: Key = track.key.as_deref().expect("key found").parse().unwrap();
    assert_eq!(key, Key::minor(9), "found {key}");
    assert!((track.duration_seconds - 40.0).abs() < 1e-3);
    // The result is cached by path, size and modification time.
    let cached = AnalysisCache::new(&cache_dir).get(&path).expect("cached");
    assert_eq!(cached.bpm, Some(bpm));
    run(&mut dj, DjCommand::Eject { deck: 0 });
    run(
        &mut dj,
        DjCommand::LoadTrack {
            deck: 1,
            path: path.clone(),
            analyze: true,
            stems: Vec::new(),
        },
    );
    assert_eq!(
        dj.session().state().decks[1].track.as_ref().unwrap().bpm,
        Some(bpm)
    );
    // The analysed grid drives beat loops.
    run(&mut dj, DjCommand::Play { deck: 1 });
    dj.render_seconds(1.0).unwrap();
    run(
        &mut dj,
        DjCommand::BeatLoop {
            deck: 1,
            beats: 2.0,
        },
    );
    dj.render(256).unwrap();
    let region = dj.session().state().decks[1].loop_state.unwrap();
    let length = region.end_seconds - region.start_seconds;
    assert!((length - 2.0 * 60.0 / bpm).abs() < 1e-9);
    // Harmonic suggestions for the playing deck.
    let library = [
        LibraryTrack::new("far", "far", Some(140.0), Some(Key::major(6))),
        LibraryTrack::new("relative", "relative", Some(124.0), Some(Key::major(0))),
        LibraryTrack::new("clash", "clash", Some(124.0), Some(Key::minor(3))),
    ];
    let ranked = dj
        .session()
        .suggest(1, &library, &MatchOptions::default())
        .unwrap();
    assert_eq!(ranked[0].id, "relative");
    assert_eq!(ranked[2].id, "far");
}

#[test]
fn stems_mix_with_per_stem_gain_and_mute() {
    let seconds = 3.0;
    let bass = sine(110.0, 0.4, seconds, RATE);
    let drums = white_noise(9, 0.3, seconds, RATE);
    let mix: Vec<f32> = bass.iter().zip(&drums).map(|(a, b)| a + b).collect();
    let stems = vec![
        TrackStem::new("bass", RATE, bass.clone(), bass.clone()),
        TrackStem::new("drums", RATE, drums.clone(), drums.clone()),
    ];
    let track = DeckTrack::from_mono(RATE, &mix)
        .unwrap()
        .with_stems(stems)
        .unwrap();
    let mut dj = renderer(2, &[]);
    dj.load(0, track).unwrap();
    run(&mut dj, DjCommand::Play { deck: 0 });
    let all = dj.render_seconds(0.5).unwrap();
    assert!(null_residual_db(&all.decks[0][0], &mix[..all.frames()]) < -120.0);
    run(
        &mut dj,
        DjCommand::SetStemMute {
            deck: 0,
            stem: "drums".into(),
            muted: true,
        },
    );
    dj.render_seconds(0.5).unwrap();
    let start = seconds_to_frames(1.0);
    let muted = dj.render_seconds(0.5).unwrap();
    let expected = &bass[start..start + muted.frames()];
    assert!(null_residual_db(&muted.decks[0][0], expected) < -120.0);
    run(
        &mut dj,
        DjCommand::SetStemMute {
            deck: 0,
            stem: "drums".into(),
            muted: false,
        },
    );
    run(
        &mut dj,
        DjCommand::SetStemGain {
            deck: 0,
            stem: "bass".into(),
            gain: 0.5,
        },
    );
    dj.render_seconds(0.5).unwrap();
    let start = seconds_to_frames(2.0);
    let half = dj.render_seconds(0.5).unwrap();
    let expected: Vec<f32> = (start..start + half.frames())
        .map(|i| 0.5 * bass[i] + drums[i])
        .collect();
    assert!(null_residual_db(&half.decks[0][0], &expected) < -120.0);
    let stems = &dj.session().state().decks[0].stems;
    assert_eq!(stems[0].gain, 0.5);
    assert!(!stems[1].muted);
}

#[test]
fn stems_load_from_files_with_the_track() {
    let dir = tempfile::tempdir().unwrap();
    let bass = sine(110.0, 0.4, 2.0, RATE);
    let drums = white_noise(4, 0.3, 2.0, RATE);
    let mix: Vec<f32> = bass.iter().zip(&drums).map(|(a, b)| a + b).collect();
    let write = |name: &str, signal: &[f32]| {
        let path = dir.path().join(name);
        StereoBuffer::from_mono(RATE, signal)
            .write_wav(&path)
            .unwrap();
        path
    };
    let command = DjCommand::LoadTrack {
        deck: 1,
        path: write("mix.wav", &mix),
        analyze: false,
        stems: vec![
            StemFile {
                name: "bass".into(),
                path: write("bass.wav", &bass),
            },
            StemFile {
                name: "drums".into(),
                path: write("drums.wav", &drums),
            },
        ],
    };
    let mut dj = renderer(2, &[]);
    run(&mut dj, command);
    run(
        &mut dj,
        DjCommand::SetStemMute {
            deck: 1,
            stem: "bass".into(),
            muted: true,
        },
    );
    run(&mut dj, DjCommand::Play { deck: 1 });
    dj.render_seconds(0.5).unwrap();
    let out = dj.render_seconds(0.5).unwrap();
    let start = seconds_to_frames(0.5);
    assert!(null_residual_db(&out.decks[1][0], &drums[start..start + out.frames()]) < -120.0);
    let names: Vec<String> = dj.session().state().decks[1]
        .stems
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, ["bass", "drums"]);
}

#[test]
fn toggle_play_eject_and_auto_master() {
    let mut dj = renderer(2, &[]);
    dj.load(0, noise_track(122.0, 10.0)).unwrap();
    dj.load(1, noise_track(128.0, 10.0)).unwrap();
    run(&mut dj, DjCommand::TogglePlay { deck: 0 });
    dj.render(256).unwrap();
    assert!(dj.session().state().decks[0].playing);
    // Syncing deck 2 with no master picks the playing deck.
    run(
        &mut dj,
        DjCommand::SetSync {
            deck: 1,
            mode: SyncMode::Tempo,
        },
    );
    assert_eq!(dj.session().master(), Some(0));
    run(&mut dj, DjCommand::TogglePlay { deck: 0 });
    dj.render(256).unwrap();
    assert!(!dj.session().state().decks[0].playing);
    run(&mut dj, DjCommand::Eject { deck: 0 });
    dj.render(256).unwrap();
    let state = dj.session().state();
    assert_eq!(state.master, None);
    assert!(state.decks[0].track.is_none());
    // The ejected track was handed back and freed off the audio thread.
    assert_eq!(dj.session().state().decks[0].position_seconds, 0.0);
}
