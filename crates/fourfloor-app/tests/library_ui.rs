//! The sample browser and the settings tabs driven through egui_kittest:
//! real clicks select, audition, find similar and sort; the settings list
//! models with their licenses, remove a MIDI mapping and show the plugin
//! cache. wgpu snapshots of each are written for inspection.

mod library_common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use fourfloor_app::services::library::ResultSource;
use fourfloor_app::state::SettingsTab;
use fourfloor_app::FourfloorApp;
use fourfloor_library::{FileKind, SortKey};
use fourfloor_model::command::MapController;
use fourfloor_model::Command;
use library_common::{demo_library_app, find, install_factory, temp_dir};

fn harness(app: FourfloorApp) -> Harness<'static, FourfloorApp> {
    Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut FourfloorApp| app.frame(ui), app)
}

fn click(harness: &mut Harness<'_, FourfloorApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(3);
}

fn until(
    harness: &mut Harness<'_, FourfloorApp>,
    what: &str,
    done: impl Fn(&FourfloorApp) -> bool,
) {
    let start = Instant::now();
    while !done(harness.state()) {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "timed out: {what}"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn snapshot(harness: &mut Harness<'_, FourfloorApp>, name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fourfloor-app-snapshots");
    std::fs::create_dir_all(&dir).expect("snapshot folder");
    let image = harness.render().expect("wgpu renders");
    let colours: std::collections::HashSet<[u8; 4]> = image.pixels().map(|p| p.0).collect();
    let path = dir.join(format!("{name}.png"));
    image.save(&path).expect("png written");
    assert!(colours.len() > 60, "{name}: {} colours", colours.len());
    path
}

#[test]
fn the_browser_selects_auditions_finds_similar_and_sorts() {
    let (_guard, dir) = temp_dir();
    let mut app = demo_library_app(&dir);
    install_factory(&mut app);
    let kick = find(&app, "kick 909", FileKind::OneShot);
    app.ui_state.panels.library = true;
    let mut harness = harness(app);
    harness.run_steps(4);
    assert_eq!(harness.state().features.library.rows.len(), 18);

    click(&mut harness, &kick.title);
    assert_eq!(harness.state().features.library.selected, Some(kick.id));
    assert!(harness.state().features.library.waveform.is_some());
    click(&mut harness, "Audition");
    until(&mut harness, "audition", |app| {
        app.features.auditioning == Some(kick.id)
    });

    click(&mut harness, "Find similar");
    assert_eq!(
        harness.state().features.library.source,
        Some(ResultSource::Similar(kick.id))
    );
    click(&mut harness, "Back to filters");
    assert_eq!(
        harness.state().features.library.source,
        Some(ResultSource::Query)
    );

    click(&mut harness, "Length");
    assert_eq!(
        harness.state().library_panel.filters.sort,
        SortKey::Duration
    );
    let lengths: Vec<f64> = harness
        .state()
        .features
        .library
        .rows
        .iter()
        .map(|r| r.record.duration_seconds)
        .collect();
    assert!(lengths.windows(2).all(|w| w[0] <= w[1]), "{lengths:?}");

    harness.state_mut().library_panel.filters.text = "hat".to_owned();
    harness.run_steps(3);
    let rows = &harness.state().features.library.rows;
    assert!(!rows.is_empty());
    assert!(rows
        .iter()
        .all(|r| r.record.title.to_lowercase().contains("hat")));
    harness.state_mut().library_panel.filters.text.clear();
    harness.run_steps(3);
    snapshot(&mut harness, "library_panel");
}

#[test]
fn settings_list_models_mappings_and_plugins() {
    let (_guard, dir) = temp_dir();
    let mut app = demo_library_app(&dir);
    let (device, param) = app
        .project()
        .tracks
        .iter()
        .flat_map(|t| t.devices.iter())
        .find_map(|d| d.params.first().map(|p| (d.id, p.id)))
        .expect("a parameter");
    app.push_intent(fourfloor_app::state::Intent::Command(
        Command::MapController(MapController {
            channel: None,
            controller: 21,
            device,
            param,
            min: None,
            max: None,
        }),
    ));
    app.flush();
    app.ui_state.panels.settings = true;
    app.ui_state.settings_tab = SettingsTab::Models;
    let mut harness = harness(app);
    harness.run_steps(4);
    let models = &harness.state().features.models;
    assert_eq!(models.len(), 3, "Basic Pitch, Spleeter and CLAP");
    assert!(models.iter().all(|m| !m.cached), "a fresh cache is empty");
    for name in ["basic-pitch", "spleeter-2stems", "clap-larger-general"] {
        harness.get_by_label(name);
    }
    assert!(harness.get_all_by_label("Apache-2.0").count() >= 2);
    assert_eq!(harness.get_all_by_label("Download").count(), 3);
    snapshot(&mut harness, "settings_models");

    click(&mut harness, "MIDI mappings");
    harness.get_by_label("21");
    snapshot(&mut harness, "settings_mappings");
    click(&mut harness, "Remove");
    assert!(harness.state().project().controller_map.is_empty());
    harness
        .state_mut()
        .push_intent(fourfloor_app::state::Intent::Undo);
    harness.run_steps(2);
    assert_eq!(harness.state().project().controller_map.len(), 1);

    click(&mut harness, "Plugins");
    harness.get_by_label("No plugins in the cache.");
    assert!(
        harness.state().features.plugins.path.is_some(),
        "cache read"
    );
    snapshot(&mut harness, "settings_plugins");
}
