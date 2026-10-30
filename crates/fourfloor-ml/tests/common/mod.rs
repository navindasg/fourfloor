//! Shared helpers for the model tests.

#![allow(dead_code)]

use std::sync::OnceLock;

use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;
use fourfloor_ml::{BasicPitch, ModelCache, ModelRegistry, ModelSpec, NoteEvent};

/// Set to `1` to make a missing cached model a test failure.
pub const REQUIRE_MODELS_ENV: &str = "FOURFLOOR_REQUIRE_MODELS";

/// A mono buffer.
pub fn audio(rate: SampleRate, signal: &[f32]) -> AudioBuffer {
    AudioBuffer::new(rate, vec![signal.to_vec()])
}

/// A stereo buffer.
pub fn stereo(rate: SampleRate, left: &[f32], right: &[f32]) -> AudioBuffer {
    AudioBuffer::new(rate, vec![left.to_vec(), right.to_vec()])
}

/// The cached files of a model, from the default cache directory, without
/// any network access. `None` (with a notice) when they are not cached.
pub fn cached(name: &str) -> Option<(ModelCache, ModelSpec)> {
    let registry = ModelRegistry::builtin();
    let spec = registry.get(name).expect("built-in model").clone();
    let cache = ModelCache::in_default_dir().offline();
    if cache.is_cached(&spec) {
        return Some((cache, spec));
    }
    let message = format!(
        "model `{name}` is not in {}; run the ignored download tests to fetch it",
        cache.dir().display()
    );
    if std::env::var(REQUIRE_MODELS_ENV).is_ok_and(|value| value == "1") {
        panic!("{message}");
    }
    eprintln!("SKIPPED: {message}");
    None
}

/// The Basic Pitch model, loaded once per test binary.
pub fn basic_pitch() -> Option<BasicPitch> {
    static MODEL: OnceLock<Option<BasicPitch>> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            let (cache, _) = cached(fourfloor_ml::BASIC_PITCH)?;
            Some(BasicPitch::load(&cache, &ModelRegistry::builtin()).expect("load Basic Pitch"))
        })
        .clone()
}

/// Note-level F1 score: a found note matches an expected `(start, pitch)`
/// with the same pitch whose start is within `tolerance` seconds; each
/// expected note matches at most once.
pub fn f1_score(found: &[NoteEvent], expected: &[(f64, u8)], tolerance: f64) -> f64 {
    let mut used = vec![false; expected.len()];
    let mut hits = 0usize;
    for note in found {
        let hit = expected
            .iter()
            .enumerate()
            .position(|(index, &(start, pitch))| {
                !used[index]
                    && pitch == note.pitch
                    && (note.start_seconds - start).abs() <= tolerance
            });
        if let Some(index) = hit {
            used[index] = true;
            hits += 1;
        }
    }
    if found.is_empty() || expected.is_empty() {
        return 0.0;
    }
    let precision = hits as f64 / found.len() as f64;
    let recall = hits as f64 / expected.len() as f64;
    if hits == 0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}
