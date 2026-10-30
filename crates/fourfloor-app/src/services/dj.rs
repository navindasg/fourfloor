//! DJ mode as a service: a [`DjSession`] whose graph runs inside the app's
//! audio callback (over the project mix, cue on outputs 3 and 4), plus the
//! decoded tracks and waveform overviews the DJ view draws.
//!
//! Deck commands go through each deck's own queue; mixer and effect
//! settings go to the callback as parameter messages through
//! [`HostSink`]. Tracks are decoded and analysed by [`load_deck_job`] on a
//! worker thread, then handed to the session with [`DjService::load`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fourfloor_analysis::{WaveformOptions, WaveformOverview};
use fourfloor_core::{DeviceId, DeviceRegistry, ParamId, SampleRate, Tempo};
use fourfloor_dj::{
    DeckTrack, DjCommand, DjError, DjSession, LibraryTrack, MatchOptions, ParamSink,
    RecordingSummary, SessionConfig, SessionState, Suggestion, TrackInfo, MAX_DECKS,
};

use super::engine_host::{EngineHost, MAX_BLOCK};
use super::voices::{DeckTaps, MAX_DECK_METERS};

/// Effect slots every deck gets, in chain order.
pub const DECK_EFFECTS: [&str; 2] = ["fx.delay", "fx.reverb"];
/// Decks a new session starts with.
pub const DEFAULT_DECKS: usize = 2;

/// Sends DJ parameter changes to the audio callback.
#[derive(Debug)]
pub struct HostSink<'a>(pub &'a mut EngineHost);

impl ParamSink for HostSink<'_> {
    fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> Result<(), DjError> {
        if self.0.dj_param(device, param, value) {
            Ok(())
        } else {
            Err(DjError::QueueFull("the audio callback"))
        }
    }
}

/// A decoded track and what the DJ view draws of it.
#[derive(Clone, Debug)]
pub struct DeckMedia {
    /// The track as the deck plays it (at the engine rate).
    pub track: Arc<DeckTrack>,
    /// Waveform overview of the mono mix, at the engine rate.
    pub overview: Arc<WaveformOverview>,
    /// The file it came from.
    pub path: Option<PathBuf>,
}

/// A finished track load.
#[derive(Debug)]
pub struct DeckLoaded {
    /// Which deck asked for it.
    pub deck: usize,
    /// The track.
    pub media: DeckMedia,
}

/// Job: decode `path` at `rate`, analyse its tempo, grid and key (cached
/// in `cache_dir` when given), and build its overview.
pub fn load_deck_job(
    deck: usize,
    path: PathBuf,
    rate: SampleRate,
    cache_dir: Option<PathBuf>,
) -> impl FnOnce(&super::Progress) -> Result<DeckLoaded, String> + Send + 'static {
    move |progress| {
        progress.set(0.05);
        let buffer =
            fourfloor_audio::decode_file_at_rate(&path, rate).map_err(|e| e.to_string())?;
        progress.set(0.4);
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut track = DeckTrack::from_buffer(&buffer)
            .map_err(|e| e.to_string())?
            .with_name(name);
        let info = match cache_dir.map(fourfloor_dj::AnalysisCache::new) {
            Some(cache) => cache.get_or_analyze(&path).map(|(info, _)| info).ok(),
            None => TrackInfo::analyze(&buffer).ok(),
        };
        if let Some(info) = info {
            track = track.with_info(&info);
        }
        progress.set(0.8);
        let overview =
            WaveformOverview::build(&buffer.to_mono(), rate, &WaveformOptions::default())
                .map_err(|e| e.to_string())?;
        Ok(DeckLoaded {
            deck,
            media: DeckMedia {
                track: Arc::new(track),
                overview: Arc::new(overview),
                path: Some(path),
            },
        })
    }
}

/// The DJ session running in the callback.
pub struct DjService {
    session: DjSession,
    media: Vec<Option<DeckMedia>>,
    state: SessionState,
    last_tempo: Option<Tempo>,
    peaks: [[f32; 2]; MAX_DECK_METERS],
    /// Recordings that closed, newest last.
    pub recordings: Vec<RecordingSummary>,
}

impl std::fmt::Debug for DjService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DjService")
            .field("decks", &self.media.len())
            .finish_non_exhaustive()
    }
}

fn taps_of(session: &DjSession) -> DeckTaps {
    let mut taps = DeckTaps::default();
    for (deck, slot) in taps.strips.iter_mut().enumerate() {
        *slot = session.deck_ids(deck).map(|ids| ids.strip_output());
    }
    taps
}

impl DjService {
    /// Builds a session of `decks` decks and installs its graph in `host`.
    pub fn start(
        decks: usize,
        host: &mut EngineHost,
        registry: &DeviceRegistry,
    ) -> Result<Self, DjError> {
        let decks = decks.clamp(2, MAX_DECKS);
        let config = SessionConfig::new(decks, host.sample_rate(), MAX_BLOCK)
            .with_deck_effects(&DECK_EFFECTS);
        let (session, graph) = DjSession::new(&config, registry)?;
        if !host.install_dj(Some(Box::new(graph)), taps_of(&session)) {
            return Err(DjError::QueueFull("the audio callback"));
        }
        let state = session.state();
        Ok(Self {
            session,
            media: vec![None; decks],
            state,
            last_tempo: None,
            peaks: [[0.0; 2]; MAX_DECK_METERS],
            recordings: Vec::new(),
        })
    }

    /// Rebuilds with `decks` decks, keeping loaded tracks and mixer
    /// settings (playback position and loops start over).
    pub fn resize(
        &mut self,
        decks: usize,
        host: &mut EngineHost,
        registry: &DeviceRegistry,
    ) -> Result<(), DjError> {
        let settings = self.state.settings_commands();
        let media = std::mem::take(&mut self.media);
        let mut fresh = Self::start(decks, host, registry)?;
        for (deck, slot) in media.into_iter().enumerate() {
            if let Some(media) = slot.filter(|_| deck < fresh.media.len()) {
                fresh.load(deck, media)?;
            }
        }
        let deck_count = fresh.media.len();
        for command in settings {
            let fits = command_deck(&command).is_none_or(|deck| deck < deck_count);
            if fits {
                // Settings of a deck without a track may not apply; skip them.
                let _ = fresh.session.apply(&command, &mut HostSink(host));
            }
        }
        fresh.recordings = std::mem::take(&mut self.recordings);
        *self = fresh;
        Ok(())
    }

    /// Removes the graph from the callback.
    pub fn stop(self, host: &mut EngineHost) {
        host.install_dj(None, DeckTaps::default());
    }

    /// Number of decks.
    pub fn deck_count(&self) -> usize {
        self.media.len()
    }

    /// Loads a decoded track into `deck`.
    pub fn load(&mut self, deck: usize, media: DeckMedia) -> Result<(), DjError> {
        let slot = self.media.get_mut(deck).ok_or(DjError::NoSuchDeck(deck))?;
        self.session
            .load_shared(deck, Arc::clone(&media.track), media.path.as_deref())?;
        *slot = Some(media);
        self.state = self.session.state();
        Ok(())
    }

    /// Carries out a command. Loading by path is not done here (it would
    /// block the UI); use [`load_deck_job`] and [`DjService::load`].
    pub fn apply(&mut self, command: &DjCommand, host: &mut EngineHost) -> Result<(), DjError> {
        if let DjCommand::Eject { deck } = command {
            if let Some(slot) = self.media.get_mut(*deck) {
                *slot = None;
            }
        }
        self.session.apply(command, &mut HostSink(host))?;
        self.state = self.session.state();
        Ok(())
    }

    /// Per frame: frees replaced tracks, writes recorded audio, follows the
    /// tempo master and refreshes the snapshot. Returns a recording that
    /// just closed.
    pub fn tick(&mut self, host: &mut EngineHost) -> Result<Option<RecordingSummary>, DjError> {
        let closed = self.session.pump()?;
        if let Some(summary) = &closed {
            self.recordings.push(summary.clone());
        }
        let tempo = self.session.tempo();
        if tempo != self.last_tempo {
            if let Some(tempo) = tempo {
                host.set_dj_tempo(tempo);
            }
            self.last_tempo = tempo;
        }
        self.peaks = host.take_deck_peaks();
        self.state = self.session.state();
        Ok(closed)
    }

    /// The session snapshot as of the last tick.
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// What deck `deck` has loaded.
    pub fn media(&self, deck: usize) -> Option<&DeckMedia> {
        self.media.get(deck).and_then(Option::as_ref)
    }

    /// Strip peaks per deck as of the last tick.
    pub fn peaks(&self) -> [[f32; 2]; MAX_DECK_METERS] {
        self.peaks
    }

    /// Library tracks ranked by how well they follow deck `deck`.
    pub fn suggest(
        &self,
        deck: usize,
        library: &[LibraryTrack],
        options: &MatchOptions,
    ) -> Result<Vec<Suggestion>, DjError> {
        self.session.suggest(deck, library, options)
    }

    /// Path of deck `deck`'s file, if it came from one.
    pub fn path(&self, deck: usize) -> Option<&Path> {
        self.media(deck).and_then(|m| m.path.as_deref())
    }
}

/// The deck a command addresses, if any.
pub fn command_deck(command: &DjCommand) -> Option<usize> {
    let value = serde_json::to_value(command).ok()?;
    value.get("deck")?.as_u64().map(|deck| deck as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_name_their_deck() {
        assert_eq!(command_deck(&DjCommand::Play { deck: 2 }), Some(2));
        assert_eq!(
            command_deck(&DjCommand::SetCrossfader { position: 0.0 }),
            None
        );
        assert_eq!(command_deck(&DjCommand::SetMaster { deck: None }), None);
    }
}
