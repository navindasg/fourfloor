//! Drives the real app UI headlessly with egui_kittest: a click or key on
//! a widget must send a command through the bus and change the project
//! (and undo must bring it back), or move the engine.

mod common;

use common::{click_at, drag, harness, ready_app, track};
use egui::Key;
use egui_kittest::kittest::Queryable;
use fourfloor_app::state::ViewId;
use fourfloor_model::TrackKind;

#[test]
fn the_play_button_and_space_drive_the_transport() {
    let mut harness = harness(ready_app());
    harness.run_steps(2);
    assert!(!harness.state().host.readout().playing);
    harness.get_by_label("Play").click();
    harness.run_steps(15);
    let readout = harness.state().host.readout();
    assert!(readout.playing, "playing after the click");
    // 15 steps of 1/30 s at 124 BPM is about one beat.
    assert!(
        readout.position > 400.0,
        "the playhead moved: {}",
        readout.position
    );
    harness.key_press(Key::Space);
    harness.run_steps(3);
    assert!(!harness.state().host.readout().playing, "space stops");
    harness.key_press(Key::Space);
    harness.run_steps(3);
    assert!(harness.state().host.readout().playing, "space plays");
    // Playing the demo reaches the master meter.
    harness.run_steps(10);
    let [left, right] = harness.state().live.meters.master();
    assert!(
        left.max(right) > 0.05,
        "master meter shows signal: {left} {right}"
    );
}

#[test]
fn clicking_a_step_writes_the_pattern_and_undo_restores_it() {
    let mut app = ready_app();
    let kick = track(app.project(), "Kick");
    app.ui_state.selection.select_track(kick);
    app.ui_state.view = ViewId::SEQUENCER;
    let mut harness = harness(app);
    harness.run_steps(2);
    let notes = |h: &egui_kittest::Harness<'_, fourfloor_app::FourfloorApp>| {
        let project = h.state().project();
        project.track(track(project, "Kick")).expect("kick").clips[0]
            .notes()
            .len()
    };
    let before = notes(&harness);
    // Step 2 (an off-sixteenth) is empty in a four-to-the-floor pattern.
    harness.get_by_label("Step Kick 2").click();
    harness.run_steps(2);
    assert_eq!(notes(&harness), before + 1, "one note added");
    let first =
        harness.state().project().track(kick).expect("kick").clips[0].notes()[0..2].to_vec();
    // Step 2 is an off-sixteenth: the project swing (56 %) delays it.
    assert_eq!(first[1].start.get(), 240 + 29, "swung onto step 2");
    harness.get_by_label("Undo").click();
    harness.run_steps(2);
    assert_eq!(notes(&harness), before, "undo removes it");
    harness.get_by_label("Redo").click();
    harness.run_steps(2);
    assert_eq!(notes(&harness), before + 1, "redo puts it back");
}

#[test]
fn mute_solo_and_arm_buttons_change_the_track() {
    let mut harness = harness(ready_app());
    harness.run_steps(2);
    harness.get_by_label("Mute Kick").click();
    harness.run_steps(2);
    let kick = track(harness.state().project(), "Kick");
    assert!(harness.state().project().track(kick).expect("kick").mute);
    harness.get_by_label("Solo Bass").click();
    harness.run_steps(2);
    let bass = track(harness.state().project(), "Bass");
    assert!(harness.state().project().track(bass).expect("bass").solo);
    harness.get_by_label("Arm Bass").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().ui_state.armed,
        Some(bass),
        "arm is UI state, not a command"
    );
    // Ctrl+Z twice undoes solo then mute.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    let project = harness.state().project();
    assert!(!project.track(kick).expect("kick").mute && !project.track(bass).expect("bass").solo);
}

#[test]
fn view_tabs_and_shortcuts_switch_views() {
    let mut harness = harness(ready_app());
    harness.run_steps(2);
    harness.get_by_label("Mixer").click();
    harness.run_steps(2);
    assert_eq!(harness.state().ui_state.view, ViewId::MIXER);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::Num2);
    harness.run_steps(2);
    assert_eq!(harness.state().ui_state.view, ViewId::LAUNCHER);
    harness.get_by_label("Arrangement").click();
    harness.run_steps(2);
    assert_eq!(harness.state().ui_state.view, ViewId::ARRANGEMENT);
}

#[test]
fn the_browser_adds_a_device_from_the_registry() {
    let mut app = ready_app();
    let bass = track(app.project(), "Bass");
    app.ui_state.selection.select_track(bass);
    app.ui_state.view = ViewId::MIXER;
    let mut harness = harness(app);
    harness.run_steps(2);
    let devices = |h: &egui_kittest::Harness<'_, fourfloor_app::FourfloorApp>| {
        h.state().project().track(bass).expect("bass").devices.len()
    };
    let before = devices(&harness);
    harness.get_by_label("+ Add device").click();
    harness.run_steps(3);
    harness.get_by_label_contains("fx.chorus").scroll_to_me();
    harness.run_steps(3);
    harness.get_by_label_contains("fx.chorus").click();
    harness.run_steps(3);
    assert_eq!(
        devices(&harness),
        before + 1,
        "{:?}",
        harness.state().ui_state.status
    );
    let added = harness
        .state()
        .project()
        .track(bass)
        .expect("bass")
        .devices
        .last()
        .cloned()
        .expect("device");
    assert_eq!(added.kind, "fx.chorus");
    assert!(
        !added.params.is_empty(),
        "added with the descriptor's parameters"
    );
    assert_eq!(
        harness.state().ui_state.selection.device,
        Some(added.id),
        "selected in the dock"
    );
}

#[test]
fn dragging_a_clip_on_the_timeline_moves_it_by_whole_beats() {
    let mut harness = harness(ready_app());
    harness.run_steps(2);
    let canvas = harness.get_by_label("Arrangement timeline").rect();
    let kick = track(harness.state().project(), "Kick");
    let start = |h: &egui_kittest::Harness<'_, fourfloor_app::FourfloorApp>| {
        h.state().project().track(kick).expect("kick").clips[0]
            .start
            .get()
    };
    assert_eq!(start(&harness), 0);
    // The first row is the kick; the default zoom is 28 px per beat.
    let from = egui::pos2(canvas.left() + 60.0, canvas.top() + 30.0);
    drag(&mut harness, from, from + egui::vec2(4.0 * 28.0, 0.0));
    harness.run_steps(2);
    assert_eq!(start(&harness), 3840, "moved one bar");
    assert_eq!(harness.state().doc.bus().undo_label(), Some("move_clip"));
}

#[test]
fn drawing_in_the_piano_roll_adds_a_note() {
    let mut app = ready_app();
    let stabs = track(app.project(), "Stabs");
    app.ui_state.selection.select_track(stabs);
    app.ui_state.view = ViewId::PIANO_ROLL;
    let mut harness = harness(app);
    harness.run_steps(2);
    let count = |h: &egui_kittest::Harness<'_, fourfloor_app::FourfloorApp>| {
        h.state().project().track(stabs).expect("stabs").clips[0]
            .notes()
            .len()
    };
    let before = count(&harness);
    let grid = harness.get_by_label("Piano roll grid").rect();
    // Near the top-left: a high pitch on beat 1, where the stabs have no note.
    click_at(&mut harness, grid.left_top() + egui::vec2(10.0, 5.0));
    harness.run_steps(2);
    assert_eq!(
        count(&harness),
        before + 1,
        "{:?}",
        harness.state().ui_state.status
    );
}

#[test]
fn the_add_track_button_creates_a_track_and_selects_it() {
    let mut harness = harness(ready_app());
    harness.run_steps(2);
    let before = harness.state().project().tracks.len();
    harness.get_by_label("+ Bus").click();
    harness.run_steps(2);
    let project = harness.state().project();
    assert_eq!(project.tracks.len(), before + 1);
    let added = project.tracks.last().expect("track");
    assert_eq!(added.kind, TrackKind::Bus);
    assert_eq!(harness.state().ui_state.selection.track, Some(added.id));
}
