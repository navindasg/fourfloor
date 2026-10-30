//! The fourfloor sample and track library.
//!
//! | Task | Entry point |
//! | --- | --- |
//! | Open or create the database | [`Library::open`], [`Library::open_in_memory`] |
//! | Index folders (incremental) | [`Library::scan`], [`Library::rescan`], [`ScanOptions`] |
//! | Search and filter | [`Library::search`], [`Query`], [`SortKey`] |
//! | Sounds like this | [`Library::similar`], [`Library::search_embedding`] |
//! | Audition at project tempo and key | [`Library::preview`], [`plan_preview`] |
//! | Tags, rating, crates, playlists, history | methods on [`Library`] |
//! | Never-empty browser | [`factory::write_factory`], [`Library::install_factory`] |
//!
//! The index is one SQLite file (bundled SQLite, schema in [`schema`]
//! after Mixxx's). Analysis comes from `fourfloor-analysis`; everything
//! here runs off the audio thread.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analyse;
pub mod blob;
mod collections;
pub mod error;
pub mod factory;
mod library;
pub mod preview;
pub mod query;
pub mod scan;
pub mod schema;
mod similar;
pub mod tags;
pub mod types;

pub use analyse::{analyse_buffer, AnalyzedFile};
pub use blob::WaveformSummary;
pub use error::LibraryError;
pub use library::{Library, ManualAnalysis};
pub use preview::{plan_preview, PreviewMode, PreviewStream, PreviewTarget};
pub use query::{Query, SortKey};
pub use scan::{ScanFailure, ScanOptions, ScanPhase, ScanProgress, ScanReport};
pub use similar::Match;
pub use types::*;

/// The per-user data folder fourfloor keeps its library in:
/// `~/Library/Application Support/fourfloor` on macOS, otherwise
/// `$XDG_DATA_HOME/fourfloor` or `~/.local/share/fourfloor`. `None` when no
/// home folder is known.
pub fn default_data_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|home| home.join("Library/Application Support/fourfloor"));
    }
    std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| home.map(|home| home.join(".local/share")))
        .map(|dir| dir.join("fourfloor"))
}

/// The library database the app and the command line share by default:
/// `library.sqlite` in [`default_data_dir`].
pub fn default_database_path() -> Option<std::path::PathBuf> {
    default_data_dir().map(|dir| dir.join("library.sqlite"))
}
