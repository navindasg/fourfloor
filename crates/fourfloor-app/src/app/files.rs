//! File intents and finished background jobs.

use std::path::Path;
use std::sync::Arc;

use fourfloor_core::SampleRate;
use fourfloor_model::Project;

use crate::project::files::{
    export_dawproject_job, export_stems_job, export_wav_job, import_dawproject_job,
    load_reference_job, open_project, save_project_to, waveform_job,
};
use crate::project::{build_demo_project, JobOutput};
use crate::services::jobs::Finished;
use crate::state::{Document, FileIntent, PathPurpose};
use crate::views::shell::dialogs;

use super::FourfloorApp;

/// Sample rate of exports.
pub const EXPORT_RATE: SampleRate = SampleRate::HZ_48000;

/// Loads a project folder.
pub(super) fn open_folder(dir: &Path) -> Result<Project, String> {
    open_project(dir)
}

impl FourfloorApp {
    fn replace_document(&mut self, doc: Document) {
        self.doc = doc;
        self.assets = Default::default();
        self.host.set_loop(None);
        self.host.stop();
        self.host.seek(0);
        self.ui_state.loop_on = false;
        self.select_first_track();
    }

    pub(super) fn file(&mut self, intent: FileIntent) {
        let project = self.doc.project().clone();
        let dir = self.doc.dir.clone();
        let registry = Arc::clone(&self.registry);
        match intent {
            FileIntent::New => {
                self.replace_document(Document::new(Project::new("Untitled")));
                self.ui_state.info("New project");
            }
            FileIntent::Demo => match build_demo_project(&self.registry) {
                Ok(demo) => {
                    self.replace_document(Document::new(demo));
                    self.ui_state.info("Demo loaded");
                }
                Err(error) => self.ui_state.error(error.to_string()),
            },
            FileIntent::Open(dir) => match open_project(&dir) {
                Ok(project) => {
                    self.replace_document(Document::opened(project, dir.clone()));
                    self.ui_state.info(format!("Opened {}", dir.display()));
                }
                Err(error) => self
                    .ui_state
                    .error(format!("open {}: {error}", dir.display())),
            },
            FileIntent::Save => match dir {
                Some(dir) => self.save_to(&dir),
                None => dialogs::ask(&mut self.ui_state, &mut self.intents, PathPurpose::SaveAs),
            },
            FileIntent::SaveAs(dir) => self.save_to(&dir),
            FileIntent::ExportWav(path) => {
                self.jobs.spawn(
                    "Exporting WAV",
                    export_wav_job(project, dir, registry, path, EXPORT_RATE),
                );
            }
            FileIntent::ExportStems(out) => {
                self.jobs.spawn(
                    "Exporting stems",
                    export_stems_job(project, dir, registry, out, EXPORT_RATE),
                );
            }
            FileIntent::ImportDawproject { file, dir } => {
                self.jobs
                    .spawn("Importing DAWproject", import_dawproject_job(file, dir));
            }
            FileIntent::ExportDawproject(file) => {
                self.jobs.spawn(
                    "Exporting DAWproject",
                    export_dawproject_job(project, dir, registry, file),
                );
            }
            FileIntent::ImportAudio(file) => {
                let track = self.ui_state.selection.track.filter(|id| {
                    self.doc
                        .project()
                        .track(*id)
                        .is_some_and(|t| t.kind == fourfloor_model::TrackKind::Audio)
                });
                match track {
                    Some(track) => {
                        let at = self
                            .ui_state
                            .snap
                            .floor(self.host.readout().position.max(0.0) as i64);
                        self.push_intent(crate::state::Intent::ImportAudio {
                            track,
                            path: file,
                            at,
                        });
                    }
                    None => self.ui_state.error("Select an audio track to import onto"),
                }
            }
            FileIntent::LoadReference(file) => {
                let rate = self.host.sample_rate();
                self.jobs.spawn(
                    "Loading reference",
                    load_reference_job(file, project, dir, registry, rate),
                );
            }
        }
    }

    fn save_to(&mut self, dir: &Path) {
        match save_project_to(dir, self.doc.project()) {
            Ok(()) => {
                let moved = self.doc.dir.as_deref() != Some(dir);
                self.doc.dir = Some(dir.to_path_buf());
                self.doc.mark_saved();
                if moved {
                    // Audio and plugin state resolve against the new folder.
                    self.doc.invalidate_graph();
                }
                self.ui_state.info(format!("Saved to {}", dir.display()));
            }
            Err(error) => self
                .ui_state
                .error(format!("save {}: {error}", dir.display())),
        }
    }

    pub(super) fn request_waveform(&mut self, source: fourfloor_model::AudioRef) {
        let key = source.file_name();
        if !self.assets.requested.insert(key) {
            return;
        }
        if let Some(dir) = self.doc.dir.clone() {
            self.jobs
                .spawn("Reading waveform", waveform_job(dir, source));
        }
    }

    pub(super) fn job_finished(&mut self, done: Finished<JobOutput>) {
        let output = match done.result {
            Ok(output) => output,
            Err(error) => {
                self.library_job_failed(&done.label);
                self.settings_job_failed(&done.label);
                self.assistant_job_failed(&done.label, &error);
                self.ui_state
                    .error(format!("{} failed: {error}", done.label));
                return;
            }
        };
        match output {
            JobOutput::Exported {
                path,
                lufs,
                true_peak_dbtp,
            } => self.ui_state.info(format!(
                "Exported {} ({lufs:.1} LUFS, {true_peak_dbtp:.1} dBTP)",
                path.display()
            )),
            JobOutput::Stems { dir, files } => {
                self.ui_state
                    .info(format!("Wrote {files} files to {}", dir.display()));
            }
            JobOutput::Imported {
                project,
                dir,
                notes,
            } => {
                self.replace_document(Document::opened(*project, dir.clone()));
                self.ui_state.info(format!(
                    "Imported into {} ({} notes in the report)",
                    dir.display(),
                    notes.len()
                ));
            }
            JobOutput::DawprojectExported { path, notes } => {
                self.ui_state.info(format!(
                    "Exported {} ({} notes in the report)",
                    path.display(),
                    notes.len()
                ));
            }
            JobOutput::Waveform { key, overview } => {
                self.assets.waveforms.insert(key, overview);
            }
            JobOutput::Reference(loaded) => {
                self.host.set_reference(Some(Arc::clone(&loaded.audio)));
                self.ui_state.reference = crate::project::files::reference_ui(&loaded);
                self.ui_state
                    .info(format!("Reference loaded: {}", loaded.path.display()));
                self.reference = Some(*loaded);
                self.update_monitor();
            }
            JobOutput::DeckLoaded(loaded) => self.deck_loaded(*loaded),
            JobOutput::LibraryScanned(summary) => self.library_scanned(summary),
            JobOutput::Audition { file, audio } => self.audition_ready(file, audio),
            JobOutput::TextSearch { query, rows } => self.text_found(query, rows),
            JobOutput::Embedded(summary) => self.embedded(&summary),
            JobOutput::ModelDownloaded(name) => self.model_downloaded(&name),
            JobOutput::Plugins(list) => self.plugins_scanned(list),
            JobOutput::Agent(run) => self.agent_finished(*run),
            JobOutput::Separated(separated) => self.separated(*separated),
            JobOutput::Transcribed(transcribed) => self.transcribed(*transcribed),
            JobOutput::MixReport(report) => self.mix_reported(*report),
        }
    }
}
