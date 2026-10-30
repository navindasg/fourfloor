//! Services: everything that is not drawing. The engine host, the audio
//! callback, meters and analysis, background jobs, MIDI input and the
//! device registry. Views never touch these directly; the app reads them
//! into a [`crate::state::LiveReadout`] each frame and turns view intents
//! into calls on them.

pub mod analysis;
pub mod assistant;
pub mod audition;
pub mod callback;
pub mod compile_worker;
pub mod dj;
pub mod embed;
pub mod engine_host;
pub mod jobs;
pub mod library;
pub mod meters;
pub mod midi_in;
pub mod mix_report;
pub mod ml;
#[cfg(feature = "audio")]
pub mod output;
pub mod plugins;
pub mod registry;
pub mod voices;

pub use analysis::{MasterAnalysis, MasterReadout};
pub use callback::{AudioCallback, Monitor, ReferenceAudio};
pub use engine_host::{EngineHost, OutputMode, TransportReadout, MAX_BLOCK};
pub use jobs::{Jobs, Progress};
pub use meters::MeterBank;
pub use registry::{build_registry, device_params};
