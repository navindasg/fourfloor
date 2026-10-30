//! Helpers for the DJ tests: synthesized tracks, loading them into decks
//! of a headless app, and rendering what the app plays.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fourfloor_app::state::{DjIntent, FileIntent, Intent};
use fourfloor_app::{AppConfig, FourfloorApp};
use fourfloor_core::SampleRate;
use fourfloor_dj::{DeckState, DjCommand};
use fourfloor_testkit::{click_train, mix, sine, StereoBuffer};

/// The engine rate of the headless app.
pub const RATE: SampleRate = SampleRate::HZ_48000;

/// A track of `seconds`: a click on every beat at `bpm` from 0.1 s, under
/// a steady `tone_hz` sine. Written as a float WAV into `dir`.
pub fn write_track(dir: &Path, name: &str, bpm: f64, tone_hz: f64, seconds: f64) -> PathBuf {
    let period = 60.0 / bpm;
    let beats: Vec<f64> = (0..)
        .map(|i| 0.1 + f64::from(i) * period)
        .take_while(|t| *t < seconds - 0.05)
        .collect();
    let clicks = click_train(&beats, 0.9, seconds, RATE);
    let tone = sine(tone_hz, 0.25, seconds, RATE);
    let signal = mix(&[&clicks, &tone]);
    let path = dir.join(format!("{name}.wav"));
    StereoBuffer::from_mono(RATE, &signal)
        .write_wav(&path)
        .expect("track written");
    path
}

/// A canonical temporary folder (so library paths compare equal).
pub fn temp_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().canonicalize().expect("canonical");
    (dir, path)
}

/// A headless app with an empty project and the DJ decks started.
pub fn dj_app(paths_root: &Path) -> FourfloorApp {
    let config = AppConfig {
        paths: fourfloor_app::state::FeaturePaths::under(paths_root),
        ..AppConfig::headless()
    };
    let mut app = FourfloorApp::new(&config).expect("app starts");
    app.push_intent(Intent::File(FileIntent::New));
    app.push_intent(DjIntent::Start.into());
    app.flush();
    assert!(app.features.dj.is_some(), "decks started");
    app
}

/// Loads `path` into `deck` and waits for the decode and analysis.
pub fn load(app: &mut FourfloorApp, deck: usize, path: &Path) {
    app.push_intent(
        DjIntent::LoadFile {
            deck,
            path: path.to_path_buf(),
        }
        .into(),
    );
    app.flush();
    let start = Instant::now();
    loop {
        app.tick(0.0);
        let loaded = app
            .features
            .dj
            .as_ref()
            .and_then(|dj| dj.media(deck))
            .is_some();
        if loaded && !app.features.loading_decks.contains(&deck) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "deck {deck} did not load: {:?}",
            app.ui_state.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sends DJ commands and applies them.
pub fn send(app: &mut FourfloorApp, commands: &[DjCommand]) {
    for command in commands {
        app.push_intent(DjIntent::Command(command.clone()).into());
    }
    app.flush();
}

/// Renders `seconds` of what the app plays (offline backend), ticking the
/// app (DJ pump and snapshot) between blocks.
pub fn render(app: &mut FourfloorApp, seconds: f64) -> StereoBuffer {
    let total = (seconds * RATE.as_f64()).round() as usize;
    let mut interleaved = Vec::with_capacity(total * 2);
    let mut done = 0;
    while done < total {
        let frames = (total - done).min(1024);
        app.host.pump(frames);
        app.host
            .drain_master(|samples| interleaved.extend_from_slice(samples));
        app.tick(0.0);
        done += frames;
    }
    StereoBuffer::from_interleaved(RATE, &interleaved)
}

/// The snapshot of `deck`.
pub fn deck(app: &FourfloorApp, deck: usize) -> DeckState {
    app.features
        .dj
        .as_ref()
        .and_then(|dj| dj.state().decks.get(deck).cloned())
        .expect("deck exists")
}
