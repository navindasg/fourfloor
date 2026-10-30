//! Helpers for the sample browser tests: a headless app whose library,
//! models and caches live in a temporary folder, the factory sounds
//! installed into it, and capturing what the app plays.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fourfloor_app::state::{FeaturePaths, FileIntent, Intent, LibraryIntent};
use fourfloor_app::{AppConfig, FourfloorApp};
use fourfloor_core::SampleRate;
use fourfloor_library::{FileId, FileKind, FileRecord, Query};
use fourfloor_testkit::StereoBuffer;

/// The engine rate of the headless app.
pub const RATE: SampleRate = SampleRate::HZ_48000;

/// A canonical temporary folder.
pub fn temp_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().canonicalize().expect("canonical");
    (dir, path)
}

/// A headless app with an empty project, its feature files under `root`.
pub fn library_app(root: &Path) -> FourfloorApp {
    let config = AppConfig {
        paths: FeaturePaths::under(root),
        ..AppConfig::headless()
    };
    let mut app = FourfloorApp::new(&config).expect("the app starts");
    app.push_intent(Intent::File(FileIntent::New));
    app.flush();
    assert!(app.wait_for_engine(Duration::from_secs(60)), "graph built");
    app
}

/// Runs the app's services until no job is running.
pub fn wait_jobs(app: &mut FourfloorApp) {
    let start = Instant::now();
    app.tick(0.0);
    while !app.jobs.running().is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(300),
            "jobs still running: {:?}",
            app.jobs
                .running()
                .iter()
                .map(|j| j.label.clone())
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(10));
        app.tick(0.0);
    }
    app.tick(0.0);
}

/// Applies `intent` and waits for any job it starts.
pub fn run(app: &mut FourfloorApp, intent: impl Into<Intent>) {
    app.push_intent(intent.into());
    app.flush();
    wait_jobs(app);
}

/// Installs and scans the factory sounds (18 files).
pub fn install_factory(app: &mut FourfloorApp) {
    run(app, LibraryIntent::InstallFactory);
    assert!(!app.features.scan.running, "the scan finished");
}

/// Every file in the library.
pub fn all_files(app: &FourfloorApp) -> Vec<FileRecord> {
    app.features
        .library
        .library()
        .expect("library open")
        .search(&Query::new())
        .expect("search")
}

/// The file whose title contains every word of `words` (any case) and is
/// of `kind`.
pub fn find(app: &FourfloorApp, words: &str, kind: FileKind) -> FileRecord {
    let words: Vec<String> = words.split(' ').map(str::to_lowercase).collect();
    all_files(app)
        .into_iter()
        .find(|record| {
            let title = record.title.to_lowercase();
            record.kind == kind && words.iter().all(|w| title.contains(w.as_str()))
        })
        .unwrap_or_else(|| {
            let titles: Vec<_> = all_files(app).into_iter().map(|r| r.title).collect();
            panic!("no {kind:?} file named {words:?} in {titles:?}")
        })
}

/// The id of `find(..)`.
pub fn file_id(app: &FourfloorApp, words: &str, kind: FileKind) -> FileId {
    find(app, words, kind).id
}

/// Renders `seconds` of the app's output (offline engine) and returns
/// what reached the master.
pub fn capture(app: &mut FourfloorApp, seconds: f64) -> StereoBuffer {
    let frames = (seconds * RATE.as_f64()) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    let mut done = 0;
    while done < frames {
        let block = 1024.min(frames - done);
        app.host.pump(block);
        app.host
            .drain_master(|chunk| samples.extend_from_slice(chunk));
        done += block;
    }
    StereoBuffer::from_interleaved(RATE, &samples)
}

/// Seconds from the start to the last sample louder than -60 dBFS.
pub fn sounding_seconds(buffer: &StereoBuffer) -> f64 {
    let mono = buffer.mono();
    let last = mono.iter().rposition(|s| s.abs() > 1e-3).unwrap_or(0);
    last as f64 / RATE.as_f64()
}

/// A headless app with the demo project, its feature files under `root`.
pub fn demo_library_app(root: &Path) -> FourfloorApp {
    let config = AppConfig {
        paths: FeaturePaths::under(root),
        ..AppConfig::headless()
    };
    let mut app = FourfloorApp::new(&config).expect("the app starts");
    assert!(app.wait_for_engine(Duration::from_secs(60)), "graph built");
    app
}
