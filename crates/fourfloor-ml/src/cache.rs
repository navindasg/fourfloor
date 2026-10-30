//! Download-on-first-use model cache with checksum verification.
//!
//! [`ModelCache::ensure`] is the only way a model file reaches the disk. It
//! checks the model's licenses first, then for every file either verifies
//! the cached copy or downloads it to a temporary name, hashes it, and only
//! renames it into place when size and SHA-256 match the registry.
//!
//! All of this blocks; call it from a worker thread.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::MlError;
use crate::registry::{ModelFile, ModelSpec};

/// Environment variable that overrides [`ModelCache::default_dir`].
pub const MODEL_DIR_ENV: &str = "FOURFLOOR_MODEL_DIR";

/// Something that can copy the contents of a URL into a writer.
pub trait Fetcher: Send + Sync {
    /// Streams `url` into `dest` and returns the number of bytes written.
    fn fetch(&self, url: &str, dest: &mut dyn Write) -> Result<u64, MlError>;
}

/// Fetches `https://` URLs with `ureq` and `file://` URLs from the disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpFetcher;

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, dest: &mut dyn Write) -> Result<u64, MlError> {
        let failed = |message: String| MlError::Download {
            url: url.to_owned(),
            message,
        };
        if let Some(path) = url.strip_prefix("file://") {
            let mut file = fs::File::open(path).map_err(|error| failed(error.to_string()))?;
            return std::io::copy(&mut file, dest).map_err(|error| failed(error.to_string()));
        }
        if !url.starts_with("https://") {
            return Err(failed(
                "only https:// and file:// URLs are fetched".to_owned(),
            ));
        }
        let response = ureq::get(url)
            .call()
            .map_err(|error| failed(error.to_string()))?;
        let mut reader = response.into_body().into_reader();
        std::io::copy(&mut reader, dest).map_err(|error| failed(error.to_string()))
    }
}

/// Where the files of one model ended up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPaths {
    /// The model's directory inside the cache.
    pub dir: PathBuf,
    /// `(file name, path)` for every file, in registry order.
    pub files: Vec<(String, PathBuf)>,
    /// How many files this call downloaded (0 when everything was cached).
    pub downloaded: usize,
}

impl ModelPaths {
    /// The path of the file called `name`.
    pub fn file(&self, name: &str) -> Result<&Path, MlError> {
        self.files
            .iter()
            .find(|(file, _)| file == name)
            .map(|(_, path)| path.as_path())
            .ok_or_else(|| MlError::InvalidInput(format!("model has no file `{name}`")))
    }
}

/// A directory of verified model files.
pub struct ModelCache {
    dir: PathBuf,
    fetcher: Box<dyn Fetcher>,
    offline: bool,
}

impl core::fmt::Debug for ModelCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModelCache")
            .field("dir", &self.dir)
            .field("offline", &self.offline)
            .finish_non_exhaustive()
    }
}

impl ModelCache {
    /// A cache in `dir` that downloads over HTTPS.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            fetcher: Box::new(HttpFetcher),
            offline: false,
        }
    }

    /// A cache in [`ModelCache::default_dir`].
    pub fn in_default_dir() -> Self {
        Self::new(Self::default_dir())
    }

    /// Replaces the downloader (tests use this to avoid the network).
    pub fn with_fetcher(mut self, fetcher: impl Fetcher + 'static) -> Self {
        self.fetcher = Box::new(fetcher);
        self
    }

    /// Never download: a file that is not cached is an error.
    pub fn offline(mut self) -> Self {
        self.offline = true;
        self
    }

    /// The per-user model directory: `$FOURFLOOR_MODEL_DIR` if set, else
    /// `~/Library/Caches/fourfloor/models` on macOS,
    /// `%LOCALAPPDATA%\fourfloor\models` on Windows and
    /// `$XDG_CACHE_HOME/fourfloor/models` (or `~/.cache/...`) elsewhere.
    pub fn default_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os(MODEL_DIR_ENV).filter(|dir| !dir.is_empty()) {
            return PathBuf::from(dir);
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let base = if cfg!(target_os = "macos") {
            home.map(|home| home.join("Library").join("Caches"))
        } else if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.map(|home| home.join(".cache")))
        };
        base.unwrap_or_else(std::env::temp_dir)
            .join("fourfloor")
            .join("models")
    }

    /// The cache directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where `file` of `spec` lives (whether or not it is there yet).
    pub fn path(&self, spec: &ModelSpec, file: &ModelFile) -> PathBuf {
        self.dir.join(&spec.name).join(&file.name)
    }

    /// Whether every file of `spec` is present with the recorded size. This
    /// is a cheap check that does not hash; [`ModelCache::ensure`] does.
    pub fn is_cached(&self, spec: &ModelSpec) -> bool {
        spec.files.iter().all(|file| {
            fs::metadata(self.path(spec, file))
                .map(|meta| meta.is_file() && meta.len() == file.size)
                .unwrap_or(false)
        })
    }

    /// Makes every file of `spec` available and verified.
    ///
    /// The licenses are checked before anything else, so a refused model
    /// causes no network request and no file. A cached file whose checksum
    /// no longer matches is deleted and downloaded again; a download whose
    /// size or checksum is wrong is deleted and reported.
    pub fn ensure(&self, spec: &ModelSpec) -> Result<ModelPaths, MlError> {
        spec.validate()?;
        let dir = self.dir.join(&spec.name);
        let mut files = Vec::with_capacity(spec.files.len());
        let mut downloaded = 0;
        for file in &spec.files {
            let path = dir.join(&file.name);
            if !self.cached_copy_is_good(&path, file)? {
                if self.offline {
                    return Err(MlError::Offline {
                        file: file.name.clone(),
                    });
                }
                fs::create_dir_all(&dir).map_err(|error| MlError::io(&dir, error))?;
                self.download(file, &path)?;
                downloaded += 1;
            }
            files.push((file.name.clone(), path));
        }
        Ok(ModelPaths {
            dir,
            files,
            downloaded,
        })
    }

    /// Deletes the cached files of `spec`.
    pub fn remove(&self, spec: &ModelSpec) -> Result<(), MlError> {
        let dir = self.dir.join(&spec.name);
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(MlError::io(dir, error)),
        }
    }

    fn cached_copy_is_good(&self, path: &Path, file: &ModelFile) -> Result<bool, MlError> {
        let Ok(meta) = fs::metadata(path) else {
            return Ok(false);
        };
        if meta.is_file() && meta.len() == file.size && sha256_file(path)? == file.sha256 {
            return Ok(true);
        }
        // A damaged or stale copy: remove it so it is fetched again.
        fs::remove_file(path).map_err(|error| MlError::io(path, error))?;
        Ok(false)
    }

    fn download(&self, file: &ModelFile, path: &Path) -> Result<(), MlError> {
        let partial = path.with_extension(format!("part{}", std::process::id()));
        let result = self
            .download_to(file, &partial)
            .and_then(|()| fs::rename(&partial, path).map_err(|error| MlError::io(path, error)));
        if result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        result
    }

    fn download_to(&self, file: &ModelFile, partial: &Path) -> Result<(), MlError> {
        let out = fs::File::create(partial).map_err(|error| MlError::io(partial, error))?;
        let mut writer = HashingWriter {
            inner: std::io::BufWriter::new(out),
            hasher: Sha256::new(),
            written: 0,
            limit: file.size,
        };
        let fetched = self.fetcher.fetch(&file.url, &mut writer);
        let HashingWriter {
            mut inner,
            hasher,
            written,
            ..
        } = writer;
        inner.flush().map_err(|error| MlError::io(partial, error))?;
        drop(inner);
        if written != file.size {
            // A wrong size is the more precise report than the transport
            // error an over-long body is cut off with.
            if written > file.size || fetched.is_ok() {
                return Err(MlError::Size {
                    file: file.name.clone(),
                    expected: file.size,
                    actual: written,
                });
            }
        }
        fetched?;
        let actual = hex(&hasher.finalize());
        if actual != file.sha256 {
            return Err(MlError::Checksum {
                file: file.name.clone(),
                expected: file.sha256.clone(),
                actual,
            });
        }
        Ok(())
    }
}

/// Hashes what passes through and refuses to store more than `limit` bytes,
/// so a wrong URL cannot fill the disk.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
    limit: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.written += bytes.len() as u64;
        if self.written > self.limit {
            return Err(std::io::Error::other(
                "body is larger than the recorded size",
            ));
        }
        self.hasher.update(bytes);
        self.inner.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// SHA-256 of a byte slice as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String, MlError> {
    let mut file = fs::File::open(path).map_err(|error| MlError::io(path, error))?;
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; 1 << 16];
    loop {
        let read = file
            .read(&mut block)
            .map_err(|error| MlError::io(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
    }
    Ok(hex(&hasher.finalize()))
}
