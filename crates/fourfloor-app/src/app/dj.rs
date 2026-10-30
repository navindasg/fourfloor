//! DJ intents, finished deck loads and the per-frame DJ tick.

use fourfloor_dj::DjCommand;
use fourfloor_library::FileId;

use crate::project::JobOutput;
use crate::services::dj::{load_deck_job, DeckLoaded, DjService, DEFAULT_DECKS};
use crate::state::DjIntent;
use crate::viewmodel::dj::rank_next;
use crate::views::dj::deck::DECK_NAMES;

use super::FourfloorApp;

/// Rows kept in the "what mixes next" list.
pub const SUGGESTIONS: usize = 24;

impl FourfloorApp {
    /// Starts the decks if they are not running. `false` if they would not
    /// start (the error is shown).
    fn ensure_dj(&mut self, decks: usize) -> bool {
        if self.features.dj.is_some() {
            return true;
        }
        match DjService::start(decks, &mut self.host, &self.registry) {
            Ok(service) => {
                self.features.dj = Some(service);
                true
            }
            Err(error) => {
                self.ui_state.error(format!("DJ decks: {error}"));
                false
            }
        }
    }

    pub(super) fn dj_intent(&mut self, intent: DjIntent) {
        match intent {
            DjIntent::Start => {
                self.ensure_dj(DEFAULT_DECKS);
            }
            DjIntent::SetDecks(decks) => {
                if !self.ensure_dj(decks) {
                    return;
                }
                let Some(dj) = self.features.dj.as_mut() else {
                    return;
                };
                if dj.deck_count() != decks {
                    if let Err(error) = dj.resize(decks, &mut self.host, &self.registry) {
                        self.ui_state.error(format!("DJ decks: {error}"));
                    }
                    self.features.loading_decks.retain(|d| *d < decks);
                }
            }
            DjIntent::Command(command) => self.dj_command(&command),
            DjIntent::LoadFile { deck, path } => self.load_deck(deck, path),
            DjIntent::Suggest(deck) => self.suggest(deck),
            DjIntent::Record(path) => self.dj_command(&DjCommand::StartRecording { path }),
        }
    }

    fn dj_command(&mut self, command: &DjCommand) {
        if !self.ensure_dj(DEFAULT_DECKS) {
            return;
        }
        let Some(dj) = self.features.dj.as_mut() else {
            return;
        };
        if let Err(error) = dj.apply(command, &mut self.host) {
            self.ui_state.error(format!("DJ: {error}"));
            return;
        }
        if let DjCommand::StartRecording { path } = command {
            self.ui_state
                .info(format!("Recording the mix to {}", path.display()));
        }
    }

    /// Decodes and analyses `path` on a worker, then loads it into `deck`.
    pub(super) fn load_deck(&mut self, deck: usize, path: std::path::PathBuf) {
        if !self.ensure_dj(DEFAULT_DECKS) {
            return;
        }
        let count = self.features.dj.as_ref().map_or(0, DjService::deck_count);
        if deck >= count {
            self.ui_state
                .error(format!("there is no deck {}", deck + 1));
            return;
        }
        if !self.features.loading_decks.contains(&deck) {
            self.features.loading_decks.push(deck);
        }
        let rate = self.host.sample_rate();
        let cache = self.features.paths.dj_cache.clone();
        let job = load_deck_job(deck, path, rate, cache);
        let name = DECK_NAMES.get(deck).copied().unwrap_or("?");
        self.jobs
            .spawn(format!("Loading deck {name}"), move |progress| {
                job(progress).map(|loaded| JobOutput::DeckLoaded(Box::new(loaded)))
            });
    }

    /// Loads a library file into a deck.
    pub(super) fn load_library_file(&mut self, file: FileId, deck: usize) {
        match self.features.library.record(file) {
            Some(record) => self.load_deck(deck, record.path),
            None => self.ui_state.error("that file is no longer in the library"),
        }
    }

    pub(super) fn deck_loaded(&mut self, loaded: DeckLoaded) {
        let deck = loaded.deck;
        self.features.loading_decks.retain(|d| *d != deck);
        let Some(dj) = self.features.dj.as_mut() else {
            return;
        };
        let title = loaded.media.track.name().to_owned();
        match dj.load(deck, loaded.media) {
            Ok(()) => {
                let name = DECK_NAMES.get(deck).copied().unwrap_or("?");
                self.ui_state.info(format!("Deck {name}: {title}"));
                if self.features.suggestions_for.is_none_or(|d| d == deck) {
                    self.suggest(deck);
                }
            }
            Err(error) => self.ui_state.error(format!("DJ: {error}")),
        }
    }

    /// Recomputes "what mixes next" after `deck`.
    pub(super) fn suggest(&mut self, deck: usize) {
        self.features.suggestions_for = Some(deck);
        self.features.suggestions.clear();
        let Some(dj) = self.features.dj.as_ref() else {
            return;
        };
        let Some(state) = dj.state().decks.get(deck) else {
            return;
        };
        let library = &self.features.library;
        if !library.is_open() {
            return;
        }
        let own = dj.path(deck).and_then(|path| library.by_path(path));
        let similarity = own
            .as_ref()
            .map(|record| library.similarity_scores(record.id))
            .unwrap_or_default();
        let candidates = library.mixable();
        self.features.suggestions = rank_next(
            state,
            own.map(|record| record.id),
            &candidates,
            &similarity,
            SUGGESTIONS,
        );
    }

    /// Per frame: the DJ session's pump and snapshot.
    pub(super) fn tick_dj(&mut self) {
        let Some(dj) = self.features.dj.as_mut() else {
            return;
        };
        match dj.tick(&mut self.host) {
            Ok(Some(summary)) => {
                let seconds = summary.frames as f64 / self.host.sample_rate().as_f64();
                let dropped = if summary.dropped_frames > 0 {
                    format!(", {} frames dropped", summary.dropped_frames)
                } else {
                    String::new()
                };
                self.ui_state.info(format!(
                    "Recorded {seconds:.1} s to {}{dropped}",
                    summary.path.display()
                ));
            }
            Ok(None) => {}
            Err(error) => self.ui_state.error(format!("DJ: {error}")),
        }
    }

    /// Puts the decks back into a new engine host (after an audio restart).
    pub(super) fn reinstall_dj(&mut self) {
        let Some(dj) = self.features.dj.as_mut() else {
            return;
        };
        let decks = dj.deck_count();
        if let Err(error) = dj.resize(decks, &mut self.host, &self.registry) {
            self.ui_state.error(format!("DJ decks: {error}"));
        }
    }
}
