//! Export a fourfloor project, import it again, and compare the model and
//! the rendered audio.

mod common;

use common::{plain_project, registry, render, renumber, rich_project};
use fourfloor_dawproject::container::{Container, EXTRAS_JSON, PROJECT_XML};
use fourfloor_dawproject::{
    export_project, export_to_file, import_file, import_project, ExportOptions, ImportOptions,
    Metadata, Outcome, Subject,
};
use fourfloor_model::{AudioPool, PluginStateStore};
use fourfloor_testkit::{assert_buffer_ok, null_residual_db, rms_db};

#[test]
fn a_rich_project_comes_back_unchanged_and_renders_identically() {
    let registry = registry();
    let source_dir = tempfile::tempdir().unwrap();
    let project = rich_project(&registry, source_dir.path());
    let options = ExportOptions::new().with_registry(&registry);
    let exported = export_project(&project, Some(source_dir.path()), &options).unwrap();

    // What the export could not say in plain DAWproject is listed, and
    // nothing is missing.
    let report = &exported.report;
    assert!(
        report.with_outcome(Outcome::Missing).next().is_none(),
        "{report}"
    );
    assert!(
        report.with_outcome(Outcome::Dropped).next().is_none(),
        "{report}"
    );
    for subject in [
        Subject::Project,
        Subject::Modulation,
        Subject::Scene,
        Subject::Device,
        Subject::Clip,
    ] {
        assert!(
            report.has(Outcome::Preserved, subject),
            "no preserved {subject:?}: {report}"
        );
    }
    assert!(
        report.has(Outcome::Approximated, Subject::Automation),
        "{report}"
    );

    let target_dir = tempfile::tempdir().unwrap();
    let imported =
        import_project(&exported.bytes, target_dir.path(), &ImportOptions::new()).unwrap();
    assert!(imported.report.is_empty(), "{}", imported.report);
    assert_eq!(imported.project, project);
    assert_eq!(imported.metadata.title.as_deref(), Some("Rich Project"));

    // Audio and plugin state landed in the new folder's pools.
    assert!(AudioPool::new(target_dir.path())
        .missing(&imported.project)
        .is_empty());
    let blob = imported.project.tracks[5].devices[1]
        .state_blob
        .clone()
        .unwrap();
    assert_eq!(
        PluginStateStore::new(target_dir.path()).get(&blob).unwrap(),
        b"opaque plugin state \x00\x01\x02"
    );

    let before = render(&project, &registry, source_dir.path());
    let after = render(&imported.project, &registry, target_dir.path());
    assert_buffer_ok(&before);
    assert!(rms_db(&before.mono()) > -40.0, "the render is silent");
    assert_eq!(before.frames(), after.frames());
    for (a, b) in before.channels().into_iter().zip(after.channels()) {
        let residual = null_residual_db(a, b);
        assert!(residual <= -140.0, "residual {residual} dB");
    }
}

#[test]
fn a_plain_project_round_trips_through_the_xml_alone() {
    let registry = registry();
    let source_dir = tempfile::tempdir().unwrap();
    let project = plain_project(&rich_project(&registry, source_dir.path()));
    let options = ExportOptions::new().with_extras(false);
    let exported = export_project(&project, Some(source_dir.path()), &options).unwrap();
    let files = Container::read(&exported.bytes, u64::MAX).unwrap();
    assert!(!files.contains(EXTRAS_JSON));

    let target_dir = tempfile::tempdir().unwrap();
    let imported =
        import_project(&exported.bytes, target_dir.path(), &ImportOptions::new()).unwrap();
    assert!(imported.report.is_empty(), "{}", imported.report);
    assert_eq!(imported.project, renumber(&project));

    let before = render(&project, &registry, source_dir.path());
    let after = render(&imported.project, &registry, target_dir.path());
    assert!(rms_db(&before.mono()) > -40.0);
    for (a, b) in before.channels().into_iter().zip(after.channels()) {
        let residual = null_residual_db(a, b);
        assert!(residual <= -140.0, "residual {residual} dB");
    }
}

#[test]
fn without_the_extension_file_everything_lost_is_reported() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = rich_project(&registry, dir.path());
    let exported = export_project(
        &project,
        Some(dir.path()),
        &ExportOptions::new().with_extras(false),
    )
    .unwrap();
    let report = &exported.report;
    for subject in [
        Subject::Project,
        Subject::Modulation,
        Subject::Scene,
        Subject::Device,
        Subject::Clip,
    ] {
        assert!(
            report.has(Outcome::Dropped, subject),
            "no dropped {subject:?}: {report}"
        );
    }
    let dropped = report.with_outcome(Outcome::Dropped).count();
    // Swing, quantize, 1 modulator, 1 macro, mappings, 1 sidechain, 1 gain.
    assert_eq!(dropped, 7, "{report}");

    // The import still gives a valid, playable project.
    let target = tempfile::tempdir().unwrap();
    let imported = import_project(&exported.bytes, target.path(), &ImportOptions::new()).unwrap();
    imported.project.validate().unwrap();
    assert_eq!(imported.project.tracks.len(), project.tracks.len());
    let rendered = render(&imported.project, &registry, target.path());
    assert!(rms_db(&rendered.mono()) > -40.0);
}

#[test]
fn ignoring_the_extension_file_on_import_still_sounds_the_same_where_xml_can_say_it() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = plain_project(&rich_project(&registry, dir.path()));
    let exported = export_project(&project, Some(dir.path()), &ExportOptions::new()).unwrap();
    let target = tempfile::tempdir().unwrap();
    let imported = import_project(
        &exported.bytes,
        target.path(),
        &ImportOptions::new().with_extras(false),
    )
    .unwrap();
    assert_eq!(imported.project, renumber(&project));
}

#[test]
fn without_a_project_folder_media_is_reported_missing() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = rich_project(&registry, dir.path());
    let exported = export_project(&project, None, &ExportOptions::new()).unwrap();
    assert!(
        exported.report.has(Outcome::Missing, Subject::Audio),
        "{}",
        exported.report
    );
    assert!(exported.report.has(Outcome::Missing, Subject::Clip));
    assert!(exported.report.has(Outcome::Missing, Subject::PluginState));
    let files = Container::read(&exported.bytes, u64::MAX).unwrap();
    assert!(files.names().all(|name| !name.starts_with("audio/")));
}

#[test]
fn files_on_disk_and_metadata_round_trip() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = plain_project(&rich_project(&registry, dir.path()));
    let metadata = Metadata {
        title: Some("Night Drive".to_owned()),
        artist: Some("Four Floor".to_owned()),
        genre: Some("Deep House".to_owned()),
        ..Metadata::default()
    };
    let path = dir.path().join("song.dawproject");
    let options = ExportOptions::new().with_metadata(&metadata);
    export_to_file(&project, Some(dir.path()), &path, &options).unwrap();
    let target = tempfile::tempdir().unwrap();
    let imported = import_file(&path, target.path(), &ImportOptions::new()).unwrap();
    assert_eq!(imported.metadata, metadata);
    // The extension file keeps the project name even when the title differs.
    assert_eq!(imported.project.name, project.name);
    assert_eq!(imported.project, project);
}

#[test]
fn the_same_project_always_gives_the_same_bytes() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = rich_project(&registry, dir.path());
    let a = export_project(&project, Some(dir.path()), &ExportOptions::new()).unwrap();
    let b = export_project(&project, Some(dir.path()), &ExportOptions::new()).unwrap();
    assert_eq!(a.bytes, b.bytes);
    let files = Container::read(&a.bytes, u64::MAX).unwrap();
    assert_eq!(
        files.names().next(),
        Some("audio/".to_owned() + &project.audio_refs()[0].file_name()).as_deref()
    );
    assert!(files.contains(PROJECT_XML));
}
