//! Errors of DAWproject import and export.

use std::io;
use std::path::PathBuf;

use fourfloor_model::ModelError;

/// Why a DAWproject file could not be read or written.
///
/// Things the two formats merely disagree on are not errors: they are
/// listed in the [`crate::Report`] that every import and export returns.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DawprojectError {
    /// A file system operation failed.
    #[error("cannot {action} {}: {source}", .path.display())]
    Io {
        /// What was being attempted, for example `read`.
        action: &'static str,
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file is not a readable ZIP archive, or writing one failed.
    #[error("ZIP container error: {0}")]
    Zip(String),
    /// The archive has no entry with this name.
    #[error("the archive has no `{0}` entry")]
    MissingEntry(String),
    /// An archive entry is larger than the importer accepts.
    #[error("archive entry `{name}` is {size} bytes, over the limit of {limit}")]
    EntryTooLarge {
        /// The entry.
        name: String,
        /// Its uncompressed size.
        size: u64,
        /// The limit it broke.
        limit: u64,
    },
    /// `project.xml` is not well-formed XML or not UTF-8.
    #[error("project.xml is not well-formed: {0}")]
    Xml(String),
    /// The XML is well-formed but is not a DAWproject document.
    #[error("not a DAWproject document: {0}")]
    NotDawproject(String),
    /// The project to export breaks a model invariant, or the imported
    /// project could not be made consistent.
    #[error("project is inconsistent: {0}")]
    Model(#[from] ModelError),
    /// Storing audio or plugin state in the project folder failed.
    #[error("project folder error: {0}")]
    Storage(#[from] fourfloor_model::StorageError),
    /// The fourfloor extension data could not be encoded.
    #[error("extension data error: {0}")]
    Extras(#[from] serde_json::Error),
}

pub(crate) fn io_error(
    action: &'static str,
    path: &std::path::Path,
) -> impl FnOnce(io::Error) -> DawprojectError {
    let path = path.to_path_buf();
    move |source| DawprojectError::Io {
        action,
        path,
        source,
    }
}
