//! Content-addressed stores: the audio pool and plugin state blobs.
//!
//! Files are named by the BLAKE3 hash of their contents, so importing the
//! same audio twice stores it once, and a file can always be checked
//! against its name.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{io_error, StorageError, AUDIO_DIR, PLUGIN_STATE_DIR};
use crate::clip::AudioRef;
use crate::hash::ContentHash;
use crate::project::Project;
use crate::validate;

const COPY_BUFFER_BYTES: usize = 64 * 1024;
const PLUGIN_STATE_EXTENSION: &str = "bin";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn checked_extension(extension: &str) -> Result<String, StorageError> {
    let lowered = extension.to_ascii_lowercase();
    validate::extension(&lowered)
        .map(|()| lowered)
        .map_err(|_| StorageError::InvalidExtension(extension.to_owned()))
}

/// Copies `reader` into `dir` under `<hash>.<extension>`, hashing as it goes.
///
/// The data lands in a temporary file first and is renamed once complete,
/// so a half-written file never carries a content-hash name.
fn store(dir: &Path, mut reader: impl Read, extension: &str) -> Result<ContentHash, StorageError> {
    fs::create_dir_all(dir).map_err(io_error("create", dir))?;
    let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = dir.join(format!(".import-{}-{serial}.tmp", std::process::id()));
    let finish = |hash: ContentHash| {
        let target = dir.join(format!("{hash}.{extension}"));
        if target.exists() {
            fs::remove_file(&temporary).map_err(io_error("remove", &temporary))?;
        } else {
            fs::rename(&temporary, &target).map_err(io_error("store", &target))?;
        }
        Ok(hash)
    };
    copy_and_hash(&mut reader, &temporary)
        .and_then(finish)
        .inspect_err(|_| {
            // Best effort: the original error is the one worth reporting.
            let _ = fs::remove_file(&temporary);
        })
}

fn copy_and_hash(reader: &mut impl Read, target: &Path) -> Result<ContentHash, StorageError> {
    let mut file = fs::File::create(target).map_err(io_error("create", target))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(io_error("read into", target))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        file.write_all(&buffer[..count])
            .map_err(io_error("write", target))?;
    }
    file.sync_all().map_err(io_error("flush", target))?;
    Ok(ContentHash::from_blake3(&hasher.finalize()))
}

fn hash_file(path: &Path) -> Result<ContentHash, StorageError> {
    let file = fs::File::open(path).map_err(io_error("open", path))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file).map_err(io_error("read", path))?;
    Ok(ContentHash::from_blake3(&hasher.finalize()))
}

fn verify_file(path: &Path, expected: &ContentHash) -> Result<(), StorageError> {
    let actual = hash_file(path)?;
    if &actual == expected {
        Ok(())
    } else {
        Err(StorageError::HashMismatch {
            path: path.to_path_buf(),
            expected: expected.clone(),
            actual,
        })
    }
}

/// The `audio/` directory of a project folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPool {
    dir: PathBuf,
}

impl AudioPool {
    /// The audio pool of the project stored in `project_dir`.
    pub fn new(project_dir: &Path) -> Self {
        Self {
            dir: project_dir.join(AUDIO_DIR),
        }
    }

    /// The pool directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the file for `source` lives. The file may not exist.
    pub fn path_for(&self, source: &AudioRef) -> Result<PathBuf, StorageError> {
        // The hash is hex by construction; the extension is re-checked so a
        // hand-built reference cannot point outside the pool.
        validate::extension(&source.extension)
            .map_err(|_| StorageError::InvalidExtension(source.extension.clone()))?;
        Ok(self.dir.join(source.file_name()))
    }

    /// Stores audio data held in memory. `extension` has no dot.
    pub fn import_bytes(&self, bytes: &[u8], extension: &str) -> Result<AudioRef, StorageError> {
        let extension = checked_extension(extension)?;
        let hash = store(&self.dir, bytes, &extension)?;
        Ok(AudioRef { hash, extension })
    }

    /// Copies an audio file into the pool, keeping its extension.
    ///
    /// The file's contents are not decoded or checked to be audio.
    pub fn import_file(&self, source: &Path) -> Result<AudioRef, StorageError> {
        let raw = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let extension = checked_extension(raw)?;
        let file = fs::File::open(source).map_err(io_error("open", source))?;
        let hash = store(&self.dir, file, &extension)?;
        Ok(AudioRef { hash, extension })
    }

    /// Whether the pool holds the file for `source`.
    pub fn contains(&self, source: &AudioRef) -> bool {
        self.path_for(source).is_ok_and(|path| path.is_file())
    }

    /// Re-hashes the stored file and compares it with the reference.
    pub fn verify(&self, source: &AudioRef) -> Result<(), StorageError> {
        verify_file(&self.path_for(source)?, &source.hash)
    }

    /// The audio files `project` refers to that are not in the pool.
    pub fn missing(&self, project: &Project) -> Vec<AudioRef> {
        project
            .audio_refs()
            .into_iter()
            .filter(|source| !self.contains(source))
            .collect()
    }
}

/// The `plugin-state/` directory of a project folder: opaque blobs that
/// devices refer to by hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginStateStore {
    dir: PathBuf,
}

impl PluginStateStore {
    /// The plugin state store of the project stored in `project_dir`.
    pub fn new(project_dir: &Path) -> Self {
        Self {
            dir: project_dir.join(PLUGIN_STATE_DIR),
        }
    }

    /// Where the blob with this hash lives. The file may not exist.
    pub fn path_for(&self, hash: &ContentHash) -> PathBuf {
        self.dir.join(format!("{hash}.{PLUGIN_STATE_EXTENSION}"))
    }

    /// Stores a blob and returns its hash.
    pub fn put(&self, bytes: &[u8]) -> Result<ContentHash, StorageError> {
        store(&self.dir, bytes, PLUGIN_STATE_EXTENSION)
    }

    /// Reads a blob back, checking it against its hash.
    pub fn get(&self, hash: &ContentHash) -> Result<Vec<u8>, StorageError> {
        let path = self.path_for(hash);
        let bytes = fs::read(&path).map_err(io_error("read", &path))?;
        let actual = ContentHash::of_bytes(&bytes);
        if &actual == hash {
            Ok(bytes)
        } else {
            Err(StorageError::HashMismatch {
                path,
                expected: hash.clone(),
                actual,
            })
        }
    }
}
