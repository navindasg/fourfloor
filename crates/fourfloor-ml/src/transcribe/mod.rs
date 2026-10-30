//! Audio to MIDI with Spotify Basic Pitch (Apache-2.0 code and weights).
//!
//! The audio is mixed to mono, resampled to 22.05 kHz and cut into
//! overlapping two-second windows exactly as basic-pitch's `inference.py`
//! does; each window runs through the ONNX model with tract, the overlaps
//! are trimmed, and [`decode_notes`] turns the frames into notes. The
//! result converts into a [`WriteMidiClip`] command for the edit model.

mod notes;

use std::path::Path;

use fourfloor_audio::{resample, AudioBuffer};
use fourfloor_core::{SampleRate, Tempo, Ticks, TrackId, TICKS_PER_BEAT};
use fourfloor_model::command::WriteMidiClip;
use fourfloor_model::MidiNote;
use serde::{Deserialize, Serialize};

pub use notes::{
    decode_notes, NoteEvent, Posteriorgram, TranscribeOptions, MIDI_OFFSET, NOTE_BINS,
};

use crate::cache::ModelCache;
use crate::error::MlError;
use crate::onnx::{tensor_f32, InputShape, OnnxModel};
use crate::registry::{ModelRegistry, BASIC_PITCH};

/// Sample rate the model expects.
pub const MODEL_SAMPLE_RATE: u32 = 22_050;
/// Samples between output frames.
pub const FFT_HOP: usize = 256;
/// Samples per model window (two seconds less one hop).
pub const WINDOW_SAMPLES: usize = MODEL_SAMPLE_RATE as usize * 2 - FFT_HOP;
/// Output frames per window.
const WINDOW_FRAMES: usize = 172;
/// Frames shared by neighbouring windows; half is trimmed from each side.
const OVERLAP_FRAMES: usize = 30;
const OVERLAP_SAMPLES: usize = OVERLAP_FRAMES * FFT_HOP;
const HOP_SAMPLES: usize = WINDOW_SAMPLES - OVERLAP_SAMPLES;
const KEPT_FRAMES: usize = WINDOW_FRAMES - OVERLAP_FRAMES;
const CONTOUR_BINS: usize = NOTE_BINS * 3;

const MODEL_FILE: &str = "nmp.onnx";
const OUTPUT_NOTE: &str = "StatefulPartitionedCall:1";
const OUTPUT_ONSET: &str = "StatefulPartitionedCall:2";
const OUTPUT_CONTOUR: &str = "StatefulPartitionedCall:0";

/// The loaded Basic Pitch model. Loading takes well under a second; keep
/// one per worker and reuse it.
#[derive(Clone, Debug)]
pub struct BasicPitch {
    model: OnnxModel,
    outputs: [usize; 3],
}

/// Notes transcribed from a piece of audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcription {
    /// The notes, sorted by start then pitch.
    pub notes: Vec<NoteEvent>,
    /// Length of the audio, in seconds.
    pub duration_seconds: f64,
}

impl BasicPitch {
    /// Downloads the model on first use (verifying its checksum) and loads it.
    pub fn load(cache: &ModelCache, registry: &ModelRegistry) -> Result<Self, MlError> {
        let paths = cache.ensure(registry.get(BASIC_PITCH)?)?;
        Self::from_file(paths.file(MODEL_FILE)?)
    }

    /// Loads the model from an `nmp.onnx` file.
    pub fn from_file(path: &Path) -> Result<Self, MlError> {
        let model = OnnxModel::load(
            BASIC_PITCH,
            path,
            &[InputShape::f32(&[1, WINDOW_SAMPLES, 1])],
        )?;
        let outputs = [
            model.output_index(OUTPUT_NOTE)?,
            model.output_index(OUTPUT_ONSET)?,
            model.output_index(OUTPUT_CONTOUR)?,
        ];
        Ok(Self { model, outputs })
    }

    /// Runs the model over a whole buffer (any rate, any channel count).
    pub fn posteriorgram(&self, audio: &AudioBuffer) -> Result<Posteriorgram, MlError> {
        if audio.frames() == 0 || audio.channel_count() == 0 {
            return Err(MlError::InvalidInput("audio is empty".to_owned()));
        }
        let rate = SampleRate::new(MODEL_SAMPLE_RATE)
            .map_err(|error| MlError::InvalidInput(error.to_string()))?;
        let mono = audio.to_mono();
        let signal = if audio.sample_rate == rate {
            mono
        } else {
            resample(&mono, audio.sample_rate, rate)
        };
        self.posteriorgram_22k(&signal)
    }

    /// Runs the model over mono audio already at 22.05 kHz.
    pub fn posteriorgram_22k(&self, signal: &[f32]) -> Result<Posteriorgram, MlError> {
        let length = signal.len();
        let mut padded = vec![0.0f32; OVERLAP_SAMPLES / 2];
        padded.extend(signal.iter().map(|s| if s.is_finite() { *s } else { 0.0 }));
        let windows = padded.len().div_ceil(HOP_SAMPLES);
        let expected = (length as f64 / HOP_SAMPLES as f64 * KEPT_FRAMES as f64) as usize;
        let mut gram = Posteriorgram {
            frames: 0,
            frame_times: Vec::with_capacity(windows * KEPT_FRAMES),
            note: Vec::with_capacity(windows * KEPT_FRAMES * NOTE_BINS),
            onset: Vec::with_capacity(windows * KEPT_FRAMES * NOTE_BINS),
            contour: Vec::with_capacity(windows * KEPT_FRAMES * CONTOUR_BINS),
        };
        let mut window = vec![0.0f32; WINDOW_SAMPLES];
        for index in 0..windows {
            let start = index * HOP_SAMPLES;
            let end = (start + WINDOW_SAMPLES).min(padded.len());
            window.fill(0.0);
            window[..end - start].copy_from_slice(&padded[start..end]);
            let outputs = self
                .model
                .run(vec![tensor_f32(&[1, WINDOW_SAMPLES, 1], &window)?])?;
            let take = |which: usize, bins: usize| -> Result<&[f32], MlError> {
                let (shape, data) = &outputs[self.outputs[which]];
                if shape.as_slice() != [1, WINDOW_FRAMES, bins] {
                    return Err(MlError::inference(
                        BASIC_PITCH,
                        format!("unexpected output shape {shape:?}"),
                    ));
                }
                let half = OVERLAP_FRAMES / 2;
                Ok(&data[half * bins..(WINDOW_FRAMES - half) * bins])
            };
            gram.note.extend_from_slice(take(0, NOTE_BINS)?);
            gram.onset.extend_from_slice(take(1, NOTE_BINS)?);
            gram.contour.extend_from_slice(take(2, CONTOUR_BINS)?);
            // Kept frame j of window w starts at padded sample
            // w * hop + (j + 15) * 256, which is w * hop + j * 256 in the
            // original signal.
            gram.frame_times.extend(
                (0..KEPT_FRAMES)
                    .map(|frame| (start + frame * FFT_HOP) as f64 / f64::from(MODEL_SAMPLE_RATE)),
            );
        }
        let frames = expected.min(gram.frame_times.len());
        gram.frames = frames;
        gram.frame_times.truncate(frames);
        gram.note.truncate(frames * NOTE_BINS);
        gram.onset.truncate(frames * NOTE_BINS);
        gram.contour.truncate(frames * CONTOUR_BINS);
        Ok(gram)
    }

    /// Transcribes a buffer into notes.
    pub fn transcribe(
        &self,
        audio: &AudioBuffer,
        options: &TranscribeOptions,
    ) -> Result<Transcription, MlError> {
        let gram = self.posteriorgram(audio)?;
        Ok(Transcription {
            notes: decode_notes(&gram, options),
            duration_seconds: audio.duration_seconds(),
        })
    }
}

impl Transcription {
    /// The notes as clip notes at `tempo`, starting at 0 ticks, sorted by
    /// start then pitch. Notes shorter than one tick are lengthened to one.
    pub fn to_midi_notes(&self, tempo: Tempo) -> Vec<MidiNote> {
        let ticks = |seconds: f64| {
            (seconds.max(0.0) / tempo.seconds_per_beat() * TICKS_PER_BEAT as f64).round() as i64
        };
        let mut notes: Vec<MidiNote> = self
            .notes
            .iter()
            .filter(|note| note.pitch <= 127)
            .map(|note| {
                let start = ticks(note.start_seconds);
                let end = ticks(note.end_seconds).max(start + 1);
                MidiNote {
                    start: Ticks::new(start),
                    length: Ticks::new(end - start),
                    pitch: note.pitch,
                    velocity: note.velocity.clamp(1, 127),
                }
            })
            .collect();
        notes.sort_by_key(|note| (note.start, note.pitch));
        notes
    }

    /// An edit-model command that writes the notes as a new MIDI clip on
    /// `track` at `start`. The clip is whole bars long and covers both the
    /// audio and the last note.
    pub fn to_clip_command(
        &self,
        track: TrackId,
        name: &str,
        start: Ticks,
        tempo: Tempo,
    ) -> WriteMidiClip {
        let notes = self.to_midi_notes(tempo);
        let bar = TICKS_PER_BEAT * 4;
        let audio_end = (self.duration_seconds / tempo.seconds_per_beat() * TICKS_PER_BEAT as f64)
            .ceil() as i64;
        let last = notes
            .iter()
            .map(|note| (note.start + note.length).get())
            .max()
            .unwrap_or(0);
        let covered = audio_end.max(last).max(1);
        let length = (covered + bar - 1) / bar * bar;
        WriteMidiClip {
            track,
            clip: None,
            name: name.chars().take(128).collect(),
            start,
            length: Ticks::new(length),
            notes,
        }
    }
}
