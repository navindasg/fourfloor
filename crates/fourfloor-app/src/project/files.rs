//! File work the app does: open and save project folders, export audio,
//! import and export DAWproject, load a reference track, build waveform
//! overviews. The slow ones are written as job closures for
//! [`crate::services::jobs::Jobs`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fourfloor_analysis::{
    compare_to_reference, ReferenceReport, WaveformOptions, WaveformOverview,
};
use fourfloor_audio::{
    decode_file, decode_file_at_rate, measure, write_wav_file, AudioBuffer, WavSpec,
};
use fourfloor_compile::{render_project, render_stems, RenderOptions, SampleFormat};
use fourfloor_core::{DeviceRegistry, SampleRate};
use fourfloor_dawproject::{export_to_file, import_file, ExportOptions, ImportOptions, Report};
use fourfloor_model::{load_project, save_project, AudioPool, AudioRef, Project};

use crate::services::callback::ReferenceAudio;
use crate::services::jobs::Progress;

/// Seconds of tail rendered after the last clip on export.
pub const EXPORT_TAIL_SECONDS: f64 = 1.0;
/// Longest export, in seconds, as a guard against runaway renders.
pub const MAX_EXPORT_SECONDS: u64 = 60 * 30;

/// What a finished background job hands back to the app.
#[derive(Debug)]
pub enum JobOutput {
    /// A mixdown was written.
    Exported {
        /// The WAV file.
        path: PathBuf,
        /// Its integrated loudness.
        lufs: f64,
        /// Its true peak.
        true_peak_dbtp: f64,
    },
    /// Stems were written.
    Stems {
        /// The folder.
        dir: PathBuf,
        /// How many files.
        files: usize,
    },
    /// A DAWproject file was imported into a new project folder.
    Imported {
        /// The new project.
        project: Box<Project>,
        /// Its folder.
        dir: PathBuf,
        /// What did not come across exactly.
        notes: Vec<String>,
    },
    /// A DAWproject file was written.
    DawprojectExported {
        /// The file.
        path: PathBuf,
        /// What did not go across exactly.
        notes: Vec<String>,
    },
    /// A reference track is ready for A/B.
    Reference(Box<ReferenceLoaded>),
    /// A waveform overview of a pool file.
    Waveform {
        /// The pool file's name.
        key: String,
        /// The overview.
        overview: Arc<WaveformOverview>,
    },
    /// A DJ deck's track is decoded and analysed.
    DeckLoaded(Box<crate::services::dj::DeckLoaded>),
    /// A library scan (or factory install) finished.
    LibraryScanned(crate::services::library::ScanSummary),
    /// An audition is rendered.
    Audition {
        /// The library file.
        file: fourfloor_library::FileId,
        /// Stereo audio at the engine rate.
        audio: Arc<ReferenceAudio>,
    },
    /// A text search through audio-text embeddings finished.
    TextSearch {
        /// The description searched for.
        query: String,
        /// Matches, best first.
        rows: Vec<crate::services::library::ResultRow>,
    },
    /// Audio-text embeddings were computed for library files.
    Embedded(crate::services::embed::EmbedSummary),
    /// The assistant finished a request in its sandbox.
    Agent(Box<crate::services::assistant::AgentRun>),
    /// A clip was separated into stems.
    Separated(Box<crate::services::ml::Separated>),
    /// A clip was transcribed to MIDI.
    Transcribed(Box<crate::services::ml::Transcribed>),
    /// The AI panel's mix report is ready.
    MixReport(Box<crate::services::mix_report::MixReport>),
    /// A model was downloaded and verified.
    ModelDownloaded(String),
    /// The plugin scan finished.
    Plugins(crate::services::plugins::PluginList),
}

/// A decoded, measured reference track.
#[derive(Debug)]
pub struct ReferenceLoaded {
    /// The file it came from.
    pub path: PathBuf,
    /// Stereo audio at the engine rate.
    pub audio: Arc<ReferenceAudio>,
    /// Integrated loudness of the reference.
    pub reference_lufs: f64,
    /// Integrated loudness of an offline render of the project.
    pub mix_lufs: f64,
    /// The full comparison, when both could be measured.
    pub report: Option<ReferenceReport>,
}

impl ReferenceLoaded {
    /// Gain in dB that brings the reference to the mix's loudness.
    pub fn match_gain_db(&self) -> f64 {
        if self.reference_lufs.is_finite() && self.mix_lufs.is_finite() {
            (self.mix_lufs - self.reference_lufs).clamp(-40.0, 40.0)
        } else {
            0.0
        }
    }
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Loads a project folder.
pub fn open_project(dir: &Path) -> Result<Project, String> {
    load_project(dir).map_err(text)
}

/// Saves a project folder (creating it if needed).
pub fn save_project_to(dir: &Path, project: &Project) -> Result<(), String> {
    save_project(dir, project).map_err(text)
}

fn render_options(rate: SampleRate, dir: Option<&Path>) -> RenderOptions {
    let mut options = RenderOptions::new(rate).with_tail(EXPORT_TAIL_SECONDS);
    options.max_frames = Some(u64::from(rate.hz()) * MAX_EXPORT_SECONDS);
    if let Some(dir) = dir {
        options.compile = options.compile.with_project_dir(dir);
    }
    options
}

/// Renders `project` offline to a stereo buffer.
pub fn render_mix(
    project: &Project,
    registry: &DeviceRegistry,
    dir: Option<&Path>,
    rate: SampleRate,
) -> Result<AudioBuffer, String> {
    let rendered = render_project(project, registry, &render_options(rate, dir)).map_err(text)?;
    Ok(AudioBuffer::from_interleaved(
        rate,
        rendered.channels,
        &rendered.samples,
    ))
}

/// Job: render the mixdown to a float WAV file.
pub fn export_wav_job(
    project: Project,
    dir: Option<PathBuf>,
    registry: Arc<DeviceRegistry>,
    path: PathBuf,
    rate: SampleRate,
) -> impl FnOnce(&Progress) -> Result<JobOutput, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        let mix = render_mix(&project, &registry, dir.as_deref(), rate)?;
        progress.set(0.8);
        write_wav_file(&path, WavSpec::float_stereo(rate), &mix.interleaved()).map_err(text)?;
        let planes = mix.planes();
        let loudness = measure(&planes, rate).map_err(text)?;
        Ok(JobOutput::Exported {
            path,
            lufs: loudness.integrated_lufs,
            true_peak_dbtp: loudness.true_peak_dbtp,
        })
    }
}

/// Job: render stems and the master into `out_dir`.
pub fn export_stems_job(
    project: Project,
    dir: Option<PathBuf>,
    registry: Arc<DeviceRegistry>,
    out_dir: PathBuf,
    rate: SampleRate,
) -> impl FnOnce(&Progress) -> Result<JobOutput, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        let stems = render_stems(&project, &registry, &render_options(rate, dir.as_deref()))
            .map_err(text)?;
        progress.set(0.8);
        std::fs::create_dir_all(&out_dir).map_err(text)?;
        let files = stems
            .write_wav(&out_dir, SampleFormat::Float32)
            .map_err(text)?;
        Ok(JobOutput::Stems {
            dir: out_dir,
            files: files.len(),
        })
    }
}

fn report_notes(report: &Report) -> Vec<String> {
    report
        .items
        .iter()
        .map(|item| {
            format!(
                "{:?} {:?} {}: {}",
                item.outcome, item.subject, item.location, item.message
            )
        })
        .collect()
}

/// Job: import a `.dawproject` file into a new project folder `dir`.
pub fn import_dawproject_job(
    file: PathBuf,
    dir: PathBuf,
) -> impl FnOnce(&Progress) -> Result<JobOutput, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        std::fs::create_dir_all(&dir).map_err(text)?;
        let imported = import_file(&file, &dir, &ImportOptions::new()).map_err(text)?;
        progress.set(0.8);
        save_project(&dir, &imported.project).map_err(text)?;
        Ok(JobOutput::Imported {
            project: Box::new(imported.project),
            dir,
            notes: report_notes(&imported.report),
        })
    }
}

/// Job: export the project to a `.dawproject` file.
pub fn export_dawproject_job(
    project: Project,
    dir: Option<PathBuf>,
    registry: Arc<DeviceRegistry>,
    file: PathBuf,
) -> impl FnOnce(&Progress) -> Result<JobOutput, String> + Send + 'static {
    move |progress| {
        progress.set(0.2);
        let options = ExportOptions::new().with_registry(&registry);
        let report = export_to_file(&project, dir.as_deref(), &file, &options).map_err(text)?;
        Ok(JobOutput::DawprojectExported {
            path: file,
            notes: report_notes(&report),
        })
    }
}

/// Job: decode and measure a reference track, render the project to
/// compare against it.
pub fn load_reference_job(
    file: PathBuf,
    project: Project,
    dir: Option<PathBuf>,
    registry: Arc<DeviceRegistry>,
    rate: SampleRate,
) -> impl FnOnce(&Progress) -> Result<JobOutput, String> + Send + 'static {
    move |progress| {
        progress.set(0.05);
        let reference = decode_file_at_rate(&file, rate).map_err(text)?.to_stereo();
        progress.set(0.3);
        let reference_lufs = measure(&reference.planes(), rate)
            .map(|l| l.integrated_lufs)
            .unwrap_or(f64::NEG_INFINITY);
        let mix = render_mix(&project, &registry, dir.as_deref(), rate);
        progress.set(0.8);
        let (mix_lufs, report) = match &mix {
            Ok(mix) => (
                measure(&mix.planes(), rate)
                    .map(|l| l.integrated_lufs)
                    .unwrap_or(f64::NEG_INFINITY),
                compare_to_reference(mix, &reference).ok(),
            ),
            Err(_) => (f64::NEG_INFINITY, None),
        };
        Ok(JobOutput::Reference(Box::new(ReferenceLoaded {
            path: file,
            audio: Arc::new(ReferenceAudio {
                samples: reference.interleaved(),
            }),
            reference_lufs,
            mix_lufs,
            report,
        })))
    }
}

/// Job: a waveform overview of an audio pool file.
pub fn waveform_job(
    dir: PathBuf,
    source: AudioRef,
) -> impl FnOnce(&Progress) -> Result<JobOutput, String> + Send + 'static {
    move |_| {
        let path = AudioPool::new(&dir).path_for(&source).map_err(text)?;
        let audio = decode_file(&path).map_err(text)?;
        let mono = audio.to_mono();
        let overview =
            WaveformOverview::build(&mono, audio.sample_rate, &WaveformOptions::default())
                .map_err(text)?;
        Ok(JobOutput::Waveform {
            key: source.file_name(),
            overview: Arc::new(overview),
        })
    }
}

/// The master panel's summary of a loaded reference.
pub fn reference_ui(loaded: &ReferenceLoaded) -> crate::state::ui::ReferenceUi {
    let mut notes = Vec::new();
    if let Some(report) = &loaded.report {
        for band in [&report.low, &report.mid, &report.high] {
            notes.push(format!(
                "{:.0}-{:.0} Hz: mix {:+.1} dB vs reference",
                band.low_hz, band.high_hz, band.delta_db
            ));
        }
        notes.push(format!(
            "Width {:+.2}, crest {:+.1} dB",
            report.width_delta, report.crest_delta_db
        ));
        notes.extend(report.suggestions.iter().map(|s| s.message.clone()));
    }
    crate::state::ui::ReferenceUi {
        name: loaded
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        monitoring: false,
        matched: true,
        reference_lufs: loaded.reference_lufs,
        mix_lufs: loaded.mix_lufs,
        notes,
    }
}
