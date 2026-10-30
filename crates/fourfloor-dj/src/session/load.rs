//! Loading tracks into decks: decoding, analysis (cached) and stems.

use std::path::Path;
use std::sync::Arc;

use fourfloor_audio::AudioBuffer;

use super::{DjSession, LoadedTrack};
use crate::command::StemFile;
use crate::deck::DeckCommand;
use crate::error::{DjError, TrackError};
use crate::info::TrackInfo;
use crate::state::{StemState, TrackSummary};
use crate::track::{DeckTrack, TrackStem};

/// Stems whose length differs from the mix by at most this many frames
/// (resampling rounding) are padded or trimmed to fit.
const STEM_SLACK_FRAMES: usize = 1_024;

fn fit(mut channel: Vec<f32>, frames: usize) -> Vec<f32> {
    channel.resize(frames, 0.0);
    channel
}

fn stem_from(name: &str, buffer: &AudioBuffer, frames: usize) -> TrackStem {
    let stem = TrackStem::from_buffer(name, buffer);
    if stem.left.len().abs_diff(frames) <= STEM_SLACK_FRAMES {
        TrackStem::new(
            stem.name,
            stem.sample_rate,
            fit(stem.left, frames),
            fit(stem.right, frames),
        )
    } else {
        stem
    }
}

impl DjSession {
    fn track_info(&self, path: &Path, buffer: &AudioBuffer) -> Option<TrackInfo> {
        if let Some(info) = self.cache.as_ref().and_then(|cache| cache.get(path)) {
            return Some(info);
        }
        // Material without a steady beat or key fails analysis; it still
        // loads, just without a grid.
        let info = TrackInfo::analyze(buffer).ok()?;
        if let Some(cache) = &self.cache {
            // A cache that cannot be written only costs a re-analysis.
            let _ = cache.put(path, &info);
        }
        Some(info)
    }

    /// Decodes `path` (and its stems) at the engine rate and loads it into
    /// `deck`. Blocks while decoding and analysing.
    pub fn load_file(
        &mut self,
        deck: usize,
        path: &Path,
        analyze: bool,
        stems: &[StemFile],
    ) -> Result<(), DjError> {
        if deck >= self.decks.len() {
            return Err(DjError::NoSuchDeck(deck));
        }
        let buffer = fourfloor_audio::decode_file_at_rate(path, self.sample_rate)
            .map_err(TrackError::from)?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut track = DeckTrack::from_buffer(&buffer)?.with_name(name);
        if analyze {
            if let Some(info) = self.track_info(path, &buffer) {
                track = track.with_info(&info);
            }
        }
        if !stems.is_empty() {
            let frames = track.frames();
            let mut decoded = Vec::with_capacity(stems.len());
            for stem in stems {
                let buffer = fourfloor_audio::decode_file_at_rate(&stem.path, self.sample_rate)
                    .map_err(TrackError::from)?;
                decoded.push(stem_from(&stem.name, &buffer, frames));
            }
            track = track.with_stems(decoded)?;
        }
        self.load_shared(deck, Arc::new(track), Some(path))
    }

    /// Loads a prepared track into `deck`.
    pub fn load_track(&mut self, deck: usize, track: DeckTrack) -> Result<(), DjError> {
        self.load_shared(deck, Arc::new(track), None)
    }

    /// Loads a shared track (for example from [`spawn_load`](crate::spawn_load)).
    pub fn load_shared(
        &mut self,
        deck: usize,
        track: Arc<DeckTrack>,
        path: Option<&Path>,
    ) -> Result<(), DjError> {
        let control = self.decks.get_mut(deck).ok_or(DjError::NoSuchDeck(deck))?;
        let loaded = LoadedTrack {
            summary: TrackSummary {
                name: track.name().to_owned(),
                path: path.map(Path::to_path_buf),
                bpm: track.bpm(),
                key: track.key().map(|key| key.name()),
                camelot: track.key().map(|key| key.camelot()),
                duration_seconds: track.seconds(),
            },
            sample_rate: track.sample_rate(),
            grid: track.grid().copied(),
            key: track.key(),
        };
        let stems = track
            .stems()
            .iter()
            .map(|stem| StemState {
                name: stem.name.clone(),
                gain: 1.0,
                muted: false,
            })
            .collect();
        control.handle.collect();
        control.handle.send(DeckCommand::Load(track))?;
        control.track = Some(loaded);
        control.stems = stems;
        // The deck resets these on load; send the session's settings again.
        let commands = [
            DeckCommand::SetPitch(1.0 + control.pitch_percent / 100.0),
            DeckCommand::SetKeyLock(control.key_lock),
            DeckCommand::SetSync(control.sync),
            DeckCommand::SetQuantize(control.quantize),
        ];
        for command in commands {
            control.handle.send(command)?;
        }
        Ok(())
    }

    /// Unloads `deck`.
    pub(super) fn eject(&mut self, deck: usize) -> Result<(), DjError> {
        let control = self.decks.get_mut(deck).ok_or(DjError::NoSuchDeck(deck))?;
        control.handle.send(DeckCommand::Eject)?;
        control.track = None;
        control.stems.clear();
        if self.master == Some(deck) {
            self.master = None;
            self.bus.set_master(None);
        }
        Ok(())
    }
}
