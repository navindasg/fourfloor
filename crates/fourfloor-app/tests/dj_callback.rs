//! The DJ graph inside the app's audio callback, on a four-channel device:
//! the master mix on outputs 1-2, the headphone cue on 3-4, and nothing in
//! the callback allocates (this binary installs the guard allocator).

use std::sync::Arc;

use assert_no_alloc::assert_no_alloc;
use fourfloor_app::services::callback::{AudioCallback, CallbackLink, CallbackMessage};
use fourfloor_app::services::dj::DECK_EFFECTS;
use fourfloor_app::services::meters::MeterBank;
use fourfloor_app::services::voices::DeckTaps;
use fourfloor_app::services::{build_registry, MAX_BLOCK};
use fourfloor_core::{DeviceId, ParamId, SampleRate};
use fourfloor_dj::{
    CrossfaderSide, DeckTrack, DjCommand, DjError, DjSession, ParamSink, SessionConfig,
};
use fourfloor_engine::{engine, CompiledGraph, EngineConfig};
use fourfloor_testkit::{dominant_frequency, rms_db, sine};

#[global_allocator]
static ALLOCATOR: fourfloor_testkit::AllocDisabler = fourfloor_testkit::AllocDisabler;

const RATE: SampleRate = SampleRate::HZ_48000;

struct Sink<'a>(&'a mut CallbackLink);

impl ParamSink for Sink<'_> {
    fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> Result<(), DjError> {
        self.0
            .tx
            .push(CallbackMessage::DjParam {
                device,
                param,
                value,
            })
            .map_err(|_| DjError::QueueFull("callback"))
    }
}

fn track(hz: f64) -> DeckTrack {
    let tone = sine(hz, 0.3, 10.0, RATE);
    DeckTrack::from_mono(RATE, &tone)
        .expect("track")
        .with_bpm(124.0, 0.0)
        .expect("grid")
}

/// Splits a four-channel render into (master mono, cue mono).
fn split(out: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let master = out.chunks_exact(4).map(|f| 0.5 * (f[0] + f[1])).collect();
    let cue = out.chunks_exact(4).map(|f| 0.5 * (f[2] + f[3])).collect();
    (master, cue)
}

#[test]
fn the_cue_bus_reaches_outputs_three_and_four_without_allocating() {
    let registry = build_registry(false).expect("registry");
    let mut config = EngineConfig::new(RATE);
    config.max_block = MAX_BLOCK;
    let (mut controller, processor) = engine(
        config,
        CompiledGraph::empty(2, RATE, MAX_BLOCK).expect("graph"),
    )
    .expect("engine");
    let (mut callback, mut link) = AudioCallback::new(processor, Arc::new(MeterBank::new()));
    let session_config = SessionConfig::new(2, RATE, MAX_BLOCK).with_deck_effects(&DECK_EFFECTS);
    let (mut session, graph) = DjSession::new(&session_config, &registry).expect("session");
    let mut taps = DeckTaps::default();
    for (deck, slot) in taps.strips.iter_mut().enumerate() {
        *slot = session.deck_ids(deck).map(|ids| ids.strip_output());
    }
    link.tx
        .push(CallbackMessage::DjGraph(Some(Box::new(graph)), taps))
        .expect("installed");
    session.load_track(0, track(1000.0)).expect("deck A");
    session.load_track(1, track(400.0)).expect("deck B");
    let commands = [
        DjCommand::SetCrossfaderSide {
            deck: 0,
            side: CrossfaderSide::A,
        },
        DjCommand::SetCrossfaderSide {
            deck: 1,
            side: CrossfaderSide::B,
        },
        // Full A: deck B is cued in the headphones but not on the master.
        DjCommand::SetCrossfader { position: -1.0 },
        DjCommand::SetCueMonitor { deck: 1, on: true },
        DjCommand::SetCueMix { value: 0.0 },
        DjCommand::Play { deck: 0 },
        DjCommand::Play { deck: 1 },
    ];
    for command in &commands {
        session
            .apply(command, &mut Sink(&mut link))
            .expect("applies");
    }
    let mut out = vec![0.0f32; 512 * 4];
    let mut rendered = Vec::new();
    for round in 0..200 {
        if round == 100 {
            // Move the cue mix all the way to the master mid-play.
            session
                .apply(&DjCommand::SetCueMix { value: 1.0 }, &mut Sink(&mut link))
                .expect("applies");
        }
        assert_no_alloc(|| callback.render(&mut out, 4));
        rendered.extend_from_slice(&out);
        session.pump().expect("pump");
        link.collect_garbage();
        controller.collect_garbage();
        while link.master_rx.pop().is_ok() {}
    }
    let frames_per_round = 512;
    let first = &rendered[20 * frames_per_round * 4..100 * frames_per_round * 4];
    let (master, cue) = split(first);
    let master_hz = dominant_frequency(&master, RATE);
    let cue_hz = dominant_frequency(&cue, RATE);
    assert!(
        (master_hz - 1000.0).abs() < 10.0,
        "master plays A: {master_hz}"
    );
    assert!(
        (cue_hz - 400.0).abs() < 5.0,
        "headphones play cued B: {cue_hz}"
    );
    assert!(rms_db(&cue) > -25.0);
    let last = &rendered[140 * frames_per_round * 4..];
    let (_, cue_master) = split(last);
    let blended = dominant_frequency(&cue_master, RATE);
    assert!(
        (blended - 1000.0).abs() < 10.0,
        "cue mix at the master: {blended}"
    );
    let peaks = callback_peaks(&link);
    assert!(peaks[0][0] > 0.0 || peaks[1][0] > 0.0, "deck meters move");
}

fn callback_peaks(link: &CallbackLink) -> [[f32; 2]; 4] {
    link.deck_meters.take()
}
