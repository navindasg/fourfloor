//! File work through the app's intents: save and reopen, audio import with
//! waveform and playback, WAV and stem export, DAWproject round trip and a
//! reference track for A/B. Everything runs on the app's job threads.

mod common;

use std::time::Duration;

use common::{harness, ready_app, track};
use fourfloor_app::state::{FileIntent, Intent, TransportIntent};
use fourfloor_app::FourfloorApp;
use fourfloor_core::SampleRate;
use fourfloor_model::command::CreateTrack;
use fourfloor_model::{ClipContent, Command, TrackKind};
use fourfloor_testkit::{lufs, sine, StereoBuffer};

const RATE: SampleRate = SampleRate::HZ_48000;

/// Runs frames until the app has no jobs left (or a timeout).
fn finish_jobs(app: &mut FourfloorApp) {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !app.jobs.running().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.tick(0.0);
    }
    app.tick(0.0);
    assert!(app.jobs.running().is_empty(), "jobs finished");
}

fn status(app: &FourfloorApp) -> String {
    app.ui_state
        .status
        .as_ref()
        .map(|s| s.text.clone())
        .unwrap_or_default()
}

#[test]
fn save_reopen_and_export_wav_and_stems() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = ready_app();
    let folder = dir.path().join("demo-song");
    app.push_intent(Intent::File(FileIntent::SaveAs(folder.clone())));
    app.flush();
    assert!(!app.doc.is_dirty(), "{}", status(&app));
    assert!(folder.join("project.json").exists());
    let wav = dir.path().join("mix.wav");
    app.push_intent(Intent::File(FileIntent::ExportWav(wav.clone())));
    app.push_intent(Intent::File(FileIntent::ExportStems(
        dir.path().join("stems"),
    )));
    app.flush();
    finish_jobs(&mut app);
    let mix = StereoBuffer::read_wav(&wav).expect("exported wav reads");
    assert!(mix.seconds() > 15.0);
    let loudness = lufs(&mix);
    assert!((-20.0..-8.0).contains(&loudness), "{loudness}");
    let stems = std::fs::read_dir(dir.path().join("stems"))
        .expect("stems")
        .count();
    assert!(stems >= 2, "master plus the mix bus stem: {stems}");
    // Reopen in a fresh app: same tracks.
    let mut config = fourfloor_app::AppConfig::headless();
    config.project_dir = Some(folder);
    let reopened = FourfloorApp::new(&config).expect("opens");
    assert_eq!(reopened.project(), app.project());
}

#[test]
fn imported_audio_gets_a_clip_a_waveform_and_plays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = ready_app();
    app.push_intent(Intent::File(FileIntent::New));
    app.push_intent(Intent::File(FileIntent::SaveAs(
        dir.path().join("audio-song"),
    )));
    app.push_intent(Intent::Command(Command::CreateTrack(CreateTrack {
        name: "Loop".to_owned(),
        kind: TrackKind::Audio,
        index: None,
    })));
    app.flush();
    let source = dir.path().join("tone.wav");
    StereoBuffer::from_mono(RATE, &sine(220.0, 0.5, 2.0, RATE))
        .write_wav(&source)
        .expect("wav");
    app.push_intent(Intent::File(FileIntent::ImportAudio(source)));
    app.flush();
    app.flush();
    let loop_track = track(app.project(), "Loop");
    let clip = app
        .project()
        .track(loop_track)
        .expect("track")
        .clips
        .first()
        .cloned()
        .expect("audio clip");
    let ClipContent::Audio { source, .. } = &clip.content else {
        panic!("an audio clip");
    };
    // 2 s at 120 BPM is 4 beats.
    assert_eq!(clip.length.get(), 3840, "{}", status(&app));
    let key = source.file_name();
    // Draw the arrangement once so it asks for the waveform.
    let mut harness = harness(app);
    harness.run_steps(3);
    let mut app = harness.into_state();
    finish_jobs(&mut app);
    let overview = app.assets.waveforms.get(&key).expect("waveform decoded");
    let peak = overview.range(0, overview.frames);
    assert!((peak.max - 0.5).abs() < 0.01, "{peak:?}");
    // And the engine plays it.
    assert!(app.wait_for_engine(Duration::from_secs(30)));
    app.push_intent(Intent::Transport(TransportIntent::Play));
    app.flush();
    for _ in 0..30 {
        app.tick(1.0 / 30.0);
    }
    assert!(
        app.live.meters.track(loop_track)[0] > 0.3,
        "audio clip reaches its meter"
    );
}

#[test]
fn dawproject_export_then_import_keeps_the_tracks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = ready_app();
    let file = dir.path().join("demo.dawproject");
    app.push_intent(Intent::File(FileIntent::ExportDawproject(file.clone())));
    app.flush();
    finish_jobs(&mut app);
    assert!(file.exists(), "{}", status(&app));
    let names: Vec<String> = app
        .project()
        .tracks
        .iter()
        .map(|t| t.name.clone())
        .collect();
    app.push_intent(Intent::File(FileIntent::ImportDawproject {
        file,
        dir: dir.path().join("imported"),
    }));
    app.flush();
    finish_jobs(&mut app);
    let imported: Vec<String> = app
        .project()
        .tracks
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(imported, names, "{}", status(&app));
    assert_eq!(
        app.doc.dir.as_deref(),
        Some(dir.path().join("imported").as_path())
    );
}

#[test]
fn a_reference_loads_measures_and_is_monitored_matched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = ready_app();
    let reference = dir.path().join("reference.wav");
    // A loud 1 kHz tone, much hotter than the demo.
    StereoBuffer::from_mono(RATE, &sine(1000.0, 0.9, 6.0, RATE))
        .write_wav(&reference)
        .expect("wav");
    app.push_intent(Intent::File(FileIntent::LoadReference(reference)));
    app.flush();
    finish_jobs(&mut app);
    let summary = app.ui_state.reference.clone();
    assert_eq!(
        summary.name.as_deref(),
        Some("reference.wav"),
        "{}",
        status(&app)
    );
    assert!(summary.reference_lufs > summary.mix_lufs, "{summary:?}");
    assert!(!summary.notes.is_empty(), "comparison notes");
    app.push_intent(Intent::MonitorReference(true));
    app.push_intent(Intent::Transport(TransportIntent::Play));
    app.flush();
    let monitor = app.host.monitor();
    assert!(monitor.reference);
    let expected = 10f64.powf((summary.mix_lufs - summary.reference_lufs) / 20.0) as f32;
    assert!(
        (monitor.reference_gain - expected).abs() < 1e-4,
        "matched gain"
    );
    // The analysis now hears the tone at about the mix's loudness.
    for _ in 0..90 {
        app.tick(1.0 / 30.0);
    }
    let heard = app.live.master.short_term_lufs;
    assert!(
        (heard - summary.mix_lufs).abs() < 1.5,
        "heard {heard}, mix {}",
        summary.mix_lufs
    );
    let spectrum = &app.live.master;
    let loudest = spectrum
        .spectrum_db
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| spectrum.band_centers_hz[i])
        .unwrap_or_default();
    assert!(
        (700.0..1400.0).contains(&loudest),
        "spectrum peak {loudest} Hz"
    );
}
