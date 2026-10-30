//! UI state that is not part of the project: the open view, selection,
//! panels, settings and the status line.

use fourfloor_core::{ClipId, DeviceId, TrackId};

use crate::geometry::Snap;

/// Identifies a view. A plain string so new views (DJ, browser, chat) can
/// be added without touching an enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewId(pub &'static str);

impl ViewId {
    /// The arrangement timeline.
    pub const ARRANGEMENT: Self = Self("arrangement");
    /// The clip launcher.
    pub const LAUNCHER: Self = Self("launcher");
    /// The step sequencer.
    pub const SEQUENCER: Self = Self("sequencer");
    /// The piano roll.
    pub const PIANO_ROLL: Self = Self("piano_roll");
    /// The mixer.
    pub const MIXER: Self = Self("mixer");
    /// DJ decks.
    pub const DJ: Self = Self("dj");
}

/// Tabs of the settings window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    /// Audio output and MIDI input.
    #[default]
    Audio,
    /// MIDI controller mappings.
    Mappings,
    /// Machine-learning models.
    Models,
    /// Hosted plugins.
    Plugins,
    /// The assistant's chat model and MCP server.
    Assistant,
}

/// Seconds a toast stays up.
pub const TOAST_SECONDS: f32 = 6.0;
/// Toasts shown at once.
pub const MAX_TOASTS: usize = 4;

/// A short message over the bottom-right corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// The text.
    pub text: String,
    /// `true` for errors.
    pub error: bool,
    /// Seconds it has been up.
    pub age: f32,
}

/// What is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The focused track.
    pub track: Option<TrackId>,
    /// The focused clip (timeline or launcher slot).
    pub clip: Option<ClipId>,
    /// The device shown in the device panel.
    pub device: Option<DeviceId>,
}

impl Selection {
    /// Selects a track (and clears a clip or device on another track).
    pub fn select_track(&mut self, track: TrackId) {
        if self.track != Some(track) {
            self.clip = None;
            self.device = None;
        }
        self.track = Some(track);
    }
}

/// Which side panels and windows are open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panels {
    /// The device chain dock at the bottom.
    pub devices: bool,
    /// The master metering panel on the right.
    pub master: bool,
    /// The add-device browser window.
    pub browser: bool,
    /// The audio/MIDI settings window.
    pub settings: bool,
    /// The file path prompt window (when no native dialogs).
    pub prompt: bool,
    /// The sample browser on the left.
    pub library: bool,
    /// The AI assistant panel.
    pub assistant: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            devices: true,
            master: true,
            browser: false,
            settings: false,
            prompt: false,
            library: false,
            assistant: false,
        }
    }
}

/// A message in the status bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// The text.
    pub text: String,
    /// `true` for errors.
    pub error: bool,
}

/// Audio and MIDI settings as shown in the settings window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSettings {
    /// Output devices found at the last refresh.
    pub outputs: Vec<String>,
    /// The chosen output (`None` = system default).
    pub output: Option<String>,
    /// Buffer size (`None` = device default).
    pub buffer_frames: Option<u32>,
    /// MIDI input ports found at the last refresh.
    pub midi_inputs: Vec<String>,
    /// The open MIDI input.
    pub midi_input: Option<String>,
}

/// What a path prompt is for (used when native dialogs are off).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPurpose {
    /// Open a project folder.
    Open,
    /// Save to a folder.
    SaveAs,
    /// Export a WAV.
    ExportWav,
    /// Export stems.
    ExportStems,
    /// Import a DAWproject.
    ImportDawproject,
    /// Export a DAWproject.
    ExportDawproject,
    /// Load a reference track.
    Reference,
    /// Import an audio file onto the selected audio track.
    ImportAudio,
    /// Scan a folder into the sample library.
    ScanFolder,
    /// Load a file into a DJ deck.
    LoadDeck(usize),
    /// Record the DJ mix to a WAV file.
    RecordMix,
}

/// A reference track, as the master panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReferenceUi {
    /// File name of the loaded reference.
    pub name: Option<String>,
    /// Playing the reference instead of the mix.
    pub monitoring: bool,
    /// Loudness matching on.
    pub matched: bool,
    /// Reference loudness, LUFS.
    pub reference_lufs: f64,
    /// Offline mix loudness, LUFS.
    pub mix_lufs: f64,
    /// Lines of comparison (band deltas, suggestions).
    pub notes: Vec<String>,
}

/// All UI-only state.
#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    /// The view in the centre.
    pub view: ViewId,
    /// The view to return to with Back / Esc after jumping into an editor.
    pub previous_view: Option<ViewId>,
    /// Selection.
    pub selection: Selection,
    /// Snap for time edits.
    pub snap: Snap,
    /// Open panels.
    pub panels: Panels,
    /// Status bar message.
    pub status: Option<Status>,
    /// Track whose instrument plays live MIDI input.
    pub armed: Option<TrackId>,
    /// Metronome on.
    pub metronome: bool,
    /// Loop region last set (kept while the loop is off).
    pub loop_range: (i64, i64),
    /// Loop on.
    pub loop_on: bool,
    /// Scroll the timeline with the playhead.
    pub follow: bool,
    /// Audio/MIDI settings.
    pub devices: DeviceSettings,
    /// Reference A/B state.
    pub reference: ReferenceUi,
    /// The open path prompt and its text.
    pub prompt: Option<(PathPurpose, String)>,
    /// Use native file dialogs (off in tests and headless runs).
    pub native_dialogs: bool,
    /// The open settings tab.
    pub settings_tab: SettingsTab,
    /// Messages over the bottom-right corner, oldest first.
    pub toasts: Vec<Toast>,
    /// The AI panel's MCP and chat-model fields.
    pub assistant_fields: crate::views::assistant::server::ServerFields,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            view: ViewId::ARRANGEMENT,
            previous_view: None,
            selection: Selection::default(),
            snap: Snap::Beat,
            panels: Panels::default(),
            status: None,
            armed: None,
            metronome: false,
            loop_range: (0, crate::geometry::BAR_TICKS * 4),
            loop_on: false,
            follow: false,
            devices: DeviceSettings::default(),
            reference: ReferenceUi::default(),
            prompt: None,
            native_dialogs: false,
            settings_tab: SettingsTab::default(),
            toasts: Vec::new(),
            assistant_fields: Default::default(),
        }
    }
}

impl UiState {
    /// Jumps to `view` from the current one, remembering where to go back to.
    pub fn open_view(&mut self, view: ViewId) {
        if view != self.view {
            self.previous_view = Some(self.view);
            self.view = view;
        }
    }

    /// Switches to `view` directly (tabs, menu, shortcuts); forgets any back target.
    pub fn switch_view(&mut self, view: ViewId) {
        self.view = view;
        self.previous_view = None;
    }

    /// Returns to the view that was open before the last jump. True if it moved.
    pub fn go_back(&mut self) -> bool {
        match self.previous_view.take() {
            Some(view) => {
                self.view = view;
                true
            }
            None => false,
        }
    }

    /// Shows `text` in the status bar.
    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
        });
    }

    /// Shows `text` in the status bar and as a toast, as an error.
    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.toast(text.clone(), true);
        self.status = Some(Status { text, error: true });
    }

    /// Shows `text` in the status bar and as a toast.
    pub fn notify(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.toast(text.clone(), false);
        self.status = Some(Status { text, error: false });
    }

    fn toast(&mut self, text: String, error: bool) {
        // The same message again restarts its timer instead of stacking.
        self.toasts.retain(|toast| toast.text != text);
        self.toasts.push(Toast {
            text,
            error,
            age: 0.0,
        });
        let excess = self.toasts.len().saturating_sub(MAX_TOASTS);
        self.toasts.drain(..excess);
    }

    /// Ages the toasts by `dt` seconds and drops expired ones.
    pub fn age_toasts(&mut self, dt: f32) {
        for toast in &mut self.toasts {
            toast.age += dt;
        }
        self.toasts.retain(|toast| toast.age < TOAST_SECONDS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_toast_and_toasts_expire() {
        let mut ui = UiState::default();
        ui.info("quiet");
        assert!(ui.toasts.is_empty(), "plain status messages do not toast");
        ui.error("boom");
        ui.error("boom");
        assert_eq!(ui.toasts.len(), 1);
        for n in 0..6 {
            ui.notify(format!("note {n}"));
        }
        assert_eq!(ui.toasts.len(), MAX_TOASTS);
        assert_eq!(ui.toasts[0].text, "note 2");
        ui.age_toasts(TOAST_SECONDS + 0.1);
        assert!(ui.toasts.is_empty());
        assert_eq!(ui.status.as_ref().map(|s| s.text.as_str()), Some("note 5"));
    }
}
