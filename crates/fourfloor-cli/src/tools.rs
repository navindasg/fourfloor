//! The subcommands that wrap the analysis, interchange, library, plugin and
//! agent crates. Each one returns a summary; [`print`] writes it.

use std::io::Write;
use std::path::{Path, PathBuf};

use fourfloor_analysis::{analyze_file, Key, TrackAnalysis};
use fourfloor_dawproject::{
    export_to_file, import_file, ExportOptions, ImportOptions, Outcome, Report,
};
use fourfloor_library::{FileKind, FileRecord, Library, Query, ScanOptions, ScanReport};
use fourfloor_model::storage::PROJECT_FILE;
use fourfloor_model::{load_project, save_project};

use crate::args_tools::{SearchOptions, ToolInvocation};
use crate::commands::device_registry;
use crate::error::CliError;

/// What a tool subcommand produced.
#[derive(Debug)]
pub enum ToolSummary {
    /// `analyze`.
    Analysis {
        /// The file analysed.
        path: PathBuf,
        /// What was found.
        analysis: Box<TrackAnalysis>,
    },
    /// `export` or `import`: what did not fit the other side exactly.
    Interchange {
        /// The file or folder written.
        written: PathBuf,
        /// Tracks in the project.
        tracks: usize,
        /// The loss report.
        report: Report,
    },
    /// `library scan`.
    Scanned {
        /// The database.
        db: PathBuf,
        /// What changed.
        report: ScanReport,
        /// Files in the library afterwards.
        total: usize,
    },
    /// `library search`.
    Found(Vec<FileRecord>),
    /// `plugins scan` and `plugins list`: `(kind, name, vendor, loadable)`.
    Plugins {
        /// The cache file.
        path: PathBuf,
        /// One row per plugin.
        rows: Vec<(String, String, String, bool)>,
        /// Files or plugins that failed to scan.
        failures: usize,
    },
    /// An agent command ran to completion.
    AgentDone,
}

fn library_path(db: Option<&Path>) -> Result<PathBuf, CliError> {
    db.map(Path::to_path_buf)
        .or_else(fourfloor_library::default_database_path)
        .ok_or_else(|| CliError::Usage("no home folder is set; pass --db <file>".to_owned()))
}

fn search(options: &SearchOptions) -> Result<Vec<FileRecord>, CliError> {
    let db = library_path(options.db.as_deref())?;
    if !db.is_file() {
        return Err(CliError::Usage(format!(
            "no library at {}; run `fourfloor library scan <folder>` first",
            db.display()
        )));
    }
    let library = Library::open(&db)?;
    let mut query = Query::new().limit(options.limit);
    if let Some(text) = &options.text {
        query = query.text(text);
    }
    if let Some((low, high)) = options.bpm {
        query = query.bpm_range(low, high);
    }
    if let Some(key) = &options.key {
        let key: Key = key
            .parse()
            .map_err(|_| CliError::Usage(format!("--key: `{key}` is not a key (try Am or 8A)")))?;
        query = query.key(key);
    }
    if let Some(kind) = &options.kind {
        let kind: FileKind = kind.parse().map_err(|_| {
            CliError::Usage(format!("--kind: `{kind}` is not one-shot, loop or track"))
        })?;
        query = query.kind(kind);
    }
    Ok(library.search(&query)?)
}

fn plugins(scan: bool) -> Result<ToolSummary, CliError> {
    use fourfloor_host::{thread::HostThread, PluginDatabase, ScanOptions as PluginScan};
    let failed = |error: &dyn std::fmt::Display| CliError::Plugins(error.to_string());
    let path = PluginDatabase::default_path()
        .ok_or_else(|| CliError::Plugins("no cache folder is known".to_owned()))?;
    let mut database = PluginDatabase::load_or_empty(&path);
    if scan {
        let thread = HostThread::global().map_err(|e| failed(&e))?;
        database.rescan(&thread, &PluginScan::default());
        database.save(&path).map_err(|e| failed(&e))?;
    }
    let rows = database
        .plugins
        .iter()
        .map(|info| {
            (
                info.kind(),
                info.name.clone(),
                info.vendor.clone(),
                info.loadable,
            )
        })
        .collect();
    Ok(ToolSummary::Plugins {
        path,
        rows,
        failures: database.failures.len(),
    })
}

/// Runs one tool subcommand.
pub fn run_tool(invocation: ToolInvocation) -> Result<ToolSummary, CliError> {
    Ok(match invocation {
        ToolInvocation::Analyze { path } => ToolSummary::Analysis {
            analysis: Box::new(analyze_file(&path)?),
            path,
        },
        ToolInvocation::Export { dir, output } => {
            let project = load_project(&dir)?;
            let registry = device_registry()?;
            let options = ExportOptions::new().with_registry(&registry);
            let report = export_to_file(&project, Some(&dir), &output, &options)?;
            ToolSummary::Interchange {
                written: output,
                tracks: project.tracks.len(),
                report,
            }
        }
        ToolInvocation::Import { file, dir } => {
            if dir.join(PROJECT_FILE).exists() {
                return Err(CliError::ProjectExists(dir));
            }
            let imported = import_file(&file, &dir, &ImportOptions::new())?;
            save_project(&dir, &imported.project)?;
            ToolSummary::Interchange {
                written: dir,
                tracks: imported.project.tracks.len(),
                report: imported.report,
            }
        }
        ToolInvocation::LibraryScan { db, folders } => {
            let db = library_path(db.as_deref())?;
            let mut library = Library::open(&db)?;
            let report = library.scan(&folders, &ScanOptions::new(), |_| {})?;
            let total = library.len()?;
            ToolSummary::Scanned { db, report, total }
        }
        ToolInvocation::LibrarySearch(options) => ToolSummary::Found(search(&options)?),
        ToolInvocation::PluginsScan => plugins(true)?,
        ToolInvocation::PluginsList => plugins(false)?,
        ToolInvocation::Agent(args) => {
            if fourfloor_agent::cli::run("fourfloor agent", &args)
                != std::process::ExitCode::SUCCESS
            {
                return Err(CliError::Agent);
            }
            ToolSummary::AgentDone
        }
    })
}

fn print_analysis(out: &mut dyn Write, path: &Path, a: &TrackAnalysis) -> std::io::Result<()> {
    writeln!(
        out,
        "{}: {:.2} s, {} Hz, {} channels",
        path.display(),
        a.duration_seconds,
        a.sample_rate,
        a.channels
    )?;
    match &a.tempo {
        Some(tempo) => writeln!(
            out,
            "tempo {:.2} BPM (confidence {:.2}), first beat at {:.3} s",
            tempo.bpm(),
            tempo.confidence,
            tempo.grid.first_beat_seconds
        )?,
        None => writeln!(out, "tempo: no steady beat")?,
    }
    match &a.key {
        Some(key) => writeln!(
            out,
            "key {} ({}, confidence {:.2})",
            key.key,
            key.key.camelot(),
            key.confidence
        )?,
        None => writeln!(out, "key: no tonal content")?,
    }
    let l = &a.levels;
    writeln!(
        out,
        "integrated {:.2} LUFS, range {:.1} LU, true peak {:.2} dBTP, crest {:.1} dB, correlation {:.2}",
        l.integrated_lufs, l.loudness_range_lu, l.true_peak_dbtp, l.crest_factor_db, l.correlation
    )
}

fn print_record(out: &mut dyn Write, record: &FileRecord) -> std::io::Result<()> {
    let bpm = record
        .bpm
        .map_or_else(|| "-".to_owned(), |bpm| format!("{bpm:.1}"));
    let key = record
        .key
        .map_or_else(|| "-".to_owned(), |key| key.camelot());
    writeln!(
        out,
        "{:>6} BPM  {:>3}  {:<8}  {:>6.2} s  {}",
        bpm,
        key,
        record.kind,
        record.duration_seconds,
        record.path.display()
    )
}

/// Writes `summary` for a person to read.
pub fn print(out: &mut dyn Write, summary: &ToolSummary) -> std::io::Result<()> {
    match summary {
        ToolSummary::Analysis { path, analysis } => print_analysis(out, path, analysis),
        ToolSummary::Interchange {
            written,
            tracks,
            report,
        } => {
            writeln!(out, "wrote {} ({tracks} tracks)", written.display())?;
            let (kept, lost): (Vec<_>, Vec<_>) = report
                .items
                .iter()
                .partition(|item| item.outcome == Outcome::Preserved);
            lost.iter()
                .try_for_each(|item| writeln!(out, "note: {item}"))?;
            if kept.is_empty() {
                Ok(())
            } else {
                writeln!(
                    out,
                    "{} fourfloor-only settings kept in the file's fourfloor extension \
                     (other applications ignore them)",
                    kept.len()
                )
            }
        }
        ToolSummary::Scanned { db, report, total } => writeln!(
            out,
            "{}: {} added, {} updated, {} unchanged, {} moved, {} missing, {} failed; {total} files",
            db.display(),
            report.added.len(),
            report.updated.len(),
            report.unchanged,
            report.moved.len(),
            report.missing.len(),
            report.failed.len()
        ),
        ToolSummary::Found(records) => {
            records.iter().try_for_each(|r| print_record(out, r))?;
            writeln!(out, "{} matches", records.len())
        }
        ToolSummary::Plugins {
            path,
            rows,
            failures,
        } => {
            for (kind, name, vendor, loadable) in rows {
                let note = if *loadable { "" } else { " (not loadable)" };
                writeln!(out, "{kind}  {name} by {vendor}{note}")?;
            }
            writeln!(
                out,
                "{} plugins, {failures} failures, in {}",
                rows.len(),
                path.display()
            )
        }
        ToolSummary::AgentDone => Ok(()),
    }
}
