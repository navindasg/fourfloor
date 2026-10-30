//! House basslines: rhythms that leave the beat to the kick, with pitches
//! from a Markov chain over chord tones of the key.

use fourfloor_core::Ticks;
use fourfloor_model::sequencer::{BAR_TICKS, STEPS_PER_BAR, STEP_TICKS};
use fourfloor_model::MidiNote;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{clamp_unit, GenerateError, Key, MAX_BARS};
use crate::rng::RandomSource;

/// The kind of bassline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BassStyle {
    /// One note on every offbeat eighth, mostly the root: the classic
    /// house bass that answers the kick.
    #[default]
    Offbeat,
    /// Sixteenths rolling between the kicks.
    Rolling,
    /// Offbeats that jump between the root and its octave.
    Octave,
    /// A syncopated line that moves through the chord tones.
    Walking,
}

/// What to generate.
#[derive(Clone, Debug, PartialEq)]
pub struct BassOptions {
    /// Length in 4/4 bars, 1 to 64.
    pub bars: u32,
    /// The key every note is taken from.
    pub key: Key,
    /// The kind of line.
    pub style: BassStyle,
    /// Octave of the root note (1 puts A at MIDI 33). `None` picks the
    /// octave that puts the root between E1 and D#2.
    pub octave: Option<i32>,
    /// The scale degree (0 is the tonic) the line sits on in each bar; it
    /// repeats if shorter than the pattern. Empty stays on the tonic.
    pub progression: Vec<i32>,
    /// How busy the line is, 0 to 1. 0.5 is the style's default.
    pub density: f64,
}

impl Default for BassOptions {
    fn default() -> Self {
        Self {
            bars: 4,
            key: Key::default(),
            style: BassStyle::Offbeat,
            octave: None,
            progression: Vec::new(),
            density: 0.5,
        }
    }
}

/// The chord tones a bass note can take, as states of the pitch chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Root,
    Third,
    Fifth,
    Seventh,
    Octave,
    Under,
}

const TONES: [Tone; 6] = [
    Tone::Root,
    Tone::Third,
    Tone::Fifth,
    Tone::Seventh,
    Tone::Octave,
    Tone::Under,
];

impl Tone {
    /// Offset from the chord root in scale steps.
    fn degree_offset(self, key: &Key) -> i32 {
        match self {
            Self::Root => 0,
            Self::Third => 2,
            Self::Fifth => 4,
            Self::Seventh => 6,
            Self::Octave => key.degrees(),
            Self::Under => -1,
        }
    }

    fn index(self) -> usize {
        TONES.iter().position(|tone| *tone == self).unwrap_or(0)
    }
}

/// Transition weights of the pitch chain: `row[from][to]`, in
/// [`TONES`] order.
fn transitions(style: BassStyle) -> [[f64; 6]; 6] {
    match style {
        BassStyle::Offbeat => [[8.0, 0.3, 1.0, 0.5, 1.5, 0.7]; 6],
        BassStyle::Rolling => [[6.0, 0.2, 1.0, 0.5, 2.5, 0.5]; 6],
        BassStyle::Octave => [
            [0.5, 0.0, 0.6, 0.0, 8.0, 0.0],
            [4.0, 0.0, 0.5, 0.0, 4.0, 0.0],
            [4.0, 0.0, 0.2, 0.0, 4.0, 0.0],
            [4.0, 0.0, 0.5, 0.0, 4.0, 0.0],
            [8.0, 0.0, 0.6, 0.0, 0.5, 0.0],
            [8.0, 0.0, 0.5, 0.0, 2.0, 0.0],
        ],
        BassStyle::Walking => [
            [3.0, 1.5, 2.5, 1.0, 2.0, 1.5],
            [3.0, 0.5, 3.0, 0.5, 0.5, 0.5],
            [3.0, 1.0, 1.0, 2.0, 2.0, 0.5],
            [2.0, 0.3, 2.0, 0.5, 4.0, 0.2],
            [3.0, 0.3, 2.0, 3.0, 1.0, 0.3],
            [6.0, 0.5, 0.5, 0.2, 0.2, 0.5],
        ],
    }
}

/// The steps of one bar that play. The beat itself (every fourth step) is
/// always left free for the kick.
fn rhythm(style: BassStyle, density: f64, rng: &mut dyn RandomSource) -> Vec<usize> {
    let scale = 0.5 + density;
    let mut steps = Vec::new();
    match style {
        BassStyle::Offbeat => {
            for beat in 0..4 {
                steps.push(beat * 4 + 2);
            }
            if rng.chance(0.25 * scale) {
                steps.push(15);
            }
            if rng.chance(0.12 * scale) {
                steps.push(7);
            }
        }
        BassStyle::Rolling => {
            for beat in 0..4 {
                if rng.chance(0.3 * scale) {
                    steps.push(beat * 4 + 1);
                }
                steps.push(beat * 4 + 2);
                steps.push(beat * 4 + 3);
            }
        }
        BassStyle::Octave => {
            for beat in 0..4 {
                steps.push(beat * 4 + 2);
                if rng.chance(0.3 * scale) {
                    steps.push(beat * 4 + 3);
                }
            }
        }
        BassStyle::Walking => {
            let mut previous = false;
            for step in 0..STEPS_PER_BAR {
                if step % 4 == 0 {
                    previous = false;
                    continue;
                }
                let base = if step % 4 == 2 {
                    0.9
                } else if previous {
                    0.3
                } else {
                    0.45
                };
                let hit = rng.chance((base * scale).clamp(0.0, 1.0));
                if hit {
                    steps.push(step);
                }
                previous = hit;
            }
            if steps.is_empty() {
                steps.extend([2, 6, 10, 14]);
            }
        }
    }
    steps.sort_unstable();
    steps.dedup();
    steps
}

/// One bar of the motif: which steps play and which chord tone each takes.
struct MotifBar {
    steps: Vec<usize>,
    tones: Vec<Tone>,
}

fn tones_for(
    count: usize,
    table: &[[f64; 6]; 6],
    density: f64,
    rng: &mut dyn RandomSource,
) -> Vec<Tone> {
    // More density means more movement away from the root.
    let movement = 0.3 + 1.4 * density;
    let mut tones = Vec::with_capacity(count);
    let mut current = Tone::Root;
    for index in 0..count {
        if index > 0 {
            let mut row = table[current.index()];
            for weight in row.iter_mut().skip(1) {
                *weight *= movement;
            }
            current = TONES[rng.weighted(&row)];
        }
        tones.push(current);
    }
    tones
}

fn motif_bar(style: BassStyle, density: f64, rng: &mut dyn RandomSource) -> MotifBar {
    let steps = rhythm(style, density, rng);
    let tones = tones_for(steps.len(), &transitions(style), density, rng);
    MotifBar { steps, tones }
}

fn default_octave(key: &Key) -> i32 {
    // Roots from E to B sit in octave 1 (E1 = 28), C to D# in octave 2.
    if key.root >= 4 {
        1
    } else {
        2
    }
}

/// Generates a bassline.
///
/// The line is a two-bar motif that repeats, moved to each bar's chord
/// root; the last bar of every four-bar phrase gets fresh pitches. Rules
/// that always hold: no note starts on a beat (that is the kick's place),
/// every bar starts on its chord root, every pitch is in the key, every
/// note starts on a sixteenth and ends before the next note starts.
pub fn bassline(
    options: &BassOptions,
    rng: &mut dyn RandomSource,
) -> Result<Vec<MidiNote>, GenerateError> {
    if !(1..=MAX_BARS).contains(&options.bars) {
        return Err(GenerateError::Bars(options.bars));
    }
    let octave = options
        .octave
        .unwrap_or_else(|| default_octave(&options.key));
    if !(-1..=6).contains(&octave) {
        return Err(GenerateError::Option {
            field: "octave",
            reason: format!("{octave} is outside -1 to 6"),
        });
    }
    let density = clamp_unit(options.density);
    let key = options.key;
    let motif = [
        motif_bar(options.style, density, rng),
        motif_bar(options.style, density, rng),
    ];
    let table = transitions(options.style);

    let mut notes = Vec::new();
    for bar in 0..options.bars as usize {
        let chord_degree = if options.progression.is_empty() {
            0
        } else {
            options.progression[bar % options.progression.len()]
        };
        let source = &motif[bar % 2];
        let varied;
        let tones = if bar % 4 == 3 {
            varied = tones_for(source.steps.len(), &table, density, rng);
            &varied
        } else {
            &source.tones
        };
        for (index, (step, tone)) in source.steps.iter().zip(tones).enumerate() {
            let next = source
                .steps
                .get(index + 1)
                .copied()
                .unwrap_or(STEPS_PER_BAR);
            // Hold until the next note, at most two steps, with a small
            // gap so that notes never overlap (which would make a
            // monophonic synth slide).
            let held = (next - step).min(2) as i64 * STEP_TICKS - 20;
            let offbeat = step % 4 == 2;
            let base_velocity: i32 = if offbeat { 104 } else { 88 };
            let velocity = (base_velocity + rng.below(9) as i32 - 4).clamp(1, 127) as u8;
            notes.push(MidiNote {
                start: Ticks::new(bar as i64 * BAR_TICKS + *step as i64 * STEP_TICKS),
                length: Ticks::new(held),
                pitch: key.degree_pitch(chord_degree + tone.degree_offset(&key), octave),
                velocity,
            });
        }
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use fourfloor_model::Scale;

    use super::*;
    use crate::rng::SeededRng;

    const STYLES: [BassStyle; 4] = [
        BassStyle::Offbeat,
        BassStyle::Rolling,
        BassStyle::Octave,
        BassStyle::Walking,
    ];

    fn line(style: BassStyle, key: Key, seed: u64) -> Vec<MidiNote> {
        let options = BassOptions {
            bars: 8,
            key,
            style,
            progression: vec![0, 0, 5, 6],
            ..BassOptions::default()
        };
        bassline(&options, &mut SeededRng::new(seed)).unwrap()
    }

    #[test]
    fn lines_are_in_key_on_grid_off_the_beat_and_never_overlap() {
        let keys = [
            Key::new(9, Scale::Minor),
            Key::new(6, Scale::Dorian),
            Key::new(0, Scale::Major),
            Key::new(3, Scale::MinorPentatonic),
        ];
        for style in STYLES {
            for key in keys {
                for seed in 0..25 {
                    let notes = line(style, key, seed);
                    assert!(notes.len() >= 8 * 3, "{style:?}: {} notes", notes.len());
                    for note in &notes {
                        assert!(key.contains(note.pitch), "{style:?} {key}: {}", note.pitch);
                        assert_eq!(note.start.get() % STEP_TICKS, 0);
                        assert_ne!(note.start.get() % (4 * STEP_TICKS), 0, "on the beat");
                        assert!(note.length.get() > 0);
                        assert!((1..=127).contains(&note.velocity));
                        assert!((20..=72).contains(&note.pitch), "pitch {}", note.pitch);
                    }
                    for pair in notes.windows(2) {
                        assert!(pair[0].start + pair[0].length <= pair[1].start);
                    }
                    assert!(notes.iter().all(|n| n.start.get() < 8 * BAR_TICKS));
                }
            }
        }
    }

    #[test]
    fn every_bar_starts_on_its_chord_root() {
        let key = Key::new(9, Scale::Minor);
        for style in STYLES {
            let notes = line(style, key, 4);
            let progression = [0, 0, 5, 6];
            for bar in 0..8i64 {
                let first = notes
                    .iter()
                    .find(|note| note.start.get() / BAR_TICKS == bar)
                    .unwrap();
                let root = key.degree_pitch(progression[bar as usize % 4], 1);
                assert_eq!(first.pitch, root, "{style:?} bar {bar}");
            }
        }
    }

    #[test]
    fn offbeat_lines_play_every_offbeat() {
        let notes = line(BassStyle::Offbeat, Key::default(), 9);
        for bar in 0..8 {
            for beat in 0..4 {
                let tick = bar * BAR_TICKS + beat * 960 + 480;
                assert!(notes.iter().any(|note| note.start.get() == tick));
            }
        }
        // Mostly the root (A1 = 33) and its octave.
        let roots = notes
            .iter()
            .filter(|note| note.pitch % 12 == 9 || note.pitch % 12 == 5 || note.pitch % 12 == 7)
            .count();
        assert!(roots * 10 >= notes.len() * 6);
    }

    #[test]
    fn seeds_are_deterministic_and_distinct() {
        for style in STYLES {
            assert_eq!(
                line(style, Key::default(), 21),
                line(style, Key::default(), 21)
            );
        }
        let distinct: std::collections::HashSet<Vec<MidiNote>> = (0..20)
            .map(|seed| line(BassStyle::Walking, Key::default(), seed))
            .collect();
        assert!(distinct.len() >= 18);
    }

    #[test]
    fn bad_options_are_refused() {
        let bad_bars = BassOptions {
            bars: 0,
            ..BassOptions::default()
        };
        assert_eq!(
            bassline(&bad_bars, &mut SeededRng::new(1)),
            Err(GenerateError::Bars(0))
        );
        let bad_octave = BassOptions {
            octave: Some(9),
            ..BassOptions::default()
        };
        assert!(matches!(
            bassline(&bad_octave, &mut SeededRng::new(1)),
            Err(GenerateError::Option {
                field: "octave",
                ..
            })
        ));
    }
}
