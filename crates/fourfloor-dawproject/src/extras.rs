//! fourfloor's extension file: the parts of a project DAWproject has no
//! element for. It rides along in the archive as `fourfloor/extras.json`,
//! where other applications ignore it, so a fourfloor project comes back
//! from its own export unchanged.

use std::sync::Arc;

use fourfloor_core::{ClipId, DeviceId, Ticks};
use fourfloor_model::{
    Clip, ClipContent, ControllerMapping, DeviceInstance, Groove, Macro, Modulator, Project,
    SessionSlot, SidechainSource, Track, DEFAULT_LAUNCH_QUANTIZE,
};
use serde::{Deserialize, Serialize};

use crate::report::{Outcome, Report, Subject};

/// Version of the extension file layout.
pub(crate) const EXTRAS_FORMAT: u32 = 1;

fn default_quantize() -> Ticks {
    DEFAULT_LAUNCH_QUANTIZE
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SidechainExtra {
    pub device: DeviceId,
    pub source: SidechainSource,
}

/// What an audio clip loses in DAWproject: its gain, and how its start
/// point splits into a file offset and a content offset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ClipExtra {
    pub clip: ClipId,
    #[serde(default)]
    pub gain_db: f32,
    /// The clip's content offset, when DAWproject's single `playStart`
    /// cannot tell it from the file offset (a clip that does not loop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Ticks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_offset_frames: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Extras {
    pub format: u32,
    /// The project's id counter, so ids written into the XML keep their
    /// meaning.
    pub next_id: u64,
    /// The project name, which `metadata.xml` may not carry unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub groove: Groove,
    #[serde(default = "default_quantize")]
    pub launch_quantize: Ticks,
    #[serde(default)]
    pub modulators: Vec<Arc<Modulator>>,
    #[serde(default)]
    pub macros: Vec<Arc<Macro>>,
    #[serde(default)]
    pub controller_map: Vec<ControllerMapping>,
    #[serde(default)]
    pub sidechains: Vec<SidechainExtra>,
    #[serde(default)]
    pub clips: Vec<ClipExtra>,
}

impl Extras {
    /// Collects everything in `project` that the XML cannot say, and lists
    /// it in `report`: as preserved when the file will be written, as
    /// dropped when the caller turned the extension file off.
    pub(crate) fn collect(project: &Project, written: bool, report: &mut Report) -> Self {
        let outcome = if written {
            Outcome::Preserved
        } else {
            Outcome::Dropped
        };
        let how = if written {
            "kept in fourfloor's extension file; other applications will not see it"
        } else {
            "left out, because the extension file is turned off"
        };
        let mut note = |subject: Subject, at: String, what: &str| {
            report.push(outcome, subject, at, format!("{what}: {how}"));
        };
        if project.groove != Groove::STRAIGHT {
            note(Subject::Project, "project".to_owned(), "the swing setting");
        }
        if project.launch_quantize != DEFAULT_LAUNCH_QUANTIZE {
            note(
                Subject::Scene,
                "project".to_owned(),
                "the launch quantization",
            );
        }
        for modulator in &project.modulators {
            note(Subject::Modulation, modulator.name.clone(), "a modulator");
        }
        for control in &project.macros {
            note(Subject::Modulation, control.name.clone(), "a macro control");
        }
        if !project.controller_map.is_empty() {
            note(
                Subject::Modulation,
                "project".to_owned(),
                "the MIDI controller mappings",
            );
        }
        let mut sidechains = Vec::new();
        let mut clips = Vec::new();
        for track in &project.tracks {
            for device in &track.devices {
                if let Some(source) = device.sidechain {
                    note(
                        Subject::Device,
                        format!("{} / {}", track.name, device.name),
                        "the sidechain routing",
                    );
                    sidechains.push(SidechainExtra {
                        device: device.id,
                        source,
                    });
                }
            }
            let all = track
                .clips
                .iter()
                .chain(track.slots.iter().map(|slot| &slot.clip));
            for clip in all {
                // DAWproject wraps a play start past the loop end; the model
                // keeps the raw offset. Same sound, so not reported.
                let past_loop = clip.loop_length.is_some_and(|period| clip.offset >= period);
                let (gain_db, split, source_offset_frames) = match &clip.content {
                    ClipContent::Audio {
                        gain_db,
                        source_offset_frames,
                        ..
                    } => (
                        *gain_db,
                        clip.loop_length.is_none() && clip.offset != Ticks::ZERO,
                        *source_offset_frames,
                    ),
                    ClipContent::Midi { .. } => (0.0, false, 0),
                };
                if gain_db != 0.0 {
                    note(
                        Subject::Clip,
                        format!("{} / {}", track.name, clip.name),
                        "the clip gain",
                    );
                }
                if gain_db != 0.0 || split || past_loop {
                    clips.push(ClipExtra {
                        clip: clip.id,
                        gain_db,
                        offset: (split || past_loop).then_some(clip.offset),
                        source_offset_frames: split.then_some(source_offset_frames),
                    });
                }
            }
        }
        Self {
            format: EXTRAS_FORMAT,
            next_id: project.next_id,
            name: Some(project.name.clone()),
            groove: project.groove,
            launch_quantize: project.launch_quantize,
            modulators: project.modulators.clone(),
            macros: project.macros.clone(),
            controller_map: project.controller_map.clone(),
            sidechains,
            clips,
        }
    }

    pub(crate) fn clip(&self, id: ClipId) -> Option<&ClipExtra> {
        self.clips.iter().find(|extra| extra.clip == id)
    }

    /// `project` with everything in the extension file put back. The
    /// result still has to pass validation: the file may not match.
    pub(crate) fn apply(&self, project: &Project) -> Project {
        let tracks = project
            .tracks
            .iter()
            .map(|track| Arc::new(self.apply_track(track)))
            .collect();
        Project {
            name: self.name.clone().unwrap_or_else(|| project.name.clone()),
            groove: self.groove,
            launch_quantize: self.launch_quantize,
            modulators: self.modulators.clone(),
            macros: self.macros.clone(),
            controller_map: self.controller_map.clone(),
            next_id: project.next_id.max(self.next_id),
            tracks,
            ..project.clone()
        }
    }

    fn apply_track(&self, track: &Track) -> Track {
        let devices = track
            .devices
            .iter()
            .map(
                |device| match self.sidechains.iter().find(|e| e.device == device.id) {
                    Some(extra) => Arc::new(DeviceInstance {
                        sidechain: Some(extra.source),
                        ..(**device).clone()
                    }),
                    None => Arc::clone(device),
                },
            )
            .collect();
        let clips = track
            .clips
            .iter()
            .map(|clip| self.apply_clip(clip))
            .collect();
        let slots = track
            .slots
            .iter()
            .map(|slot| SessionSlot {
                scene: slot.scene,
                clip: self.apply_clip(&slot.clip),
            })
            .collect();
        Track {
            devices,
            clips,
            slots,
            ..track.clone()
        }
    }

    fn apply_clip(&self, clip: &Arc<Clip>) -> Arc<Clip> {
        let Some(extra) = self.clip(clip.id) else {
            return Arc::clone(clip);
        };
        let content = match &clip.content {
            ClipContent::Audio {
                source,
                source_offset_frames,
                fade_in,
                fade_out,
                warp,
                ..
            } => ClipContent::Audio {
                source: source.clone(),
                source_offset_frames: extra.source_offset_frames.unwrap_or(*source_offset_frames),
                gain_db: extra.gain_db,
                fade_in: *fade_in,
                fade_out: *fade_out,
                warp: *warp,
            },
            midi @ ClipContent::Midi { .. } => midi.clone(),
        };
        Arc::new(Clip {
            offset: extra.offset.unwrap_or(clip.offset),
            content,
            ..(**clip).clone()
        })
    }
}
