//! Intents: what views ask for. Views never change the project or the
//! engine themselves; they push intents, and the app applies them after the
//! frame is drawn (commands through the command bus, the rest to services).

use std::path::PathBuf;

use fourfloor_core::{DeviceId, ParamId, TrackId};
use fourfloor_model::{AudioRef, Command, SceneId};

pub use super::feature_intent::{
    AssistantIntent, DjIntent, LibraryIntent, MlIntent, SettingsIntent,
};

/// Transport requests.
#[derive(Clone, Debug, PartialEq)]
pub enum TransportIntent {
    /// Start playing.
    Play,
    /// Stop.
    Stop,
    /// Play if stopped, stop if playing.
    Toggle,
    /// Move the playhead to a tick.
    Seek(i64),
    /// Set or clear the loop, in ticks.
    SetLoop(Option<(i64, i64)>),
    /// Turn the loop on (with the last region) or off.
    ToggleLoop,
    /// Metronome on or off.
    Metronome(bool),
}

/// File requests (paths already chosen).
#[derive(Clone, Debug, PartialEq)]
pub enum FileIntent {
    /// A new empty project.
    New,
    /// Load the built-in demo.
    Demo,
    /// Open a project folder.
    Open(PathBuf),
    /// Save to the current folder (asks for one if there is none).
    Save,
    /// Save to a new folder.
    SaveAs(PathBuf),
    /// Render a WAV mixdown.
    ExportWav(PathBuf),
    /// Render stems into a folder.
    ExportStems(PathBuf),
    /// Import a `.dawproject` into a new project folder.
    ImportDawproject {
        /// The `.dawproject` file.
        file: PathBuf,
        /// The new project's folder.
        dir: PathBuf,
    },
    /// Export a `.dawproject`.
    ExportDawproject(PathBuf),
    /// Load a reference track for A/B.
    LoadReference(PathBuf),
    /// Import an audio file onto the selected audio track at the playhead.
    ImportAudio(PathBuf),
}

/// One thing a view asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Run one command as one undo step.
    Command(Command),
    /// Run several commands as one undo step.
    Transaction {
        /// Label for the undo history.
        label: String,
        /// The commands, in order.
        commands: Vec<Command>,
    },
    /// Undo the last step.
    Undo,
    /// Redo the last undone step.
    Redo,
    /// Transport control.
    Transport(TransportIntent),
    /// Move a device parameter in the engine only (during a drag; the
    /// command follows on release).
    LiveParam {
        /// The device.
        device: DeviceId,
        /// The parameter.
        param: ParamId,
        /// Plain value.
        value: f32,
    },
    /// Move a track's fader in the engine only.
    LiveTrackGain {
        /// The track.
        track: TrackId,
        /// Gain in dB.
        gain_db: f32,
    },
    /// Move a track's pan in the engine only.
    LiveTrackPan {
        /// The track.
        track: TrackId,
        /// Pan, -1 to 1.
        pan: f32,
    },
    /// Launch one track's slot (`None` returns it to the arrangement).
    Launch {
        /// The track.
        track: TrackId,
        /// The scene row.
        scene: Option<SceneId>,
    },
    /// Launch a whole scene (`None` returns every launcher to the
    /// arrangement).
    LaunchScene(Option<SceneId>),
    /// Preview a note on a track's instrument (piano keys, step clicks).
    PreviewNote {
        /// The track.
        track: TrackId,
        /// MIDI pitch.
        pitch: u8,
        /// MIDI velocity.
        velocity: u8,
    },
    /// A file operation.
    File(FileIntent),
    /// Build a waveform overview of a pool file.
    NeedWaveform(AudioRef),
    /// Play the reference instead of the mix (or back).
    MonitorReference(bool),
    /// Loudness-match the reference.
    MatchReference(bool),
    /// Bind the next MIDI controller moved to this parameter.
    MidiLearn {
        /// The device.
        device: DeviceId,
        /// The parameter.
        param: ParamId,
        /// Value at controller 0.
        min: f32,
        /// Value at controller 127.
        max: f32,
    },
    /// Import an audio file into the pool and place it on a track.
    ImportAudio {
        /// An audio track.
        track: TrackId,
        /// The file.
        path: PathBuf,
        /// Where it starts, in ticks.
        at: i64,
    },
    /// Open a MIDI input port by name (`None` closes it).
    OpenMidiInput(Option<String>),
    /// Restart the audio output with these settings.
    RestartAudio {
        /// Device name, `None` for the default.
        device: Option<String>,
        /// Buffer size in frames, `None` for the device default.
        buffer_frames: Option<u32>,
    },
    /// DJ decks and mixer.
    Dj(DjIntent),
    /// The sample browser.
    Library(LibraryIntent),
    /// Show or hide the sample browser panel.
    ToggleLibrary,
    /// The AI panel.
    Assistant(AssistantIntent),
    /// Stems, audio-to-MIDI and models.
    Ml(MlIntent),
    /// Plugins and other settings.
    Settings(SettingsIntent),
}

impl From<DjIntent> for Intent {
    fn from(intent: DjIntent) -> Self {
        Self::Dj(intent)
    }
}

impl From<LibraryIntent> for Intent {
    fn from(intent: LibraryIntent) -> Self {
        Self::Library(intent)
    }
}

impl From<AssistantIntent> for Intent {
    fn from(intent: AssistantIntent) -> Self {
        Self::Assistant(intent)
    }
}

impl From<MlIntent> for Intent {
    fn from(intent: MlIntent) -> Self {
        Self::Ml(intent)
    }
}

impl From<SettingsIntent> for Intent {
    fn from(intent: SettingsIntent) -> Self {
        Self::Settings(intent)
    }
}

impl From<Command> for Intent {
    fn from(command: Command) -> Self {
        Self::Command(command)
    }
}

/// The intents queued during one frame.
#[derive(Debug, Default)]
pub struct Intents {
    queue: Vec<Intent>,
}

impl Intents {
    /// Queues an intent.
    pub fn push(&mut self, intent: impl Into<Intent>) {
        self.queue.push(intent.into());
    }

    /// Queues a command.
    pub fn command(&mut self, command: impl Into<Command>) {
        self.queue.push(Intent::Command(command.into()));
    }

    /// Queues several commands as one undo step (one command runs plain).
    pub fn transaction(&mut self, label: impl Into<String>, commands: Vec<Command>) {
        match commands.len() {
            0 => {}
            1 => self.queue.extend(commands.into_iter().map(Intent::Command)),
            _ => self.queue.push(Intent::Transaction {
                label: label.into(),
                commands,
            }),
        }
    }

    /// Takes everything queued.
    pub fn take(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.queue)
    }

    /// Looks at what is queued.
    pub fn pending(&self) -> &[Intent] {
        &self.queue
    }

    /// `true` when nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
