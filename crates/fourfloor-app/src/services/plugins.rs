//! The hosted-plugin list for the settings: the scan cache, and a rescan
//! job that refreshes and saves it.
//!
//! Scanning loads every plugin in this process (there is no out-of-process
//! scanner yet), so a plugin that crashes on load takes the app down; the
//! settings say so next to the button.

use std::path::PathBuf;

use fourfloor_host::{HostThread, PluginDatabase, PluginInfo, ScanOptions};

use super::Progress;

/// A plugin as the settings list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRow {
    /// Registry kind (`plugin.clap:...`).
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Format tag (`clap`, `au`, `vst3`).
    pub format: String,
    /// Instrument, effect or note effect.
    pub category: String,
    /// Whether fourfloor can load it.
    pub loadable: bool,
}

impl From<&PluginInfo> for PluginRow {
    fn from(info: &PluginInfo) -> Self {
        Self {
            kind: info.kind(),
            name: info.name.clone(),
            vendor: info.vendor.clone(),
            format: info.format.tag().to_owned(),
            category: format!("{:?}", info.category),
            loadable: info.loadable,
        }
    }
}

/// The plugin list and where it is cached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginList {
    /// Plugins found by the last scan.
    pub rows: Vec<PluginRow>,
    /// Failures of the last scan (path and reason).
    pub failures: Vec<(String, String)>,
    /// The cache file.
    pub path: Option<PathBuf>,
}

impl PluginList {
    /// Reads the scan cache at `path` (an empty list if there is none).
    pub fn load(path: Option<PathBuf>) -> Self {
        let database = path
            .as_deref()
            .map(PluginDatabase::load_or_empty)
            .unwrap_or_default();
        Self::from_database(&database, path)
    }

    fn from_database(database: &PluginDatabase, path: Option<PathBuf>) -> Self {
        let mut rows: Vec<PluginRow> = database.plugins.iter().map(PluginRow::from).collect();
        rows.sort_by_key(|row| row.name.to_lowercase());
        Self {
            rows,
            failures: database
                .failures
                .iter()
                .map(|f| (f.path.display().to_string(), f.reason.clone()))
                .collect(),
            path,
        }
    }
}

/// Job: rescan the system plugin folders (`options`) into the cache at
/// `path` and save it. Unchanged files are not reloaded.
pub fn scan_job(
    path: PathBuf,
    options: ScanOptions,
) -> impl FnOnce(&Progress) -> Result<PluginList, String> + Send + 'static {
    move |progress| {
        progress.set(0.05);
        let thread = HostThread::global().map_err(|e| e.to_string())?;
        let mut database = PluginDatabase::load_or_empty(&path);
        database.rescan(&thread, &options);
        progress.set(0.9);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        database.save(&path).map_err(|e| e.to_string())?;
        Ok(PluginList::from_database(&database, Some(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_cache_is_an_empty_list_and_an_empty_scan_saves_one() {
        let dir = std::env::temp_dir().join(format!("ff-plugins-{}", std::process::id()));
        let path = dir.join("plugins.json");
        let list = PluginList::load(Some(path.clone()));
        assert!(list.rows.is_empty());
        let progress = Progress::default();
        let scanned = scan_job(path.clone(), ScanOptions::none())(&progress).expect("scans");
        assert!(scanned.rows.is_empty());
        assert!(path.exists(), "the cache was written");
        let _ = std::fs::remove_dir_all(dir);
    }
}
