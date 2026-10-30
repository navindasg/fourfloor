//! DJ mode on the fourfloor engine: two to four decks, beat sync with a
//! tempo master, loops and hot cues, a DJ mixer with isolator EQ, filter,
//! crossfader and cue bus, deck effects, mix recording, a harmonic-mixing
//! helper and stem mixing.
//!
//! - [`DeckTrack`] is a preloaded track (decoded with `fourfloor-audio`),
//!   optionally with a [`DeckGrid`], a key and pre-separated stems.
//!   [`TrackInfo`] and [`AnalysisCache`] get tempo, grid and key from
//!   `fourfloor-analysis` and cache them as JSON.
//! - [`Deck`] is an engine device that plays a track: varispeed, key lock
//!   (built-in WSOLA, or Signalsmith Stretch with the `stretch` feature),
//!   cue, nudge and bend, beat loops, loop roll, hot cues and beat jump.
//!   Decks follow a tempo master through a [`SyncBus`].
//! - [`DjMixer`], [`DjFilter`] and [`FxSlot`] make the channel strips and
//!   the mixer; [`Recorder`] taps the master into a WAV file.
//! - [`DjSession`] builds all of it as one engine graph and carries out
//!   serializable [`DjCommand`]s; [`SessionState`] is the snapshot the UI
//!   and the agent read. [`DjRenderer`] runs a session offline.
//! - [`rank_compatible`] ranks library tracks by key and tempo.

#![warn(missing_docs)]

mod command;
pub mod deck;
mod error;
mod filter;
mod fxslot;
mod harmonic;
mod info;
mod inner;
pub mod mixer;
mod offline;
mod recorder;
mod session;
mod state;
pub mod sync;
mod track;

pub use fourfloor_core as core;

pub use command::{DjCommand, EqBand, StemFile};
pub use deck::{
    Deck, DeckCommand, DeckHandle, DeckSnapshot, LoopRegion, SyncMode, HOT_CUES, MAX_RATE, MIN_RATE,
};
pub use error::{DjError, TrackError};
pub use filter::{filter_cutoffs, DjFilter, HIGH_PASS_CEILING_HZ, LOW_PASS_FLOOR_HZ};
pub use fxslot::FxSlot;
pub use harmonic::{rank_compatible, KeyMatch, LibraryTrack, MatchOptions, Suggestion};
pub use info::{AnalysisCache, TrackInfo, CACHE_VERSION};
pub use mixer::{crossfader_gains, fader_gain, CrossfaderCurve, CrossfaderSide, DjMixer};
pub use offline::{DjOutput, DjRenderer, Stereo};
pub use recorder::{Recorder, RecorderHandle, RecordingSummary};
pub use session::{
    default_registry, DeckIds, DjSession, ParamSink, SessionConfig, DJ_CHANNELS, MIXER_ID,
    RECORDER_ID,
};
pub use state::{
    DeckState, EffectState, LoopState, MixerState, SessionState, StemState, TrackSummary,
};
pub use sync::{DeckClock, SyncBus, MAX_DECKS};
pub use track::{spawn_load, DeckGrid, DeckTrack, TrackStem, MAX_STEMS};
