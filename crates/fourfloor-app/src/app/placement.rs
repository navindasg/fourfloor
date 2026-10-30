//! Putting library files on tracks and into samplers.

use fourfloor_core::{DeviceCategory, DeviceId, TrackId, TICKS_PER_BEAT};
use fourfloor_library::{FileId, FileRecord};
use fourfloor_model::command::{AddAudioClip, AddDevice, CreateTrack, SetDeviceState};
use fourfloor_model::{
    AudioPool, Command, ContentHash, ModelError, PluginStateStore, Project, TrackKind,
};

use crate::services::device_params;
use crate::state::Intent;
use crate::viewmodel::planned_commands;

use super::FourfloorApp;

/// Largest file loaded into a sampler, in bytes.
pub const MAX_SAMPLER_BYTES: u64 = 256 * 1024 * 1024;

/// The sampler's registry kind.
pub const SAMPLER: &str = fourfloor_instruments::SAMPLER_KIND;

/// Where a sample loaded into a sampler goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SamplerTarget {
    /// The selected track's sampler.
    Existing(DeviceId),
    /// A new sampler on the selected MIDI track, which has no instrument.
    Add(TrackId),
    /// A new MIDI track with a sampler.
    NewTrack,
}

/// Clip length in ticks of `seconds` at `bpm`.
pub fn seconds_to_ticks(seconds: f64, bpm: f64) -> i64 {
    (seconds * bpm / 60.0 * TICKS_PER_BEAT as f64)
        .round()
        .max(1.0) as i64
}

/// Index just after `track` in `project`, for new tracks.
fn after(project: &Project, track: Option<TrackId>) -> Option<usize> {
    let track = track?;
    project
        .tracks
        .iter()
        .position(|t| t.id == track)
        .map(|i| i + 1)
}

/// A track name from a file title (the model caps names at 128).
fn track_name(record: &FileRecord) -> String {
    record.title.chars().take(128).collect()
}

/// Commands that create a track of `kind` and then run `then` on it.
fn new_track_plan(
    project: &Project,
    name: String,
    kind: TrackKind,
    index: Option<usize>,
    then: impl FnOnce(TrackId) -> Vec<Command>,
) -> Result<Vec<Command>, ModelError> {
    planned_commands(project, |tx| {
        let track = tx
            .execute(CreateTrack { name, kind, index })?
            .track()
            .ok_or(ModelError::NothingToUndo)?;
        for command in then(track) {
            tx.execute(command)?;
        }
        Ok(())
    })
}

impl FourfloorApp {
    pub(super) fn add_file_to_track(&mut self, file: FileId) {
        let Some(record) = self.features.library.record(file) else {
            self.ui_state.error("that file is no longer in the library");
            return;
        };
        let Some(dir) = self.doc.dir.clone() else {
            self.ui_state
                .error("Save the project first: audio is copied into its folder");
            return;
        };
        let project = self.doc.project().clone();
        let selected = self.ui_state.selection.track;
        let at = self
            .ui_state
            .snap
            .floor(self.host.readout().position.max(0.0) as i64);
        if let Some(track) = selected.filter(|id| {
            project
                .track(*id)
                .is_some_and(|t| t.kind == TrackKind::Audio)
        }) {
            self.push_intent(Intent::ImportAudio {
                track,
                path: record.path,
                at,
            });
            return;
        }
        let source = match AudioPool::new(&dir).import_file(&record.path) {
            Ok(source) => source,
            Err(error) => {
                self.ui_state.error(format!("import failed: {error}"));
                return;
            }
        };
        let length = seconds_to_ticks(record.duration_seconds, project.tempo.bpm());
        let name = track_name(&record);
        let clip_name = name.clone();
        let plan = new_track_plan(
            &project,
            name,
            TrackKind::Audio,
            after(&project, selected),
            |track| {
                vec![Command::AddAudioClip(AddAudioClip {
                    track,
                    name: clip_name,
                    start: fourfloor_core::Ticks::new(at.max(0)),
                    length: fourfloor_core::Ticks::new(length),
                    source,
                    source_offset_frames: 0,
                })]
            },
        );
        self.run_plan(&format!("Add {}", record.title), plan);
    }

    pub(super) fn load_into_sampler(&mut self, file: FileId) {
        let Some(record) = self.features.library.record(file) else {
            self.ui_state.error("that file is no longer in the library");
            return;
        };
        let Some(dir) = self.doc.dir.clone() else {
            self.ui_state
                .error("Save the project first: sampler audio is stored in its folder");
            return;
        };
        if record.size_bytes > MAX_SAMPLER_BYTES {
            self.ui_state
                .error("That file is too large for the sampler");
            return;
        }
        let hash = match std::fs::read(&record.path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                PluginStateStore::new(&dir)
                    .put(&bytes)
                    .map_err(|e| e.to_string())
            }) {
            Ok(hash) => hash,
            Err(error) => {
                self.ui_state.error(format!("sampler: {error}"));
                return;
            }
        };
        let project = self.doc.project().clone();
        let plan = match self.sampler_target(&project) {
            SamplerTarget::Existing(device) => Ok(vec![Command::SetDeviceState(SetDeviceState {
                device,
                state_blob: Some(hash),
            })]),
            SamplerTarget::Add(track) => self.add_sampler_plan(&project, track, hash),
            SamplerTarget::NewTrack => {
                let index = after(&project, self.ui_state.selection.track);
                let name = track_name(&record);
                self.new_sampler_track_plan(&project, name, index, hash)
            }
        };
        self.run_plan(&format!("Load {} into a sampler", record.title), plan);
    }

    fn run_plan(&mut self, label: &str, plan: Result<Vec<Command>, ModelError>) {
        match plan {
            Ok(commands) => {
                let before: Vec<TrackId> = self.doc.project().tracks.iter().map(|t| t.id).collect();
                self.push_intent(Intent::Transaction {
                    label: label.to_owned(),
                    commands,
                });
                self.flush();
                let created = self
                    .doc
                    .project()
                    .tracks
                    .iter()
                    .map(|t| t.id)
                    .find(|id| !before.contains(id));
                if let Some(track) = created {
                    self.ui_state.selection.select_track(track);
                }
            }
            Err(error) => self.ui_state.error(format!("{label}: {error}")),
        }
    }

    fn sampler_target(&self, project: &Project) -> SamplerTarget {
        let Some(track) = self
            .ui_state
            .selection
            .track
            .and_then(|id| project.track(id))
            .filter(|t| t.kind == TrackKind::Midi)
        else {
            return SamplerTarget::NewTrack;
        };
        let selected = self.ui_state.selection.device;
        let samplers: Vec<_> = track
            .devices
            .iter()
            .filter(|d| d.kind == SAMPLER)
            .map(|d| d.id)
            .collect();
        if let Some(device) = samplers
            .iter()
            .find(|id| Some(**id) == selected)
            .or(samplers.first())
        {
            return SamplerTarget::Existing(*device);
        }
        let has_instrument = track.devices.iter().any(|d| {
            self.registry
                .descriptor(&d.kind)
                .is_some_and(|desc| desc.category == DeviceCategory::Instrument)
        });
        if has_instrument {
            SamplerTarget::NewTrack
        } else {
            SamplerTarget::Add(track.id)
        }
    }

    fn sampler_device(&self, track: TrackId) -> Result<AddDevice, ModelError> {
        let params =
            device_params(&self.registry, SAMPLER, &[]).map_err(|e| ModelError::NotFound {
                what: e.to_string(),
            })?;
        Ok(AddDevice {
            track,
            kind: SAMPLER.to_owned(),
            name: Some("Sampler".to_owned()),
            index: Some(0),
            params,
        })
    }

    fn add_sampler_plan(
        &self,
        project: &Project,
        track: TrackId,
        hash: ContentHash,
    ) -> Result<Vec<Command>, ModelError> {
        let add = self.sampler_device(track)?;
        planned_commands(project, |tx| {
            let device = tx.execute(add)?.device().ok_or(ModelError::NothingToUndo)?;
            tx.execute(SetDeviceState {
                device,
                state_blob: Some(hash),
            })?;
            Ok(())
        })
    }

    fn new_sampler_track_plan(
        &self,
        project: &Project,
        name: String,
        index: Option<usize>,
        hash: ContentHash,
    ) -> Result<Vec<Command>, ModelError> {
        let template = self.sampler_device(TrackId::new(0))?;
        planned_commands(project, |tx| {
            let track = tx
                .execute(CreateTrack {
                    name,
                    kind: TrackKind::Midi,
                    index,
                })?
                .track()
                .ok_or(ModelError::NothingToUndo)?;
            let device = tx
                .execute(AddDevice { track, ..template })?
                .device()
                .ok_or(ModelError::NothingToUndo)?;
            tx.execute(SetDeviceState {
                device,
                state_blob: Some(hash),
            })?;
            Ok(())
        })
    }
}
