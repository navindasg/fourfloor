//! The demo through stem export and a DAWproject round trip, checked by
//! null tests on the rendered audio.

mod demo_common;

use demo_common::*;
use fourfloor_compile::{render_stems, RenderOptions};
use fourfloor_dawproject::{export_project, import_project, ExportOptions, ImportOptions};
use fourfloor_model::command::DeleteTrack;
use fourfloor_model::Command;
use fourfloor_testkit::{assert_null, null_residual_db, StereoBuffer};

#[test]
fn stems_of_every_track_sum_to_the_master() {
    // The demo's tracks all feed a "Master" bus; take it out so each track
    // and return feeds the master and becomes a stem of its own (sends and
    // the sidechains stay).
    let project = demo();
    let master_bus = track(&project, "Master");
    let project = edit(
        &project,
        vec![Command::DeleteTrack(DeleteTrack { track: master_bus })],
    );
    let rendered = render_stems(
        &project,
        registry(),
        &RenderOptions::new(RATE).with_tail(TAIL),
    )
    .expect("stems render");
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    let names: Vec<&str> = rendered.stems.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Drums", "Bass", "Acid", "Pad", "Reverb", "Delay"]);

    let mut sum = vec![0.0f32; rendered.master.len()];
    for stem in &rendered.stems {
        assert_eq!(stem.samples.len(), sum.len(), "{}", stem.name);
        let level = rms(&stem.samples);
        assert!(level > 1e-3, "stem {} is silent", stem.name);
        for (total, sample) in sum.iter_mut().zip(&stem.samples) {
            *total += sample;
        }
    }
    let residual = null_residual_db(&sum, &rendered.master);
    eprintln!("stems minus master: {residual:.1} dB");
    assert_null(&sum, &rendered.master, -100.0);
}

#[test]
fn a_dawproject_round_trip_renders_the_same_audio() {
    let project = demo();
    let exported = export_project(
        &project,
        None,
        &ExportOptions::new().with_registry(registry()),
    )
    .expect("export");
    let folder = tempfile::tempdir().expect("tempdir");
    let imported =
        import_project(&exported.bytes, folder.path(), &ImportOptions::new()).expect("import");
    assert_eq!(imported.project.tracks.len(), project.tracks.len());
    eprintln!("{} report entries", imported.report.items.len());

    let original: &StereoBuffer = demo_mix();
    let round_trip = render(&imported.project);
    assert_eq!(round_trip.frames(), original.frames());
    let residual = null_residual_db(&round_trip.left, &original.left)
        .max(null_residual_db(&round_trip.right, &original.right));
    eprintln!("round trip minus original: {residual:.1} dB");
    assert_null(&round_trip.left, &original.left, -120.0);
    assert_null(&round_trip.right, &original.right, -120.0);
}
