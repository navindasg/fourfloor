//! A background thread that owns the models and runs ML jobs, so nothing
//! here ever runs on the audio or UI thread.
//!
//! Models load lazily on the first job that needs them and stay loaded.
//! When a model cannot be loaded (offline with an empty cache, a failed
//! download), separation and embedding fall back to their DSP versions and
//! say so in the result; transcription has no fallback and reports the
//! error.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use fourfloor_audio::AudioBuffer;

use crate::cache::ModelCache;
use crate::embed::{AudioEmbedder, DspEmbedder, Embedding};
use crate::error::MlError;
use crate::registry::ModelRegistry;
use crate::separate::{HpssEstimator, MaskSeparator, Separator, SpleeterOnnx, StemCache, Stems};
use crate::transcribe::{BasicPitch, TranscribeOptions, Transcription};

/// Work for the ML thread.
#[derive(Clone, Debug)]
pub enum Job {
    /// Audio to notes with Basic Pitch.
    Transcribe {
        /// The audio.
        audio: AudioBuffer,
        /// Decoder settings.
        options: TranscribeOptions,
    },
    /// Stems: Spleeter when `prefer_model` and it loads, else the fallback.
    Separate {
        /// The mix.
        audio: AudioBuffer,
        /// Try the Spleeter model first.
        prefer_model: bool,
    },
    /// An audio embedding (CLAP with the `clap` feature, else the fallback).
    EmbedAudio {
        /// The sample.
        audio: AudioBuffer,
    },
    /// A text embedding for search (needs CLAP).
    EmbedText {
        /// The query.
        text: String,
    },
}

/// What a finished job produced.
#[derive(Clone, Debug, PartialEq)]
pub enum JobOutput {
    /// From [`Job::Transcribe`].
    Transcription(Transcription),
    /// From [`Job::Separate`], with whether the stem cache had them.
    Stems {
        /// The stems; `stems.fallback` says which separator ran.
        stems: Stems,
        /// Whether they came from the stem cache.
        cached: bool,
    },
    /// From [`Job::EmbedAudio`] or [`Job::EmbedText`].
    Embedding(Embedding),
}

/// A finished job.
#[derive(Debug)]
pub struct JobResult {
    /// The id [`MlWorker::submit`] returned.
    pub id: u64,
    /// The output or the error.
    pub output: Result<JobOutput, MlError>,
}

/// How the worker finds its models.
#[derive(Debug)]
pub struct WorkerConfig {
    /// Where model files live (and are downloaded to, unless offline).
    pub models: ModelCache,
    /// The models that may be used.
    pub registry: ModelRegistry,
    /// Where separated stems are cached, if anywhere.
    pub stems: Option<StemCache>,
}

impl WorkerConfig {
    /// The default model directory, the built-in registry, no stem cache.
    pub fn new() -> Self {
        Self {
            models: ModelCache::in_default_dir(),
            registry: ModelRegistry::builtin(),
            stems: None,
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to the ML thread. Dropping it finishes queued jobs, then stops
/// the thread.
#[derive(Debug)]
pub struct MlWorker {
    jobs: Option<mpsc::Sender<(u64, Job)>>,
    results: mpsc::Receiver<JobResult>,
    next_id: u64,
    thread: Option<JoinHandle<()>>,
}

impl MlWorker {
    /// Starts the thread.
    pub fn spawn(config: WorkerConfig) -> Result<Self, MlError> {
        let (job_tx, job_rx) = mpsc::channel::<(u64, Job)>();
        let (result_tx, result_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("fourfloor-ml".to_owned())
            .spawn(move || {
                let mut state = Models::new(config);
                while let Ok((id, job)) = job_rx.recv() {
                    let output = state.run(job);
                    if result_tx.send(JobResult { id, output }).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| MlError::io("fourfloor-ml thread", error))?;
        Ok(Self {
            jobs: Some(job_tx),
            results: result_rx,
            next_id: 1,
            thread: Some(thread),
        })
    }

    /// Queues a job and returns its id.
    pub fn submit(&mut self, job: Job) -> Result<u64, MlError> {
        let id = self.next_id;
        self.jobs
            .as_ref()
            .ok_or(MlError::WorkerStopped)?
            .send((id, job))
            .map_err(|_| MlError::WorkerStopped)?;
        self.next_id += 1;
        Ok(id)
    }

    /// The next finished job, waiting for it.
    pub fn recv(&self) -> Result<JobResult, MlError> {
        self.results.recv().map_err(|_| MlError::WorkerStopped)
    }

    /// The next finished job, waiting at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<JobResult> {
        self.results.recv_timeout(timeout).ok()
    }

    /// A finished job, if one is ready.
    pub fn try_recv(&self) -> Option<JobResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for MlWorker {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The models, loaded on first use. A failed load is remembered so it is
/// not retried on every job.
struct Models {
    config: WorkerConfig,
    basic_pitch: Option<Result<BasicPitch, String>>,
    spleeter: Option<Option<MaskSeparator<SpleeterOnnx>>>,
    embedder: Option<Box<dyn AudioEmbedder>>,
    fallback_separator: MaskSeparator<HpssEstimator>,
}

impl Models {
    fn new(config: WorkerConfig) -> Self {
        Self {
            config,
            basic_pitch: None,
            spleeter: None,
            embedder: None,
            fallback_separator: HpssEstimator::separator(),
        }
    }

    fn run(&mut self, job: Job) -> Result<JobOutput, MlError> {
        match job {
            Job::Transcribe { audio, options } => {
                let config = &self.config;
                let model = self.basic_pitch.get_or_insert_with(|| {
                    BasicPitch::load(&config.models, &config.registry).map_err(|e| e.to_string())
                });
                match model {
                    Ok(model) => Ok(JobOutput::Transcription(
                        model.transcribe(&audio, &options)?,
                    )),
                    Err(message) => Err(MlError::inference(crate::registry::BASIC_PITCH, message)),
                }
            }
            Job::Separate {
                audio,
                prefer_model,
            } => {
                let separator: &dyn Separator = match prefer_model {
                    true => {
                        let config = &self.config;
                        let loaded = self.spleeter.get_or_insert_with(|| {
                            SpleeterOnnx::load(&config.models, &config.registry)
                                .ok()
                                .map(SpleeterOnnx::separator)
                        });
                        match loaded {
                            Some(spleeter) => spleeter,
                            None => &self.fallback_separator,
                        }
                    }
                    false => &self.fallback_separator,
                };
                let (stems, cached) = match &self.config.stems {
                    Some(cache) => cache.get_or_separate(separator, &audio)?,
                    None => (separator.separate(&audio)?, false),
                };
                Ok(JobOutput::Stems { stems, cached })
            }
            Job::EmbedAudio { audio } => {
                Ok(JobOutput::Embedding(self.embedder().embed_audio(&audio)?))
            }
            Job::EmbedText { text } => Ok(JobOutput::Embedding(self.embedder().embed_text(&text)?)),
        }
    }

    fn embedder(&mut self) -> &dyn AudioEmbedder {
        let config = &self.config;
        &**self.embedder.get_or_insert_with(|| load_embedder(config))
    }
}

#[cfg(feature = "clap")]
fn load_embedder(config: &WorkerConfig) -> Box<dyn AudioEmbedder> {
    match crate::embed::ClapEmbedder::load(&config.models, &config.registry) {
        Ok(clap) => Box::new(clap),
        Err(_) => Box::new(DspEmbedder),
    }
}

#[cfg(not(feature = "clap"))]
fn load_embedder(_config: &WorkerConfig) -> Box<dyn AudioEmbedder> {
    Box::new(DspEmbedder)
}
