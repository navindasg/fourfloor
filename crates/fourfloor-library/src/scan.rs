//! Scanning folders into the library: walk, hash, analyse on a worker
//! pool, and write every row from the calling thread.
//!
//! A rescan is incremental. A file whose size and modification time match
//! its row is skipped without being read. One whose time changed but whose
//! bytes hash the same only has its time updated. A new path whose hash
//! matches a file that vanished from the same folders is a move: the row
//! (with its tags, rating, history and crates) follows the file. Files no
//! longer found are flagged missing, not deleted.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use rusqlite::{params, Transaction};

use crate::analyse::{analyse_bytes, content_hash, stamp, AnalyzedFile, FileStamp};
use crate::error::LibraryError;
use crate::library::{ensure_tag, now_unix, path_text, Library};
use crate::schema::ANALYSIS_VERSION;
use crate::types::{camelot_order, key_code, FileId};

/// File extensions the scanner reads (lowercase).
pub const AUDIO_EXTENSIONS: [&str; 8] = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "oga"];

/// How to scan.
#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// Worker threads; 0 uses the machine's parallelism.
    pub threads: usize,
    /// Analyse every file again, even unchanged ones.
    pub force: bool,
    /// Hash files whose size and time are unchanged, to catch edits that
    /// kept the time.
    pub verify_hashes: bool,
    /// Follow symbolic links while walking.
    pub follow_links: bool,
    /// Set to stop the scan early; rows written so far are kept.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl ScanOptions {
    /// Default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use `count` worker threads.
    pub fn threads(mut self, count: usize) -> Self {
        self.threads = count;
        self
    }

    /// Re-analyse everything.
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    /// Hash even unchanged-looking files.
    pub fn verify_hashes(mut self) -> Self {
        self.verify_hashes = true;
        self
    }

    /// Stop when `flag` is set.
    pub fn cancel_with(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    fn worker_count(&self, jobs: usize) -> usize {
        let wanted = if self.threads == 0 {
            thread::available_parallelism().map_or(2, usize::from)
        } else {
            self.threads
        };
        wanted.clamp(1, jobs.max(1))
    }
}

/// Where a scan is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanPhase {
    /// Listing folders.
    Walking,
    /// Hashing and analysing changed files.
    Analysing,
    /// Finished.
    Done,
}

/// Progress, reported on the calling thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanProgress {
    /// Phase.
    pub phase: ScanPhase,
    /// Audio files found so far.
    pub found: usize,
    /// Files that needed reading.
    pub total: usize,
    /// How many of those are finished.
    pub done: usize,
    /// The file just finished, if any.
    pub current: Option<PathBuf>,
}

/// A file the scan could not index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanFailure {
    /// The file.
    pub path: PathBuf,
    /// Why.
    pub reason: String,
}

/// What a scan did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// New files indexed.
    pub added: Vec<FileId>,
    /// Changed files analysed again.
    pub updated: Vec<FileId>,
    /// Files skipped because nothing changed.
    pub unchanged: usize,
    /// Files whose time changed but whose bytes did not.
    pub touched: usize,
    /// Rows that followed a file to a new path.
    pub moved: Vec<FileId>,
    /// Rows newly flagged missing.
    pub missing: Vec<FileId>,
    /// Files that could not be read or decoded.
    pub failed: Vec<ScanFailure>,
    /// The scan was stopped early.
    pub cancelled: bool,
}

/// What the library already knows about a path.
#[derive(Clone, Debug)]
struct Known {
    id: FileId,
    stamp: FileStamp,
    hash: String,
    version: i64,
    missing: bool,
}

/// Rows whose file is gone, by content hash: (id, old file name), by id.
type Vanished = HashMap<String, Vec<(FileId, String)>>;

/// Takes the vanished row with `hash` for a file now called `filename`:
/// one with the same name if there is one, else the oldest.
fn claim_vanished(vanished: &mut Vanished, hash: &str, filename: &str) -> Option<FileId> {
    let rows = vanished.get_mut(hash)?;
    let index = rows
        .iter()
        .position(|(_, name)| name == filename)
        .unwrap_or(0);
    let claimed = (index < rows.len()).then(|| rows.remove(index).0);
    if rows.is_empty() {
        vanished.remove(hash);
    }
    claimed
}

struct Job {
    path: PathBuf,
    stamp: FileStamp,
    known: Option<Known>,
}

enum Outcome {
    Analysed(Box<AnalyzedFile>),
    Touched {
        stamp: FileStamp,
    },
    /// The bytes match a row whose file vanished; [`Library::apply`]
    /// picks which row.
    Moved {
        hash: String,
        stamp: FileStamp,
    },
    Failed(String),
}

impl Library {
    /// Scans `folders` (remembering them as roots) and indexes every audio
    /// file below them. `progress` is called on this thread.
    pub fn scan(
        &mut self,
        folders: &[PathBuf],
        options: &ScanOptions,
        mut progress: impl FnMut(&ScanProgress),
    ) -> Result<ScanReport, LibraryError> {
        let mut roots = Vec::with_capacity(folders.len());
        for folder in folders {
            let root = folder
                .canonicalize()
                .map_err(|error| LibraryError::io(folder, error))?;
            if !root.is_dir() {
                return Err(LibraryError::invalid(
                    "scan folder",
                    format!("{} is not a folder", root.display()),
                ));
            }
            self.add_root(&root)?;
            roots.push(root);
        }
        let mut report = ScanReport::default();
        let mut state = ScanProgress {
            phase: ScanPhase::Walking,
            found: 0,
            total: 0,
            done: 0,
            current: None,
        };
        progress(&state);
        let found = walk(&roots, options, &mut state, &mut progress)?;
        let known = self.known_under(&roots)?;
        let found_set: HashSet<&PathBuf> = found.iter().collect();
        // Rows whose file is gone: candidates for a move, else missing.
        // Several rows can share a hash (identical copies), so each hash
        // keeps every one of its rows.
        let mut vanished: Vanished = HashMap::new();
        for (path, row) in known.iter().filter(|(path, _)| !found_set.contains(path)) {
            vanished
                .entry(row.hash.clone())
                .or_default()
                .push((row.id, file_name(path)));
        }
        for rows in vanished.values_mut() {
            rows.sort();
        }

        let mut jobs = Vec::new();
        for path in &found {
            let file_stamp = match stamp(path) {
                Ok(file_stamp) => file_stamp,
                Err(error) => {
                    report.failed.push(failure(path, &error.to_string()));
                    continue;
                }
            };
            let row = known.get(path).cloned();
            let fresh = row
                .as_ref()
                .is_some_and(|row| row.stamp == file_stamp && row.version == ANALYSIS_VERSION);
            if fresh && !options.force && !options.verify_hashes {
                if let Some(row) = row.filter(|row| row.missing) {
                    self.set_missing(row.id, false)?;
                }
                report.unchanged += 1;
                continue;
            }
            jobs.push(Job {
                path: path.clone(),
                stamp: file_stamp,
                known: row,
            });
        }

        state.phase = ScanPhase::Analysing;
        state.total = jobs.len();
        progress(&state);
        let moved_from: HashSet<String> = if options.force {
            HashSet::new()
        } else {
            vanished.keys().cloned().collect()
        };
        self.run_jobs(
            jobs,
            options,
            &moved_from,
            &mut state,
            &mut progress,
            &mut report,
            &mut vanished,
        )?;

        // Whatever vanished and did not move is missing.
        // A cancelled walk saw only part of the folders, so absence there
        // proves nothing.
        report.cancelled = report.cancelled || options.cancelled();
        if !report.cancelled {
            for (id, _) in vanished.into_values().flatten() {
                if !self.is_missing(id)? {
                    self.set_missing(id, true)?;
                    report.missing.push(id);
                }
            }
        }
        report.missing.sort();
        state.phase = ScanPhase::Done;
        state.current = None;
        progress(&state);
        Ok(report)
    }

    /// Scans every remembered root again.
    pub fn rescan(
        &mut self,
        options: &ScanOptions,
        progress: impl FnMut(&ScanProgress),
    ) -> Result<ScanReport, LibraryError> {
        let roots: Vec<PathBuf> = self
            .roots()?
            .into_iter()
            .filter(|root| root.is_dir())
            .collect();
        self.scan(&roots, options, progress)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_jobs(
        &mut self,
        jobs: Vec<Job>,
        options: &ScanOptions,
        moved_from: &HashSet<String>,
        state: &mut ScanProgress,
        progress: &mut impl FnMut(&ScanProgress),
        report: &mut ScanReport,
        vanished: &mut Vanished,
    ) -> Result<(), LibraryError> {
        if jobs.is_empty() {
            return Ok(());
        }
        let next = AtomicUsize::new(0);
        let workers = options.worker_count(jobs.len());
        let (sender, receiver) = mpsc::channel::<(usize, Outcome)>();
        let jobs = &jobs;
        thread::scope(|scope| -> Result<(), LibraryError> {
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let sender = sender.clone();
                let next = &next;
                handles.push(scope.spawn(move || loop {
                    if options.cancelled() {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(index) else { break };
                    let outcome = catch_unwind(AssertUnwindSafe(|| work(job, options, moved_from)))
                        .unwrap_or_else(|_| Outcome::Failed("analysis panicked".to_owned()));
                    if sender.send((index, outcome)).is_err() {
                        break;
                    }
                }));
            }
            drop(sender);
            let mut result = Ok(());
            for (index, outcome) in receiver {
                let Some(job) = jobs.get(index) else { continue };
                if result.is_ok() {
                    result = self.apply(job, outcome, report, vanished);
                }
                state.done += 1;
                state.current = Some(job.path.clone());
                progress(state);
            }
            for handle in handles {
                if handle.join().is_err() {
                    return Err(LibraryError::WorkerFailed);
                }
            }
            result
        })?;
        report.cancelled = options.cancelled() && state.done < jobs.len();
        Ok(())
    }

    fn apply(
        &mut self,
        job: &Job,
        outcome: Outcome,
        report: &mut ScanReport,
        vanished: &mut Vanished,
    ) -> Result<(), LibraryError> {
        let tx = self.conn.transaction()?;
        match outcome {
            Outcome::Failed(reason) => report.failed.push(failure(&job.path, &reason)),
            Outcome::Touched { stamp } => {
                if let Some(known) = &job.known {
                    tx.execute(
                        "UPDATE files SET size_bytes = ?2, mtime_ns = ?3, missing = 0 WHERE id = ?1",
                        params![known.id.0, stamp.size_bytes as i64, stamp.mtime_ns],
                    )?;
                }
                report.touched += 1;
            }
            Outcome::Moved { hash, stamp } => {
                // Claim one vanished row with these bytes, so a second copy
                // does not also move it.
                match claim_vanished(vanished, &hash, &file_name(&job.path)) {
                    Some(from) => {
                        tx.execute(
                            "UPDATE files SET path = ?2, filename = ?3, title = ?4, size_bytes = ?5,
                                mtime_ns = ?6, missing = 0 WHERE id = ?1",
                            params![
                                from.0,
                                path_text(&job.path)?,
                                file_name(&job.path),
                                file_title(&job.path),
                                stamp.size_bytes as i64,
                                stamp.mtime_ns
                            ],
                        )?;
                        report.moved.push(from);
                    }
                    None => {
                        // Already claimed: analyse it as a new file instead.
                        tx.commit()?;
                        let bytes = std::fs::read(&job.path)
                            .map_err(|error| LibraryError::io(&job.path, error))?;
                        let hash = content_hash(&bytes);
                        let outcome = match analyse_bytes(&job.path, bytes, hash, stamp) {
                            Ok(file) => Outcome::Analysed(Box::new(file)),
                            Err(error) => Outcome::Failed(error.to_string()),
                        };
                        return self.apply(job, outcome, report, vanished);
                    }
                }
            }
            Outcome::Analysed(file) => {
                let (id, new) = write_file(&tx, &file, job.known.as_ref().map(|k| k.id))?;
                if new {
                    report.added.push(id);
                } else {
                    report.updated.push(id);
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn known_under(&self, roots: &[PathBuf]) -> Result<HashMap<PathBuf, Known>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT id, path, size_bytes, mtime_ns, content_hash, analysis_version, missing FROM files",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                PathBuf::from(row.get::<_, String>(1)?),
                Known {
                    id: FileId(row.get(0)?),
                    stamp: FileStamp {
                        size_bytes: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                        mtime_ns: row.get(3)?,
                    },
                    hash: row.get(4)?,
                    version: row.get(5)?,
                    missing: row.get::<_, i64>(6)? != 0,
                },
            ))
        })?;
        let mut known = HashMap::new();
        for row in rows {
            let (path, entry) = row?;
            if roots.iter().any(|root| path.starts_with(root)) {
                known.insert(path, entry);
            }
        }
        Ok(known)
    }

    fn set_missing(&self, id: FileId, missing: bool) -> Result<(), LibraryError> {
        self.conn.execute(
            "UPDATE files SET missing = ?2 WHERE id = ?1",
            params![id.0, i64::from(missing)],
        )?;
        Ok(())
    }

    fn is_missing(&self, id: FileId) -> Result<bool, LibraryError> {
        let flag: i64 =
            self.conn
                .query_row("SELECT missing FROM files WHERE id = ?1", [id.0], |row| {
                    row.get(0)
                })?;
        Ok(flag != 0)
    }

    /// Analyses one file and writes it, outside a folder scan. Returns its
    /// id. Does not add a root.
    pub fn index_file(&mut self, path: &Path) -> Result<FileId, LibraryError> {
        let path = path
            .canonicalize()
            .map_err(|error| LibraryError::io(path, error))?;
        let file_stamp = stamp(&path)?;
        let bytes = std::fs::read(&path).map_err(|error| LibraryError::io(&path, error))?;
        let hash = content_hash(&bytes);
        let file = analyse_bytes(&path, bytes, hash, file_stamp)?;
        let existing = self.file_by_path(&path)?.map(|record| record.id);
        let tx = self.conn.transaction()?;
        let (id, _) = write_file(&tx, &file, existing)?;
        tx.commit()?;
        Ok(id)
    }
}

fn failure(path: &Path, reason: &str) -> ScanFailure {
    ScanFailure {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_audio(path: &Path) -> bool {
    let hidden = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| name.starts_with('.'));
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    !hidden && extension.is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.as_str()))
}

fn walk(
    roots: &[PathBuf],
    options: &ScanOptions,
    state: &mut ScanProgress,
    progress: &mut impl FnMut(&ScanProgress),
) -> Result<Vec<PathBuf>, LibraryError> {
    let mut found = Vec::new();
    for root in roots {
        let walker = walkdir::WalkDir::new(root)
            .follow_links(options.follow_links)
            .sort_by_file_name();
        for entry in walker {
            if options.cancelled() {
                break;
            }
            // Unreadable folders are skipped, not fatal.
            let Ok(entry) = entry else { continue };
            if entry.file_type().is_file() && is_audio(entry.path()) {
                found.push(entry.into_path());
                state.found = found.len();
                if found.len() % 64 == 0 {
                    progress(state);
                }
            }
        }
    }
    found.sort();
    found.dedup();
    state.found = found.len();
    Ok(found)
}

/// What a worker does with one file.
fn work(job: &Job, options: &ScanOptions, moved_from: &HashSet<String>) -> Outcome {
    let bytes = match std::fs::read(&job.path) {
        Ok(bytes) => bytes,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let hash = content_hash(&bytes);
    let stamp = job.stamp.clone();
    if !options.force {
        if let Some(known) = &job.known {
            if known.hash == hash && known.version == ANALYSIS_VERSION {
                return Outcome::Touched { stamp };
            }
        } else if moved_from.contains(&hash) {
            return Outcome::Moved { hash, stamp };
        }
    }
    match analyse_bytes(&job.path, bytes, hash, stamp) {
        Ok(file) => Outcome::Analysed(Box::new(file)),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// Inserts or updates a file row and replaces its automatic tags. A locked
/// row keeps its tempo, key and kind. Returns the id and whether it is new.
pub(crate) fn write_file(
    tx: &Transaction<'_>,
    file: &AnalyzedFile,
    existing: Option<FileId>,
) -> Result<(FileId, bool), LibraryError> {
    let now = now_unix();
    let path = path_text(&file.path)?;
    let key = file.key.map(key_code);
    let camelot = file.key.map(camelot_order);
    let (id, new) = match existing {
        Some(id) => {
            tx.execute(
                "UPDATE files SET path = ?2, filename = ?3, title = ?4, content_hash = ?5,
                    size_bytes = ?6, mtime_ns = ?7, duration_seconds = ?8, sample_rate = ?9,
                    channels = ?10, loudness_lufs = ?11, true_peak_dbtp = ?12, waveform = ?13,
                    features = ?14, analyzed_at = ?15, analysis_version = ?16, missing = 0,
                    first_beat_seconds = ?17
                 WHERE id = ?1",
                params![
                    id.0,
                    path,
                    file.filename,
                    file.title,
                    file.content_hash,
                    file.size_bytes as i64,
                    file.mtime_ns,
                    file.duration_seconds,
                    file.sample_rate,
                    file.channels,
                    file.loudness_lufs,
                    file.true_peak_dbtp,
                    file.waveform,
                    file.features,
                    now,
                    ANALYSIS_VERSION,
                    file.first_beat_seconds
                ],
            )?;
            tx.execute(
                "UPDATE files SET kind = ?2, bpm = ?3, bpm_confidence = ?4, key = ?5,
                    key_camelot = ?6, key_confidence = ?7
                 WHERE id = ?1 AND locked = 0",
                params![
                    id.0,
                    file.kind.as_str(),
                    file.bpm,
                    file.bpm_confidence.map(f64::from),
                    key,
                    camelot,
                    file.key_confidence.map(f64::from)
                ],
            )?;
            tx.execute(
                "DELETE FROM file_tags WHERE file_id = ?1 AND source = 'auto'",
                [id.0],
            )?;
            (id, false)
        }
        None => {
            tx.execute(
                "INSERT INTO files (path, filename, title, content_hash, size_bytes, mtime_ns,
                    kind, duration_seconds, sample_rate, channels, bpm, bpm_confidence,
                    first_beat_seconds, key, key_camelot, key_confidence, loudness_lufs,
                    true_peak_dbtp, waveform, features, added_at, analyzed_at, analysis_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20, ?21, ?21, ?22)",
                params![
                    path,
                    file.filename,
                    file.title,
                    file.content_hash,
                    file.size_bytes as i64,
                    file.mtime_ns,
                    file.kind.as_str(),
                    file.duration_seconds,
                    file.sample_rate,
                    file.channels,
                    file.bpm,
                    file.bpm_confidence.map(f64::from),
                    file.first_beat_seconds,
                    key,
                    camelot,
                    file.key_confidence.map(f64::from),
                    file.loudness_lufs,
                    file.true_peak_dbtp,
                    file.waveform,
                    file.features,
                    now,
                    ANALYSIS_VERSION
                ],
            )?;
            (FileId(tx.last_insert_rowid()), true)
        }
    };
    for tag in &file.tags {
        let tag_id = ensure_tag(tx, tag)?;
        tx.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, source) VALUES (?1, ?2, 'auto')",
            params![id.0, tag_id],
        )?;
    }
    Ok((id, new))
}
