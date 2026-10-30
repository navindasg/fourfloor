//! The crate's error type.

use std::fmt;
use std::path::PathBuf;

use fourfloor_core::DeviceError;

use crate::thread::HostThreadError;

/// Why a plugin could not be scanned, loaded or driven.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// The plugin host thread is unavailable or a call on it failed.
    Thread(HostThreadError),
    /// The plugin file or bundle could not be opened.
    Load {
        /// The file or bundle.
        path: PathBuf,
        /// What the loader reported.
        reason: String,
    },
    /// The file loaded, but holds no plugin with this id.
    NotFound {
        /// The id that was asked for.
        id: String,
    },
    /// A plugin id is not valid for its format.
    InvalidId(String),
    /// The plugin refused to be created.
    Instantiate(String),
    /// The plugin refused to start at the requested sample rate and block.
    Activate(String),
    /// Saving or restoring the plugin's state failed.
    State(String),
    /// The plugin, or this build, does not support the operation.
    Unsupported(String),
    /// Reading or writing the plugin database failed.
    Io(String),
    /// The plugin database file is not valid.
    Database(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thread(error) => write!(f, "{error}"),
            Self::Load { path, reason } => {
                write!(f, "cannot load plugin file {}: {reason}", path.display())
            }
            Self::NotFound { id } => write!(f, "no plugin with id \"{id}\" was found"),
            Self::InvalidId(id) => write!(f, "\"{id}\" is not a valid plugin id"),
            Self::Instantiate(reason) => write!(f, "cannot create the plugin: {reason}"),
            Self::Activate(reason) => write!(f, "cannot activate the plugin: {reason}"),
            Self::State(reason) => write!(f, "plugin state error: {reason}"),
            Self::Unsupported(what) => write!(f, "not supported: {what}"),
            Self::Io(reason) => write!(f, "plugin database I/O error: {reason}"),
            Self::Database(reason) => write!(f, "invalid plugin database: {reason}"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Thread(error) => Some(error),
            _ => None,
        }
    }
}

impl From<HostThreadError> for HostError {
    fn from(error: HostThreadError) -> Self {
        Self::Thread(error)
    }
}

impl From<HostError> for DeviceError {
    fn from(error: HostError) -> Self {
        DeviceError::new(error.to_string())
    }
}
