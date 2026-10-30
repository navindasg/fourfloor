//! Intents of the DJ view, the sample browser, the AI panel and the
//! settings. Like every intent, the app applies them after the frame.

use std::net::SocketAddr;
use std::path::PathBuf;

use fourfloor_core::ClipId;
use fourfloor_dj::DjCommand;
use fourfloor_library::{FileId, PreviewMode, Query};
use serde_json::Value;

use crate::services::assistant::LlmConfig;

/// DJ requests.
#[derive(Clone, Debug, PartialEq)]
pub enum DjIntent {
    /// Start the decks (they are built on first use).
    Start,
    /// Change the number of decks (2 to 4).
    SetDecks(usize),
    /// A deck or mixer command.
    Command(DjCommand),
    /// Decode, analyse and load a file into a deck.
    LoadFile {
        /// The deck.
        deck: usize,
        /// The file.
        path: PathBuf,
    },
    /// Recompute "what mixes next" for a deck.
    Suggest(usize),
    /// Start recording the mix to a WAV file.
    Record(PathBuf),
}

/// Sample browser requests.
#[derive(Clone, Debug, PartialEq)]
pub enum LibraryIntent {
    /// Open the library (at the configured path) if it is not open.
    Open,
    /// Scan a folder into the library.
    Scan(PathBuf),
    /// Rescan every folder scanned before.
    Rescan,
    /// Stop a running scan.
    CancelScan,
    /// Write the factory samples and scan them.
    InstallFactory,
    /// Run a filter query.
    Search(Query),
    /// Select a file (reads its waveform).
    Select(FileId),
    /// List files that sound like this one.
    FindSimilar(FileId, Query),
    /// Search by description through audio-text embeddings.
    TextSearch(String),
    /// Compute audio-text embeddings for files that lack them.
    Embed,
    /// Audition a file at the project tempo, moved to a key (the project
    /// has no key of its own, so the browser chooses one).
    Audition(FileId, PreviewMode, Option<fourfloor_analysis::Key>),
    /// Stop auditioning.
    StopAudition,
    /// Put a file on the selected audio track at the playhead.
    AddToTrack(FileId),
    /// Load a file into the selected track's sampler (adds one if needed).
    LoadSampler(FileId),
    /// Load a file into a DJ deck.
    LoadDeck(FileId, usize),
}

/// AI panel requests.
#[derive(Clone, Debug, PartialEq)]
pub enum AssistantIntent {
    /// Carry out a typed instruction.
    Instruct(String),
    /// Run one agent tool with arguments.
    Tool(String, Value),
    /// Undo the assistant's step with this undo depth and label.
    UndoStep {
        /// Undo depth right after the step.
        depth: usize,
        /// Its undo label.
        label: String,
    },
    /// Report the mix against the loaded reference (or alone).
    MixReport,
    /// Start the MCP server at an address.
    StartMcp(SocketAddr),
    /// Stop the MCP server.
    StopMcp,
    /// Use (or stop using) a local chat model for what the phrase parser
    /// does not understand.
    SetLlm(Option<LlmConfig>),
    /// Clear the chat log.
    ClearLog,
}

/// Machine-learning requests on clips and models.
#[derive(Clone, Debug, PartialEq)]
pub enum MlIntent {
    /// Separate an audio clip into stems on new tracks.
    Separate {
        /// The clip.
        clip: ClipId,
        /// Use the Spleeter model when downloaded.
        prefer_model: bool,
    },
    /// Transcribe an audio clip to a new MIDI track.
    Transcribe(ClipId),
    /// Download a model by registry name.
    Download(String),
    /// Re-read which models are downloaded.
    Refresh,
}

/// Settings requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsIntent {
    /// Rescan the system's plugins into the cache.
    ScanPlugins,
    /// Re-read the plugin cache.
    ReloadPlugins,
}
