//! Tracks: audio, MIDI or bus, each with clips, a device chain and automation.

use std::fmt;
use std::sync::Arc;

use fourfloor_core::{ClipId, DeviceId, ParamId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::device::{AutomationLane, AutomationPoint, DeviceInstance};
use crate::ids::SceneId;

/// What a track carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Holds audio clips.
    Audio,
    /// Holds MIDI clips that feed an instrument device.
    Midi,
    /// Holds no clips; sums the tracks routed to it.
    Bus,
}

impl fmt::Display for TrackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Audio => "audio",
            Self::Midi => "MIDI",
            Self::Bus => "bus",
        })
    }
}

/// A send: a copy of a track's signal mixed into a bus (a "return").
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Send {
    /// The bus track that receives the send.
    #[schemars(with = "u64")]
    pub target: TrackId,
    /// Send level in decibels, -96 to 24. -96 is silence.
    pub gain_db: f32,
    /// `true` taps the signal before the track's fader (so the fader, pan
    /// and mute do not affect the send); `false` taps it after.
    #[serde(default)]
    pub pre_fader: bool,
}

/// A mixer control of a track that can be automated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MixerParam {
    /// Fader gain, in decibels.
    Gain,
    /// Pan, -1 to 1.
    Pan,
}

/// Automation for a track's fader gain or pan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerLane {
    /// The automated control.
    pub param: MixerParam,
    /// Breakpoints sorted by time.
    pub points: Vec<AutomationPoint>,
}

/// A clip in the clip launcher: one cell of the session grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSlot {
    /// The scene (row) the slot belongs to.
    pub scene: SceneId,
    /// The clip. It loops for as long as it plays; its `start` is unused.
    pub clip: Arc<Clip>,
}

/// One track of the project.
///
/// Clips, devices and lanes sit behind `Arc`, so an edit to one of them
/// shares the rest with the previous project value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Unique id.
    pub id: TrackId,
    /// Display name.
    pub name: String,
    /// Audio, MIDI or bus.
    pub kind: TrackKind,
    /// Fader gain in decibels.
    pub gain_db: f32,
    /// Pan from -1.0 (left) to 1.0 (right).
    pub pan: f32,
    /// Whether the track is muted.
    pub mute: bool,
    /// Whether the track is soloed.
    pub solo: bool,
    /// The bus this track feeds; `None` means the master output.
    pub output: Option<TrackId>,
    /// Clips on the timeline, in the order they were added.
    pub clips: Vec<Arc<Clip>>,
    /// The device chain, first to last.
    pub devices: Vec<Arc<DeviceInstance>>,
    /// Automation lanes for this track's devices.
    pub automation: Vec<Arc<AutomationLane>>,
    /// Sends to bus tracks, at most one per target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<Send>,
    /// Automation of the track's own fader gain and pan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mixer_automation: Vec<Arc<MixerLane>>,
    /// Clip-launcher slots, at most one per scene.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<SessionSlot>,
}

impl Track {
    /// An empty track at unity gain, centred, routed to the master.
    pub fn new(id: TrackId, name: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            output: None,
            clips: Vec::new(),
            devices: Vec::new(),
            automation: Vec::new(),
            sends: Vec::new(),
            mixer_automation: Vec::new(),
            slots: Vec::new(),
        }
    }

    /// Looks up a clip on this track.
    pub fn clip(&self, id: ClipId) -> Option<&Arc<Clip>> {
        self.clips.iter().find(|clip| clip.id == id)
    }

    /// Looks up a device on this track.
    pub fn device(&self, id: DeviceId) -> Option<&Arc<DeviceInstance>> {
        self.devices.iter().find(|device| device.id == id)
    }

    /// Looks up the send to one bus.
    pub fn send(&self, target: TrackId) -> Option<&Send> {
        self.sends.iter().find(|send| send.target == target)
    }

    /// Looks up the launcher slot for one scene.
    pub fn slot(&self, scene: SceneId) -> Option<&SessionSlot> {
        self.slots.iter().find(|slot| slot.scene == scene)
    }

    /// Looks up a launcher clip by its id.
    pub fn slot_clip(&self, id: ClipId) -> Option<&SessionSlot> {
        self.slots.iter().find(|slot| slot.clip.id == id)
    }

    /// Looks up the automation lane for the fader gain or pan.
    pub fn mixer_lane(&self, param: MixerParam) -> Option<&Arc<MixerLane>> {
        self.mixer_automation
            .iter()
            .find(|lane| lane.param == param)
    }

    /// Looks up the automation lane for one parameter.
    pub fn lane(&self, device: DeviceId, param: ParamId) -> Option<&Arc<AutomationLane>> {
        self.automation
            .iter()
            .find(|lane| lane.device == device && lane.param == param)
    }
}
