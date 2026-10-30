//! The DJ mixer, isolator, filter, cue bus, deck effects and mix
//! recording, measured on rendered audio.

mod common;

use common::{noise_track, renderer, run, seconds_to_frames, RATE};
use fourfloor_dj::{
    crossfader_gains, CrossfaderCurve, CrossfaderSide, DeckTrack, DjCommand, DjRenderer, EqBand,
};
use fourfloor_testkit::{
    impulse, null_residual_db, rms, sine, spectrum_band_energy_db, StereoBuffer,
};

fn db(ratio: f32) -> f32 {
    20.0 * ratio.log10()
}

fn sine_track(frequency: f64, seconds: f64) -> DeckTrack {
    DeckTrack::from_mono(RATE, &sine(frequency, 0.5, seconds, RATE)).unwrap()
}

/// 60 Hz, 790 Hz (the geometric centre of the isolator mid band) and
/// 8 kHz together.
fn three_band_track(seconds: f64) -> DeckTrack {
    let low = sine(60.0, 0.3, seconds, RATE);
    let mid = sine(790.0, 0.3, seconds, RATE);
    let high = sine(8_000.0, 0.3, seconds, RATE);
    let mix: Vec<f32> = (0..low.len()).map(|i| low[i] + mid[i] + high[i]).collect();
    DeckTrack::from_mono(RATE, &mix).unwrap()
}

/// Renders `seconds` and returns the second half of every output, so
/// gain glides and filter transients are over.
fn settled(dj: &mut DjRenderer, seconds: f64) -> fourfloor_dj::DjOutput {
    let mut out = dj.render_seconds(seconds).unwrap();
    let half = out.frames() / 2;
    let cut = |stereo: &mut [Vec<f32>; 2]| {
        for channel in stereo.iter_mut() {
            channel.drain(..half);
        }
    };
    cut(&mut out.master);
    cut(&mut out.cue);
    out.decks.iter_mut().for_each(cut);
    out.strips.iter_mut().for_each(cut);
    out
}

#[test]
fn crossfader_curves_give_their_gains() {
    for curve in [
        CrossfaderCurve::Linear,
        CrossfaderCurve::ConstantPower,
        CrossfaderCurve::Sharp,
    ] {
        for position in [-1.0f32, -0.5, 0.0, 0.5, 0.97, 1.0] {
            for deck in [0usize, 1] {
                let mut dj = renderer(2, &[]);
                dj.load(deck, sine_track(1_000.0, 2.0)).unwrap();
                run(&mut dj, DjCommand::SetCrossfaderCurve { curve });
                run(&mut dj, DjCommand::SetCrossfader { position });
                run(&mut dj, DjCommand::Play { deck });
                let out = settled(&mut dj, 0.4);
                let (a, b) = crossfader_gains(position, curve);
                let expected = if deck == 0 { a } else { b };
                let measured = rms(&out.master[0]) / rms(&out.strips[deck][0]);
                assert!(
                    (measured - expected).abs() < 0.002,
                    "{curve:?} at {position}, deck {deck}: {measured} vs {expected}"
                );
            }
        }
    }
    // The documented centres.
    let (a, _) = crossfader_gains(0.0, CrossfaderCurve::ConstantPower);
    assert!((db(a) + 3.01).abs() < 0.02);
    let (a, _) = crossfader_gains(0.0, CrossfaderCurve::Linear);
    assert!((db(a) + 6.02).abs() < 0.02);
}

#[test]
fn thru_decks_ignore_the_crossfader() {
    let mut dj = renderer(3, &[]);
    dj.load(2, sine_track(500.0, 2.0)).unwrap();
    run(&mut dj, DjCommand::SetCrossfader { position: -1.0 });
    run(&mut dj, DjCommand::Play { deck: 2 });
    let out = settled(&mut dj, 0.4);
    let ratio = rms(&out.master[0]) / rms(&out.strips[2][0]);
    assert!((ratio - 1.0).abs() < 1e-3, "{ratio}");
    run(
        &mut dj,
        DjCommand::SetCrossfaderSide {
            deck: 2,
            side: CrossfaderSide::B,
        },
    );
    let out = settled(&mut dj, 0.4);
    assert!(rms(&out.master[0]) < 1e-4);
}

#[test]
fn fader_and_trim_set_the_channel_level() {
    let mut dj = renderer(2, &[]);
    dj.load(0, sine_track(1_000.0, 3.0)).unwrap();
    run(&mut dj, DjCommand::SetCrossfader { position: -1.0 });
    run(&mut dj, DjCommand::Play { deck: 0 });
    run(
        &mut dj,
        DjCommand::SetFader {
            deck: 0,
            value: 0.5,
        },
    );
    let out = settled(&mut dj, 0.4);
    let gain = db(rms(&out.master[0]) / rms(&out.strips[0][0]));
    assert!((gain + 12.04).abs() < 0.05, "fader at 0.5: {gain} dB");
    run(&mut dj, DjCommand::SetTrim { deck: 0, db: 6.0 });
    run(&mut dj, DjCommand::SetMasterGain { db: -3.0 });
    let out = settled(&mut dj, 0.4);
    let gain = db(rms(&out.master[0]) / rms(&out.strips[0][0]));
    assert!(
        (gain + 9.04).abs() < 0.05,
        "with trim and master: {gain} dB"
    );
}

fn bands(signal: &[f32]) -> [f64; 3] {
    [
        spectrum_band_energy_db(signal, RATE, 40.0, 100.0),
        spectrum_band_energy_db(signal, RATE, 600.0, 1_000.0),
        spectrum_band_energy_db(signal, RATE, 6_000.0, 10_000.0),
    ]
}

#[test]
fn eq_kill_removes_its_band_and_leaves_the_others() {
    let reference = {
        let mut dj = renderer(2, &[]);
        dj.load(0, three_band_track(3.0)).unwrap();
        run(&mut dj, DjCommand::Play { deck: 0 });
        bands(&settled(&mut dj, 1.0).strips[0][0])
    };
    for band in EqBand::ALL {
        let mut dj = renderer(2, &[]);
        dj.load(0, three_band_track(3.0)).unwrap();
        run(
            &mut dj,
            DjCommand::SetEqKill {
                deck: 0,
                band,
                on: true,
            },
        );
        run(&mut dj, DjCommand::Play { deck: 0 });
        let killed = bands(&settled(&mut dj, 1.0).strips[0][0]);
        for other in EqBand::ALL {
            let drop = reference[other.index()] - killed[other.index()];
            if other == band {
                // The 24 dB/octave crossovers bound how far a tone inside
                // a band falls when that band is killed: the mid tone is
                // 1.66 octaves from either crossover, and both neighbours
                // leak about -40 dB of it (34 dB measured).
                let needed = if band == EqBand::Mid { 30.0 } else { 40.0 };
                assert!(drop > needed, "{band:?} kill: only {drop:.1} dB down");
            } else {
                assert!(
                    drop.abs() < 1.0,
                    "{band:?} kill moved {other:?} by {drop:.2} dB"
                );
            }
        }
        // Releasing the kill brings the band back at its gain.
        run(
            &mut dj,
            DjCommand::SetEqKill {
                deck: 0,
                band,
                on: false,
            },
        );
        let back = bands(&settled(&mut dj, 1.0).strips[0][0]);
        assert!((back[band.index()] - reference[band.index()]).abs() < 0.5);
    }
}

#[test]
fn eq_band_gain_moves_its_band() {
    let mut dj = renderer(2, &[]);
    dj.load(0, three_band_track(3.0)).unwrap();
    run(&mut dj, DjCommand::Play { deck: 0 });
    let reference = bands(&settled(&mut dj, 1.0).strips[0][0]);
    run(
        &mut dj,
        DjCommand::SetEq {
            deck: 0,
            band: EqBand::Mid,
            db: -12.0,
        },
    );
    let cut = bands(&settled(&mut dj, 1.0).strips[0][0]);
    let drop = reference[1] - cut[1];
    assert!((drop - 12.0).abs() < 1.0, "mid -12 dB measured {drop:.2}");
    assert!((reference[0] - cut[0]).abs() < 1.0);
}

#[test]
fn filter_knob_low_passes_left_and_high_passes_right() {
    let measure = |amount: f32| {
        let mut dj = renderer(2, &[]);
        dj.load(0, three_band_track(3.0)).unwrap();
        run(&mut dj, DjCommand::SetFilter { deck: 0, amount });
        run(&mut dj, DjCommand::Play { deck: 0 });
        bands(&settled(&mut dj, 1.0).strips[0][0])
    };
    let open = measure(0.0);
    let low = measure(-0.8);
    let high = measure(0.8);
    assert!(
        open[2] - low[2] > 40.0,
        "low-pass leaves 8 kHz at {:.1} dB",
        low[2] - open[2]
    );
    assert!((open[0] - low[0]).abs() < 3.0, "low-pass moved 60 Hz");
    assert!(
        open[0] - high[0] > 40.0,
        "high-pass leaves 60 Hz at {:.1} dB",
        high[0] - open[0]
    );
    assert!((open[2] - high[2]).abs() < 3.0, "high-pass moved 8 kHz");
}

#[test]
fn filter_at_centre_passes_the_strip_unchanged() {
    // Isolator at 0 dB is an all-pass, so compare filter-open strips with
    // and without a knob round trip.
    let render = |round_trip: bool| {
        let mut dj = renderer(2, &[]);
        dj.load(0, noise_track(120.0, 3.0)).unwrap();
        if round_trip {
            run(
                &mut dj,
                DjCommand::SetFilter {
                    deck: 0,
                    amount: 0.0,
                },
            );
        }
        run(&mut dj, DjCommand::Play { deck: 0 });
        dj.render_seconds(1.0).unwrap().strips[0][0].clone()
    };
    assert_eq!(render(false), render(true));
}

#[test]
fn cue_bus_carries_prefader_decks_and_blends_with_master() {
    let mut dj = renderer(2, &[]);
    dj.load(0, sine_track(440.0, 3.0)).unwrap();
    dj.load(1, sine_track(2_000.0, 3.0)).unwrap();
    run(
        &mut dj,
        DjCommand::SetFader {
            deck: 0,
            value: 0.0,
        },
    );
    run(&mut dj, DjCommand::SetCueMonitor { deck: 0, on: true });
    run(&mut dj, DjCommand::Play { deck: 0 });
    run(&mut dj, DjCommand::Play { deck: 1 });
    let out = settled(&mut dj, 0.6);
    // Master: deck 2 only. Headphones: deck 1 only, at full level.
    assert!(spectrum_band_energy_db(&out.master[0], RATE, 400.0, 480.0) < -100.0);
    assert!(spectrum_band_energy_db(&out.master[0], RATE, 1_900.0, 2_100.0) > -20.0);
    assert!(null_residual_db(&out.cue[0], &out.strips[0][0]) < -100.0);
    // Cue mix fully to master: the headphones hear the master.
    run(&mut dj, DjCommand::SetCueMix { value: 1.0 });
    run(&mut dj, DjCommand::SetHeadphoneGain { db: -6.0 });
    let out = settled(&mut dj, 0.6);
    let gain = db(rms(&out.cue[1]) / rms(&out.master[1]));
    assert!((gain + 6.0).abs() < 0.05, "headphone level {gain} dB");
    assert!(spectrum_band_energy_db(&out.cue[0], RATE, 400.0, 480.0) < -100.0);
}

#[test]
fn deck_effect_slot_adds_a_tempo_synced_echo_when_on() {
    let render = |on: bool| {
        let mut dj = renderer(2, &["fx.delay"]);
        let track = DeckTrack::from_mono(RATE, &impulse(0, 0.8, seconds_to_frames(3.0)))
            .unwrap()
            .with_bpm(120.0, 0.0)
            .unwrap();
        dj.load(0, track).unwrap();
        for (param, value) in [
            ("Time", 1.0),
            ("Mix", 0.5),
            ("Feedback", 0.0),
            ("Ping Pong", 0.0),
        ] {
            run(
                &mut dj,
                DjCommand::SetDeckEffectParam {
                    deck: 0,
                    slot: 0,
                    param: param.into(),
                    value,
                },
            );
        }
        run(
            &mut dj,
            DjCommand::SetDeckEffect {
                deck: 0,
                slot: 0,
                on,
            },
        );
        run(&mut dj, DjCommand::SetMaster { deck: Some(0) });
        run(&mut dj, DjCommand::Play { deck: 0 });
        dj.render_seconds(1.0).unwrap().strips[0][0].clone()
    };
    let echo_window = |audio: &[f32]| {
        let at = seconds_to_frames(0.5);
        rms(&audio[at - 2_000..at + 4_000])
    };
    let on = render(true);
    let off = render(false);
    // One beat at 120 BPM is 0.5 s.
    assert!(echo_window(&on) > 0.002, "no echo: {}", echo_window(&on));
    assert!(
        echo_window(&off) < 1e-5,
        "echo while off: {}",
        echo_window(&off)
    );
    let state = {
        let mut dj = renderer(2, &["fx.delay"]);
        run(
            &mut dj,
            DjCommand::SetDeckEffectParam {
                deck: 1,
                slot: 0,
                param: "Mix".into(),
                value: 0.7,
            },
        );
        dj.session().state()
    };
    assert_eq!(state.decks[1].effects[0].kind, "fx.delay");
    assert_eq!(state.decks[1].effects[0].params["Mix"], 0.7);
}

#[test]
fn recording_nulls_against_the_master_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mix.wav");
    let mut dj = renderer(2, &[]);
    dj.load(0, noise_track(120.0, 20.0)).unwrap();
    dj.load(1, sine_track(300.0, 20.0)).unwrap();
    run(&mut dj, DjCommand::Play { deck: 0 });
    run(&mut dj, DjCommand::Play { deck: 1 });
    dj.render_seconds(0.5).unwrap();
    run(&mut dj, DjCommand::StartRecording { path: path.clone() });
    let mut master = StereoBuffer::silent(RATE, 0);
    for step in 0..12 {
        let position = -1.0 + step as f32 / 6.0;
        run(&mut dj, DjCommand::SetCrossfader { position });
        let out = dj.render(seconds_to_frames(0.25) + 37).unwrap();
        master.left.extend_from_slice(&out.master[0]);
        master.right.extend_from_slice(&out.master[1]);
    }
    assert_eq!(
        dj.session().state().recording.as_deref(),
        Some(path.as_path())
    );
    run(&mut dj, DjCommand::StopRecording);
    let out = dj.render(1_024).unwrap();
    let summary = out.recordings.first().expect("the recording closed");
    assert_eq!(summary.frames as usize, master.frames());
    assert_eq!(summary.dropped_frames, 0);
    let recorded = StereoBuffer::read_wav(&path).unwrap();
    assert_eq!(recorded.frames(), master.frames());
    assert!(null_residual_db(&recorded.left, &master.left) < -140.0);
    assert!(null_residual_db(&recorded.right, &master.right) < -140.0);
    assert!(dj.session().state().recording.is_none());
}
