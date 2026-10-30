//! `fourfloor-app --smoke`: runs the whole app without a window or an
//! audio device. It loads the demo, waits for the engine graph, plays,
//! draws every view for a number of frames through a real egui context,
//! and checks that time moved and sound came out.

use std::time::Duration;

use egui::{vec2, Rect};

use crate::app::{AppConfig, FourfloorApp};
use crate::state::{Intent, TransportIntent};

/// What a smoke run saw.
#[derive(Clone, Debug, PartialEq)]
pub struct SmokeReport {
    /// Frames drawn.
    pub frames: u64,
    /// Tracks in the loaded project.
    pub tracks: usize,
    /// Playhead at the end, in beats.
    pub beats: f64,
    /// Loudest master peak shown, linear.
    pub max_peak: f32,
    /// Integrated loudness at the end.
    pub integrated_lufs: f64,
    /// Compiler warnings.
    pub warnings: usize,
    /// DJ decks running at the end (the DJ view starts them when shown).
    pub dj_decks: usize,
    /// Edits the assistant made from the phrase sent during the run.
    pub assistant_edits: usize,
}

/// Runs `frames` frames at 60 per second of simulated time.
pub fn run(frames: u64) -> Result<SmokeReport, String> {
    let mut app = FourfloorApp::new(&AppConfig::headless()).map_err(|e| e.to_string())?;
    if !app.wait_for_engine(Duration::from_secs(60)) {
        return Err(format!(
            "the engine graph was not built: {}",
            app.host
                .compile_error
                .clone()
                .unwrap_or_else(|| "timed out".to_owned())
        ));
    }
    app.push_intent(Intent::Transport(TransportIntent::Play));
    // Draw the sample browser, the AI panel and the settings window too,
    // and ask the assistant for something.
    app.ui_state.panels.library = true;
    app.ui_state.panels.settings = true;
    app.ui_state.panels.assistant = true;
    app.push_intent(Intent::Assistant(crate::state::AssistantIntent::Instruct(
        "set tempo to 126".to_owned(),
    )));
    let ctx = egui::Context::default();
    let views: Vec<_> = app.views().iter().map(|v| v.id()).collect();
    let mut max_peak = 0.0f32;
    for frame in 0..frames {
        if !views.is_empty() {
            app.ui_state.view = views[(frame as usize / 20) % views.len()];
        }
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 1000.0))),
            time: Some(frame as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.frame(ui));
        // No renderer: drop the texture updates on purpose.
        output.textures_delta.clear();
        let [left, right] = app.live.meters.master();
        max_peak = max_peak.max(left).max(right);
    }
    let waited = std::time::Instant::now();
    while app.features.assistant_busy && waited.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(10));
        app.tick(0.0);
    }
    let report = SmokeReport {
        frames: app.frames,
        tracks: app.project().tracks.len(),
        beats: app.live.transport.position / 960.0,
        max_peak,
        integrated_lufs: app.live.master.integrated_lufs,
        warnings: app.live.warnings.len(),
        dj_decks: app
            .features
            .dj
            .as_ref()
            .map_or(0, crate::services::dj::DjService::deck_count),
        assistant_edits: app.features.chat.iter().map(|e| e.applied.len()).sum(),
    };
    if let Some(error) = &app.live.compile_error {
        return Err(format!("compile error: {error}"));
    }
    if report.beats <= 0.0 {
        return Err(format!("the playhead did not move: {report:?}"));
    }
    if views.contains(&crate::state::ViewId::DJ) && frames >= 120 && report.dj_decks < 2 {
        return Err(format!("the DJ view did not start the decks: {report:?}"));
    }
    if report.assistant_edits == 0 {
        return Err(format!(
            "the assistant made no edit: {:?}",
            app.features.chat.last()
        ));
    }
    if report.max_peak <= 0.0 {
        return Err(format!("no sound reached the master meter: {report:?}"));
    }
    Ok(report)
}
