//! Clips: MIDI notes or a reference into the audio pool.

use fourfloor_core::{ClipId, Ticks};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;

/// One MIDI note inside a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MidiNote {
    /// Note start in ticks from the start of the clip (960 ticks per beat).
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// Note length in ticks; must be positive.
    #[schemars(with = "i64")]
    pub length: Ticks,
    /// MIDI pitch, 0 to 127.
    pub pitch: u8,
    /// MIDI velocity, 1 to 127.
    pub velocity: u8,
}

/// A file in the project's audio pool, named by the hash of its contents.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioRef {
    /// BLAKE3 hash of the file contents, as 64 hex characters.
    pub hash: ContentHash,
    /// File extension without the dot: 1 to 8 lowercase letters or digits.
    pub extension: String,
}

impl AudioRef {
    /// The file name inside the audio pool directory: `<hash>.<extension>`.
    pub fn file_name(&self) -> String {
        format!("{}.{}", self.hash, self.extension)
    }
}

fn default_true() -> bool {
    true
}

/// How an audio clip follows the project tempo.
///
/// A warped clip is stretched so that material recorded at `source_bpm`
/// plays in time at the project tempo: its length changes by
/// `source_bpm / project_bpm`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Warp {
    /// Tempo of the recorded material, 20 to 999 BPM.
    pub source_bpm: f64,
    /// `true` keeps the pitch (time-stretch); `false` lets the pitch follow
    /// the speed, like a record deck (resampling).
    #[serde(default = "default_true")]
    pub key_lock: bool,
}

fn is_zero_gain(gain_db: &f32) -> bool {
    *gain_db == 0.0
}

fn is_zero_ticks(ticks: &Ticks) -> bool {
    *ticks == Ticks::ZERO
}

/// What a clip plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    /// MIDI notes, sorted by start then pitch.
    Midi {
        /// The notes, with starts relative to the clip's content.
        notes: Vec<MidiNote>,
    },
    /// A region of a file in the audio pool.
    Audio {
        /// The pool file.
        source: AudioRef,
        /// Frames to skip at the start of the file.
        source_offset_frames: u64,
        /// Clip gain in decibels, -96 to 24.
        #[serde(default, skip_serializing_if = "is_zero_gain")]
        gain_db: f32,
        /// Length of the linear fade-in at the clip start, in ticks.
        #[serde(default, skip_serializing_if = "is_zero_ticks")]
        fade_in: Ticks,
        /// Length of the linear fade-out at the clip end, in ticks.
        #[serde(default, skip_serializing_if = "is_zero_ticks")]
        fade_out: Ticks,
        /// Tempo following; `None` plays the file at its own speed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warp: Option<Warp>,
    },
}

impl ClipContent {
    /// An audio region at unity gain with no fades and no warp.
    pub fn audio(source: AudioRef, source_offset_frames: u64) -> Self {
        Self::Audio {
            source,
            source_offset_frames,
            gain_db: 0.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
            warp: None,
        }
    }
}

/// A clip on a track's timeline, or in a clip-launcher slot.
///
/// A clip is a window onto its content. At `p` ticks after the clip start
/// the clip plays content time `offset + p`; with a loop it plays
/// `(offset + p) mod loop_length`, so the first `loop_length` ticks of the
/// content repeat for as long as the clip lasts. Content outside the window
/// is kept but not played, so shortening or splitting a clip loses nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Unique id.
    pub id: ClipId,
    /// Display name.
    pub name: String,
    /// Position on the timeline, in ticks from the project start. Ignored
    /// for a clip in a launcher slot, which starts when it is launched.
    pub start: Ticks,
    /// Length in ticks; always positive.
    pub length: Ticks,
    /// Content time at the clip start, in ticks. Splitting a clip gives the
    /// right-hand part an offset.
    #[serde(default, skip_serializing_if = "is_zero_ticks")]
    pub offset: Ticks,
    /// Length of the repeating region of the content, if the clip loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_length: Option<Ticks>,
    /// MIDI notes or an audio reference.
    pub content: ClipContent,
}

impl Clip {
    /// The first tick after the clip.
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// The notes of a MIDI clip; empty for an audio clip.
    pub fn notes(&self) -> &[MidiNote] {
        match &self.content {
            ClipContent::Midi { notes } => notes,
            ClipContent::Audio { .. } => &[],
        }
    }

    /// The pool file of an audio clip.
    pub fn audio_ref(&self) -> Option<&AudioRef> {
        match &self.content {
            ClipContent::Audio { source, .. } => Some(source),
            ClipContent::Midi { .. } => None,
        }
    }

    /// Whether this clip holds MIDI notes.
    pub fn is_midi(&self) -> bool {
        matches!(self.content, ClipContent::Midi { .. })
    }

    /// The clip's notes as they sound, with starts relative to the clip
    /// start: the offset and the loop are applied and every note is cut at
    /// the clip end. Sorted by start then pitch. Empty for an audio clip.
    pub fn sounding_notes(&self) -> Vec<MidiNote> {
        let length = self.length;
        let reach = self.loop_length.unwrap_or(Ticks::new(i64::MAX));
        let place = |shift: i64| {
            let notes = self.notes();
            notes
                .iter()
                .filter(move |note| note.start < reach)
                .filter_map(move |note| {
                    let start = Ticks::new(note.start.get() + shift);
                    (start >= Ticks::ZERO && start < length).then(|| MidiNote {
                        start,
                        length: note.length.min(length - start),
                        ..*note
                    })
                })
        };
        let mut sounding: Vec<MidiNote> = match self.loop_length.map(Ticks::get) {
            Some(period) if period > 0 => {
                let offset = self.offset.get().rem_euclid(period);
                let passes = (offset + length.get() + period - 1) / period;
                (0..passes)
                    .flat_map(|pass| place(pass * period - offset))
                    .collect()
            }
            _ => place(-self.offset.get()).collect(),
        };
        sounding.sort_by_key(|note| (note.start, note.pitch));
        sounding
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: Ticks::new(start),
            length: Ticks::new(length),
            pitch,
            velocity: 100,
        }
    }

    fn clip(length: i64, offset: i64, loop_length: Option<i64>, notes: Vec<MidiNote>) -> Clip {
        Clip {
            id: ClipId::new(1),
            name: "Clip".to_owned(),
            start: Ticks::new(9_600),
            length: Ticks::new(length),
            offset: Ticks::new(offset),
            loop_length: loop_length.map(Ticks::new),
            content: ClipContent::Midi { notes },
        }
    }

    fn starts(clip: &Clip) -> Vec<(i64, i64, u8)> {
        clip.sounding_notes()
            .iter()
            .map(|n| (n.start.get(), n.length.get(), n.pitch))
            .collect()
    }

    #[test]
    fn a_plain_clip_sounds_its_notes_cut_at_the_end() {
        let plain = clip(960, 0, None, vec![note(0, 240, 36), note(720, 480, 38)]);
        assert_eq!(starts(&plain), [(0, 240, 36), (720, 240, 38)]);
        let short = clip(480, 0, None, vec![note(0, 240, 36), note(720, 480, 38)]);
        assert_eq!(starts(&short), [(0, 240, 36)], "notes past the end wait");
    }

    #[test]
    fn an_offset_skips_the_start_of_the_content() {
        let right = clip(480, 480, None, vec![note(0, 240, 36), note(720, 240, 38)]);
        assert_eq!(starts(&right), [(240, 240, 38)]);
    }

    #[test]
    fn a_loop_repeats_the_content_for_the_clip_length() {
        let looped = clip(
            2_400,
            0,
            Some(960),
            vec![note(0, 240, 36), note(480, 240, 38)],
        );
        assert_eq!(
            starts(&looped),
            [
                (0, 240, 36),
                (480, 240, 38),
                (960, 240, 36),
                (1_440, 240, 38),
                (1_920, 240, 36)
            ]
        );
        let shifted = clip(
            960,
            480,
            Some(960),
            vec![note(0, 240, 36), note(480, 240, 38)],
        );
        assert_eq!(starts(&shifted), [(0, 240, 38), (480, 240, 36)]);
        let outside = clip(
            1_920,
            0,
            Some(960),
            vec![note(0, 240, 36), note(1_200, 240, 38)],
        );
        assert_eq!(
            starts(&outside),
            [(0, 240, 36), (960, 240, 36)],
            "content past the loop end never plays"
        );
    }

    #[test]
    fn old_audio_clips_load_with_default_gain_fades_and_warp() {
        let json = serde_json::json!({
            "type": "audio",
            "source": { "hash": ContentHash::of_bytes(b"x"), "extension": "wav" },
            "source_offset_frames": 3
        });
        let content: ClipContent = serde_json::from_value(json.clone()).unwrap();
        let source = AudioRef {
            hash: ContentHash::of_bytes(b"x"),
            extension: "wav".to_owned(),
        };
        assert_eq!(content, ClipContent::audio(source, 3));
        assert_eq!(serde_json::to_value(&content).unwrap(), json);
    }
}
