//! What goes into a compile and what comes out of it.

use std::path::PathBuf;

use fourfloor_core::{AudioMessage, DeviceId, SampleRate, Ticks, TrackId};
use fourfloor_engine::devices::{ClipLauncher, KIND_PLACEHOLDER};
use fourfloor_engine::{CompiledGraph, GraphError};
use fourfloor_model::SceneId;

/// Channels every compiled project graph carries: stereo.
pub const CHANNELS: usize = 2;

/// The registry kind a MIDI track with no instrument of its own plays
/// through, unless [`CompileOptions::default_instrument`] says otherwise.
pub const DEFAULT_INSTRUMENT: &str = KIND_PLACEHOLDER;

/// How to compile a project.
#[derive(Clone, Debug, PartialEq)]
pub struct CompileOptions {
    /// Sample rate the graph runs at.
    pub sample_rate: SampleRate,
    /// Largest block the graph will be asked to process.
    pub max_block: usize,
    /// Registry kind of the instrument given to a MIDI track that has none
    /// in its chain. `None` leaves such a track silent (with a warning).
    pub default_instrument: Option<String>,
    /// The project folder, used to load audio clips and device state
    /// blobs. Without it audio clips are left out and a device that has
    /// saved state is built from its parameters alone (with warnings).
    pub project_dir: Option<PathBuf>,
}

impl CompileOptions {
    /// Options with the default instrument and no project folder.
    pub fn new(sample_rate: SampleRate, max_block: usize) -> Self {
        Self {
            sample_rate,
            max_block,
            default_instrument: Some(DEFAULT_INSTRUMENT.to_owned()),
            project_dir: None,
        }
    }

    /// The same options with a different default instrument, or none.
    pub fn with_default_instrument(mut self, kind: Option<&str>) -> Self {
        self.default_instrument = kind.map(str::to_owned);
        self
    }

    /// The same options, loading audio and device state from this project
    /// folder.
    pub fn with_project_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.project_dir = Some(dir.into());
        self
    }
}

/// Why a project could not be compiled.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// The engine rejected the graph.
    #[error("cannot build the audio graph: {0}")]
    Graph(#[from] GraphError),
    /// The project has used up the id space, so no graph node ids are left.
    #[error("the project has run out of ids")]
    IdExhausted,
}

/// The graph nodes one track became.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledTrack {
    /// The model track.
    pub track: TrackId,
    /// The clip player: a note player feeding the instrument of a MIDI
    /// track, or the audio clip player of an audio track. `None` for a bus
    /// and for a MIDI track with no notes.
    pub player: Option<DeviceId>,
    /// The track's [`ClipLauncher`], if it has clip-launcher slots.
    pub launcher: Option<DeviceId>,
    /// The instrument node; `None` for a bus or an audio track. Send live
    /// notes here with `AudioMessage::SendEvent`.
    pub instrument: Option<DeviceId>,
    /// The summing node of a bus; `None` for other tracks.
    pub input: Option<DeviceId>,
    /// The chain devices that were built, in signal order: note effects,
    /// the instrument if it came from the chain, then effects. These are
    /// the model's own device ids.
    pub devices: Vec<DeviceId>,
    /// The node whose output is the track's signal before its fader: the
    /// last effect, or else the instrument, audio player or bus input.
    pub pre_fader: DeviceId,
    /// The track's `Fader`: set `Fader::GAIN_DB`, `Fader::PAN` and
    /// `Fader::MUTE` on it. Its output is the track's post-fader signal.
    pub fader: DeviceId,
    /// The gain node of each send, by target bus: set `Gain::GAIN`
    /// (linear) on it for the send level.
    pub sends: Vec<(TrackId, DeviceId)>,
    /// Whether the fader feeds the master directly (not through a bus).
    pub to_master: bool,
}

/// A project compiled for the engine.
#[derive(Debug)]
pub struct CompiledProject {
    /// The stereo graph, ready for [`fourfloor_engine::engine()`].
    pub graph: CompiledGraph,
    /// The master `Gain` node; set `Gain::GAIN` on it for output volume.
    pub master: DeviceId,
    /// End of the last clip, in ticks from the project start.
    pub length: Ticks,
    /// Tracks with notes or audio clips whose sound reaches the master
    /// output.
    pub sounding_tracks: usize,
    /// Notes scheduled across all sounding tracks.
    pub notes: usize,
    /// The tracks that are in the graph, in display order.
    pub tracks: Vec<CompiledTrack>,
    /// The project's scenes, top to bottom; a scene's position is its row
    /// in every [`ClipLauncher`].
    pub scenes: Vec<SceneId>,
    /// Things in the project the compiler left out or changed.
    pub warnings: Vec<String>,
}

impl CompiledProject {
    /// The nodes of one track, if it is in the graph.
    pub fn track(&self, id: TrackId) -> Option<&CompiledTrack> {
        self.tracks.iter().find(|track| track.track == id)
    }

    /// The messages that launch a scene: one per track that has a clip
    /// launcher. Each launcher switches on its next launch-quantize
    /// boundary; a track with no clip in that scene falls silent, and a
    /// track with no launcher keeps playing the arrangement. `None` hands
    /// every track back to the arrangement.
    ///
    /// An unknown scene yields no messages.
    pub fn launch_messages(&self, scene: Option<SceneId>) -> Vec<AudioMessage> {
        let value = match scene {
            None => 0.0,
            Some(scene) => match self.scenes.iter().position(|own| *own == scene) {
                Some(row) => (row + 1) as f32,
                None => return Vec::new(),
            },
        };
        self.tracks
            .iter()
            .filter_map(|track| track.launcher)
            .map(|device| AudioMessage::SetParam {
                device,
                param: ClipLauncher::LAUNCH,
                value,
            })
            .collect()
    }
}
