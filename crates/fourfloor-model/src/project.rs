//! The project: the root value every command reads and replaces.

use std::sync::Arc;

use fourfloor_core::{ClipId, DeviceId, Tempo, Ticks, TrackId, TICKS_PER_BEAT};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::{AudioRef, Clip};
use crate::device::DeviceInstance;
use crate::error::ModelError;
use crate::ids::{MacroId, ModulatorId, SceneId};
use crate::modulation::{ControllerMapping, Macro, Modulator, Scene};
use crate::track::{SessionSlot, Track};
use crate::validate;

/// Which pair of steps swing is applied to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SwingGrid {
    /// Delays every second eighth note (the offbeat).
    Eighth,
    /// Delays every second sixteenth note (the classic drum-machine shuffle).
    #[default]
    Sixteenth,
}

/// The project's groove setting.
///
/// Swing is baked into notes when they are written (see
/// [`crate::sequencer`]); the engine plays notes exactly where they are
/// stored and does not apply this again.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Groove {
    /// Where the off step falls within its pair, as a percentage of the
    /// pair's length. 50 is straight, about 66.7 is triplet feel; the
    /// allowed range is 50 to 75.
    pub swing_percent: f64,
    /// The step size swing applies to.
    pub grid: SwingGrid,
}

impl Groove {
    /// No swing, on the sixteenth grid.
    pub const STRAIGHT: Self = Self {
        swing_percent: 50.0,
        grid: SwingGrid::Sixteenth,
    };
}

impl Default for Groove {
    fn default() -> Self {
        Self::STRAIGHT
    }
}

/// A whole project.
///
/// Projects are values: a command takes `&Project` and returns a new one.
/// Tracks sit behind `Arc`, so the new value shares every track the command
/// did not touch, which is what makes snapshot undo cheap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Display name.
    pub name: String,
    /// The single project tempo (there is no tempo map yet).
    pub tempo: Tempo,
    /// Swing setting used when patterns are written.
    pub groove: Groove,
    /// Tracks in display order.
    pub tracks: Vec<Arc<Track>>,
    /// The next id to hand out. Tracks, clips, devices, scenes, macros and
    /// modulators share one counter, so no two objects in a project have
    /// the same number.
    pub next_id: u64,
    /// Clip-launcher scenes (rows), top to bottom.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<Scene>,
    /// The grid a launched clip waits for, in ticks: it starts on the next
    /// multiple. Zero launches at once. One 4/4 bar by default.
    #[serde(
        default = "default_launch_quantize",
        skip_serializing_if = "is_default_launch_quantize"
    )]
    pub launch_quantize: Ticks,
    /// LFOs and envelopes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modulators: Vec<Arc<Modulator>>,
    /// Macro controls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<Arc<Macro>>,
    /// MIDI controllers bound to parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controller_map: Vec<ControllerMapping>,
}

/// Launch quantization of a new project: one 4/4 bar.
pub const DEFAULT_LAUNCH_QUANTIZE: Ticks = Ticks::new(TICKS_PER_BEAT * 4);

fn default_launch_quantize() -> Ticks {
    DEFAULT_LAUNCH_QUANTIZE
}

fn is_default_launch_quantize(value: &Ticks) -> bool {
    *value == DEFAULT_LAUNCH_QUANTIZE
}

impl Default for Project {
    fn default() -> Self {
        Self::new("Untitled")
    }
}

impl Project {
    /// An empty project at 120 BPM with no swing.
    ///
    /// The name is not validated here; [`Project::validate`] reports a bad one.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tempo: Tempo::DEFAULT,
            groove: Groove::STRAIGHT,
            tracks: Vec::new(),
            next_id: 1,
            scenes: Vec::new(),
            launch_quantize: DEFAULT_LAUNCH_QUANTIZE,
            modulators: Vec::new(),
            macros: Vec::new(),
            controller_map: Vec::new(),
        }
    }

    /// Looks up a track.
    pub fn track(&self, id: TrackId) -> Option<&Arc<Track>> {
        self.tracks.iter().find(|track| track.id == id)
    }

    /// Position of a track in display order.
    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|track| track.id == id)
    }

    /// Finds a clip and the track it is on.
    pub fn find_clip(&self, id: ClipId) -> Option<(&Arc<Track>, &Arc<Clip>)> {
        self.tracks
            .iter()
            .find_map(|track| track.clip(id).map(|clip| (track, clip)))
    }

    /// Finds a device and the track it is on.
    pub fn find_device(&self, id: DeviceId) -> Option<(&Arc<Track>, &Arc<DeviceInstance>)> {
        self.tracks
            .iter()
            .find_map(|track| track.device(id).map(|device| (track, device)))
    }

    /// Finds a clip-launcher clip and the track it is on.
    pub fn find_slot_clip(&self, id: ClipId) -> Option<(&Arc<Track>, &SessionSlot)> {
        self.tracks
            .iter()
            .find_map(|track| track.slot_clip(id).map(|slot| (track, slot)))
    }

    /// Finds a clip on the timeline or in the launcher.
    pub fn find_any_clip(&self, id: ClipId) -> Option<(&Arc<Track>, &Arc<Clip>)> {
        self.find_clip(id).or_else(|| {
            self.find_slot_clip(id)
                .map(|(track, slot)| (track, &slot.clip))
        })
    }

    /// Looks up a scene.
    pub fn scene(&self, id: SceneId) -> Option<&Scene> {
        self.scenes.iter().find(|scene| scene.id == id)
    }

    /// Position of a scene in the launcher, top to bottom.
    pub fn scene_index(&self, id: SceneId) -> Option<usize> {
        self.scenes.iter().position(|scene| scene.id == id)
    }

    /// Looks up a modulator.
    pub fn modulator(&self, id: ModulatorId) -> Option<&Arc<Modulator>> {
        self.modulators.iter().find(|modulator| modulator.id == id)
    }

    /// Looks up a macro control.
    pub fn macro_control(&self, id: MacroId) -> Option<&Arc<Macro>> {
        self.macros.iter().find(|control| control.id == id)
    }

    /// The controller mapping that answers to a controller message.
    pub fn controller_mapping(&self, channel: u8, controller: u8) -> Option<&ControllerMapping> {
        self.controller_map
            .iter()
            .find(|mapping| mapping.matches(channel, controller))
    }

    /// Every audio pool file the project's clips refer to, on the timeline
    /// and in the launcher, without duplicates.
    pub fn audio_refs(&self) -> Vec<AudioRef> {
        let mut refs: Vec<AudioRef> = self
            .tracks
            .iter()
            .flat_map(|track| {
                track
                    .clips
                    .iter()
                    .chain(track.slots.iter().map(|slot| &slot.clip))
            })
            .filter_map(|clip| clip.audio_ref().cloned())
            .collect();
        refs.sort_by(|a, b| (&a.hash, &a.extension).cmp(&(&b.hash, &b.extension)));
        refs.dedup();
        refs
    }

    /// Checks every invariant the commands maintain.
    ///
    /// Projects built through the command bus always pass. Loading calls
    /// this because a file on disk may have been edited by hand.
    pub fn validate(&self) -> Result<(), ModelError> {
        validate::project(self)
    }

    /// A copy with a different track list.
    pub(crate) fn with_tracks(&self, tracks: Vec<Arc<Track>>) -> Self {
        Self {
            tracks,
            ..self.clone()
        }
    }

    /// A copy in which one track is replaced by `update(track)`.
    pub(crate) fn map_track(
        &self,
        id: TrackId,
        update: impl FnOnce(&Track) -> Result<Track, ModelError>,
    ) -> Result<Self, ModelError> {
        let index = self.track_index(id).ok_or(ModelError::TrackNotFound(id))?;
        let updated = Arc::new(update(&self.tracks[index])?);
        let tracks = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| Arc::clone(if i == index { &updated } else { track }))
            .collect();
        Ok(self.with_tracks(tracks))
    }

    /// A copy in which one device is replaced by `update(device)`.
    pub(crate) fn map_device(
        &self,
        id: DeviceId,
        update: impl Fn(&DeviceInstance) -> Result<DeviceInstance, ModelError>,
    ) -> Result<Self, ModelError> {
        let (owner, _) = self.find_device(id).ok_or(ModelError::DeviceNotFound(id))?;
        self.map_track(owner.id, |track| {
            let devices = track
                .devices
                .iter()
                .map(|device| {
                    if device.id == id {
                        update(device).map(Arc::new)
                    } else {
                        Ok(Arc::clone(device))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Track {
                devices,
                ..track.clone()
            })
        })
    }

    /// A copy in which one clip, on the timeline or in the launcher, is
    /// replaced by `update(clip)`.
    pub(crate) fn map_any_clip(
        &self,
        id: ClipId,
        update: impl Fn(&Clip) -> Result<Clip, ModelError>,
    ) -> Result<Self, ModelError> {
        let (owner, _) = self.find_any_clip(id).ok_or(ModelError::ClipNotFound(id))?;
        self.map_track(owner.id, |track| {
            let replace = |clip: &Arc<Clip>| {
                if clip.id == id {
                    update(clip).map(Arc::new)
                } else {
                    Ok(Arc::clone(clip))
                }
            };
            let clips = track.clips.iter().map(replace).collect::<Result<_, _>>()?;
            let slots = track
                .slots
                .iter()
                .map(|slot| {
                    replace(&slot.clip).map(|clip| SessionSlot {
                        scene: slot.scene,
                        clip,
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(Track {
                clips,
                slots,
                ..track.clone()
            })
        })
    }

    /// The id the next new object gets, and the counter value after it.
    pub(crate) fn reserve_id(&self) -> Result<(u64, u64), ModelError> {
        let next = self.next_id.checked_add(1).ok_or(ModelError::IdExhausted)?;
        Ok((self.next_id, next))
    }
}
