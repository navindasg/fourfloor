//! The error type of the library.

use std::path::PathBuf;

use fourfloor_analysis::AnalysisError;
use fourfloor_audio::{DecodeError, WavError};

/// Why a library operation failed.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// SQLite reported an error.
    #[error("library database error: {0}")]
    Database(#[from] rusqlite::Error),
    /// A file or folder could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The database was written by a newer fourfloor than this one.
    #[error(
        "library database is schema version {found}, this build reads up to {supported}; \
         open it with a newer fourfloor"
    )]
    SchemaTooNew {
        /// Version found in the file.
        found: i64,
        /// Newest version this build understands.
        supported: i64,
    },
    /// The file is a SQLite database but not a fourfloor library.
    #[error("{0} is not a fourfloor library database")]
    NotALibrary(String),
    /// A stored blob (waveform, features, embedding) could not be decoded.
    #[error("stored {what} is corrupt: {reason}")]
    CorruptBlob {
        /// Which blob.
        what: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// A stored column holds a value this build does not understand.
    #[error("stored {column} has an unknown value {value:?}")]
    UnknownValue {
        /// Which column.
        column: &'static str,
        /// The value found.
        value: String,
    },
    /// No file, crate or playlist has this id or name.
    #[error("{what} {id} is not in the library")]
    NotFound {
        /// What was looked up.
        what: &'static str,
        /// Its id or name.
        id: String,
    },
    /// A name or argument was not acceptable.
    #[error("invalid {what}: {reason}")]
    Invalid {
        /// What was supplied.
        what: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// A path is not valid UTF-8, so it cannot be stored.
    #[error("{0} is not a UTF-8 path")]
    NonUtf8Path(PathBuf),
    /// The file has no stored feature vector to compare with.
    #[error("file {0} has no feature vector")]
    NoFeatures(i64),
    /// An audio file could not be decoded.
    #[error("could not decode audio: {0}")]
    Decode(#[from] DecodeError),
    /// An audio file could not be analysed.
    #[error("could not analyse audio: {0}")]
    Analysis(#[from] AnalysisError),
    /// A factory sample could not be written.
    #[error("could not write audio: {0}")]
    Wav(#[from] WavError),
    /// A scan worker thread died.
    #[error("a scan worker stopped unexpectedly")]
    WorkerFailed,
}

impl LibraryError {
    /// An I/O error tagged with the path it happened on.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn not_found(what: &'static str, id: impl ToString) -> Self {
        Self::NotFound {
            what,
            id: id.to_string(),
        }
    }

    pub(crate) fn invalid(what: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            what,
            reason: reason.into(),
        }
    }
}
