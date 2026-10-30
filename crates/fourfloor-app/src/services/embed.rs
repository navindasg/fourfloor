//! Audio-text embeddings for the sample browser's description search.
//!
//! With the `clap-search` feature the browser embeds library files with
//! LAION CLAP (Apache-2.0 weights, from the `fourfloor-ml` registry) and
//! ranks them against an embedded text query. The model is used only when
//! it is already in the model cache (Settings, Models downloads it); it is
//! loaded once on a worker and kept in an [`EmbedderSlot`] shared by later
//! jobs. Without the feature the jobs report [`TEXT_SEARCH_UNAVAILABLE`]
//! and the browser falls back to matching words in names and tags.

use std::path::PathBuf;
#[cfg(feature = "clap-search")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "clap-search")]
use fourfloor_library::Library;
use fourfloor_library::Query;

use super::library::ResultRow;
use super::Progress;

/// Why text search is off in this build.
pub const TEXT_SEARCH_UNAVAILABLE: &str =
    "description search needs the `clap-search` build feature; searched names and tags instead";

/// Matches returned by a description search.
pub const TEXT_RESULTS: usize = 50;

/// `true` when this build can embed text.
pub const fn text_search_available() -> bool {
    cfg!(feature = "clap-search")
}

/// The loaded embedder, shared by jobs so the model loads once.
#[derive(Clone, Default)]
pub struct EmbedderSlot {
    #[cfg(feature = "clap-search")]
    inner: Arc<Mutex<Option<Arc<fourfloor_ml::ClapEmbedder>>>>,
}

impl std::fmt::Debug for EmbedderSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbedderSlot").finish_non_exhaustive()
    }
}

#[cfg(feature = "clap-search")]
impl EmbedderSlot {
    /// The embedder, loading it from `models` (offline) on first use.
    fn get(&self, models: &std::path::Path) -> Result<Arc<fourfloor_ml::ClapEmbedder>, String> {
        use fourfloor_ml::{ModelCache, ModelRegistry, CLAP_GENERAL};
        let mut slot = self
            .inner
            .lock()
            .map_err(|_| "the embedder lock was poisoned".to_owned())?;
        if let Some(embedder) = slot.as_ref() {
            return Ok(Arc::clone(embedder));
        }
        let registry = ModelRegistry::builtin();
        let cache = ModelCache::new(models).offline();
        let spec = registry.get(CLAP_GENERAL).map_err(|e| e.to_string())?;
        if !cache.is_cached(spec) {
            return Err(format!(
                "{CLAP_GENERAL} is not downloaded yet (Settings, Models: about 207 MB)"
            ));
        }
        let embedder = Arc::new(
            fourfloor_ml::ClapEmbedder::load(&cache, &registry).map_err(|e| e.to_string())?,
        );
        *slot = Some(Arc::clone(&embedder));
        Ok(embedder)
    }
}

/// What an embedding job did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedSummary {
    /// Files embedded.
    pub files: usize,
    /// Files that would not decode or embed.
    pub failed: usize,
    /// The embedding model.
    pub model: String,
}

/// Job: embed every library file at `db` that has no embedding from the
/// model yet.
pub fn embed_job(
    db: PathBuf,
    models: PathBuf,
    slot: EmbedderSlot,
) -> impl FnOnce(&Progress) -> Result<EmbedSummary, String> + Send + 'static {
    move |progress| embed_files(&db, &models, &slot, progress)
}

#[cfg(feature = "clap-search")]
fn embed_files(
    db: &std::path::Path,
    models: &std::path::Path,
    slot: &EmbedderSlot,
    progress: &Progress,
) -> Result<EmbedSummary, String> {
    use fourfloor_ml::AudioEmbedder;
    progress.set(0.02);
    let embedder = slot.get(models)?;
    let model = embedder.model_name().to_owned();
    let library = Library::open(db).map_err(|e| e.to_string())?;
    let todo = library
        .files_without_embedding(&model)
        .map_err(|e| e.to_string())?;
    let mut summary = EmbedSummary {
        files: 0,
        failed: 0,
        model: model.clone(),
    };
    for (index, file) in todo.iter().enumerate() {
        progress.set(0.05 + 0.95 * index as f32 / todo.len().max(1) as f32);
        let embedded = library
            .file(*file)
            .map_err(|e| e.to_string())
            .and_then(|record| {
                fourfloor_audio::decode_file(&record.path).map_err(|e| e.to_string())
            })
            .and_then(|audio| embedder.embed_audio(&audio).map_err(|e| e.to_string()))
            .and_then(|embedding| {
                library
                    .set_embedding(*file, &embedding.model, &embedding.vector)
                    .map_err(|e| e.to_string())
            });
        match embedded {
            Ok(()) => summary.files += 1,
            Err(_) => summary.failed += 1,
        }
    }
    Ok(summary)
}

#[cfg(not(feature = "clap-search"))]
fn embed_files(
    _db: &std::path::Path,
    _models: &std::path::Path,
    _slot: &EmbedderSlot,
    _progress: &Progress,
) -> Result<EmbedSummary, String> {
    Err(TEXT_SEARCH_UNAVAILABLE.to_owned())
}

/// Job: rank library files at `db` against the description `text`, within
/// `filter`.
pub fn text_search_job(
    db: PathBuf,
    models: PathBuf,
    slot: EmbedderSlot,
    text: String,
    filter: Query,
) -> impl FnOnce(&Progress) -> Result<Vec<ResultRow>, String> + Send + 'static {
    move |progress| search_text(&db, &models, &slot, &text, &filter, progress)
}

#[cfg(feature = "clap-search")]
fn search_text(
    db: &std::path::Path,
    models: &std::path::Path,
    slot: &EmbedderSlot,
    text: &str,
    filter: &Query,
    progress: &Progress,
) -> Result<Vec<ResultRow>, String> {
    use fourfloor_ml::AudioEmbedder;
    progress.set(0.1);
    let embedder = slot.get(models)?;
    let query = embedder.embed_text(text).map_err(|e| e.to_string())?;
    progress.set(0.6);
    let library = Library::open(db).map_err(|e| e.to_string())?;
    let matches = library
        .search_embedding(&query.model, &query.vector, filter, TEXT_RESULTS)
        .map_err(|e| e.to_string())?;
    Ok(matches
        .into_iter()
        .map(|found| ResultRow {
            record: found.file,
            score: Some(found.score),
        })
        .collect())
}

#[cfg(not(feature = "clap-search"))]
fn search_text(
    _db: &std::path::Path,
    _models: &std::path::Path,
    _slot: &EmbedderSlot,
    _text: &str,
    _filter: &Query,
    _progress: &Progress,
) -> Result<Vec<ResultRow>, String> {
    Err(TEXT_SEARCH_UNAVAILABLE.to_owned())
}
