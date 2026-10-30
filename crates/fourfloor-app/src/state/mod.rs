//! App state: the document, UI state, intents and the per-frame readout.

pub mod document;
pub mod feature_intent;
pub mod features;
pub mod intent;
pub mod readout;
pub mod ui;

pub use document::Document;
pub use feature_intent::{AssistantIntent, DjIntent, LibraryIntent, MlIntent, SettingsIntent};
pub use features::{FeaturePaths, Features, ScanState};
pub use intent::{FileIntent, Intent, Intents, TransportIntent};
pub use readout::{Assets, LiveReadout, MeterSmoother};
pub use ui::{PathPurpose, Selection, SettingsTab, Toast, UiState, ViewId};
