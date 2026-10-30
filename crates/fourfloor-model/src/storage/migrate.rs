//! Schema versioning and the migration hook for `project.json`.
//!
//! To change the file format: bump [`CURRENT_SCHEMA_VERSION`] and add a
//! [`Migration`] to [`MIGRATIONS`] that rewrites a document of the previous
//! version into the new shape. Loading runs every step from the file's
//! version up to the current one, in order, on the raw JSON before it is
//! decoded into model structs.

use serde_json::Value;

use super::StorageError;

/// The schema version this build writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Rewrites a whole `project.json` document from one schema version to the
/// next. Returns a message on failure. The runner updates `schema_version`
/// afterwards, so the step does not need to.
pub type MigrationFn = fn(Value) -> Result<Value, String>;

/// One step in the migration chain.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// The schema version this step reads; it produces `from + 1`.
    pub from: u32,
    /// The rewrite.
    pub apply: MigrationFn,
}

/// The registered migrations. Empty while version 1 is the only format;
/// there must be exactly one step for every version below the current one.
pub const MIGRATIONS: &[Migration] = &[];

/// Reads the `schema_version` field of a project document.
pub fn schema_version(document: &Value) -> Result<u32, StorageError> {
    document
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or(StorageError::MissingSchemaVersion)
}

/// Brings a project document up to [`CURRENT_SCHEMA_VERSION`].
pub fn migrate(document: Value) -> Result<Value, StorageError> {
    migrate_with(document, MIGRATIONS, CURRENT_SCHEMA_VERSION)
}

/// Brings a project document up to `target` using `steps`.
///
/// Fails if the document is newer than `target` or a step is missing.
pub fn migrate_with(
    document: Value,
    steps: &[Migration],
    target: u32,
) -> Result<Value, StorageError> {
    let found = schema_version(&document)?;
    if found > target {
        return Err(StorageError::UnsupportedVersion {
            found,
            supported: target,
        });
    }
    (found..target).try_fold(document, |document, from| {
        let step = steps
            .iter()
            .find(|step| step.from == from)
            .ok_or(StorageError::MissingMigration { from })?;
        let migrated = (step.apply)(document)
            .map_err(|reason| StorageError::MigrationFailed { from, reason })?;
        with_version(migrated, from + 1).ok_or_else(|| StorageError::MigrationFailed {
            from,
            reason: "the migration did not return a JSON object".to_owned(),
        })
    })
}

fn with_version(document: Value, version: u32) -> Option<Value> {
    match document {
        Value::Object(fields) => Some(Value::Object(
            fields
                .into_iter()
                .filter(|(key, _)| key != "schema_version")
                .chain(std::iter::once((
                    "schema_version".to_owned(),
                    Value::from(version),
                )))
                .collect(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A made-up old format, used only to exercise the runner: version 1
    /// stored the tempo as a top-level `bpm`.
    fn bpm_to_tempo(document: Value) -> Result<Value, String> {
        let bpm = document.get("bpm").cloned().ok_or("missing bpm")?;
        Ok(json!({ "schema_version": 1, "tempo": bpm }))
    }

    /// Version 2 of the made-up format gained a `name`.
    fn add_name(document: Value) -> Result<Value, String> {
        Ok(json!({ "tempo": document["tempo"], "name": "Untitled" }))
    }

    const STEPS: &[Migration] = &[
        Migration {
            from: 1,
            apply: bpm_to_tempo,
        },
        Migration {
            from: 2,
            apply: add_name,
        },
    ];

    #[test]
    fn registered_migrations_cover_every_old_version_once() {
        let froms: Vec<u32> = MIGRATIONS.iter().map(|step| step.from).collect();
        let expected: Vec<u32> = (1..CURRENT_SCHEMA_VERSION).collect();
        assert_eq!(froms, expected);
    }

    #[test]
    fn steps_run_in_order_and_stamp_each_version() {
        let migrated = migrate_with(json!({ "schema_version": 1, "bpm": 124 }), STEPS, 3).unwrap();
        assert_eq!(
            migrated,
            json!({ "schema_version": 3, "tempo": 124, "name": "Untitled" })
        );
    }

    #[test]
    fn migration_starts_from_the_documents_version() {
        let migrated =
            migrate_with(json!({ "schema_version": 2, "tempo": 126 }), STEPS, 3).unwrap();
        assert_eq!(migrated["name"], "Untitled");
        assert_eq!(migrated["tempo"], 126);
        assert_eq!(migrated["schema_version"], 3);
    }

    #[test]
    fn a_current_document_is_returned_unchanged() {
        let document = json!({ "schema_version": 3, "tempo": 120, "name": "Mine" });
        assert_eq!(migrate_with(document.clone(), STEPS, 3).unwrap(), document);
    }

    #[test]
    fn a_newer_document_is_refused() {
        let error = migrate_with(json!({ "schema_version": 9 }), STEPS, 3).unwrap_err();
        assert!(matches!(
            error,
            StorageError::UnsupportedVersion {
                found: 9,
                supported: 3
            }
        ));
    }

    #[test]
    fn a_missing_or_malformed_version_is_refused() {
        for document in [
            json!({}),
            json!({ "schema_version": "1" }),
            json!({ "schema_version": -1 }),
            json!({ "schema_version": 1.5 }),
            json!([1]),
        ] {
            assert!(matches!(
                migrate_with(document, STEPS, 3),
                Err(StorageError::MissingSchemaVersion)
            ));
        }
    }

    #[test]
    fn a_gap_in_the_chain_is_reported() {
        let error = migrate_with(json!({ "schema_version": 0 }), STEPS, 3).unwrap_err();
        assert!(matches!(error, StorageError::MissingMigration { from: 0 }));
    }

    #[test]
    fn a_failing_step_is_reported_with_its_version() {
        let error = migrate_with(json!({ "schema_version": 1 }), STEPS, 3).unwrap_err();
        match error {
            StorageError::MigrationFailed { from, reason } => {
                assert_eq!(from, 1);
                assert_eq!(reason, "missing bpm");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn a_step_that_returns_a_non_object_is_reported() {
        fn broken(_: Value) -> Result<Value, String> {
            Ok(json!(42))
        }
        let steps = [Migration {
            from: 1,
            apply: broken,
        }];
        let error = migrate_with(json!({ "schema_version": 1 }), &steps, 2).unwrap_err();
        assert!(matches!(
            error,
            StorageError::MigrationFailed { from: 1, .. }
        ));
    }
}
