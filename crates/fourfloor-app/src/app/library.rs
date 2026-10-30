//! Sample browser intents: opening and scanning the library, searches,
//! auditions, and putting a file on a track, into a sampler or on a deck.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use fourfloor_analysis::Key;
use fourfloor_library::{plan_preview, FileId, PreviewMode, PreviewTarget, Query};

use crate::project::JobOutput;
use crate::services::audition::render_audition;
use crate::services::embed::{
    embed_job, text_search_available, text_search_job, TEXT_SEARCH_UNAVAILABLE,
};
use crate::services::library::{factory_job, scan_job, ScanSummary};
use crate::state::LibraryIntent;

use super::FourfloorApp;

/// Job labels, so a failed job can clear its busy flag.
pub mod labels {
    /// A folder scan.
    pub const SCAN: &str = "Scanning library";
    /// The factory install.
    pub const FACTORY: &str = "Installing factory samples";
    /// An audition render.
    pub const AUDITION: &str = "Preparing audition";
    /// A description search.
    pub const TEXT: &str = "Searching by description";
    /// Embedding library files.
    pub const EMBED: &str = "Embedding library";
}

impl FourfloorApp {
    pub(super) fn library_intent(&mut self, intent: LibraryIntent) {
        if !matches!(intent, LibraryIntent::StopAudition) && !self.ensure_library() {
            return;
        }
        match intent {
            LibraryIntent::Open => self.refresh_results(),
            LibraryIntent::Scan(folder) => self.start_scan(vec![folder]),
            LibraryIntent::Rescan => {
                let roots = self.features.library.roots();
                if roots.is_empty() {
                    self.ui_state
                        .error("No folders scanned yet: add a folder first");
                } else {
                    self.start_scan(roots);
                }
            }
            LibraryIntent::CancelScan => self.features.library.cancel_scan(),
            LibraryIntent::InstallFactory => self.install_factory(),
            LibraryIntent::Search(query) => self.library_search(&query),
            LibraryIntent::Select(file) => self.features.library.select(file),
            LibraryIntent::FindSimilar(file, filter) => {
                if let Err(error) = self.features.library.find_similar(file, &filter) {
                    self.ui_state.error(format!("find similar: {error}"));
                }
            }
            LibraryIntent::TextSearch(text) => self.text_search(text),
            LibraryIntent::Embed => self.embed_library(),
            LibraryIntent::Audition(file, mode, key) => self.audition(file, mode, key),
            LibraryIntent::StopAudition => {
                self.host.set_preview(None);
                self.features.auditioning = None;
            }
            LibraryIntent::AddToTrack(file) => self.add_file_to_track(file),
            LibraryIntent::LoadSampler(file) => self.load_into_sampler(file),
            LibraryIntent::LoadDeck(file, deck) => self.load_library_file(file, deck),
        }
    }

    /// Opens the library at the configured path. `false` (with a message)
    /// when the library is off or will not open.
    fn ensure_library(&mut self) -> bool {
        if self.features.library.is_open() {
            return true;
        }
        let Some(path) = self.features.paths.library.clone() else {
            self.ui_state
                .error("The sample library is off in this run (no library path)");
            return false;
        };
        match self.features.library.open(&path) {
            Ok(()) => {
                self.refresh_results();
                true
            }
            Err(error) => {
                self.ui_state.error(format!("library: {error}"));
                false
            }
        }
    }

    /// Re-runs the last filter query.
    fn refresh_results(&mut self) {
        let query = self.features.library.query.clone();
        self.library_search(&query);
    }

    fn library_search(&mut self, query: &Query) {
        if let Err(error) = self.features.library.search(query) {
            self.ui_state.error(format!("search: {error}"));
        }
    }

    fn start_scan(&mut self, folders: Vec<std::path::PathBuf>) {
        if self.features.scan.running {
            self.ui_state.info("A scan is already running");
            return;
        }
        let Some(db) = self
            .features
            .library
            .path()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.features.library.cancel = Some(Arc::clone(&cancel));
        self.features.scan.running = true;
        let job = scan_job(db, folders, cancel);
        self.jobs.spawn(labels::SCAN, move |progress| {
            job(progress).map(JobOutput::LibraryScanned)
        });
    }

    fn install_factory(&mut self) {
        let (Some(db), Some(folder)) = (
            self.features
                .library
                .path()
                .map(std::path::Path::to_path_buf),
            self.features.paths.factory.clone(),
        ) else {
            self.ui_state
                .error("No folder for the factory samples in this run");
            return;
        };
        if self.features.scan.running {
            self.ui_state.info("A scan is already running");
            return;
        }
        self.features.scan.running = true;
        let job = factory_job(db, folder);
        self.jobs.spawn(labels::FACTORY, move |progress| {
            job(progress).map(JobOutput::LibraryScanned)
        });
    }

    pub(super) fn library_scanned(&mut self, summary: ScanSummary) {
        self.features.scan.running = false;
        self.features.library.cancel = None;
        let mut text = format!(
            "{} added, {} updated, {} unchanged",
            summary.added, summary.updated, summary.unchanged
        );
        if !summary.failed.is_empty() {
            text.push_str(&format!(", {} unreadable", summary.failed.len()));
        }
        if summary.cancelled {
            text.push_str(" (stopped)");
        }
        self.ui_state.info(format!("Library scan: {text}"));
        self.features.scan.last = Some(text);
        self.refresh_results();
    }

    /// A library job failed: clear its busy flag.
    pub(super) fn library_job_failed(&mut self, label: &str) {
        match label {
            labels::SCAN | labels::FACTORY => {
                self.features.scan.running = false;
                self.features.library.cancel = None;
            }
            labels::TEXT | labels::EMBED => self.features.library.embedding = false,
            _ => {}
        }
    }

    fn text_search(&mut self, text: String) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if !text_search_available() {
            self.ui_state.info(TEXT_SEARCH_UNAVAILABLE);
            let query = Query {
                text: Some(text),
                ..self.features.library.query.clone()
            };
            self.library_search(&query);
            return;
        }
        let Some(db) = self
            .features
            .library
            .path()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let filter = Query {
            text: None,
            ..self.features.library.query.clone()
        };
        let models = self.features.paths.models.clone();
        let slot = self.features.library.embedder.clone();
        self.features.library.embedding = true;
        let job = text_search_job(db, models, slot, text.clone(), filter);
        self.jobs.spawn(labels::TEXT, move |progress| {
            job(progress).map(|rows| JobOutput::TextSearch { query: text, rows })
        });
    }

    fn embed_library(&mut self) {
        if !text_search_available() {
            self.ui_state.info(TEXT_SEARCH_UNAVAILABLE);
            return;
        }
        let Some(db) = self
            .features
            .library
            .path()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let models = self.features.paths.models.clone();
        let slot = self.features.library.embedder.clone();
        self.features.library.embedding = true;
        let job = embed_job(db, models, slot);
        self.jobs.spawn(labels::EMBED, move |progress| {
            job(progress).map(JobOutput::Embedded)
        });
    }

    fn audition(&mut self, file: FileId, mode: PreviewMode, key: Option<Key>) {
        let Some(record) = self.features.library.record(file) else {
            self.ui_state.error("that file is no longer in the library");
            return;
        };
        let target =
            PreviewTarget::new(Some(self.doc.project().tempo), key, self.host.sample_rate())
                .with_mode(mode);
        let plan = plan_preview(&record, &target);
        let path = record.path.clone();
        self.jobs.spawn(labels::AUDITION, move |progress| {
            progress.set(0.1);
            let source = fourfloor_audio::decode_file(&path).map_err(|e| e.to_string())?;
            progress.set(0.4);
            let audio = render_audition(&plan, &source)?;
            Ok(JobOutput::Audition {
                file,
                audio: Arc::new(audio),
            })
        });
    }

    pub(super) fn text_found(
        &mut self,
        query: String,
        rows: Vec<crate::services::library::ResultRow>,
    ) {
        self.features.library.embedding = false;
        let found = rows.len();
        self.ui_state
            .info(format!("{found} files ranked against \"{query}\""));
        self.features.library.text_results(query, rows);
    }

    pub(super) fn embedded(&mut self, summary: &crate::services::embed::EmbedSummary) {
        self.features.library.embedding = false;
        let failed = if summary.failed > 0 {
            format!(", {} failed", summary.failed)
        } else {
            String::new()
        };
        self.ui_state.info(format!(
            "Embedded {} files with {}{failed}",
            summary.files, summary.model
        ));
    }

    pub(super) fn audition_ready(
        &mut self,
        file: FileId,
        audio: Arc<crate::services::ReferenceAudio>,
    ) {
        self.host.set_preview(Some(audio));
        self.features.auditioning = Some(file);
        if let Some(library) = self.features.library.library() {
            let _ = library.record_play(file, "audition");
        }
    }
}
