//! The plugin database: what the last scan found, cached as JSON.
//!
//! Scanning loads every plugin once, which is slow and is the step most
//! likely to hit a broken plugin. The database keeps the result so the app
//! starts without touching a plugin, and [`PluginDatabase::rescan`] only
//! looks again at files that changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::clap;
use crate::error::HostError;
use crate::info::{parse_plugin_kind, PluginFormat, PluginInfo};
use crate::thread::HostThread;
use crate::vst3;

/// Version of the database file format.
pub const DATABASE_VERSION: u32 = 1;

/// A plugin file, or one plugin in it, that a scan could not use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFailure {
    /// The format being scanned.
    pub format: PluginFormat,
    /// The file or bundle; empty for an Audio Unit.
    pub path: PathBuf,
    /// The plugin's id, when the failure is about one plugin in the file.
    #[serde(default)]
    pub id: Option<String>,
    /// What went wrong.
    pub reason: String,
}

/// Size and modification time of a scanned file, used to skip it next time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    /// The file or bundle.
    pub path: PathBuf,
    /// Seconds since the Unix epoch.
    pub modified: u64,
    /// File size in bytes; 0 for a bundle directory.
    pub size: u64,
}

impl FileStamp {
    /// Reads the stamp of `path`. For a macOS bundle the newest of the
    /// bundle directory and its `Contents/MacOS` directory is used, since
    /// replacing the binary does not touch the bundle directory itself.
    pub fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let seconds = |m: &std::fs::Metadata| {
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs())
        };
        let mut modified = seconds(&metadata);
        let mut size = 0;
        if metadata.is_dir() {
            let binaries = path.join("Contents/MacOS");
            if let Ok(entries) = std::fs::read_dir(&binaries) {
                for entry in entries.flatten() {
                    if let Ok(inner) = entry.metadata() {
                        modified = modified.max(seconds(&inner));
                        size += inner.len();
                    }
                }
            }
        } else {
            size = metadata.len();
        }
        Some(Self {
            path: path.to_path_buf(),
            modified,
            size,
        })
    }
}

/// What to scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    /// Directories searched for `.clap` files and bundles.
    pub clap_dirs: Vec<PathBuf>,
    /// Directories searched for `.vst3` bundles (listed, not loaded).
    pub vst3_dirs: Vec<PathBuf>,
    /// Whether to list the system's Audio Units (macOS only).
    pub audio_units: bool,
}

impl Default for ScanOptions {
    /// The system's standard plugin locations.
    fn default() -> Self {
        Self {
            clap_dirs: clap::default_clap_dirs(),
            vst3_dirs: vst3::default_vst3_dirs(),
            audio_units: cfg!(target_os = "macos"),
        }
    }
}

impl ScanOptions {
    /// Scans nothing. Add directories with the `with_` methods.
    pub fn none() -> Self {
        Self {
            clap_dirs: Vec::new(),
            vst3_dirs: Vec::new(),
            audio_units: false,
        }
    }

    /// Also scans `dir` for CLAP plugins.
    pub fn with_clap_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.clap_dirs.push(dir.into());
        self
    }

    /// Also lists the VST3 bundles in `dir`.
    pub fn with_vst3_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.vst3_dirs.push(dir.into());
        self
    }

    /// Also lists the system's Audio Units.
    pub fn with_audio_units(mut self) -> Self {
        self.audio_units = true;
        self
    }
}

/// What a rescan changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Files loaded and described in this scan.
    pub scanned_files: usize,
    /// Files skipped because they had not changed since the last scan.
    pub reused_files: usize,
    /// Plugins now in the database.
    pub plugins: usize,
    /// Failures now in the database.
    pub failures: usize,
}

/// The cached result of a plugin scan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginDatabase {
    /// File format version, [`DATABASE_VERSION`].
    pub version: u32,
    /// Every plugin found, sorted by kind.
    #[serde(default)]
    pub plugins: Vec<PluginInfo>,
    /// Files and plugins that failed.
    #[serde(default)]
    pub failures: Vec<ScanFailure>,
    /// Stamps of the files the entries came from.
    #[serde(default)]
    pub files: Vec<FileStamp>,
}

impl PluginDatabase {
    /// An empty database.
    pub fn new() -> Self {
        Self {
            version: DATABASE_VERSION,
            ..Self::default()
        }
    }

    /// Where the database lives by default: the user's cache directory,
    /// `fourfloor/plugins.json`. `None` when no home directory is known.
    pub fn default_path() -> Option<PathBuf> {
        let base = if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
        } else if cfg!(target_os = "windows") {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        };
        base.map(|base| base.join("fourfloor").join("plugins.json"))
    }

    /// Reads a database file. A file written by a newer version is refused.
    pub fn load(path: &Path) -> Result<Self, HostError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| HostError::Io(format!("{}: {error}", path.display())))?;
        let database: Self = serde_json::from_str(&text)
            .map_err(|error| HostError::Database(format!("{}: {error}", path.display())))?;
        if database.version != DATABASE_VERSION {
            return Err(HostError::Database(format!(
                "{} has version {}, this build reads version {DATABASE_VERSION}",
                path.display(),
                database.version
            )));
        }
        Ok(database)
    }

    /// Reads a database file, or returns an empty database when the file
    /// is missing, unreadable or from another version: a cache is always
    /// safe to rebuild.
    pub fn load_or_empty(path: &Path) -> Self {
        Self::load(path).unwrap_or_else(|_| Self::new())
    }

    /// Writes the database as JSON, creating the parent directory. The file
    /// is written to a temporary name and renamed, so a crash cannot leave
    /// half a file behind.
    pub fn save(&self, path: &Path) -> Result<(), HostError> {
        let io = |error: std::io::Error| HostError::Io(format!("{}: {error}", path.display()));
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| HostError::Database(error.to_string()))?;
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, text).map_err(io)?;
        std::fs::rename(&temporary, path).map_err(io)
    }

    /// Scans from scratch.
    pub fn scan(thread: &Arc<HostThread>, options: &ScanOptions) -> Self {
        let mut database = Self::new();
        database.rescan(thread, options);
        database
    }

    /// Brings the database up to date with what is installed.
    ///
    /// A file whose stamp matches the last scan keeps its entries without
    /// being loaded. Changed and new files are loaded; entries for files
    /// that are gone are removed. Audio Units have no file to stamp: they
    /// are listed again each time, and one whose id and version are
    /// unchanged keeps its cached description.
    pub fn rescan(&mut self, thread: &Arc<HostThread>, options: &ScanOptions) -> ScanReport {
        let old_stamps: HashMap<PathBuf, FileStamp> = std::mem::take(&mut self.files)
            .into_iter()
            .map(|stamp| (stamp.path.clone(), stamp))
            .collect();
        let old_plugins = std::mem::take(&mut self.plugins);
        let old_failures = std::mem::take(&mut self.failures);
        let mut report = ScanReport::default();
        self.version = DATABASE_VERSION;

        let mut files: Vec<(PluginFormat, PathBuf)> = Vec::new();
        for dir in &options.clap_dirs {
            files.extend(
                clap::find_clap_bundles(dir)
                    .into_iter()
                    .map(|path| (PluginFormat::Clap, path)),
            );
        }
        for dir in &options.vst3_dirs {
            files.extend(
                vst3::find_vst3_bundles(dir)
                    .into_iter()
                    .map(|path| (PluginFormat::Vst3, path)),
            );
        }
        files.sort();
        files.dedup();

        for (format, path) in files {
            let stamp = FileStamp::read(&path);
            let unchanged = stamp.is_some() && stamp.as_ref() == old_stamps.get(&path);
            if unchanged {
                report.reused_files += 1;
                self.plugins.extend(
                    old_plugins
                        .iter()
                        .filter(|plugin| plugin.format == format && plugin.path == path)
                        .cloned(),
                );
                self.failures.extend(
                    old_failures
                        .iter()
                        .filter(|failure| failure.format == format && failure.path == path)
                        .cloned(),
                );
            } else {
                report.scanned_files += 1;
                self.scan_file(thread, format, &path);
            }
            self.files.extend(stamp);
        }

        if options.audio_units {
            self.scan_audio_units(thread, &old_plugins);
        }

        self.plugins.sort_by_key(PluginInfo::kind);
        self.plugins.dedup_by_key(|plugin| plugin.kind());
        report.plugins = self.plugins.len();
        report.failures = self.failures.len();
        report
    }

    fn scan_file(&mut self, thread: &Arc<HostThread>, format: PluginFormat, path: &Path) {
        match format {
            PluginFormat::Clap => match clap::scan_bundle(thread, path) {
                Ok(scan) => {
                    self.plugins.extend(scan.plugins);
                    self.failures
                        .extend(scan.failures.into_iter().map(|(id, reason)| ScanFailure {
                            format,
                            path: path.to_path_buf(),
                            id: Some(id),
                            reason,
                        }));
                }
                Err(error) => self.failures.push(ScanFailure {
                    format,
                    path: path.to_path_buf(),
                    id: None,
                    reason: error.to_string(),
                }),
            },
            PluginFormat::Vst3 => self.plugins.push(vst3::describe_bundle(path)),
            PluginFormat::Au => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn scan_audio_units(&mut self, thread: &Arc<HostThread>, old_plugins: &[PluginInfo]) {
        let cached: HashMap<(&str, &str), &PluginInfo> = old_plugins
            .iter()
            .filter(|plugin| plugin.format == PluginFormat::Au)
            .map(|plugin| ((plugin.id.as_str(), plugin.version.as_str()), plugin))
            .collect();
        for component in crate::au::list_components() {
            let key = (component.id.as_str(), component.version.as_str());
            if let Some(plugin) = cached.get(&key) {
                self.plugins.push((*plugin).clone());
                continue;
            }
            match crate::au::describe(thread, &component) {
                Ok(plugin) => self.plugins.push(plugin),
                Err(error) => self.failures.push(ScanFailure {
                    format: PluginFormat::Au,
                    path: PathBuf::new(),
                    id: Some(component.id.clone()),
                    reason: error.to_string(),
                }),
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn scan_audio_units(&mut self, _thread: &Arc<HostThread>, _old_plugins: &[PluginInfo]) {}

    /// Finds a plugin by its device kind, e.g. `plugin.clap:com.u-he.diva`.
    pub fn find(&self, kind: &str) -> Option<&PluginInfo> {
        let (format, id) = parse_plugin_kind(kind)?;
        self.plugins
            .iter()
            .find(|plugin| plugin.format == format && plugin.id == id)
    }

    /// The plugins of one format.
    pub fn by_format(&self, format: PluginFormat) -> impl Iterator<Item = &PluginInfo> {
        self.plugins
            .iter()
            .filter(move |plugin| plugin.format == format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_core::{DeviceCategory, ParamId, ParamInfo, ParamUnit};

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fourfloor-host-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_plugin() -> PluginInfo {
        PluginInfo {
            format: PluginFormat::Clap,
            id: "org.example.synth".into(),
            name: "Example Synth".into(),
            vendor: "Example".into(),
            version: "1.2.3".into(),
            category: DeviceCategory::Instrument,
            path: PathBuf::from("/plugins/Example.clap"),
            features: vec!["instrument".into(), "stereo".into()],
            params: vec![
                ParamInfo::new(ParamId(7), "", 20.0, 20_000.0, 1000.0, ParamUnit::Hertz)
                    .with_name("Cutoff")
                    .logarithmic(),
            ],
            audio_inputs: 0,
            audio_outputs: 2,
            sidechain: false,
            note_input: true,
            note_output: false,
            loadable: true,
        }
    }

    #[test]
    fn a_database_round_trips_through_json() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("nested/plugins.json");
        let mut database = PluginDatabase::new();
        database.plugins.push(sample_plugin());
        database.failures.push(ScanFailure {
            format: PluginFormat::Clap,
            path: PathBuf::from("/plugins/Broken.clap"),
            id: None,
            reason: "not a library".into(),
        });
        database.files.push(FileStamp {
            path: PathBuf::from("/plugins/Example.clap"),
            modified: 1_700_000_000,
            size: 4096,
        });
        database.save(&path).unwrap();
        assert!(!dir.join("nested/plugins.json.tmp").exists());
        let loaded = PluginDatabase::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(loaded, database);
        assert_eq!(
            loaded
                .find("plugin.clap:org.example.synth")
                .map(|p| &p.name),
            Some(&"Example Synth".to_owned())
        );
        assert!(loaded.find("plugin.au:org.example.synth").is_none());
        assert!(loaded.find("drum.kick909").is_none());
    }

    #[test]
    fn bad_files_are_typed_errors_and_load_or_empty_recovers() {
        let dir = scratch_dir("bad");
        let missing = dir.join("missing.json");
        assert!(matches!(
            PluginDatabase::load(&missing),
            Err(HostError::Io(_))
        ));

        let garbage = dir.join("garbage.json");
        std::fs::write(&garbage, "{ not json").unwrap();
        assert!(matches!(
            PluginDatabase::load(&garbage),
            Err(HostError::Database(_))
        ));

        let future = dir.join("future.json");
        std::fs::write(&future, r#"{"version": 999}"#).unwrap();
        let error = PluginDatabase::load(&future).unwrap_err();
        assert!(error.to_string().contains("version 999"), "{error}");

        for path in [&missing, &garbage, &future] {
            assert_eq!(PluginDatabase::load_or_empty(path), PluginDatabase::new());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_old_file_without_optional_fields_still_loads() {
        let dir = scratch_dir("minimal");
        let path = dir.join("plugins.json");
        std::fs::write(
            &path,
            r#"{"version":1,"plugins":[{"format":"clap","id":"a.b","name":"AB","category":"effect"}]}"#,
        )
        .unwrap();
        let database = PluginDatabase::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let plugin = &database.plugins[0];
        assert!(plugin.loadable && plugin.params.is_empty());
        assert_eq!(plugin.kind(), "plugin.clap:a.b");
    }

    #[test]
    fn scanning_empty_and_missing_directories_finds_nothing() {
        let dir = scratch_dir("empty");
        let thread = HostThread::spawn().unwrap();
        let options = ScanOptions::none()
            .with_clap_dir(&dir)
            .with_clap_dir("/definitely/not/a/dir")
            .with_vst3_dir(&dir);
        let database = PluginDatabase::scan(&thread, &options);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(database, PluginDatabase::new());
    }

    #[test]
    fn a_broken_file_is_recorded_then_reused_until_it_changes() {
        let dir = scratch_dir("broken");
        let bogus = dir.join("bogus.clap");
        std::fs::write(&bogus, b"not a library").unwrap();
        let thread = HostThread::spawn().unwrap();
        let options = ScanOptions::none().with_clap_dir(&dir);

        let mut database = PluginDatabase::new();
        let first = database.rescan(&thread, &options);
        assert_eq!(
            (first.scanned_files, first.reused_files, first.failures),
            (1, 0, 1)
        );
        assert_eq!(database.failures[0].path, bogus);
        assert_eq!(database.files.len(), 1);

        let second = database.rescan(&thread, &options);
        assert_eq!(
            (second.scanned_files, second.reused_files, second.failures),
            (0, 1, 1)
        );

        std::fs::write(&bogus, b"still not a library, but a different size").unwrap();
        let third = database.rescan(&thread, &options);
        assert_eq!((third.scanned_files, third.reused_files), (1, 0));

        std::fs::remove_file(&bogus).unwrap();
        let fourth = database.rescan(&thread, &options);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(fourth, ScanReport::default());
        assert!(database.files.is_empty() && database.failures.is_empty());
    }

    #[test]
    fn the_default_path_ends_in_the_database_file_name() {
        if let Some(path) = PluginDatabase::default_path() {
            assert!(
                path.ends_with("fourfloor/plugins.json"),
                "{}",
                path.display()
            );
        }
    }
}
