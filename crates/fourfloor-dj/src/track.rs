//! The audio a deck plays: a preloaded stereo track with an optional beat
//! grid, key and stems. Everything here allocates; build tracks off the
//! audio thread and hand them to a deck as an `Arc`.

use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use fourfloor_analysis::Key;
use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;

use crate::error::TrackError;
use crate::info::TrackInfo;

/// Most stems one track can carry.
pub const MAX_STEMS: usize = 8;

/// A constant-tempo beat grid in track samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckGrid {
    /// Tempo of the track at normal speed.
    pub bpm: f64,
    /// Position of a beat, in track samples (may be fractional).
    pub first_beat: f64,
    /// Beats in a bar.
    pub beats_per_bar: u32,
    /// Which beat (counted from `first_beat`) is the first downbeat.
    pub downbeat_index: u32,
    /// Track samples per beat at normal speed.
    samples_per_beat: f64,
}

impl DeckGrid {
    /// A 4/4 grid of `bpm` whose beat zero is `first_beat_seconds` into a
    /// track sampled at `sample_rate`.
    pub fn new(
        bpm: f64,
        first_beat_seconds: f64,
        sample_rate: SampleRate,
    ) -> Result<Self, TrackError> {
        if !bpm.is_finite() || !(20.0..=999.0).contains(&bpm) {
            return Err(TrackError::InvalidGrid("tempo must be 20 to 999 BPM"));
        }
        if !first_beat_seconds.is_finite() {
            return Err(TrackError::InvalidGrid("first beat must be finite"));
        }
        Ok(Self {
            bpm,
            first_beat: first_beat_seconds * sample_rate.as_f64(),
            beats_per_bar: 4,
            downbeat_index: 0,
            samples_per_beat: 60.0 / bpm * sample_rate.as_f64(),
        })
    }

    /// The same grid with another bar layout.
    pub fn with_bar(mut self, beats_per_bar: u32, downbeat_index: u32) -> Self {
        self.beats_per_bar = beats_per_bar.max(1);
        self.downbeat_index = downbeat_index % self.beats_per_bar;
        self
    }

    /// Track samples in one beat at normal speed.
    pub fn samples_per_beat(&self) -> f64 {
        self.samples_per_beat
    }

    /// Beat position (fractional, may be negative) of a track sample.
    pub fn beat_at(&self, sample: f64) -> f64 {
        (sample - self.first_beat) / self.samples_per_beat
    }

    /// Track sample of a beat position.
    pub fn sample_at(&self, beat: f64) -> f64 {
        self.first_beat + beat * self.samples_per_beat
    }

    /// The grid line closest to `sample`, for a grid of `step_beats`.
    pub fn quantize(&self, sample: f64, step_beats: f64) -> f64 {
        let step = step_beats.max(1.0 / 64.0);
        self.sample_at((self.beat_at(sample) / step).round() * step)
    }
}

/// One pre-separated stem of a track (drums, bass, vocals...).
#[derive(Clone, Debug, PartialEq)]
pub struct TrackStem {
    /// Stable name, e.g. `"drums"`.
    pub name: String,
    /// Sample rate; must equal the track's.
    pub sample_rate: SampleRate,
    /// Left channel.
    pub left: Vec<f32>,
    /// Right channel.
    pub right: Vec<f32>,
}

impl TrackStem {
    /// A stem from two channels.
    pub fn new(
        name: impl Into<String>,
        sample_rate: SampleRate,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            name: name.into(),
            sample_rate,
            left,
            right,
        }
    }

    /// A stem from a decoded buffer (mono is doubled, extra channels are
    /// dropped).
    pub fn from_buffer(name: impl Into<String>, buffer: &AudioBuffer) -> Self {
        let stereo = buffer.to_stereo();
        let mut channels = stereo.channels.into_iter();
        let left = channels.next().unwrap_or_default();
        let right = channels.next().unwrap_or_else(|| left.clone());
        Self::new(name, buffer.sample_rate, left, right)
    }
}

/// A track ready for a deck.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckTrack {
    name: String,
    sample_rate: SampleRate,
    left: Vec<f32>,
    right: Vec<f32>,
    stems: Vec<TrackStem>,
    grid: Option<DeckGrid>,
    key: Option<Key>,
}

impl DeckTrack {
    /// A track from two channels of equal length.
    pub fn new(
        sample_rate: SampleRate,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Result<Self, TrackError> {
        if left.len() != right.len() {
            return Err(TrackError::ChannelMismatch {
                left: left.len(),
                right: right.len(),
            });
        }
        if left.is_empty() {
            return Err(TrackError::Empty);
        }
        Ok(Self {
            name: String::new(),
            sample_rate,
            left,
            right,
            stems: Vec::new(),
            grid: None,
            key: None,
        })
    }

    /// A track from a mono signal, played on both sides.
    pub fn from_mono(sample_rate: SampleRate, signal: &[f32]) -> Result<Self, TrackError> {
        Self::new(sample_rate, signal.to_vec(), signal.to_vec())
    }

    /// A track from a decoded buffer (mono is doubled, extra channels are
    /// dropped).
    pub fn from_buffer(buffer: &AudioBuffer) -> Result<Self, TrackError> {
        let stem = TrackStem::from_buffer("", buffer);
        Self::new(stem.sample_rate, stem.left, stem.right)
    }

    /// Decodes `path` and converts it to `engine_rate`, so the deck plays
    /// it without a rate correction. Slow: call it on a worker thread, or
    /// use [`spawn_load`].
    pub fn load(path: &Path, engine_rate: SampleRate) -> Result<Self, TrackError> {
        let buffer = fourfloor_audio::decode_file_at_rate(path, engine_rate)?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::from_buffer(&buffer)?.with_name(name))
    }

    /// The same track with a display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// The same track with a beat grid.
    pub fn with_grid(mut self, grid: DeckGrid) -> Self {
        self.grid = Some(grid);
        self
    }

    /// The same track with a 4/4 grid of `bpm` starting at
    /// `first_beat_seconds`.
    pub fn with_bpm(self, bpm: f64, first_beat_seconds: f64) -> Result<Self, TrackError> {
        let grid = DeckGrid::new(bpm, first_beat_seconds, self.sample_rate)?;
        Ok(self.with_grid(grid))
    }

    /// The same track with a key.
    pub fn with_key(mut self, key: Key) -> Self {
        self.key = Some(key);
        self
    }

    /// The same track with the grid and key of an analysis result. Fields
    /// the analysis could not determine are left as they were.
    pub fn with_info(mut self, info: &TrackInfo) -> Self {
        if let Some(grid) = info.grid(self.sample_rate) {
            self.grid = Some(grid);
        }
        if let Some(key) = info.key() {
            self.key = Some(key);
        }
        self
    }

    /// The same track with pre-separated stems. Every stem must have the
    /// track's sample rate and length. A deck that has stems plays their
    /// sum (each with its own gain) instead of the mix.
    pub fn with_stems(mut self, stems: Vec<TrackStem>) -> Result<Self, TrackError> {
        if stems.len() > MAX_STEMS {
            return Err(TrackError::TooManyStems {
                count: stems.len(),
                max: MAX_STEMS,
            });
        }
        for stem in &stems {
            let reason = if stem.sample_rate != self.sample_rate {
                Some("different sample rate")
            } else if stem.left.len() != self.left.len() || stem.right.len() != self.left.len() {
                Some("different length")
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(TrackError::StemMismatch {
                    name: stem.name.clone(),
                    reason,
                });
            }
        }
        self.stems = stems;
        Ok(self)
    }

    /// Display name (empty when none was given).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sample rate of the audio.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Length in frames.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Length in seconds.
    pub fn seconds(&self) -> f64 {
        self.left.len() as f64 / self.sample_rate.as_f64()
    }

    /// Left channel of the mix.
    pub fn left(&self) -> &[f32] {
        &self.left
    }

    /// Right channel of the mix.
    pub fn right(&self) -> &[f32] {
        &self.right
    }

    /// The stems, in the order their gains are addressed.
    pub fn stems(&self) -> &[TrackStem] {
        &self.stems
    }

    /// Index of the stem called `name`.
    pub fn stem_index(&self, name: &str) -> Option<usize> {
        self.stems.iter().position(|stem| stem.name == name)
    }

    /// The beat grid, when known.
    pub fn grid(&self) -> Option<&DeckGrid> {
        self.grid.as_ref()
    }

    /// Tempo at normal speed, when known.
    pub fn bpm(&self) -> Option<f64> {
        self.grid.map(|grid| grid.bpm)
    }

    /// The key, when known.
    pub fn key(&self) -> Option<Key> {
        self.key
    }
}

/// Decodes and resamples a track on a new thread, analysing it when
/// `analyze` is set (tempo, beat grid and key from `fourfloor-analysis`).
/// Join the handle to get the track.
pub fn spawn_load(
    path: impl AsRef<Path> + Send + 'static,
    engine_rate: SampleRate,
    analyze: bool,
) -> JoinHandle<Result<Arc<DeckTrack>, TrackError>> {
    std::thread::spawn(move || {
        let buffer = fourfloor_audio::decode_file_at_rate(path.as_ref(), engine_rate)?;
        let name = path
            .as_ref()
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut track = DeckTrack::from_buffer(&buffer)?.with_name(name);
        if analyze {
            if let Ok(info) = TrackInfo::analyze(&buffer) {
                track = track.with_info(&info);
            }
        }
        Ok(Arc::new(track))
    })
}
