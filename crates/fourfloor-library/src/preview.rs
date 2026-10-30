//! Preview hook: how to audition a library file at the project's tempo and
//! key. [`plan_preview`] turns a file's analysed tempo and key into a
//! [`PreviewStream`] of playback ratios; the engine or UI plays it (a
//! resampler for varispeed, a stretcher for key-locked playback).
//! [`PreviewStream::render_varispeed`] renders the varispeed case offline,
//! so the maths can be checked by ear and by test.

use std::path::PathBuf;

use fourfloor_analysis::Key;
use fourfloor_audio::AudioBuffer;
use fourfloor_core::{SampleRate, Tempo};

use crate::error::LibraryError;
use crate::library::Library;
use crate::types::{FileId, FileKind, FileRecord};

/// How a preview follows the project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    /// Play as recorded (only the sample rate is converted).
    Original,
    /// Speed changes tempo and pitch together, like a turntable. Loops
    /// follow the tempo; one-shots with a key are repitched to the key.
    Varispeed,
    /// Time-stretch to the tempo and pitch-shift to the key separately.
    #[default]
    KeyLocked,
}

/// The project a preview should fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewTarget {
    /// Project tempo; `None` leaves the tempo alone.
    pub tempo: Option<Tempo>,
    /// Project key; `None` leaves the pitch alone.
    pub key: Option<Key>,
    /// Engine output rate.
    pub sample_rate: SampleRate,
    /// How to follow.
    pub mode: PreviewMode,
    /// Allow playing a loop at half or double speed when that is nearer
    /// the project tempo (a 62 BPM loop in a 124 BPM project plays at 1x).
    pub half_double: bool,
}

impl PreviewTarget {
    /// Key-locked preview at `tempo` and `key`, half/double allowed.
    pub fn new(tempo: Option<Tempo>, key: Option<Key>, sample_rate: SampleRate) -> Self {
        Self {
            tempo,
            key,
            sample_rate,
            mode: PreviewMode::KeyLocked,
            half_double: true,
        }
    }

    /// The same target in another mode.
    pub fn with_mode(mut self, mode: PreviewMode) -> Self {
        self.mode = mode;
        self
    }
}

/// What the engine needs to audition a file.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewStream {
    /// The file.
    pub file: FileId,
    /// Where it is.
    pub path: PathBuf,
    /// Mode used.
    pub mode: PreviewMode,
    /// Rate of the file in Hz.
    pub source_rate: u32,
    /// Rate to play at in Hz.
    pub output_rate: u32,
    /// Loop playback: true for loops.
    pub looped: bool,
    /// Tempo of the file the plan used (after any half/double choice).
    pub source_bpm: Option<f64>,
    /// Tempo it will sound at.
    pub target_bpm: Option<f64>,
    /// Musical speed: 1 is as recorded, 2 is twice as fast.
    pub speed: f64,
    /// Pitch change heard, in semitones.
    pub pitch_semitones: f64,
    /// Frequency ratio of the pitch change, `2^(semitones / 12)`.
    pub pitch_ratio: f64,
    /// For key-locked playback: output length over source length
    /// (`1 / speed`), for a time stretcher. 1 for the other modes.
    pub time_stretch: f64,
    /// Source frames to advance per output frame, for a resampler: the
    /// rate conversion times the varispeed speed. In key-locked mode this
    /// is only the rate conversion; the stretcher does the rest.
    pub source_step: f64,
    /// Source length in frames.
    pub source_frames: u64,
    /// Output length of one pass in frames.
    pub output_frames: u64,
}

/// Semitones (-5 to 6) that move `from` onto `to`, or onto its relative
/// when the modes differ (an A minor loop in a C major project stays put).
pub fn key_shift(from: Key, to: Key) -> i32 {
    let target = if from.mode() == to.mode() {
        to
    } else {
        to.relative()
    };
    let up = (i32::from(target.tonic()) - i32::from(from.tonic())).rem_euclid(12);
    if up > 6 {
        up - 12
    } else {
        up
    }
}

/// The source tempo, doubled or halved when that is nearer `target`.
fn fit_tempo(source: f64, target: f64, half_double: bool) -> f64 {
    if !half_double {
        return source;
    }
    [source, source * 2.0, source / 2.0]
        .into_iter()
        .min_by(|a, b| {
            (target / a)
                .log2()
                .abs()
                .total_cmp(&(target / b).log2().abs())
        })
        .unwrap_or(source)
}

/// Works out how to play `file` to fit `target`.
pub fn plan_preview(file: &FileRecord, target: &PreviewTarget) -> PreviewStream {
    let output_rate = target.sample_rate.hz();
    let rate_step = if output_rate == 0 {
        1.0
    } else {
        f64::from(file.sample_rate) / f64::from(output_rate)
    };
    let source_frames = (file.duration_seconds * f64::from(file.sample_rate))
        .round()
        .max(0.0) as u64;
    let looped = file.kind == FileKind::Loop;
    let tempo_follows = file.kind != FileKind::OneShot;
    let fitted = match (file.bpm, target.tempo) {
        (Some(bpm), Some(tempo)) if tempo_follows && bpm > 0.0 => {
            Some((fit_tempo(bpm, tempo.bpm(), target.half_double), tempo.bpm()))
        }
        _ => None,
    };
    let tempo_speed = fitted.map_or(1.0, |(source, wanted)| wanted / source);
    let key_semitones = match (file.key, target.key) {
        (Some(from), Some(to)) => f64::from(key_shift(from, to)),
        _ => 0.0,
    };
    let (speed, pitch_semitones, time_stretch) = match target.mode {
        PreviewMode::Original => (1.0, 0.0, 1.0),
        PreviewMode::KeyLocked => (tempo_speed, key_semitones, 1.0 / tempo_speed),
        PreviewMode::Varispeed if fitted.is_some() => (tempo_speed, 12.0 * tempo_speed.log2(), 1.0),
        // No tempo to follow: repitch to the key, sampler style.
        PreviewMode::Varispeed => {
            let speed = 2f64.powf(key_semitones / 12.0);
            (speed, key_semitones, 1.0)
        }
    };
    let source_step = match target.mode {
        PreviewMode::Varispeed => rate_step * speed,
        _ => rate_step,
    };
    let output_frames = (source_frames as f64 / rate_step / speed).round() as u64;
    PreviewStream {
        file: file.id,
        path: file.path.clone(),
        mode: target.mode,
        source_rate: file.sample_rate,
        output_rate,
        looped,
        source_bpm: fitted.map(|(source, _)| source).or(file.bpm),
        target_bpm: match target.mode {
            PreviewMode::Original => file.bpm,
            _ => fitted.map(|(_, wanted)| wanted).or(file.bpm),
        },
        speed,
        pitch_semitones,
        pitch_ratio: 2f64.powf(pitch_semitones / 12.0),
        time_stretch,
        source_step,
        source_frames,
        output_frames,
    }
}

impl PreviewStream {
    /// Renders one pass of the varispeed (or original) plan from decoded
    /// audio, by linear interpolation. Key-locked plans need a time
    /// stretcher, which this crate does not have, so they are refused.
    pub fn render_varispeed(&self, source: &AudioBuffer) -> Result<AudioBuffer, LibraryError> {
        if self.mode == PreviewMode::KeyLocked && (self.speed != 1.0 || self.pitch_semitones != 0.0)
        {
            return Err(LibraryError::invalid(
                "preview mode",
                "key-locked playback needs a time stretcher; render it in the engine",
            ));
        }
        let rate = SampleRate::new(self.output_rate)
            .map_err(|_| LibraryError::invalid("preview rate", "zero"))?;
        let frames = usize::try_from(self.output_frames).unwrap_or(0);
        let channels = source
            .channels
            .iter()
            .map(|channel| {
                (0..frames)
                    .map(|frame| {
                        let position = frame as f64 * self.source_step;
                        let index = position.floor() as usize;
                        let fraction = (position - index as f64) as f32;
                        let a = channel.get(index).copied().unwrap_or(0.0);
                        let b = channel.get(index + 1).copied().unwrap_or(0.0);
                        a + (b - a) * fraction
                    })
                    .collect()
            })
            .collect();
        Ok(AudioBuffer::new(rate, channels))
    }
}

impl Library {
    /// The preview plan for a file at `target`.
    pub fn preview(
        &self,
        file: FileId,
        target: &PreviewTarget,
    ) -> Result<PreviewStream, LibraryError> {
        Ok(plan_preview(&self.file(file)?, target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shift_takes_the_short_way_and_honours_relatives() {
        let a_minor = Key::minor(9);
        assert_eq!(key_shift(a_minor, Key::minor(9)), 0);
        assert_eq!(key_shift(a_minor, Key::minor(11)), 2); // B minor: up 2
        assert_eq!(key_shift(a_minor, Key::minor(4)), -5); // E minor: down 5
        assert_eq!(key_shift(a_minor, Key::minor(3)), 6); // D# minor: tritone up
        assert_eq!(key_shift(a_minor, Key::major(0)), 0); // C major is relative
        assert_eq!(key_shift(a_minor, Key::major(2)), 2); // D major -> B minor
    }

    #[test]
    fn half_and_double_pick_the_nearer_tempo() {
        assert_eq!(fit_tempo(62.0, 124.0, true), 124.0);
        assert_eq!(fit_tempo(250.0, 124.0, true), 125.0);
        assert_eq!(fit_tempo(62.0, 124.0, false), 62.0);
        assert_eq!(fit_tempo(120.0, 124.0, true), 120.0);
    }
}
