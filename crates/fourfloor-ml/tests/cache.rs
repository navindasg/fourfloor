//! The model cache: license refusal before any download, checksum and size
//! verification, re-download of damaged copies, offline mode.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use fourfloor_ml::cache::sha256_hex;
use fourfloor_ml::{Fetcher, MlError, ModelCache, ModelFile, ModelRegistry, ModelSpec};

/// Serves fixed bytes for every URL and counts requests.
#[derive(Clone)]
struct FakeFetcher {
    body: Arc<Vec<u8>>,
    calls: Arc<AtomicUsize>,
}

impl FakeFetcher {
    fn new(body: &[u8]) -> Self {
        Self {
            body: Arc::new(body.to_vec()),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Fetcher for FakeFetcher {
    fn fetch(&self, _url: &str, dest: &mut dyn Write) -> Result<u64, MlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        for chunk in self.body.chunks(7) {
            dest.write_all(chunk).map_err(|error| MlError::Download {
                url: String::new(),
                message: error.to_string(),
            })?;
        }
        Ok(self.body.len() as u64)
    }
}

const BODY: &[u8] = b"pretend this is an onnx model";

fn spec(weights: &str, sha256: &str, size: u64) -> ModelSpec {
    ModelSpec {
        name: "toy".to_owned(),
        description: "test".to_owned(),
        homepage: "https://example.org".to_owned(),
        code_license: "MIT".to_owned(),
        weights_license: weights.to_owned(),
        files: vec![ModelFile::new(
            "model.onnx",
            "https://example.org/model.onnx",
            sha256,
            size,
        )],
    }
}

fn good_spec() -> ModelSpec {
    spec("Apache-2.0", &sha256_hex(BODY), BODY.len() as u64)
}

#[test]
fn a_verified_download_lands_in_the_cache_and_is_reused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fetcher = FakeFetcher::new(BODY);
    let cache = ModelCache::new(dir.path()).with_fetcher(fetcher.clone());
    let paths = cache.ensure(&good_spec()).expect("ensure");
    assert_eq!(paths.downloaded, 1);
    let path = paths.file("model.onnx").expect("file");
    assert_eq!(std::fs::read(path).expect("read"), BODY);
    assert!(cache.is_cached(&good_spec()));

    let again = cache.ensure(&good_spec()).expect("ensure again");
    assert_eq!(again.downloaded, 0);
    assert_eq!(fetcher.calls(), 1);
}

#[test]
fn a_checksum_mismatch_is_reported_and_nothing_is_kept() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fetcher = FakeFetcher::new(BODY);
    let cache = ModelCache::new(dir.path()).with_fetcher(fetcher.clone());
    let wrong = spec("MIT", &"0".repeat(64), BODY.len() as u64);
    match cache.ensure(&wrong) {
        Err(MlError::Checksum {
            file,
            expected,
            actual,
        }) => {
            assert_eq!(file, "model.onnx");
            assert_eq!(expected, "0".repeat(64));
            assert_eq!(actual, sha256_hex(BODY));
        }
        other => panic!("expected a checksum error, got {other:?}"),
    }
    assert!(!cache.is_cached(&wrong));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("toy"))
        .expect("model dir")
        .collect();
    assert!(
        leftovers.is_empty(),
        "partial files left behind: {leftovers:?}"
    );
}

#[test]
fn a_wrong_size_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = ModelCache::new(dir.path()).with_fetcher(FakeFetcher::new(BODY));
    for size in [BODY.len() as u64 - 3, BODY.len() as u64 + 3] {
        let entry = spec("MIT", &sha256_hex(BODY), size);
        assert!(
            matches!(cache.ensure(&entry), Err(MlError::Size { .. })),
            "size {size}"
        );
    }
}

#[test]
fn a_damaged_cached_copy_is_fetched_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fetcher = FakeFetcher::new(BODY);
    let cache = ModelCache::new(dir.path()).with_fetcher(fetcher.clone());
    let paths = cache.ensure(&good_spec()).expect("ensure");
    let path = paths.file("model.onnx").expect("file").to_owned();
    let mut damaged = BODY.to_vec();
    damaged[3] ^= 0xff;
    std::fs::write(&path, &damaged).expect("damage");
    // Same size, so only the hash can tell.
    assert!(cache.is_cached(&good_spec()));
    let again = cache.ensure(&good_spec()).expect("ensure again");
    assert_eq!(again.downloaded, 1);
    assert_eq!(std::fs::read(&path).expect("read"), BODY);
    assert_eq!(fetcher.calls(), 2);
}

#[test]
fn non_commercial_models_are_refused_before_any_download() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fetcher = FakeFetcher::new(BODY);
    let cache = ModelCache::new(dir.path()).with_fetcher(fetcher.clone());
    for license in [
        "CC-BY-NC-4.0",
        "CC-BY-NC-SA-4.0",
        "research-only",
        "other",
        "",
    ] {
        let entry = spec(license, &sha256_hex(BODY), BODY.len() as u64);
        assert!(
            matches!(cache.ensure(&entry), Err(MlError::LicenseRefused { .. })),
            "{license:?}"
        );
        let mut registry = ModelRegistry::new();
        assert!(matches!(
            registry.register(entry),
            Err(MlError::LicenseRefused { .. })
        ));
    }
    assert_eq!(fetcher.calls(), 0, "a refused model must cause no request");
    assert!(!dir.path().join("toy").exists());
}

#[test]
fn offline_mode_never_fetches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fetcher = FakeFetcher::new(BODY);
    let cache = ModelCache::new(dir.path())
        .with_fetcher(fetcher.clone())
        .offline();
    assert!(matches!(
        cache.ensure(&good_spec()),
        Err(MlError::Offline { .. })
    ));
    assert_eq!(fetcher.calls(), 0);
}

#[test]
fn file_urls_serve_local_mirrors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("mirror.onnx");
    std::fs::write(&source, BODY).expect("write mirror");
    let mut entry = good_spec();
    entry.files[0].url = format!("file://{}", source.display());
    let cache = ModelCache::new(dir.path().join("cache"));
    let paths = cache.ensure(&entry).expect("ensure");
    assert_eq!(paths.downloaded, 1);
    cache.remove(&entry).expect("remove");
    assert!(!cache.is_cached(&entry));
}

#[test]
fn every_builtin_model_is_permissive_and_small_enough() {
    let registry = ModelRegistry::builtin();
    for spec in registry.iter() {
        assert!(
            fourfloor_ml::is_allowed(&spec.code_license),
            "{}",
            spec.name
        );
        assert!(
            fourfloor_ml::is_allowed(&spec.weights_license),
            "{}",
            spec.name
        );
    }
    assert!(registry.total_size() < 1_000_000_000);
}
