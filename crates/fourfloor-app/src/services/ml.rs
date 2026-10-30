//! Machine-learning jobs on clips: stem separation and audio-to-MIDI, plus
//! the model registry's licenses and download state for the settings.
//!
//! Models come from `fourfloor-ml`'s registry (open weights only; the
//! registry refuses anything non-commercial). They are used only when
//! already in the cache, unless the user downloads them from the settings;
//! separation falls back to the labelled DSP separator meanwhile.

use std::path::PathBuf;

use fourfloor_audio::{decode_file, encode_wav, AudioBuffer, SampleFormat, WavSpec};
use fourfloor_core::{ClipId, Tempo, Ticks, TrackId, TICKS_PER_BEAT};
use fourfloor_ml::{
    BasicPitch, HpssEstimator, ModelCache, ModelRegistry, Separator, SpleeterOnnx,
    TranscribeOptions, BASIC_PITCH, SPLEETER_2STEMS,
};
use fourfloor_model::command::{AddAudioClip, CreateTrack};
use fourfloor_model::{AudioPool, AudioRef, ClipContent, Command, Project, TrackKind};

use super::Progress;
use crate::viewmodel::planned_commands;

/// One model as the settings list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelStatus {
    /// Registry name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// License of its code (SPDX).
    pub code_license: String,
    /// License of its weights (SPDX).
    pub weights_license: String,
    /// Download size in bytes.
    pub size: u64,
    /// Already downloaded and verified.
    pub cached: bool,
    /// Project page.
    pub homepage: String,
}

/// The built-in models and whether each is in `cache`.
pub fn model_statuses(cache: &ModelCache) -> Vec<ModelStatus> {
    ModelRegistry::builtin()
        .iter()
        .map(|spec| ModelStatus {
            name: spec.name.clone(),
            description: spec.description.clone(),
            code_license: spec.code_license.clone(),
            weights_license: spec.weights_license.clone(),
            size: spec.total_size(),
            cached: cache.is_cached(spec),
            homepage: spec.homepage.clone(),
        })
        .collect()
}

/// Job: download (and verify) model `name` into `cache_dir`.
pub fn download_job(
    name: String,
    cache_dir: PathBuf,
) -> impl FnOnce(&Progress) -> Result<String, String> + Send + 'static {
    move |progress| {
        progress.set(0.05);
        let registry = ModelRegistry::builtin();
        let spec = registry.get(&name).map_err(|e| e.to_string())?;
        ModelCache::new(cache_dir)
            .ensure(spec)
            .map_err(|e| e.to_string())?;
        Ok(name)
    }
}

/// A clip's audio region, read from the project folder.
#[derive(Clone, Debug)]
pub struct ClipAudio {
    /// The track it is on.
    pub track: TrackId,
    /// The clip.
    pub clip: ClipId,
    /// Clip name.
    pub name: String,
    /// Clip start in ticks.
    pub start: Ticks,
    /// Clip length in ticks.
    pub length: Ticks,
    /// The pool file.
    pub source: AudioRef,
    /// Frames skipped at the file's start.
    pub offset: u64,
    /// The project folder.
    pub dir: PathBuf,
    /// Project tempo.
    pub tempo: Tempo,
}

/// Finds audio clip `clip` in `project` saved at `dir`.
pub fn clip_audio(
    project: &Project,
    dir: Option<PathBuf>,
    clip: ClipId,
) -> Result<ClipAudio, String> {
    let dir = dir.ok_or("save the project first: clip audio lives in its folder")?;
    let (track, found) = project
        .find_any_clip(clip)
        .ok_or("that clip no longer exists")?;
    let ClipContent::Audio {
        source,
        source_offset_frames,
        ..
    } = &found.content
    else {
        return Err("pick an audio clip".to_owned());
    };
    Ok(ClipAudio {
        track: track.id,
        clip,
        name: found.name.clone(),
        start: found.start,
        length: found.length,
        source: source.clone(),
        offset: *source_offset_frames,
        dir,
        tempo: project.tempo,
    })
}

impl ClipAudio {
    fn decode_region(&self) -> Result<AudioBuffer, String> {
        let path = AudioPool::new(&self.dir)
            .path_for(&self.source)
            .map_err(|e| e.to_string())?;
        let audio = decode_file(&path).map_err(|e| e.to_string())?;
        let seconds =
            self.length.get() as f64 / TICKS_PER_BEAT as f64 * self.tempo.seconds_per_beat();
        let frames = (seconds * audio.sample_rate.as_f64()).ceil() as usize;
        let start = (self.offset as usize).min(audio.frames());
        let end = start.saturating_add(frames).min(audio.frames());
        let channels = audio
            .channels
            .iter()
            .map(|channel| channel[start..end].to_vec())
            .collect();
        Ok(AudioBuffer::new(audio.sample_rate, channels))
    }
}

/// What a separation produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Separated {
    /// The clip.
    pub clip: ClipId,
    /// Name of the separator.
    pub separator: String,
    /// Whether it was the DSP fallback.
    pub fallback: bool,
    /// Commands that add one audio track per stem, as one undo step.
    pub commands: Vec<Command>,
    /// Stem names in order.
    pub stems: Vec<String>,
}

fn separator(cache: &ModelCache, prefer_model: bool) -> Result<Box<dyn Separator>, String> {
    let registry = ModelRegistry::builtin();
    let model_ready = registry
        .get(SPLEETER_2STEMS)
        .is_ok_and(|spec| cache.is_cached(spec));
    if prefer_model && model_ready {
        let model = SpleeterOnnx::load(cache, &registry).map_err(|e| e.to_string())?;
        return Ok(Box::new(model.separator()));
    }
    Ok(Box::new(HpssEstimator::separator()))
}

/// Job: separate a clip into stems with Spleeter when it is downloaded
/// (and `prefer_model`), else with the DSP fallback; write the stems into
/// the pool and plan one new audio track per stem.
pub fn separate_job(
    clip: ClipAudio,
    project: Project,
    cache_dir: PathBuf,
    prefer_model: bool,
) -> impl FnOnce(&Progress) -> Result<Separated, String> + Send + 'static {
    move |progress| {
        progress.set(0.05);
        let audio = clip.decode_region()?;
        progress.set(0.2);
        let cache = ModelCache::new(cache_dir).offline();
        let separator = separator(&cache, prefer_model)?;
        let stems = separator.separate(&audio).map_err(|e| e.to_string())?;
        progress.set(0.8);
        let pool = AudioPool::new(&clip.dir);
        let mut written = Vec::new();
        for stem in &stems.stems {
            let spec = WavSpec {
                sample_rate: stem.audio.sample_rate,
                channels: stem.audio.channel_count() as u16,
                format: SampleFormat::Float32,
            };
            let bytes = encode_wav(spec, &stem.audio.interleaved()).map_err(|e| e.to_string())?;
            let source = pool
                .import_bytes(&bytes, "wav")
                .map_err(|e| e.to_string())?;
            written.push((stem.name.clone(), source));
        }
        let index = project
            .tracks
            .iter()
            .position(|t| t.id == clip.track)
            .map(|i| i + 1);
        let commands = planned_commands(&project, |tx| {
            for (offset, (name, source)) in written.iter().enumerate() {
                let track = tx
                    .execute(CreateTrack {
                        name: format!("{} {name}", clip.name).chars().take(128).collect(),
                        kind: TrackKind::Audio,
                        index: index.map(|i| i + offset),
                    })?
                    .track()
                    .ok_or(fourfloor_model::ModelError::NothingToUndo)?;
                tx.execute(AddAudioClip {
                    track,
                    name: name.clone(),
                    start: clip.start,
                    length: clip.length,
                    source: source.clone(),
                    source_offset_frames: 0,
                })?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        Ok(Separated {
            clip: clip.clip,
            separator: stems.separator.clone(),
            fallback: stems.fallback,
            stems: written.into_iter().map(|(name, _)| name).collect(),
            commands,
        })
    }
}

/// What a transcription produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcribed {
    /// The clip.
    pub clip: ClipId,
    /// Notes found.
    pub notes: usize,
    /// Commands that add a MIDI track with the notes, as one undo step.
    pub commands: Vec<Command>,
}

/// Job: transcribe a clip to MIDI with Basic Pitch (it must be downloaded)
/// and plan a new MIDI track holding the notes at the clip's position.
pub fn transcribe_job(
    clip: ClipAudio,
    project: Project,
    cache_dir: PathBuf,
) -> impl FnOnce(&Progress) -> Result<Transcribed, String> + Send + 'static {
    move |progress| {
        progress.set(0.05);
        let registry = ModelRegistry::builtin();
        let cache = ModelCache::new(cache_dir).offline();
        let spec = registry.get(BASIC_PITCH).map_err(|e| e.to_string())?;
        if !cache.is_cached(spec) {
            return Err(
                "Basic Pitch is not downloaded yet (Settings, Models: about 230 kB)".to_owned(),
            );
        }
        let model = BasicPitch::load(&cache, &registry).map_err(|e| e.to_string())?;
        let audio = clip.decode_region()?;
        progress.set(0.3);
        let transcription = model
            .transcribe(&audio, &TranscribeOptions::default())
            .map_err(|e| e.to_string())?;
        progress.set(0.9);
        let notes = transcription.notes.len();
        let index = project
            .tracks
            .iter()
            .position(|t| t.id == clip.track)
            .map(|i| i + 1);
        let commands = planned_commands(&project, |tx| {
            let track = tx
                .execute(CreateTrack {
                    name: format!("{} MIDI", clip.name).chars().take(128).collect(),
                    kind: TrackKind::Midi,
                    index,
                })?
                .track()
                .ok_or(fourfloor_model::ModelError::NothingToUndo)?;
            tx.execute(transcription.to_clip_command(track, &clip.name, clip.start, clip.tempo))?;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        Ok(Transcribed {
            clip: clip.clip,
            notes,
            commands,
        })
    }
}
