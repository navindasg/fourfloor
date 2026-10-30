//! Turning one audio file into a library row: decode, analyse, decide
//! kind, tempo and key, and pick tags.

use std::path::{Path, PathBuf};

use fourfloor_analysis::{analyze_track, Key, TrackAnalysis, TrackOptions};
use fourfloor_audio::{decode_bytes, AudioBuffer};

use crate::blob::{encode_features, WaveformSummary};
use crate::error::LibraryError;
use crate::tags::{auto_tags, is_unpitched, name_hints, NameHints};
use crate::types::FileKind;

/// Files this long or longer are tracks, not samples.
pub const TRACK_SECONDS: f64 = 60.0;
/// A key from analysis is kept only when the best key profile correlates
/// with the audio's chroma at least this well. Set on fourfloor's own
/// sounds: chords and notes score 0.7 to 0.9, drums 0.2 to 0.52.
pub const MIN_KEY_SCORE: f32 = 0.6;
/// A loop's length may be off a whole number of beats by this much.
const LOOP_LENGTH_TOLERANCE_SECONDS: f64 = 0.03;
/// Tempo assumed when a loop's length is the only clue.
pub const HOUSE_BPM: f64 = 124.0;

/// Everything the scanner stores for one file.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalyzedFile {
    /// Absolute path.
    pub path: PathBuf,
    /// File name with extension.
    pub filename: String,
    /// File name without extension.
    pub title: String,
    /// BLAKE3 of the bytes, hex.
    pub content_hash: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Modification time, ns since the epoch.
    pub mtime_ns: i64,
    /// One-shot, loop or track.
    pub kind: FileKind,
    /// Length in seconds.
    pub duration_seconds: f64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
    /// Tempo.
    pub bpm: Option<f64>,
    /// Tempo confidence (analysis regularity); `None` if from the name or
    /// the length.
    pub bpm_confidence: Option<f32>,
    /// First beat in seconds.
    pub first_beat_seconds: Option<f64>,
    /// Key.
    pub key: Option<Key>,
    /// Key confidence; `None` if from the name.
    pub key_confidence: Option<f32>,
    /// LUFS, `None` for silence or very short audio.
    pub loudness_lufs: Option<f64>,
    /// True peak dBTP, `None` for silence.
    pub true_peak_dbtp: Option<f64>,
    /// Encoded [`WaveformSummary`].
    pub waveform: Vec<u8>,
    /// Encoded feature vector.
    pub features: Vec<u8>,
    /// Automatic tags.
    pub tags: Vec<String>,
}

/// Facts about a file on disk, gathered before analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub size_bytes: u64,
    pub mtime_ns: i64,
}

/// The size and modification time of `path`.
pub(crate) fn stamp(path: &Path) -> Result<FileStamp, LibraryError> {
    let meta = std::fs::metadata(path).map_err(|error| LibraryError::io(path, error))?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX)
        });
    Ok(FileStamp {
        size_bytes: meta.len(),
        mtime_ns,
    })
}

/// BLAKE3 of `bytes` in hex.
pub(crate) fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Decodes and analyses already-read file bytes.
pub(crate) fn analyse_bytes(
    path: &Path,
    bytes: Vec<u8>,
    hash: String,
    stamp: FileStamp,
) -> Result<AnalyzedFile, LibraryError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_owned);
    let buffer = decode_bytes(bytes, extension.as_deref())?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LibraryError::NonUtf8Path(path.to_path_buf()))?
        .to_owned();
    let mut file = analyse_buffer(&filename, &buffer)?;
    file.path = path.to_path_buf();
    file.content_hash = hash;
    file.size_bytes = stamp.size_bytes;
    file.mtime_ns = stamp.mtime_ns;
    Ok(file)
}

/// Analyses decoded audio as if it were a file called `filename`. Path,
/// hash, size and time are left empty.
pub fn analyse_buffer(filename: &str, buffer: &AudioBuffer) -> Result<AnalyzedFile, LibraryError> {
    let analysis = analyze_track(buffer, &TrackOptions::default())?;
    let hints = name_hints(filename);
    let decision = decide(&analysis, &hints);
    let mut features = analysis.features.clone();
    features.tempo_bpm = decision.bpm;
    let tags = auto_tags(filename, &features, decision.kind);
    let (key, key_confidence) = if is_unpitched(&tags) {
        (None, None)
    } else if let Some(key) = hints.key {
        (Some(key), None)
    } else {
        match &analysis.key {
            Some(estimate)
                if estimate
                    .scores
                    .first()
                    .is_some_and(|s| s.score >= MIN_KEY_SCORE) =>
            {
                (Some(estimate.key), Some(estimate.confidence))
            }
            _ => (None, None),
        }
    };
    let finite = |value: f64| value.is_finite().then_some(value);
    let waveform = WaveformSummary::build(&buffer.to_mono(), buffer.sample_rate)?;
    let title = match filename.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
        _ => filename.to_owned(),
    };
    Ok(AnalyzedFile {
        path: PathBuf::new(),
        filename: filename.to_owned(),
        title,
        content_hash: String::new(),
        size_bytes: 0,
        mtime_ns: 0,
        kind: decision.kind,
        duration_seconds: analysis.duration_seconds,
        sample_rate: analysis.sample_rate,
        channels: u16::try_from(analysis.channels).unwrap_or(u16::MAX),
        bpm: decision.bpm,
        bpm_confidence: decision.bpm_confidence,
        first_beat_seconds: decision.first_beat,
        key,
        key_confidence,
        loudness_lufs: finite(analysis.levels.integrated_lufs),
        true_peak_dbtp: finite(analysis.levels.true_peak_dbtp),
        waveform: waveform.encode(),
        features: encode_features(&features),
        tags,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Decision {
    kind: FileKind,
    bpm: Option<f64>,
    bpm_confidence: Option<f32>,
    first_beat: Option<f64>,
}

/// The whole number of beats `seconds` holds at `bpm`, if it is close
/// enough to one and at least one bar.
fn whole_beats(seconds: f64, bpm: f64) -> Option<u32> {
    let beats = (seconds * bpm / 60.0).round();
    let off = (seconds - beats * 60.0 / bpm).abs();
    (beats >= 4.0 && off <= LOOP_LENGTH_TOLERANCE_SECONDS).then_some(beats as u32)
}

/// A tempo from a loop's length alone: the number of beats in 4, 8, 16,
/// 32 or 64 that gives a tempo nearest [`HOUSE_BPM`] within 70 to 180.
pub fn bpm_from_loop_length(seconds: f64) -> Option<f64> {
    if !(seconds.is_finite() && seconds > 0.0) {
        return None;
    }
    [4.0, 8.0, 16.0, 32.0, 64.0]
        .into_iter()
        .map(|beats: f64| beats * 60.0 / seconds)
        .filter(|bpm| (70.0..=180.0).contains(bpm))
        .min_by(|a, b| (a - HOUSE_BPM).abs().total_cmp(&(b - HOUSE_BPM).abs()))
}

/// Whether `a` is within 2 % of `b`, `b / 2` or `b * 2`.
fn near_with_octave(a: f64, b: f64) -> Option<f64> {
    [b, b / 2.0, b * 2.0]
        .into_iter()
        .find(|&candidate| (a / candidate - 1.0).abs() <= 0.02)
}

fn decide(analysis: &TrackAnalysis, hints: &NameHints) -> Decision {
    let duration = analysis.duration_seconds;
    let analysed = analysis.tempo.as_ref();
    let analysed_bpm = analysed.map(|tempo| tempo.bpm());
    let one_shot = Decision {
        kind: FileKind::OneShot,
        bpm: None,
        bpm_confidence: None,
        first_beat: None,
    };
    if duration >= TRACK_SECONDS {
        return Decision {
            kind: FileKind::Track,
            bpm: analysed_bpm,
            bpm_confidence: analysed.map(|tempo| tempo.regularity),
            first_beat: analysed.map(|tempo| tempo.grid.first_beat_seconds),
        };
    }
    if hints.one_shot {
        return one_shot;
    }
    let candidate = analysed_bpm.or(hints.bpm);
    let fits = candidate.and_then(|bpm| whole_beats(duration, bpm));
    let is_loop = fits.is_some() || (hints.looped && duration >= 1.0) || hints.bpm.is_some();
    if !is_loop {
        return one_shot;
    }
    // The tempo: the name when analysis agrees with it (or has nothing),
    // then the analysis snapped to the loop length, then the length.
    let (bpm, confidence) = match (analysed_bpm, hints.bpm) {
        (Some(found), Some(named)) if near_with_octave(found, named).is_some() => {
            (Some(named), analysed.map(|tempo| tempo.regularity))
        }
        (Some(found), _) => {
            let snapped = whole_beats(duration, found)
                .map(|beats| f64::from(beats) * 60.0 / duration)
                .filter(|exact| (exact / found - 1.0).abs() <= 0.01);
            (
                Some(snapped.unwrap_or(found)),
                analysed.map(|tempo| tempo.regularity),
            )
        }
        (None, Some(named)) => (Some(named), None),
        (None, None) => (bpm_from_loop_length(duration), None),
    };
    Decision {
        kind: FileKind::Loop,
        bpm,
        bpm_confidence: confidence,
        first_beat: analysed.map(|tempo| tempo.grid.first_beat_seconds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_lengths_give_house_tempos() {
        let bars4_at_124 = 16.0 * 60.0 / 124.0;
        assert!((bpm_from_loop_length(bars4_at_124).unwrap_or(0.0) - 124.0).abs() < 1e-9);
        // Two bars at 128.
        let two_bars = 8.0 * 60.0 / 128.0;
        assert!((bpm_from_loop_length(two_bars).unwrap_or(0.0) - 128.0).abs() < 1e-9);
        assert_eq!(bpm_from_loop_length(0.0), None);
        assert_eq!(bpm_from_loop_length(f64::NAN), None);
    }

    #[test]
    fn whole_beats_needs_a_bar_and_a_close_fit() {
        assert_eq!(whole_beats(2.0, 120.0), Some(4));
        assert_eq!(whole_beats(2.1, 120.0), None);
        assert_eq!(whole_beats(1.0, 120.0), None);
    }

    #[test]
    fn octave_errors_are_recognised() {
        assert_eq!(near_with_octave(62.0, 124.0), Some(62.0));
        assert_eq!(near_with_octave(124.5, 124.0), Some(124.0));
        assert_eq!(near_with_octave(100.0, 124.0), None);
    }
}
