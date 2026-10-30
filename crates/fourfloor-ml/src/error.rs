//! The error type shared by every ML feature.

use std::path::PathBuf;

/// Which license field of a model entry was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LicenseField {
    /// The license of the model's code.
    Code,
    /// The license of the trained weights.
    Weights,
}

impl core::fmt::Display for LicenseField {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Code => "code",
            Self::Weights => "weights",
        })
    }
}

/// Anything that can go wrong in the ML layer.
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    /// The model's license is not on the allow-list, so it is never
    /// registered, downloaded or loaded.
    #[error("model `{model}` refused: {field} license `{license}` is not on the allow-list (MIT, Apache-2.0, BSD, CC0, CC BY)")]
    LicenseRefused {
        /// Model name.
        model: String,
        /// Which license failed.
        field: LicenseField,
        /// The license text as written in the entry.
        license: String,
    },
    /// A registry entry is malformed.
    #[error("model `{model}` has an invalid entry: {reason}")]
    InvalidEntry {
        /// Model name.
        model: String,
        /// What is wrong.
        reason: String,
    },
    /// A model with this name is already registered.
    #[error("model `{0}` is already registered")]
    DuplicateModel(String),
    /// No model with this name is registered.
    #[error("no model named `{0}` is registered")]
    UnknownModel(String),
    /// The downloaded or cached file does not match the recorded checksum.
    #[error("checksum mismatch for `{file}`: expected sha256 {expected}, got {actual}")]
    Checksum {
        /// File name inside the model.
        file: String,
        /// The recorded digest.
        expected: String,
        /// The digest of the bytes received.
        actual: String,
    },
    /// The downloaded file does not have the recorded size.
    #[error("size mismatch for `{file}`: expected {expected} bytes, got {actual}")]
    Size {
        /// File name inside the model.
        file: String,
        /// The recorded size.
        expected: u64,
        /// The number of bytes received.
        actual: u64,
    },
    /// The file is not cached and the cache is offline.
    #[error("model file `{file}` is not cached and downloads are disabled")]
    Offline {
        /// File name inside the model.
        file: String,
    },
    /// A download failed.
    #[error("download of {url} failed: {message}")]
    Download {
        /// The URL requested.
        url: String,
        /// What the transport reported.
        message: String,
    },
    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The inference runtime rejected the model or a run.
    #[error("model `{model}` failed: {message}")]
    Inference {
        /// Model name.
        model: String,
        /// What the runtime reported.
        message: String,
    },
    /// The caller passed audio or options the feature cannot use.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The feature is not available in this build or with this backend
    /// (for example text queries on the DSP embedding fallback).
    #[error("not supported: {0}")]
    Unsupported(String),
    /// An audio file in a cache could not be read or written.
    #[error("cache audio: {0}")]
    Audio(String),
    /// The background worker has shut down.
    #[error("the ML worker thread has stopped")]
    WorkerStopped,
}

impl MlError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn inference(model: &str, error: impl core::fmt::Display) -> Self {
        Self::Inference {
            model: model.to_owned(),
            message: format!("{error:#}"),
        }
    }
}
