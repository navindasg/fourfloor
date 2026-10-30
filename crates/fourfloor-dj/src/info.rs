//! Cached track analysis: tempo, beat grid and key from
//! `fourfloor-analysis`, stored as small JSON files.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use fourfloor_analysis::{analyze_track, AnalysisError, Key, TrackOptions};
use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;
use serde::{Deserialize, Serialize};

use crate::error::DjError;
use crate::track::DeckGrid;

/// Version of the cache file layout. Files of another version are ignored
/// and rewritten.
pub const CACHE_VERSION: u32 = 1;

/// What a deck needs to know about a track, in plain serializable fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
    /// Tempo, when the track has a steady beat.
    pub bpm: Option<f64>,
    /// Time of a beat, in seconds from the start.
    pub first_beat_seconds: f64,
    /// Beats in a bar.
    pub beats_per_bar: u32,
    /// Which beat, counted from the first, is the first downbeat.
    pub downbeat_index: u32,
    /// Key name such as `"A minor"`, when one was found.
    pub key: Option<String>,
    /// Camelot code such as `"8A"`.
    pub camelot: Option<String>,
    /// Length in seconds.
    pub duration_seconds: f64,
    /// Integrated loudness in LUFS.
    pub integrated_lufs: f64,
}

impl TrackInfo {
    /// Analyses a decoded track. Slow; run it off the audio thread.
    pub fn analyze(buffer: &AudioBuffer) -> Result<Self, AnalysisError> {
        let analysis = analyze_track(buffer, &TrackOptions::default())?;
        let grid = analysis.tempo.as_ref().map(|tempo| tempo.grid);
        let key = analysis.key.as_ref().map(|estimate| estimate.key);
        Ok(Self {
            bpm: grid.map(|grid| grid.bpm),
            first_beat_seconds: grid.map_or(0.0, |grid| grid.first_beat_seconds),
            beats_per_bar: grid.map_or(4, |grid| grid.beats_per_bar),
            downbeat_index: grid.map_or(0, |grid| grid.downbeat_index),
            key: key.map(|key| key.name()),
            camelot: key.map(|key| key.camelot()),
            duration_seconds: analysis.duration_seconds,
            integrated_lufs: analysis.levels.integrated_lufs,
        })
    }

    /// Info with a known tempo and key and nothing else, for tracks whose
    /// metadata comes from elsewhere (a library, a test).
    pub fn known(bpm: f64, key: Option<Key>, duration_seconds: f64) -> Self {
        Self {
            bpm: Some(bpm),
            first_beat_seconds: 0.0,
            beats_per_bar: 4,
            downbeat_index: 0,
            key: key.map(|key| key.name()),
            camelot: key.map(|key| key.camelot()),
            duration_seconds,
            integrated_lufs: f64::NEG_INFINITY,
        }
    }

    /// The key, parsed.
    pub fn key(&self) -> Option<Key> {
        self.key.as_deref().and_then(|name| name.parse().ok())
    }

    /// The beat grid in samples of a track at `sample_rate`.
    pub fn grid(&self, sample_rate: SampleRate) -> Option<DeckGrid> {
        let grid = DeckGrid::new(self.bpm?, self.first_beat_seconds, sample_rate).ok()?;
        Some(grid.with_bar(self.beats_per_bar, self.downbeat_index))
    }
}

/// One cache file.
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    version: u32,
    path: String,
    size: u64,
    modified_nanos: u128,
    info: TrackInfo,
}

/// A directory of analysis results keyed by file path, size and
/// modification time, so a track is analysed once.
#[derive(Clone, Debug)]
pub struct AnalysisCache {
    dir: PathBuf,
}

/// FNV-1a, so file names do not depend on the standard library's hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

impl AnalysisCache {
    /// A cache in `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory holding the cache files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn stamp(path: &Path) -> Result<(String, u64, u128), DjError> {
        let io = |source| DjError::Io {
            path: path.to_path_buf(),
            source,
        };
        let full = fs::canonicalize(path).map_err(io)?;
        let meta = fs::metadata(&full).map_err(io)?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        Ok((full.to_string_lossy().into_owned(), meta.len(), modified))
    }

    fn entry_path(&self, full_path: &str) -> PathBuf {
        self.dir
            .join(format!("{:016x}.json", fnv1a(full_path.as_bytes())))
    }

    /// The cached info for `path`, if the file has not changed since it
    /// was analysed.
    pub fn get(&self, path: &Path) -> Option<TrackInfo> {
        let (full, size, modified) = Self::stamp(path).ok()?;
        let text = fs::read_to_string(self.entry_path(&full)).ok()?;
        let entry: CacheEntry = serde_json::from_str(&text).ok()?;
        (entry.version == CACHE_VERSION
            && entry.path == full
            && entry.size == size
            && entry.modified_nanos == modified)
            .then_some(entry.info)
    }

    /// Stores `info` for `path`.
    pub fn put(&self, path: &Path, info: &TrackInfo) -> Result<(), DjError> {
        let (full, size, modified) = Self::stamp(path)?;
        let target = self.entry_path(&full);
        let io = |source| DjError::Io {
            path: target.clone(),
            source,
        };
        fs::create_dir_all(&self.dir).map_err(io)?;
        let entry = CacheEntry {
            version: CACHE_VERSION,
            path: full,
            size,
            modified_nanos: modified,
            info: info.clone(),
        };
        // Written beside the target and renamed, so a crash never leaves
        // half a file behind.
        let temporary = target.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&entry)?).map_err(io)?;
        fs::rename(&temporary, &target).map_err(io)
    }

    /// The info for `path`: from the cache, or analysed now and stored.
    /// The flag says whether it came from the cache.
    pub fn get_or_analyze(&self, path: &Path) -> Result<(TrackInfo, bool), DjError> {
        if let Some(info) = self.get(path) {
            return Ok((info, true));
        }
        let buffer = fourfloor_audio::decode_file(path).map_err(crate::error::TrackError::from)?;
        let info = TrackInfo::analyze(&buffer)?;
        self.put(path, &info)?;
        Ok((info, false))
    }
}
