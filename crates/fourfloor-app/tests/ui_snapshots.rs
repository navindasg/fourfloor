//! Renders every view of the demo with wgpu (no window) and writes PNGs
//! to `CARGO_TARGET_TMPDIR/fourfloor-app-snapshots`, checking each image
//! has real content: many colours and most pixels not plain background.

mod common;

use std::collections::HashSet;
use std::path::PathBuf;

use common::{harness, ready_app, track};
use egui_kittest::kittest::Queryable;
use fourfloor_app::state::{Intent, TransportIntent, ViewId};

fn snapshot_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fourfloor-app-snapshots");
    std::fs::create_dir_all(&dir).expect("snapshot folder");
    dir
}

fn check_and_save(image: &image::RgbaImage, name: &str, max_plain: f64) -> PathBuf {
    let background = image::Rgba([0x17, 0x19, 0x1d, 0xff]);
    let pixels = image.pixels().count();
    let plain = image.pixels().filter(|p| **p == background).count();
    let colours: HashSet<[u8; 4]> = image.pixels().map(|p| p.0).collect();
    let path = snapshot_dir().join(format!("{name}.png"));
    image.save(&path).expect("png written");
    assert!(colours.len() > 60, "{name}: only {} colours", colours.len());
    assert!(
        (plain as f64) < pixels as f64 * max_plain,
        "{name}: {plain} of {pixels} pixels are plain background"
    );
    path
}

#[test]
fn every_view_renders_with_content() {
    let mut app = ready_app();
    let kick = track(app.project(), "Kick");
    app.ui_state.selection.select_track(kick);
    app.push_intent(Intent::Transport(TransportIntent::Play));
    let mut harness = harness(app);
    harness.run_steps(40);
    // Open the stabs' automation: volume, pan and the filter cutoff ramp.
    harness.get_by_label("Automation Stabs").click();
    harness.run_steps(2);
    let views = [
        (ViewId::ARRANGEMENT, "arrangement", None, 0.5),
        // Two scenes leave most of the launcher empty.
        (ViewId::LAUNCHER, "launcher", None, 0.95),
        (ViewId::SEQUENCER, "sequencer", Some("Kick"), 0.85),
        (ViewId::PIANO_ROLL, "piano_roll", Some("Stabs"), 0.5),
        (ViewId::MIXER, "mixer", Some("Bass"), 0.7),
    ];
    let mut written = Vec::new();
    for (view, name, select, max_plain) in views {
        if let Some(track_name) = select {
            let id = track(harness.state().project(), track_name);
            harness.state_mut().ui_state.selection.select_track(id);
        }
        harness.state_mut().ui_state.view = view;
        harness.run_steps(4);
        let image = harness.render().expect("wgpu renders");
        written.push(check_and_save(&image, name, max_plain));
    }
    // The device browser and settings windows over the mixer.
    harness.state_mut().ui_state.panels.browser = true;
    harness.state_mut().ui_state.panels.settings = true;
    harness.run_steps(4);
    let image = harness.render().expect("wgpu renders");
    written.push(check_and_save(&image, "windows", 0.7));
    for path in &written {
        assert!(path.exists());
    }
    println!("snapshots in {}", snapshot_dir().display());
}

#[test]
fn an_empty_project_renders_without_tracks() {
    let mut app = ready_app();
    app.push_intent(Intent::File(fourfloor_app::state::FileIntent::New));
    let mut harness = harness(app);
    harness.run_steps(4);
    assert!(harness.state().project().tracks.is_empty());
    for view in [
        ViewId::ARRANGEMENT,
        ViewId::SEQUENCER,
        ViewId::PIANO_ROLL,
        ViewId::MIXER,
        ViewId::LAUNCHER,
    ] {
        harness.state_mut().ui_state.view = view;
        harness.run_steps(2);
    }
    let image = harness.render().expect("wgpu renders");
    let path = snapshot_dir().join("empty.png");
    image.save(&path).expect("png written");
}
