//! The records the library hands out.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use fourfloor_analysis::{Key, Mode};

use crate::error::LibraryError;

/// Row id of a file in the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub i64);

/// Row id of a crate (an unordered collection of files).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CrateId(pub i64);

/// Row id of a playlist (an ordered list of files).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaylistId(pub i64);

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What sort of audio a file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// A single hit or note: no beat of its own.
    OneShot,
    /// A short phrase with a tempo, meant to repeat.
    Loop,
    /// A whole piece of music.
    Track,
}

impl FileKind {
    /// Every kind.
    pub const ALL: [Self; 3] = [Self::OneShot, Self::Loop, Self::Track];

    /// The name stored in the database: `one-shot`, `loop` or `track`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneShot => "one-shot",
            Self::Loop => "loop",
            Self::Track => "track",
        }
    }
}

impl fmt::Display for FileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FileKind {
    type Err = LibraryError;

    /// Reads `one-shot` (also `oneshot`, `one_shot`), `loop` and `track`.
    fn from_str(text: &str) -> Result<Self, LibraryError> {
        match text.trim().to_ascii_lowercase().as_str() {
            "one-shot" | "oneshot" | "one_shot" | "one shot" => Ok(Self::OneShot),
            "loop" => Ok(Self::Loop),
            "track" => Ok(Self::Track),
            other => Err(LibraryError::UnknownValue {
                column: "kind",
                value: other.to_owned(),
            }),
        }
    }
}

/// Where a tag came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagSource {
    /// Worked out by the scanner; replaced when the file is analysed again.
    Auto,
    /// Added by the user; never touched by the scanner.
    User,
}

impl TagSource {
    /// The name stored in the database: `auto` or `user`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::User => "user",
        }
    }

    pub(crate) fn parse(text: &str) -> Self {
        if text == "user" {
            Self::User
        } else {
            Self::Auto
        }
    }
}

/// A tag on a file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tag {
    /// Lowercase name.
    pub name: String,
    /// Who added it.
    pub source: TagSource,
}

/// One indexed file. The waveform and feature vector are fetched
/// separately ([`Library::waveform`](crate::Library::waveform),
/// [`Library::features`](crate::Library::features)).
#[derive(Clone, Debug, PartialEq)]
pub struct FileRecord {
    /// Row id.
    pub id: FileId,
    /// Absolute path.
    pub path: PathBuf,
    /// File name with its extension.
    pub filename: String,
    /// File name without its extension.
    pub title: String,
    /// BLAKE3 hash of the file's bytes, in hex.
    pub content_hash: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// One-shot, loop or track.
    pub kind: FileKind,
    /// Length in seconds.
    pub duration_seconds: f64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Tempo, when the audio has one.
    pub bpm: Option<f64>,
    /// How sure the analysis is of the tempo, 0 to 1. `None` when the
    /// tempo came from the file name or the loop length.
    pub bpm_confidence: Option<f32>,
    /// Time of the first beat in seconds, when a beat grid was found.
    pub first_beat_seconds: Option<f64>,
    /// Musical key, when the audio is pitched.
    pub key: Option<Key>,
    /// How sure the analysis is of the key, 0 to 1.
    pub key_confidence: Option<f32>,
    /// Integrated loudness in LUFS. `None` for silence.
    pub loudness_lufs: Option<f64>,
    /// True peak in dBTP. `None` for silence.
    pub true_peak_dbtp: Option<f64>,
    /// Tags, sorted by name.
    pub tags: Vec<Tag>,
    /// Star rating, 0 to 5.
    pub rating: u8,
    /// How many times the file was played.
    pub play_count: u32,
    /// When it was last played (Unix seconds).
    pub last_played_at: Option<i64>,
    /// The file was not found on disk by the last scan of its folder.
    pub missing: bool,
    /// Tempo, key and kind were set by hand and survive re-analysis.
    pub locked: bool,
    /// When the file was first indexed (Unix seconds).
    pub added_at: i64,
    /// When it was last analysed (Unix seconds).
    pub analyzed_at: i64,
}

impl FileRecord {
    /// Whether the file carries the tag `name` (case-insensitive).
    pub fn has_tag(&self, name: &str) -> bool {
        let wanted = name.trim().to_ascii_lowercase();
        self.tags.iter().any(|tag| tag.name == wanted)
    }

    /// The tag names, sorted.
    pub fn tag_names(&self) -> Vec<&str> {
        self.tags.iter().map(|tag| tag.name.as_str()).collect()
    }
}

/// One entry of the play history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayEvent {
    /// The file that was played.
    pub file: FileId,
    /// When (Unix seconds).
    pub played_at: i64,
    /// Where it was played from: `preview`, `deck-a`, a project name.
    pub context: String,
}

/// A crate or playlist and how many files it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection<Id> {
    /// Row id.
    pub id: Id,
    /// Name, unique among its sort.
    pub name: String,
    /// Number of files in it.
    pub len: usize,
}

/// A key as the integer stored in the database: tonic 0..=11, plus 12 for
/// minor.
pub(crate) fn key_code(key: Key) -> i64 {
    i64::from(key.tonic() % 12)
        + match key.mode() {
            Mode::Major => 0,
            Mode::Minor => 12,
        }
}

/// The key for a stored code, or `None` if the code is out of range.
pub(crate) fn key_from_code(code: i64) -> Option<Key> {
    let tonic = u8::try_from(code.rem_euclid(12)).ok()?;
    match code {
        0..=11 => Some(Key::major(tonic)),
        12..=23 => Some(Key::minor(tonic)),
        _ => None,
    }
}

/// Position on the Camelot wheel as one sortable number: 1A, 1B, 2A, ...
pub(crate) fn camelot_order(key: Key) -> i64 {
    i64::from(key.camelot_number()) * 2
        + match key.mode() {
            Mode::Minor => 0,
            Mode::Major => 1,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_codes_round_trip_for_all_24_keys() {
        for key in Key::all() {
            assert_eq!(key_from_code(key_code(key)), Some(key));
        }
        assert_eq!(key_from_code(24), None);
        assert_eq!(key_from_code(-1), None);
    }

    #[test]
    fn camelot_order_follows_the_wheel() {
        let a_minor = Key::minor(9); // 8A
        let c_major = Key::major(0); // 8B
        let e_minor = Key::minor(4); // 9A
        assert!(camelot_order(a_minor) < camelot_order(c_major));
        assert!(camelot_order(c_major) < camelot_order(e_minor));
    }

    #[test]
    fn kind_names_round_trip() {
        for kind in FileKind::ALL {
            assert_eq!(kind.as_str().parse::<FileKind>().ok(), Some(kind));
        }
        assert!("jingle".parse::<FileKind>().is_err());
    }
}
