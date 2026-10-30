//! Shared helpers for the app's integration tests.

#![allow(dead_code)]

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use fourfloor_app::{AppConfig, FourfloorApp};
use fourfloor_core::TrackId;
use fourfloor_model::Project;

/// A headless app with the demo loaded and its graph running.
pub fn ready_app() -> FourfloorApp {
    let mut app = FourfloorApp::new(&AppConfig::headless()).expect("the app starts");
    assert!(
        app.wait_for_engine(Duration::from_secs(60)),
        "graph built: {:?}",
        app.host.compile_error
    );
    app
}

/// A harness drawing the whole app at a laptop-sized window.
pub fn harness(app: FourfloorApp) -> Harness<'static, FourfloorApp> {
    Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut FourfloorApp| app.frame(ui), app)
}

/// The id of the track called `name`.
pub fn track(project: &Project, name: &str) -> TrackId {
    project
        .tracks
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.id)
        .unwrap_or_else(|| panic!("no track {name}"))
}

/// A left click at `pos`.
pub fn click_at(harness: &mut Harness<'_, FourfloorApp>, pos: Pos2) {
    harness.event(Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.step();
}

/// A left-button drag from `from` to `to` in a few moves.
pub fn drag(harness: &mut Harness<'_, FourfloorApp>, from: Pos2, to: Pos2) {
    harness.event(Event::PointerMoved(from));
    harness.step();
    harness.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        harness.event(Event::PointerMoved(from + (to - from) * t));
        harness.step();
    }
    harness.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    harness.step();
}
