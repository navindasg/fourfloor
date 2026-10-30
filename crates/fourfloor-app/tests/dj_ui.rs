//! The DJ view driven through egui_kittest: real clicks on the deck pads
//! reach the session, the "what mixes next" list ranks the library and
//! loads into the other deck, and wgpu snapshots of two and four decks
//! have content.

mod dj_common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dj_common::{deck, dj_app, load, temp_dir, write_track};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use fourfloor_app::state::ViewId;
use fourfloor_app::FourfloorApp;

fn harness(mut app: FourfloorApp) -> Harness<'static, FourfloorApp> {
    app.ui_state.view = ViewId::DJ;
    Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut FourfloorApp| app.frame(ui), app)
}

fn click(harness: &mut Harness<'_, FourfloorApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(3);
}

fn snapshot(harness: &mut Harness<'_, FourfloorApp>, name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fourfloor-app-snapshots");
    std::fs::create_dir_all(&dir).expect("snapshot folder");
    let image = harness.render().expect("wgpu renders");
    let background = image::Rgba([0x17, 0x19, 0x1d, 0xff]);
    let plain = image.pixels().filter(|p| **p == background).count();
    let colours: std::collections::HashSet<[u8; 4]> = image.pixels().map(|p| p.0).collect();
    let path = dir.join(format!("{name}.png"));
    image.save(&path).expect("png written");
    assert!(colours.len() > 60, "{name}: {} colours", colours.len());
    let total = image.pixels().count() as f64;
    assert!((plain as f64) < total * 0.8, "{name}: {plain} plain pixels");
    path
}

#[test]
fn deck_pads_play_cue_lock_and_resize_through_real_clicks() {
    let (_guard, dir) = temp_dir();
    let a = write_track(&dir, "Opener 124", 124.0, 1000.0, 20.0);
    let b = write_track(&dir, "Second 126", 126.0, 600.0, 20.0);
    let mut app = dj_app(&dir.join("features"));
    load(&mut app, 0, &a);
    load(&mut app, 1, &b);
    let mut harness = harness(app);
    harness.run_steps(3);
    click(&mut harness, "Play deck A");
    assert!(deck(harness.state(), 0).playing, "Play pad starts deck A");
    harness.run_steps(20);
    let moved = deck(harness.state(), 0).position_seconds;
    assert!(moved > 0.3, "the playhead runs: {moved}");
    click(&mut harness, "Hot cue 1 deck A");
    assert!(
        deck(harness.state(), 0).hot_cues[0].is_some(),
        "hot cue 1 set"
    );
    click(&mut harness, "Key lock deck A");
    assert!(deck(harness.state(), 0).key_lock);
    click(&mut harness, "Tempo master deck A");
    click(&mut harness, "Sync deck B");
    let b_state = deck(harness.state(), 1);
    assert_ne!(b_state.sync, fourfloor_dj::SyncMode::Off);
    let a_bpm = deck(harness.state(), 0).bpm.expect("A tempo");
    assert!(
        (b_state.bpm.expect("B tempo") - a_bpm).abs() < 0.05,
        "B follows A"
    );
    click(&mut harness, "Kill Low deck A");
    assert!(deck(harness.state(), 0).eq_kill[fourfloor_dj::EqBand::Low.index()]);
    click(&mut harness, "Headphone cue deck B");
    assert!(deck(harness.state(), 1).cue_monitor);
    click(&mut harness, "Loop 4 deck A");
    assert!(
        deck(harness.state(), 0).loop_state.is_some(),
        "4-beat loop on"
    );
    click(&mut harness, "Play deck B");
    harness.run_steps(10);
    let two = snapshot(&mut harness, "dj_two_decks");
    assert!(two.exists());

    // Four decks keep the loaded tracks.
    click(&mut harness, "4 decks");
    harness.run_steps(3);
    let dj = harness.state().features.dj.as_ref().expect("dj");
    assert_eq!(dj.deck_count(), 4);
    assert!(dj.media(0).is_some() && dj.media(1).is_some());
    harness.get_by_label("Play deck D");
    snapshot(&mut harness, "dj_four_decks");
}

#[test]
fn what_mixes_next_ranks_the_library_and_loads_the_other_deck() {
    let (_guard, dir) = temp_dir();
    let music = dir.join("music");
    std::fs::create_dir_all(&music).expect("music folder");
    let own = write_track(&music, "Playing 124", 124.0, 440.0, 60.0);
    let near = write_track(&music, "Near 125", 125.0, 440.0, 60.0);
    let far = write_track(&music, "Far 96", 96.0, 440.0, 60.0);
    let db = dir.join("library.sqlite");
    {
        let mut library = fourfloor_library::Library::open(&db).expect("library");
        for path in [&own, &near, &far] {
            library.index_file(path).expect("indexed");
        }
    }
    let mut app = dj_app(&dir.join("features"));
    app.features.library.open(&db).expect("opens");
    load(&mut app, 0, &own);
    let titles: Vec<String> = app
        .features
        .suggestions
        .iter()
        .map(|s| s.title.clone())
        .collect();
    assert_eq!(app.features.suggestions_for, Some(0));
    assert!(
        !titles.iter().any(|t| t.contains("Playing")),
        "own file left out: {titles:?}"
    );
    assert_eq!(
        titles.first().map(String::as_str),
        Some("Near 125"),
        "{titles:?}"
    );
    assert!(
        titles.iter().position(|t| t == "Far 96") > Some(0),
        "{titles:?}"
    );

    let mut harness = harness(app);
    harness.run_steps(3);
    harness
        .get_all_by_label("Load B")
        .next()
        .expect("a Load B button")
        .click();
    let start = Instant::now();
    while harness
        .state()
        .features
        .dj
        .as_ref()
        .and_then(|dj| dj.media(1))
        .is_none()
    {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "deck B did not load"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    let loaded = deck(harness.state(), 1).track.expect("B track").name;
    assert_eq!(loaded, "Near 125");
    snapshot(&mut harness, "dj_suggestions");
}
