//! Views: the panes that fill the centre of the window, plus the shell's
//! docks. Every view implements [`View`] and is listed in [`registry`].
//!
//! # Adding a view
//!
//! 1. Write a struct holding the view's own UI state and implement
//!    [`View`] for it. Read the project and engine state from the
//!    [`ViewCtx`]; change anything only by pushing to `ctx.intents`
//!    (commands go through the command bus and are undoable).
//! 2. Give it a [`ViewId`] (a `&'static str`, so no enum to edit).
//! 3. Add it to [`registry`]. The tab bar, keyboard switching
//!    (Ctrl+1..9) and the smoke test pick it up from there.
//! 4. Put its editing logic in a `viewmodel` module and unit-test it; drive
//!    the painted view with `egui_kittest` in `tests/`.

pub mod arrangement;
pub mod assistant;
pub mod browser;
pub mod controls;
pub mod devices;
pub mod dj;
pub mod empty;
pub mod launcher;
pub mod library;
pub mod master;
pub mod mixer;
pub mod pianoroll;
pub mod sequencer;
pub mod shell;

use fourfloor_core::DeviceRegistry;
use fourfloor_model::{Clip, Project, Track};

use crate::state::{Assets, Features, Intents, LiveReadout, UiState, ViewId};

/// Everything a view may read, and the queue it writes intents to.
pub struct ViewCtx<'a> {
    /// The project, read-only.
    pub project: &'a Project,
    /// Shared UI state (selection, snap, panels).
    pub ui: &'a mut UiState,
    /// Devices that can be added.
    pub registry: &'a DeviceRegistry,
    /// Engine state this frame.
    pub live: &'a LiveReadout,
    /// Decoded assets (waveforms).
    pub assets: &'a Assets,
    /// DJ decks, library, assistant and settings lists, read-only.
    pub features: &'a Features,
    /// Where edits and requests go.
    pub intents: &'a mut Intents,
}

impl ViewCtx<'_> {
    /// The selected track, if it still exists.
    pub fn selected_track(&self) -> Option<&Track> {
        self.ui
            .selection
            .track
            .and_then(|id| self.project.track(id))
            .map(AsRef::as_ref)
    }

    /// The selected clip (timeline or launcher) and its track, falling back
    /// to the selected track's first MIDI clip.
    pub fn focused_midi_clip(&self) -> Option<(&Track, &Clip)> {
        if let Some((track, clip)) = self
            .ui
            .selection
            .clip
            .and_then(|id| self.project.find_any_clip(id))
        {
            if clip.is_midi() {
                return Some((track.as_ref(), clip.as_ref()));
            }
        }
        let track = self.selected_track()?;
        track
            .clips
            .iter()
            .find(|clip| clip.is_midi())
            .map(|clip| (track, clip.as_ref()))
    }
}

/// A pane in the centre of the window.
pub trait View {
    /// Stable id.
    fn id(&self) -> ViewId;
    /// Tab title.
    fn title(&self) -> &str;
    /// Draws the view and queues intents.
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut ViewCtx<'_>);
}

/// Every view, in tab order.
pub fn registry() -> Vec<Box<dyn View>> {
    vec![
        Box::new(arrangement::ArrangementView::default()),
        Box::new(launcher::LauncherView),
        Box::new(sequencer::SequencerView::default()),
        Box::new(pianoroll::PianoRollView::default()),
        Box::new(mixer::MixerView::default()),
        Box::new(dj::DjView::default()),
    ]
}
