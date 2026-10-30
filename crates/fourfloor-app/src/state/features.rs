//! State of the DJ decks, the sample browser, the assistant and the
//! settings lists: owned by the app, read (never written) by the views.

use std::path::PathBuf;

use fourfloor_library::FileId;

use crate::services::assistant::{ChatEntry, LlmConfig, McpServer};
use crate::services::dj::DjService;
use crate::services::library::LibraryService;
use crate::services::mix_report::MixReport;
use crate::services::ml::ModelStatus;
use crate::services::plugins::PluginList;
use crate::viewmodel::dj::NextTrack;

/// Where the feature services keep their files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeaturePaths {
    /// The library database (`None`: the browser is off).
    pub library: Option<PathBuf>,
    /// Folder the factory samples are written to.
    pub factory: Option<PathBuf>,
    /// The model cache.
    pub models: PathBuf,
    /// The plugin scan cache (`None`: plugins are not listed).
    pub plugins: Option<PathBuf>,
    /// The DJ analysis cache.
    pub dj_cache: Option<PathBuf>,
    /// Where the assistant writes renders when the project is unsaved.
    pub scratch: PathBuf,
}

impl FeaturePaths {
    /// The user's real locations: the library and DJ cache under the app's
    /// data folder, the shared model cache and the plugin scan cache.
    pub fn user() -> Self {
        let data = crate::services::library::data_dir();
        Self {
            library: data.as_ref().map(|d| d.join("library.sqlite")),
            factory: data.as_ref().map(|d| d.join("Factory Samples")),
            models: fourfloor_ml::ModelCache::default_dir(),
            plugins: fourfloor_host::PluginDatabase::default_path(),
            dj_cache: data.as_ref().map(|d| d.join("dj-analysis")),
            scratch: std::env::temp_dir().join("fourfloor-assistant"),
        }
    }

    /// Everything under `root`, for tests and headless runs, so nothing
    /// outside it is read or written.
    pub fn under(root: &std::path::Path) -> Self {
        Self {
            library: Some(root.join("library.sqlite")),
            factory: Some(root.join("factory")),
            models: root.join("models"),
            plugins: Some(root.join("plugins.json")),
            dj_cache: Some(root.join("dj-cache")),
            scratch: root.join("scratch"),
        }
    }

    /// No library and no plugin list; models and renders in a private
    /// temporary folder.
    pub fn none() -> Self {
        let root = std::env::temp_dir().join(format!("fourfloor-{}", std::process::id()));
        Self {
            library: None,
            factory: None,
            models: root.join("models"),
            plugins: None,
            dj_cache: None,
            scratch: root.join("scratch"),
        }
    }
}

/// What a running scan is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanState {
    /// A scan is running.
    pub running: bool,
    /// Summary of the last scan.
    pub last: Option<String>,
}

/// Everything the feature views read.
#[derive(Debug, Default)]
pub struct Features {
    /// The DJ session, once started.
    pub dj: Option<DjService>,
    /// "What mixes next" for `suggestions_for`.
    pub suggestions: Vec<NextTrack>,
    /// The deck the suggestions follow.
    pub suggestions_for: Option<usize>,
    /// Decks waiting for a track to finish loading.
    pub loading_decks: Vec<usize>,
    /// The sample library.
    pub library: LibraryService,
    /// Scan progress.
    pub scan: ScanState,
    /// The file being auditioned.
    pub auditioning: Option<FileId>,
    /// The chat log, oldest first.
    pub chat: Vec<ChatEntry>,
    /// A request is running.
    pub assistant_busy: bool,
    /// What the running request asked.
    pub assistant_pending: Option<String>,
    /// A mix report is being rendered.
    pub report_busy: bool,
    /// The last mix report.
    pub last_report: Option<MixReport>,
    /// The local chat model, if configured.
    pub llm: Option<LlmConfig>,
    /// The MCP server, while running.
    pub mcp: Option<McpServer>,
    /// The model registry and download state.
    pub models: Vec<ModelStatus>,
    /// Models being downloaded.
    pub downloading: Vec<String>,
    /// A stem separation or transcription is running.
    pub ml_busy: bool,
    /// Hosted plugins from the scan cache.
    pub plugins: PluginList,
    /// A plugin scan is running.
    pub plugin_scan: bool,
    /// Where files go.
    pub paths: FeaturePaths,
}
