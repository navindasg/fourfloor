//! The sample and track library as a service: one SQLite database opened
//! on the UI thread for searches (fast, indexed), and scans that open their
//! own connection to the same file on a worker thread (WAL lets both run).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fourfloor_library::{
    FileId, FileRecord, Library, LibraryError, Query, ScanOptions, ScanPhase, WaveformSummary,
};

use super::Progress;

/// Results kept per search.
pub const RESULT_LIMIT: usize = 500;

/// What a finished library job did.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanSummary {
    /// Files added.
    pub added: usize,
    /// Files re-analysed.
    pub updated: usize,
    /// Files unchanged.
    pub unchanged: usize,
    /// Files that would not read.
    pub failed: Vec<(PathBuf, String)>,
    /// Stopped early.
    pub cancelled: bool,
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// The default library file: under the user's application data folder.
pub fn default_library_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("library.sqlite"))
}

/// The app's data folder (`~/Library/Application Support/fourfloor` on
/// macOS, `$XDG_DATA_HOME/fourfloor` or `~/.local/share/fourfloor`
/// elsewhere).
pub fn data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/fourfloor"));
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")))
        .map(|dir| dir.join("fourfloor"))
}

/// Job: scan `folders` into the library at `db`, reporting progress.
pub fn scan_job(
    db: PathBuf,
    folders: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
) -> impl FnOnce(&Progress) -> Result<ScanSummary, String> + Send + 'static {
    move |progress| {
        let mut library = Library::open(&db).map_err(text)?;
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(8));
        let options = ScanOptions::new().threads(threads).cancel_with(cancel);
        let report = library
            .scan(&folders, &options, |step| {
                let fraction = match step.phase {
                    ScanPhase::Walking => 0.02,
                    ScanPhase::Analysing if step.total > 0 => {
                        0.05 + 0.95 * step.done as f32 / step.total as f32
                    }
                    _ => 0.99,
                };
                progress.set(fraction);
            })
            .map_err(text)?;
        Ok(ScanSummary {
            added: report.added.len(),
            updated: report.updated.len(),
            unchanged: report.unchanged,
            failed: report
                .failed
                .into_iter()
                .map(|failure| (failure.path, failure.reason))
                .collect(),
            cancelled: report.cancelled,
        })
    }
}

/// Job: write the built-in CC0 factory samples to `folder` and scan them.
pub fn factory_job(
    db: PathBuf,
    folder: PathBuf,
) -> impl FnOnce(&Progress) -> Result<ScanSummary, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        fourfloor_library::factory::write_factory(&folder).map_err(text)?;
        progress.set(0.4);
        scan_job(db, vec![folder], Arc::new(AtomicBool::new(false)))(progress)
    }
}

/// One row of the result table.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultRow {
    /// The file.
    pub record: FileRecord,
    /// Similarity score, for similarity and text searches.
    pub score: Option<f32>,
}

/// How the current results were found.
#[derive(Clone, Debug, PartialEq)]
pub enum ResultSource {
    /// The filter query.
    Query,
    /// Files that sound like this one.
    Similar(FileId),
    /// A text description, through audio-text embeddings.
    Text(String),
}

/// The open library, its last results and selection.
#[derive(Default)]
pub struct LibraryService {
    library: Option<Library>,
    path: Option<PathBuf>,
    /// Current results.
    pub rows: Vec<ResultRow>,
    /// How they were found.
    pub source: Option<ResultSource>,
    /// Files matching the filters without the limit.
    pub total: usize,
    /// The selected file.
    pub selected: Option<FileId>,
    /// Its stored waveform.
    pub waveform: Option<WaveformSummary>,
    /// Set while a scan runs; setting it stops the scan.
    pub cancel: Option<Arc<AtomicBool>>,
    /// The last filter query, re-run after scans and edits.
    pub query: Query,
    /// The description-search model, loaded once.
    pub embedder: super::embed::EmbedderSlot,
    /// A description search or embedding job is running.
    pub embedding: bool,
}

impl std::fmt::Debug for LibraryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryService")
            .field("path", &self.path)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

impl LibraryService {
    /// Opens (or creates) the library at `path`.
    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        self.library = Some(Library::open(path).map_err(text)?);
        self.path = Some(path.to_path_buf());
        Ok(())
    }

    /// The database file, once opened.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// `true` once a library is open.
    pub fn is_open(&self) -> bool {
        self.library.is_some()
    }

    /// The open library.
    pub fn library(&self) -> Option<&Library> {
        self.library.as_ref()
    }

    /// The scan folders remembered so far.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.library
            .as_ref()
            .and_then(|library| library.roots().ok())
            .unwrap_or_default()
    }

    /// Runs `query` and keeps the results.
    pub fn search(&mut self, query: &Query) -> Result<(), LibraryError> {
        let Some(library) = &self.library else {
            return Ok(());
        };
        let limited = query.clone().limit(RESULT_LIMIT);
        self.total = library.count(query)?;
        self.rows = library
            .search(&limited)?
            .into_iter()
            .map(|record| ResultRow {
                record,
                score: None,
            })
            .collect();
        self.source = Some(ResultSource::Query);
        self.query = query.clone();
        Ok(())
    }

    /// Lists files that sound like `file`, within `filter`.
    pub fn find_similar(&mut self, file: FileId, filter: &Query) -> Result<(), LibraryError> {
        let Some(library) = &self.library else {
            return Ok(());
        };
        let matches = library.similar(file, filter, 50)?;
        self.total = matches.len();
        self.rows = matches
            .into_iter()
            .map(|found| ResultRow {
                record: found.file,
                score: Some(found.score),
            })
            .collect();
        self.source = Some(ResultSource::Similar(file));
        Ok(())
    }

    /// Lists files nearest an embedding of a text description.
    pub fn text_results(&mut self, query: String, rows: Vec<ResultRow>) {
        self.total = rows.len();
        self.rows = rows;
        self.source = Some(ResultSource::Text(query));
    }

    /// Selects `file` and reads its stored waveform.
    pub fn select(&mut self, file: FileId) {
        self.selected = Some(file);
        self.waveform = self
            .library
            .as_ref()
            .and_then(|library| library.waveform(file).ok().flatten());
    }

    /// The selected file's record.
    pub fn selected_record(&self) -> Option<FileRecord> {
        let library = self.library.as_ref()?;
        library.file(self.selected?).ok()
    }

    /// A record by id.
    pub fn record(&self, file: FileId) -> Option<FileRecord> {
        self.library.as_ref()?.file(file).ok()
    }

    /// Every track-like file (loops and tracks with a tempo), for the DJ
    /// suggestions.
    pub fn mixable(&self) -> Vec<FileRecord> {
        let Some(library) = &self.library else {
            return Vec::new();
        };
        let query = Query::new()
            .kind(fourfloor_library::FileKind::Track)
            .kind(fourfloor_library::FileKind::Loop)
            .limit(2_000);
        library.search(&query).unwrap_or_default()
    }

    /// Similarity of `file` to every other file, by id (empty without
    /// features).
    pub fn similarity_scores(&self, file: FileId) -> Vec<(FileId, f32)> {
        let Some(library) = &self.library else {
            return Vec::new();
        };
        library
            .similar(file, &Query::new(), 2_000)
            .map(|found| found.into_iter().map(|m| (m.file.id, m.score)).collect())
            .unwrap_or_default()
    }

    /// The file at `path`, if indexed.
    pub fn by_path(&self, path: &Path) -> Option<FileRecord> {
        self.library.as_ref()?.file_by_path(path).ok().flatten()
    }

    /// Stops a running scan.
    pub fn cancel_scan(&mut self) {
        if let Some(flag) = self.cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_folder_is_under_home() {
        if let (Some(dir), Some(home)) = (data_dir(), std::env::var_os("HOME")) {
            assert!(dir.starts_with(home));
            assert!(dir.ends_with("fourfloor"));
        }
        assert!(default_library_path().is_none_or(|p| p.ends_with("library.sqlite")));
    }
}
