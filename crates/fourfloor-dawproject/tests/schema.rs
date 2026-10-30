//! Exported XML checked against the official DAWproject schemas
//! (`tests/fixtures/Project.xsd` and `MetaData.xsd`, copied unchanged from
//! github.com/bitwig/dawproject, MIT).
//!
//! Two checks: a full XSD validation with `xmllint` (libxml2) when it is
//! installed, and an always-on structural check driven by the same XSD
//! (`common::xsd`): element names, child order and counts, required and
//! unknown attributes, enumerated attribute values and IDREF targets.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::xsd::Schema;
use common::{plain_project, registry, rich_project};
use fourfloor_dawproject::container::{Container, METADATA_XML, PROJECT_XML};
use fourfloor_dawproject::{export_project, ExportOptions, Metadata};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The exported `project.xml` and `metadata.xml` of the rich project, in
/// both flavours (with and without the extension file).
fn exported_documents() -> Vec<(String, String, String)> {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let rich = rich_project(&registry, dir.path());
    let plain = plain_project(&rich);
    let metadata = Metadata {
        title: Some("Schema <Check> & Co".to_owned()),
        artist: Some("Four Floor".to_owned()),
        comment: Some("line one\nline two".to_owned()),
        ..Metadata::default()
    };
    let mut out = Vec::new();
    for (label, project, extras) in [("rich", &rich, true), ("plain", &plain, false)] {
        let options = ExportOptions::new()
            .with_registry(&registry)
            .with_extras(extras)
            .with_metadata(&metadata);
        let exported = export_project(project, Some(dir.path()), &options).unwrap();
        let files = Container::read(&exported.bytes, u64::MAX).unwrap();
        let text = |name: &str| String::from_utf8(files.get(name).unwrap().to_vec()).unwrap();
        out.push((label.to_owned(), text(PROJECT_XML), text(METADATA_XML)));
    }
    out
}

fn xmllint() -> Option<PathBuf> {
    let output = Command::new("xmllint").arg("--version").output().ok()?;
    output.status.success().then(|| PathBuf::from("xmllint"))
}

#[test]
fn exported_xml_passes_xmllint_against_the_official_xsd() {
    let Some(xmllint) = xmllint() else {
        eprintln!("xmllint is not installed; full XSD validation skipped (the structural check still runs)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    for (label, project_xml, metadata_xml) in exported_documents() {
        for (name, text, schema) in [
            ("project", &project_xml, "Project.xsd"),
            ("metadata", &metadata_xml, "MetaData.xsd"),
        ] {
            let path = dir.path().join(format!("{label}-{name}.xml"));
            std::fs::write(&path, text).unwrap();
            let output = Command::new(&xmllint)
                .arg("--noout")
                .arg("--schema")
                .arg(fixture(schema))
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{label} {name}.xml fails {schema}:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn exported_xml_matches_the_schema_structure() {
    let project_schema = Schema::load(&fixture("Project.xsd"));
    let metadata_schema = Schema::load(&fixture("MetaData.xsd"));
    for (label, project_xml, metadata_xml) in exported_documents() {
        let errors = project_schema.check(&project_xml, "Project");
        assert!(
            errors.is_empty(),
            "{label} project.xml:\n{}",
            errors.join("\n")
        );
        let errors = metadata_schema.check(&metadata_xml, "MetaData");
        assert!(
            errors.is_empty(),
            "{label} metadata.xml:\n{}",
            errors.join("\n")
        );
    }
}

#[test]
fn the_import_fixtures_are_schema_valid() {
    let schema = Schema::load(&fixture("Project.xsd"));
    for name in ["bitwig-readme-example.xml", "handwritten.xml"] {
        let text = std::fs::read_to_string(fixture(name)).unwrap();
        let errors = schema.check(&text, "Project");
        assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
        if let Some(xmllint) = xmllint() {
            let status = Command::new(xmllint)
                .arg("--noout")
                .arg("--schema")
                .arg(fixture("Project.xsd"))
                .arg(fixture(name))
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "{name} fails xmllint");
        }
    }
}

#[test]
fn the_structural_check_catches_schema_violations() {
    let schema = Schema::load(&fixture("Project.xsd"));
    let (_, good, _) = exported_documents().remove(0);
    assert!(schema.check(&good, "Project").is_empty());

    // Children out of order: Structure before Transport.
    let swapped = good.replacen("<Transport>", "<Structure></Structure><Transport>", 1);
    assert!(!schema.check(&swapped, "Project").is_empty());
    // An element the schema does not know.
    let unknown = good.replacen("<Transport>", "<Transport><Bogus/>", 1);
    assert!(!schema.check(&unknown, "Project").is_empty());
    // A missing required attribute (Project@version).
    let unversioned = good.replacen(r#"<Project version="1.0">"#, "<Project>", 1);
    assert!(!schema.check(&unversioned, "Project").is_empty());
    // An attribute value outside its enumeration.
    let role = good.replacen(r#"role="regular""#, r#"role="lead""#, 1);
    assert!(!schema.check(&role, "Project").is_empty());
    // A reference to an id that does not exist.
    let dangling = good.replacen(r#"destination=""#, r#"destination="nowhere"#, 1);
    assert!(!schema.check(&dangling, "Project").is_empty());
}
