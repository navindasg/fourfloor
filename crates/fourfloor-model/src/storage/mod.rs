//! The on-disk project folder.
//!
//! ```text
//! <project>/
//!   project.json     {"schema_version": 1, "app": "fourfloor", "project": {...}}
//!   audio/           <blake3 hex>.<ext>, one file per distinct content
//!   plugin-state/    <blake3 hex>.bin, opaque plugin state blobs
//! ```
//!
//! `project.json` is written to a temporary file and renamed into place, so
//! a crash mid-save leaves the previous file intact. Loading migrates older
//! schema versions first (see [`migrate`]) and then checks every model
//! invariant, because the file may have been edited by hand.

mod migrate;
mod pool;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::hash::ContentHash;
use crate::project::Project;

pub use migrate::{
    migrate, migrate_with, schema_version, Migration, MigrationFn, CURRENT_SCHEMA_VERSION,
    MIGRATIONS,
};
pub use pool::{AudioPool, PluginStateStore};

/// File name of the project document inside the project folder.
pub const PROJECT_FILE: &str = "project.json";
/// Directory name of the audio pool inside the project folder.
pub const AUDIO_DIR: &str = "audio";
/// Directory name of the plugin state store inside the project folder.
pub const PLUGIN_STATE_DIR: &str = "plugin-state";
/// Value of the `app` field in `project.json`.
pub const APP_NAME: &str = "fourfloor";

/// Why saving or loading failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// A file system operation failed.
    #[error("cannot {action} {}: {source}", .path.display())]
    Io {
        /// What was being attempted, for example `read`.
        action: &'static str,
        /// The file or directory involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The project file is not JSON or does not have the expected shape.
    #[error("project file is malformed: {0}")]
    Json(#[from] serde_json::Error),
    /// The project file has no usable `schema_version` number.
    #[error("project file has no valid schema_version")]
    MissingSchemaVersion,
    /// The project was saved by a newer version of the app.
    #[error("project uses schema version {found}; this build reads up to {supported}")]
    UnsupportedVersion {
        /// Version found in the file.
        found: u32,
        /// Newest version this build understands.
        supported: u32,
    },
    /// No migration is registered for an old schema version.
    #[error("no migration from schema version {from}")]
    MissingMigration {
        /// The version that could not be migrated.
        from: u32,
    },
    /// A migration step reported an error.
    #[error("migration from schema version {from} failed: {reason}")]
    MigrationFailed {
        /// The version being migrated.
        from: u32,
        /// The step's message.
        reason: String,
    },
    /// The project decoded but breaks a model invariant.
    #[error("project is inconsistent: {0}")]
    InvalidProject(#[from] ModelError),
    /// A file extension is not 1 to 8 letters or digits.
    #[error("unsupported file extension `{0}`")]
    InvalidExtension(String),
    /// A stored file's contents do not match the hash in its name.
    #[error("{} is corrupt: expected hash {expected}, found {actual}", .path.display())]
    HashMismatch {
        /// The corrupt file.
        path: PathBuf,
        /// The hash the file should have.
        expected: ContentHash,
        /// The hash of what is on disk.
        actual: ContentHash,
    },
}

pub(crate) fn io_error(
    action: &'static str,
    path: &Path,
) -> impl FnOnce(io::Error) -> StorageError {
    let path = path.to_path_buf();
    move |source| StorageError::Io {
        action,
        path,
        source,
    }
}

/// The document stored in `project.json`.
#[derive(Serialize, Deserialize)]
struct ProjectFile {
    schema_version: u32,
    app: String,
    project: Project,
}

/// Serializes a project to the text of a `project.json`.
///
/// Fails if the project breaks a model invariant, so an inconsistent
/// project is never written.
pub fn project_to_json(project: &Project) -> Result<String, StorageError> {
    project.validate()?;
    let file = ProjectFile {
        schema_version: CURRENT_SCHEMA_VERSION,
        app: APP_NAME.to_owned(),
        project: project.clone(),
    };
    Ok(serde_json::to_string_pretty(&file)?)
}

/// Parses the text of a `project.json`, migrating older schema versions and
/// validating the result.
pub fn project_from_json(text: &str) -> Result<Project, StorageError> {
    let document: serde_json::Value = serde_json::from_str(text)?;
    let current = migrate(document)?;
    let file: ProjectFile = serde_json::from_value(current)?;
    file.project.validate()?;
    Ok(file.project)
}

/// Saves a project into `dir`, creating the folder layout if needed.
pub fn save_project(dir: &Path, project: &Project) -> Result<(), StorageError> {
    let text = project_to_json(project)?;
    for folder in [
        dir.to_path_buf(),
        dir.join(AUDIO_DIR),
        dir.join(PLUGIN_STATE_DIR),
    ] {
        fs::create_dir_all(&folder).map_err(io_error("create", &folder))?;
    }
    let target = dir.join(PROJECT_FILE);
    let temporary = dir.join(format!("{PROJECT_FILE}.tmp"));
    write_synced(&temporary, text.as_bytes())
        .and_then(|()| fs::rename(&temporary, &target).map_err(io_error("replace", &target)))
        .inspect_err(|_| {
            // Best effort: the original error is the one worth reporting.
            let _ = fs::remove_file(&temporary);
        })
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let mut file = fs::File::create(path).map_err(io_error("create", path))?;
    file.write_all(bytes).map_err(io_error("write", path))?;
    file.sync_all().map_err(io_error("flush", path))
}

/// Loads the project stored in `dir`.
pub fn load_project(dir: &Path) -> Result<Project, StorageError> {
    let path = dir.join(PROJECT_FILE);
    let text = fs::read_to_string(&path).map_err(io_error("read", &path))?;
    project_from_json(&text)
}

#[cfg(test)]
mod tests;
