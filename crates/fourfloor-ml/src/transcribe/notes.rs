//! Turning Basic Pitch posteriorgrams into notes.
//!
//! A line-by-line port of `output_to_notes_polyphonic` and
//! `get_infered_onsets` from basic-pitch's `note_creation.py`
//! (Apache-2.0), with the same defaults. Pitch bends are not decoded.

use serde::{Deserialize, Serialize};

/// Lowest MIDI pitch of the model's 88 note bins (A0).
pub const MIDI_OFFSET: u8 = 21;
/// Number of note bins.
pub const NOTE_BINS: usize = 88;
/// Frames a note may stay below the frame threshold and still continue.
const ENERGY_TOLERANCE: usize = 11;

/// Frame-wise model output for a whole signal.
#[derive(Clone, Debug, PartialEq)]
pub struct Posteriorgram {
    /// Number of frames.
    pub frames: usize,
    /// Start time of every frame, in seconds from the start of the audio.
    pub frame_times: Vec<f64>,
    /// Note activation, `frames * 88`, frame by frame, 0 to 1.
    pub note: Vec<f32>,
    /// Onset activation, `frames * 88`, frame by frame, 0 to 1.
    pub onset: Vec<f32>,
    /// Pitch contour at three bins per semitone, `frames * 264`.
    pub contour: Vec<f32>,
}

impl Posteriorgram {
    /// Note activation of `pitch` (MIDI) at `frame`, 0 outside the range.
    pub fn note_at(&self, frame: usize, pitch: u8) -> f32 {
        pitch
            .checked_sub(MIDI_OFFSET)
            .map(usize::from)
            .filter(|bin| *bin < NOTE_BINS && frame < self.frames)
            .map_or(0.0, |bin| self.note[frame * NOTE_BINS + bin])
    }
}

/// Settings of the note decoder. The defaults are basic-pitch's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscribeOptions {
    /// Onset activation needed to start a note.
    pub onset_threshold: f32,
    /// Note activation needed to keep a note going.
    pub frame_threshold: f32,
    /// Shortest note kept, in milliseconds.
    pub min_note_ms: f64,
    /// Lowest frequency kept, in hertz.
    pub min_hz: Option<f64>,
    /// Highest frequency kept, in hertz.
    pub max_hz: Option<f64>,
    /// Add onsets where the note activation jumps.
    pub infer_onsets: bool,
    /// After onset-based notes, also follow remaining strong activations
    /// ("melodia trick").
    pub melodia_trick: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            onset_threshold: 0.5,
            frame_threshold: 0.3,
            min_note_ms: 127.7,
            min_hz: None,
            max_hz: None,
            infer_onsets: true,
            melodia_trick: true,
        }
    }
}

/// One transcribed note.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteEvent {
    /// Start, in seconds from the start of the audio.
    pub start_seconds: f64,
    /// End, in seconds.
    pub end_seconds: f64,
    /// MIDI pitch.
    pub pitch: u8,
    /// Mean note activation over the note, 0 to 1.
    pub amplitude: f32,
    /// MIDI velocity derived from the amplitude, 1 to 127.
    pub velocity: u8,
}

impl NoteEvent {
    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        self.end_seconds - self.start_seconds
    }
}

/// Frames per second of the model output.
pub(crate) const FRAMES_PER_SECOND: f64 = 22_050.0 / 256.0;

fn hz_to_bin(hz: f64) -> i64 {
    (12.0 * (hz / 440.0).log2() + 69.0 - f64::from(MIDI_OFFSET)).round() as i64
}

/// Decodes notes, sorted by start then pitch.
pub fn decode_notes(gram: &Posteriorgram, options: &TranscribeOptions) -> Vec<NoteEvent> {
    let n = gram.frames;
    if n == 0 || gram.note.len() != n * NOTE_BINS || gram.onset.len() != n * NOTE_BINS {
        return Vec::new();
    }
    let mut frames = gram.note.clone();
    let mut onsets = gram.onset.clone();
    constrain_frequency(&mut frames, &mut onsets, options);
    if options.infer_onsets {
        onsets = infer_onsets(&onsets, &frames, n);
    }
    let min_len = (options.min_note_ms / 1000.0 * FRAMES_PER_SECOND).round() as usize;
    let threshold = options.frame_threshold;
    let mut remaining = frames.clone();
    let mut notes: Vec<(usize, usize, usize, f32)> = Vec::new();

    for (start, bin) in onset_peaks(&onsets, n, options.onset_threshold) {
        if start >= n - 1 {
            continue;
        }
        let mut i = start + 1;
        let mut k = 0;
        while i < n - 1 && k < ENERGY_TOLERANCE {
            if remaining[i * NOTE_BINS + bin] < threshold {
                k += 1;
            } else {
                k = 0;
            }
            i += 1;
        }
        i -= k;
        if i - start <= min_len {
            continue;
        }
        for frame in start..i {
            clear_neighbourhood(&mut remaining, frame, bin);
        }
        notes.push((start, i, bin, mean(&frames, start, i, bin)));
    }

    if options.melodia_trick {
        melodia(&frames, &mut remaining, n, threshold, min_len, &mut notes);
    }

    let mut events: Vec<NoteEvent> = notes
        .into_iter()
        .map(|(start, end, bin, amplitude)| NoteEvent {
            start_seconds: gram.frame_times[start],
            end_seconds: gram.frame_times[end.min(n - 1)],
            pitch: MIDI_OFFSET + bin as u8,
            amplitude,
            velocity: (127.0 * amplitude).round().clamp(1.0, 127.0) as u8,
        })
        .collect();
    events.sort_by(|a, b| {
        a.start_seconds
            .total_cmp(&b.start_seconds)
            .then(a.pitch.cmp(&b.pitch))
    });
    events
}

fn constrain_frequency(frames: &mut [f32], onsets: &mut [f32], options: &TranscribeOptions) {
    let low = options.min_hz.map_or(0, |hz| hz_to_bin(hz).clamp(0, 88)) as usize;
    let high = options.max_hz.map_or(88, |hz| hz_to_bin(hz).clamp(0, 88)) as usize;
    for (frame_row, onset_row) in frames
        .chunks_exact_mut(NOTE_BINS)
        .zip(onsets.chunks_exact_mut(NOTE_BINS))
    {
        for bin in (0..low).chain(high..NOTE_BINS) {
            frame_row[bin] = 0.0;
            onset_row[bin] = 0.0;
        }
    }
}

/// The larger of the predicted onsets and the (rescaled) rise of the note
/// activation over the last one and two frames.
fn infer_onsets(onsets: &[f32], frames: &[f32], n: usize) -> Vec<f32> {
    let at = |frame: isize, bin: usize| -> f32 {
        if frame < 0 {
            0.0
        } else {
            frames[frame as usize * NOTE_BINS + bin]
        }
    };
    let mut diff = vec![0.0f32; n * NOTE_BINS];
    for frame in 2..n {
        for bin in 0..NOTE_BINS {
            let now = at(frame as isize, bin);
            let d1 = now - at(frame as isize - 1, bin);
            let d2 = now - at(frame as isize - 2, bin);
            diff[frame * NOTE_BINS + bin] = d1.min(d2).max(0.0);
        }
    }
    let max_onset = onsets.iter().copied().fold(0.0f32, f32::max);
    let max_diff = diff.iter().copied().fold(0.0f32, f32::max);
    let scale = if max_diff > 0.0 {
        max_onset / max_diff
    } else {
        0.0
    };
    onsets
        .iter()
        .zip(&diff)
        .map(|(onset, diff)| onset.max(diff * scale))
        .collect()
}

/// Strict local maxima over time at or above `threshold`, latest first and,
/// within a frame, highest bin first (the order basic-pitch walks them).
fn onset_peaks(onsets: &[f32], n: usize, threshold: f32) -> Vec<(usize, usize)> {
    let mut peaks = Vec::new();
    for frame in 1..n.saturating_sub(1) {
        for bin in 0..NOTE_BINS {
            let value = onsets[frame * NOTE_BINS + bin];
            if value >= threshold
                && value > onsets[(frame - 1) * NOTE_BINS + bin]
                && value > onsets[(frame + 1) * NOTE_BINS + bin]
            {
                peaks.push((frame, bin));
            }
        }
    }
    peaks.reverse();
    peaks
}

fn clear_neighbourhood(remaining: &mut [f32], frame: usize, bin: usize) {
    let row = frame * NOTE_BINS;
    remaining[row + bin] = 0.0;
    if bin + 1 < NOTE_BINS {
        remaining[row + bin + 1] = 0.0;
    }
    if bin > 0 {
        remaining[row + bin - 1] = 0.0;
    }
}

fn mean(frames: &[f32], start: usize, end: usize, bin: usize) -> f32 {
    if end <= start {
        return 0.0;
    }
    let sum: f32 = (start..end)
        .map(|frame| frames[frame * NOTE_BINS + bin])
        .sum();
    sum / (end - start) as f32
}

/// Follows the strongest leftover activations forwards and backwards. The
/// leftover energy only ever drops to zero, so visiting cells in order of
/// their value and skipping cleared ones finds the same maxima as
/// basic-pitch's repeated `argmax`, without rescanning the matrix.
fn melodia(
    frames: &[f32],
    remaining: &mut [f32],
    n: usize,
    threshold: f32,
    min_len: usize,
    notes: &mut Vec<(usize, usize, usize, f32)>,
) {
    let mut order: Vec<usize> = (0..remaining.len())
        .filter(|&index| remaining[index] > threshold)
        .collect();
    order.sort_by(|&a, &b| remaining[b].total_cmp(&remaining[a]).then(a.cmp(&b)));
    for index in order {
        if remaining[index] <= threshold {
            continue;
        }
        let (mid, bin) = (index / NOTE_BINS, index % NOTE_BINS);
        remaining[index] = 0.0;

        let mut i = mid + 1;
        let mut k = 0;
        while i < n - 1 && k < ENERGY_TOLERANCE {
            if remaining[i * NOTE_BINS + bin] < threshold {
                k += 1;
            } else {
                k = 0;
            }
            clear_neighbourhood(remaining, i, bin);
            i += 1;
        }
        let end = (i - 1).saturating_sub(k);

        let mut i = mid as isize - 1;
        let mut k = 0;
        while i > 0 && k < ENERGY_TOLERANCE {
            if remaining[i as usize * NOTE_BINS + bin] < threshold {
                k += 1;
            } else {
                k = 0;
            }
            clear_neighbourhood(remaining, i as usize, bin);
            i -= 1;
        }
        let start = (i + 1 + k as isize).max(0) as usize;

        if end <= start || end - start <= min_len {
            continue;
        }
        notes.push((start, end, bin, mean(frames, start, end, bin)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gram(frames: usize, paint: &[(usize, usize, u8, f32)]) -> Posteriorgram {
        let mut note = vec![0.0; frames * NOTE_BINS];
        let mut onset = vec![0.0; frames * NOTE_BINS];
        for &(start, end, pitch, level) in paint {
            let bin = usize::from(pitch - MIDI_OFFSET);
            for frame in start..end {
                note[frame * NOTE_BINS + bin] = level;
            }
            onset[start * NOTE_BINS + bin] = 0.9;
        }
        Posteriorgram {
            frames,
            frame_times: (0..frames).map(|f| f as f64 / FRAMES_PER_SECOND).collect(),
            note,
            onset,
            contour: vec![0.0; frames * NOTE_BINS * 3],
        }
    }

    #[test]
    fn painted_notes_are_decoded_with_their_pitch_and_times() {
        let g = gram(
            200,
            &[(10, 60, 60, 0.8), (70, 150, 67, 0.6), (20, 90, 48, 0.7)],
        );
        let notes = decode_notes(&g, &TranscribeOptions::default());
        let summary: Vec<(u8, usize, usize)> = notes
            .iter()
            .map(|note| {
                (
                    note.pitch,
                    (note.start_seconds * FRAMES_PER_SECOND).round() as usize,
                    (note.end_seconds * FRAMES_PER_SECOND).round() as usize,
                )
            })
            .collect();
        assert_eq!(summary, vec![(60, 10, 60), (48, 20, 90), (67, 70, 150)]);
        assert_eq!(notes[0].velocity, (0.8f32 * 127.0).round() as u8);
    }

    #[test]
    fn short_blips_and_quiet_activation_are_ignored() {
        let g = gram(100, &[(10, 18, 60, 0.9), (30, 90, 64, 0.2)]);
        assert!(decode_notes(&g, &TranscribeOptions::default()).is_empty());
    }

    #[test]
    fn melodia_picks_up_notes_without_onsets() {
        let mut g = gram(120, &[(20, 100, 72, 0.7)]);
        g.onset.iter_mut().for_each(|v| *v = 0.0);
        let options = TranscribeOptions {
            infer_onsets: false,
            ..TranscribeOptions::default()
        };
        let notes = decode_notes(&g, &options);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 72);
        let without = TranscribeOptions {
            melodia_trick: false,
            ..options
        };
        assert!(decode_notes(&g, &without).is_empty());
    }

    #[test]
    fn frequency_limits_drop_notes_outside_them() {
        let g = gram(200, &[(10, 60, 40, 0.8), (10, 60, 80, 0.8)]);
        let options = TranscribeOptions {
            min_hz: Some(100.0),
            max_hz: Some(500.0),
            ..TranscribeOptions::default()
        };
        assert!(decode_notes(&g, &options).is_empty());
        let options = TranscribeOptions {
            min_hz: Some(200.0),
            ..TranscribeOptions::default()
        };
        let pitches: Vec<u8> = decode_notes(&g, &options).iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![80]);
    }

    #[test]
    fn empty_and_malformed_grams_give_no_notes() {
        let mut g = gram(0, &[]);
        assert!(decode_notes(&g, &TranscribeOptions::default()).is_empty());
        g.frames = 5;
        assert!(decode_notes(&g, &TranscribeOptions::default()).is_empty());
    }
}
