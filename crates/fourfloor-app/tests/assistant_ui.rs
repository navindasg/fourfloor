//! The AI panel driven through egui_kittest at 1440x900: open it from the
//! transport bar, run an example and a typed instruction, undo the edit
//! from its log card, generate drums with the button. Then every view is
//! rendered at that size with the panel open, for the layout pass.

mod library_common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use fourfloor_app::state::ViewId;
use fourfloor_app::FourfloorApp;
use library_common::{demo_library_app, temp_dir};

fn harness(app: FourfloorApp) -> Harness<'static, FourfloorApp> {
    Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut FourfloorApp| app.frame(ui), app)
}

fn click(harness: &mut Harness<'_, FourfloorApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(3);
}

fn until_idle(harness: &mut Harness<'_, FourfloorApp>) {
    let start = Instant::now();
    harness.run_steps(2);
    while !harness.state().jobs.running().is_empty() || harness.state().features.assistant_busy {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "assistant timed out"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
}

fn snapshot(harness: &mut Harness<'_, FourfloorApp>, name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fourfloor-app-snapshots");
    std::fs::create_dir_all(&dir).expect("snapshot folder");
    let image = harness.render().expect("wgpu renders");
    assert_eq!((image.width(), image.height()), (1440, 900), "{name}");
    let colours: std::collections::HashSet<[u8; 4]> = image.pixels().map(|p| p.0).collect();
    let path = dir.join(format!("{name}.png"));
    image.save(&path).expect("png written");
    assert!(colours.len() > 60, "{name}: {} colours", colours.len());
    path
}

#[test]
fn the_panel_runs_instructions_undoes_them_and_generates_drums() {
    let (_guard, dir) = temp_dir();
    let app = demo_library_app(&dir);
    let tempo = app.project().tempo.bpm();
    let tracks = app.project().tracks.len();
    let mut harness = harness(app);
    harness.run_steps(4);
    click(&mut harness, "AI");
    assert!(harness.state().ui_state.panels.assistant);
    harness.get_by_label("Nothing asked yet.");
    snapshot(&mut harness, "assistant_empty");

    harness.state_mut().assistant_panel.input = "set tempo to 125".to_owned();
    harness.run_steps(1);
    click(&mut harness, "Send");
    until_idle(&mut harness);
    assert!((harness.state().project().tempo.bpm() - 125.0).abs() < 1e-9);
    assert!(
        harness.state().assistant_panel.input.is_empty(),
        "the box clears"
    );

    click(&mut harness, "Undo edit");
    assert!((harness.state().project().tempo.bpm() - tempo).abs() < 1e-9);
    harness.get_by_label("Undone");

    // An example phrase from the empty state is a link; the log has an
    // entry now, so run one through the box instead.
    harness.state_mut().assistant_panel.input = "mute the bass".to_owned();
    harness.run_steps(1);
    click(&mut harness, "Send");
    until_idle(&mut harness);
    let bass = harness
        .state()
        .project()
        .tracks
        .iter()
        .find(|t| t.name == "Bass")
        .map(|t| t.mute);
    assert_eq!(
        bass,
        Some(true),
        "{:?}",
        harness.state().features.chat.last()
    );

    click(&mut harness, "Generate drums");
    until_idle(&mut harness);
    assert!(harness.state().project().tracks.len() > tracks);
    assert_eq!(harness.state().features.chat.len(), 3);
    snapshot(&mut harness, "assistant_panel");
}

#[test]
fn every_view_lays_out_at_1440_by_900_with_the_panels_open() {
    let (_guard, dir) = temp_dir();
    let mut app = demo_library_app(&dir);
    app.ui_state.panels.assistant = true;
    let mut harness = harness(app);
    harness.run_steps(6);
    let views = [
        (ViewId::ARRANGEMENT, "layout_arrangement"),
        (ViewId::LAUNCHER, "layout_launcher"),
        (ViewId::SEQUENCER, "layout_sequencer"),
        (ViewId::PIANO_ROLL, "layout_piano_roll"),
        (ViewId::MIXER, "layout_mixer"),
        (ViewId::DJ, "layout_dj"),
    ];
    for (view, name) in views {
        harness.state_mut().ui_state.view = view;
        harness.run_steps(6);
        snapshot(&mut harness, name);
    }
    harness.state_mut().ui_state.view = ViewId::ARRANGEMENT;
    harness.state_mut().ui_state.panels.library = true;
    harness
        .state_mut()
        .ui_state
        .error("Example error: the file could not be read");
    harness.run_steps(6);
    snapshot(&mut harness, "layout_library_toast");

    // Empty states of an empty project.
    harness
        .state_mut()
        .push_intent(fourfloor_app::state::Intent::File(
            fourfloor_app::state::FileIntent::New,
        ));
    harness.state_mut().ui_state.panels.library = false;
    harness.state_mut().ui_state.toasts.clear();
    harness.run_steps(4);
    harness.get_by_label("An empty project");
    snapshot(&mut harness, "layout_empty_arrangement");
    for (view, title) in [
        (ViewId::MIXER, "No channels yet"),
        (ViewId::SEQUENCER, "No track selected"),
        (ViewId::PIANO_ROLL, "No clip to edit"),
        (ViewId::LAUNCHER, "No scenes yet"),
    ] {
        harness.state_mut().ui_state.view = view;
        harness.run_steps(3);
        harness.get_by_label(title);
    }
    snapshot(&mut harness, "layout_empty_launcher");
}
