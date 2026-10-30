//! The one error type the command line reports.

use std::fmt;
use std::path::PathBuf;

use fourfloor_audio::{DecodeError, MeterError, WavError};
use fourfloor_compile::{CompileError, ExportError, RenderError};
use fourfloor_core::RegistryError;
use fourfloor_engine::EngineError;
use fourfloor_model::{ModelError, StorageError};

/// Why a `fourfloor` invocation failed.
#[derive(Debug)]
pub enum CliError {
    /// The arguments were wrong. The message says how.
    Usage(String),
    /// `new` was pointed at a folder that already holds a project.
    ProjectExists(PathBuf),
    /// The project has no clips, so there is nothing to render or play.
    EmptyProject(PathBuf),
    /// A command was refused by the edit model.
    Model(ModelError),
    /// The project folder could not be read or written.
    Storage(StorageError),
    /// The project could not be turned into an audio graph.
    Compile(CompileError),
    /// The engine refused a call.
    Engine(String),
    /// The device registry could not be built.
    Registry(RegistryError),
    /// The render is too long to hold in memory.
    RenderTooLong {
        /// Frames the render would need.
        frames: u64,
    },
    /// A file could not be read or written.
    Io {
        /// What was being attempted, for example `write`.
        action: &'static str,
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// An audio file could not be written.
    Encode {
        /// The file involved.
        path: PathBuf,
        /// The encoder's error.
        source: WavError,
    },
    /// An audio file could not be decoded.
    Decode {
        /// The file involved.
        path: PathBuf,
        /// The decoder's error.
        source: DecodeError,
    },
    /// Loudness could not be measured.
    Meter(MeterError),
    /// Stems could not be exported.
    Export(String),
    /// The audio device could not be opened, or playback was not compiled in.
    Playback(String),
    /// Audio analysis failed.
    Analysis(fourfloor_analysis::AnalysisError),
    /// A DAWproject file could not be read or written.
    Interchange(fourfloor_dawproject::DawprojectError),
    /// The sample library refused a request.
    Library(fourfloor_library::LibraryError),
    /// The plugin scan cache could not be read or written.
    Plugins(String),
    /// An agent command failed; it has already printed why.
    Agent,
}

impl CliError {
    /// Whether the user got the arguments wrong (exit code 2) rather than
    /// the command failing (exit code 1).
    pub fn is_usage(&self) -> bool {
        matches!(self, Self::Usage(_))
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::ProjectExists(dir) => {
                write!(f, "{} already contains a project", dir.display())
            }
            Self::EmptyProject(dir) => {
                write!(f, "the project in {} has no clips", dir.display())
            }
            Self::Model(error) => write!(f, "{error}"),
            Self::Storage(error) => write!(f, "{error}"),
            Self::Compile(error) => write!(f, "{error}"),
            Self::Engine(message) => write!(f, "engine error: {message}"),
            Self::Registry(error) => write!(f, "cannot set up devices: {error}"),
            Self::RenderTooLong { frames } => {
                write!(f, "a render of {frames} frames is too long")
            }
            Self::Encode { path, source } => {
                write!(f, "cannot write {}: {source}", path.display())
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "cannot {action} {}: {source}", path.display()),
            Self::Decode { path, source } => {
                write!(f, "cannot decode {}: {source}", path.display())
            }
            Self::Meter(error) => write!(f, "cannot measure loudness: {error}"),
            Self::Export(message) => write!(f, "{message}"),
            Self::Playback(message) => write!(f, "playback failed: {message}"),
            Self::Analysis(error) => write!(f, "cannot analyse: {error}"),
            Self::Interchange(error) => write!(f, "DAWproject: {error}"),
            Self::Library(error) => write!(f, "library: {error}"),
            Self::Plugins(message) => write!(f, "plugins: {message}"),
            Self::Agent => write!(f, "the agent command failed"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<ModelError> for CliError {
    fn from(error: ModelError) -> Self {
        Self::Model(error)
    }
}

impl From<StorageError> for CliError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<CompileError> for CliError {
    fn from(error: CompileError) -> Self {
        Self::Compile(error)
    }
}

impl From<RenderError> for CliError {
    fn from(error: RenderError) -> Self {
        match error {
            RenderError::Compile(error) => Self::Compile(error),
            RenderError::Engine(message) => Self::Engine(message),
            RenderError::TooLong { frames } => Self::RenderTooLong { frames },
        }
    }
}

impl From<RegistryError> for CliError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<EngineError> for CliError {
    fn from(error: EngineError) -> Self {
        // The error may own a whole compiled graph; keep only its message.
        Self::Engine(error.to_string())
    }
}

impl From<MeterError> for CliError {
    fn from(error: MeterError) -> Self {
        Self::Meter(error)
    }
}

impl From<ExportError> for CliError {
    fn from(error: ExportError) -> Self {
        match error {
            ExportError::Render(error) => error.into(),
            other => Self::Export(other.to_string()),
        }
    }
}

impl From<fourfloor_analysis::AnalysisError> for CliError {
    fn from(error: fourfloor_analysis::AnalysisError) -> Self {
        Self::Analysis(error)
    }
}

impl From<fourfloor_dawproject::DawprojectError> for CliError {
    fn from(error: fourfloor_dawproject::DawprojectError) -> Self {
        Self::Interchange(error)
    }
}

impl From<fourfloor_library::LibraryError> for CliError {
    fn from(error: fourfloor_library::LibraryError) -> Self {
        Self::Library(error)
    }
}
