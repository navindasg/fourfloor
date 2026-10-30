//! Errors of the agent layer.

use fourfloor_core::RegistryError;
use fourfloor_model::{ModelError, StorageError};

use crate::generate::{GenerateError, KeyParseError};

/// Why a tool call, resource read or instruction failed.
///
/// Every failure leaves the project as it was: edits run through the
/// command bus, which rejects a whole step or applies it whole.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// No tool has this name.
    #[error("unknown tool \"{0}\"")]
    UnknownTool(String),
    /// No resource has this URI.
    #[error("unknown resource \"{0}\"")]
    UnknownResource(String),
    /// The arguments did not match the tool's schema.
    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArgs {
        /// The tool.
        tool: String,
        /// What was wrong.
        reason: String,
    },
    /// A track, device or clip named in the arguments does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The model rejected an edit.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// A pattern generator refused its options.
    #[error(transparent)]
    Generate(#[from] GenerateError),
    /// A key name could not be read.
    #[error(transparent)]
    Key(#[from] KeyParseError),
    /// The device registry refused a request.
    #[error("device registry: {0:?}")]
    Registry(RegistryError),
    /// Saving or loading the project folder failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Rendering, exporting or measuring failed.
    #[error("render failed: {0}")]
    Render(String),
    /// A file path was refused or could not be written.
    #[error("{0}")]
    Path(String),
    /// The natural-language layer could not turn the text into tool calls.
    #[error("{0}")]
    Parse(String),
    /// The language-model backend failed.
    #[error("language model: {0}")]
    Llm(String),
    /// Nothing to undo or redo, or another precondition failed.
    #[error("{0}")]
    Unavailable(String),
}

impl From<RegistryError> for AgentError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl AgentError {
    /// An [`AgentError::InvalidArgs`] for `tool`.
    pub fn invalid(tool: &str, reason: impl Into<String>) -> Self {
        Self::InvalidArgs {
            tool: tool.to_owned(),
            reason: reason.into(),
        }
    }
}
