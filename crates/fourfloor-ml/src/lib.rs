//! Machine-learning features for fourfloor, using only openly licensed
//! weights and running in-process (pure-Rust ONNX inference with tract; no
//! Python and no native runtime in the shipped app).
//!
//! - [`registry`] and [`cache`]: every model fourfloor may run, with its
//!   code and weights licenses, downloaded on first use and checked
//!   against a SHA-256. Models whose licenses are not on the allow-list in
//!   [`license`] (MIT, Apache-2.0, BSD, CC0, CC BY) are refused.
//! - [`transcribe`]: audio to MIDI with Spotify Basic Pitch.
//! - [`separate`]: stems with Spleeter, or a labelled DSP fallback, with a
//!   content-hash cache.
//! - [`embed`]: sample embeddings for text and similarity search, with LAION
//!   CLAP (feature `clap`) or a DSP-feature fallback.
//! - [`worker`]: a background thread that owns the loaded models and runs
//!   jobs, falling back to the DSP versions when a model cannot load.
//! - [`groove`]: a rule- and statistics-based drum humanizer standing in
//!   for GrooVAE, and groove extraction from played parts.
//!
//! Everything here allocates and blocks: run it on a worker thread, never
//! on the audio thread.

#![warn(missing_docs)]

pub use fourfloor_core as core;

pub mod cache;
pub mod embed;
pub mod error;
pub mod groove;
pub mod license;
mod onnx;
pub mod registry;
pub mod separate;
pub mod stft;
pub mod transcribe;
pub mod worker;

pub use cache::{Fetcher, HttpFetcher, ModelCache, ModelPaths, MODEL_DIR_ENV};
#[cfg(feature = "clap")]
pub use embed::ClapEmbedder;
pub use embed::{rank, AudioEmbedder, DspEmbedder, Embedding, DSP_EMBEDDING};
pub use error::{LicenseField, MlError};
pub use groove::{humanize, learn_groove, HumanizeOptions};
pub use license::{classify, is_allowed, AllowedLicense, ALLOWED_LICENSES};
pub use registry::{
    ModelFile, ModelRegistry, ModelSpec, BASIC_PITCH, CLAP_GENERAL, SPLEETER_2STEMS,
};
pub use separate::{
    HpssEstimator, HpssOptions, MaskEstimator, MaskSeparator, Separator, SpleeterOnnx, Stem,
    StemCache, Stems,
};
pub use stft::{Spectrogram, Stft, Window};
pub use transcribe::{BasicPitch, NoteEvent, Posteriorgram, TranscribeOptions, Transcription};
pub use worker::{Job, JobOutput, JobResult, MlWorker, WorkerConfig};
