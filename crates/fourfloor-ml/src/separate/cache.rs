//! On-disk cache of separated stems, keyed by the content of the audio.
//!
//! The key is a BLAKE3 hash of the separator name, the sample rate, the
//! channel count and every sample, so the same audio separated by the same
//! separator is found again whatever file it came from, and any change to
//! the audio or a separator version bump misses. Stems are stored as 32-bit
//! float WAV files (lossless) with a small JSON manifest, written to a
//! temporary directory and renamed into place so a crash never leaves a
//! half-written entry.

use std::fs;
use std::path::{Path, PathBuf};

use fourfloor_audio::{decode_file, write_wav_file, AudioBuffer, SampleFormat, WavSpec};
use serde::{Deserialize, Serialize};

use super::{Separator, Stem, Stems};
use crate::error::MlError;

const MANIFEST: &str = "stems.json";

#[derive(Serialize, Deserialize)]
struct Manifest {
    separator: String,
    fallback: bool,
    stems: Vec<String>,
}

/// A directory of cached separations.
#[derive(Clone, Debug)]
pub struct StemCache {
    dir: PathBuf,
}

impl StemCache {
    /// A cache in `dir` (created when first written).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The cache directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The cache key of `audio` separated by `separator`: 64 hex characters.
    pub fn key(separator: &dyn Separator, audio: &AudioBuffer) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"fourfloor-stems-v1\0");
        hasher.update(separator.name().as_bytes());
        hasher.update(&[0]);
        hasher.update(&audio.sample_rate.hz().to_le_bytes());
        hasher.update(&(audio.channel_count() as u64).to_le_bytes());
        hasher.update(&(audio.frames() as u64).to_le_bytes());
        for channel in &audio.channels {
            for sample in channel {
                hasher.update(&sample.to_le_bytes());
            }
        }
        hasher.finalize().to_hex().to_string()
    }

    /// The cached stems of `audio`, if there are any.
    pub fn get(
        &self,
        separator: &dyn Separator,
        audio: &AudioBuffer,
    ) -> Result<Option<Stems>, MlError> {
        let entry = self.dir.join(Self::key(separator, audio));
        let manifest_path = entry.join(MANIFEST);
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            return Ok(None);
        };
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|error| MlError::Audio(format!("{}: {error}", manifest_path.display())))?;
        let mut stems = Vec::with_capacity(manifest.stems.len());
        for name in manifest.stems {
            let path = entry.join(format!("{name}.wav"));
            let audio = decode_file(&path)
                .map_err(|error| MlError::Audio(format!("{}: {error}", path.display())))?;
            stems.push(Stem { name, audio });
        }
        Ok(Some(Stems {
            separator: manifest.separator,
            fallback: manifest.fallback,
            stems,
        }))
    }

    /// Stores `stems` as the separation of `audio`.
    pub fn put(
        &self,
        separator: &dyn Separator,
        audio: &AudioBuffer,
        stems: &Stems,
    ) -> Result<(), MlError> {
        let key = Self::key(separator, audio);
        let entry = self.dir.join(&key);
        let staging = self.dir.join(format!(".{key}.{}.tmp", std::process::id()));
        fs::create_dir_all(&staging).map_err(|error| MlError::io(&staging, error))?;
        let result = write_entry(&staging, stems).and_then(|()| {
            let _ = fs::remove_dir_all(&entry);
            fs::rename(&staging, &entry).map_err(|error| MlError::io(&entry, error))
        });
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    /// The cached stems of `audio`, separating and storing them on a miss.
    /// The flag says whether they came from the cache.
    pub fn get_or_separate(
        &self,
        separator: &dyn Separator,
        audio: &AudioBuffer,
    ) -> Result<(Stems, bool), MlError> {
        if let Some(stems) = self.get(separator, audio)? {
            return Ok((stems, true));
        }
        let stems = separator.separate(audio)?;
        self.put(separator, audio, &stems)?;
        Ok((stems, false))
    }
}

fn write_entry(dir: &Path, stems: &Stems) -> Result<(), MlError> {
    for stem in &stems.stems {
        let safe = !stem.name.is_empty()
            && stem
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !safe {
            return Err(MlError::InvalidInput(format!(
                "stem name `{}` is not a plain name",
                stem.name
            )));
        }
        let channels = u16::try_from(stem.audio.channel_count())
            .map_err(|_| MlError::InvalidInput("too many channels".to_owned()))?;
        let spec = WavSpec {
            sample_rate: stem.audio.sample_rate,
            channels,
            format: SampleFormat::Float32,
        };
        let path = dir.join(format!("{}.wav", stem.name));
        write_wav_file(&path, spec, &stem.audio.interleaved())
            .map_err(|error| MlError::Audio(format!("{}: {error}", path.display())))?;
    }
    let manifest = Manifest {
        separator: stems.separator.clone(),
        fallback: stems.fallback,
        stems: stems.stems.iter().map(|stem| stem.name.clone()).collect(),
    };
    let path = dir.join(MANIFEST);
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| MlError::Audio(error.to_string()))?;
    fs::write(&path, text).map_err(|error| MlError::io(&path, error))
}
