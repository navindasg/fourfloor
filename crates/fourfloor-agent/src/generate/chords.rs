//! Chord parts: organ stabs, pads and pulses over a Markov-chain
//! progression of diatonic chords, voice-led so the top line moves little.

use fourfloor_core::Ticks;
use fourfloor_model::sequencer::{BAR_TICKS, STEP_TICKS};
use fourfloor_model::{MidiNote, Scale};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{clamp_unit, GenerateError, Key, MAX_BARS};
use crate::rng::RandomSource;

/// How the chords are played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChordStyle {
    /// Short syncopated stabs, the classic organ-house rhythm.
    #[default]
    Organ,
    /// One stab on every offbeat eighth.
    Stab,
    /// Held chords, one per bar: a pad.
    Sustained,
    /// Eighth-note pulses with a gap on each beat.
    Pulse,
}

/// What to generate.
#[derive(Clone, Debug, PartialEq)]
pub struct ChordOptions {
    /// Length in 4/4 bars, 1 to 64.
    pub bars: u32,
    /// The key every note is taken from.
    pub key: Key,
    /// How the chords are played.
    pub style: ChordStyle,
    /// Octave of the first chord's root (4 puts A at MIDI 69). 2 to 6.
    pub octave: i32,
    /// Notes per chord, 3 (triads) to 5 (ninths).
    pub voices: usize,
    /// Scale degree (0 is the tonic) for each bar; repeats if shorter than
    /// the part. Empty generates one with [`progression`].
    pub progression: Vec<i32>,
    /// How busy the rhythm is, 0 to 1.
    pub density: f64,
}

impl Default for ChordOptions {
    fn default() -> Self {
        Self {
            bars: 4,
            key: Key::default(),
            style: ChordStyle::Organ,
            octave: 3,
            voices: 4,
            progression: Vec::new(),
            density: 0.5,
        }
    }
}

/// A generated chord part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordPart {
    /// Scale degree of the chord in each bar.
    pub progression: Vec<i32>,
    /// The notes, starts relative to the part's start, on the sixteenth grid.
    pub notes: Vec<MidiNote>,
}

/// Transition weights between scale degrees 0 to 6 for minor-type keys:
/// i moves to VI, VII and iv; everything leans back towards i.
const MINOR_CHAIN: [[f64; 7]; 7] = [
    [1.0, 0.0, 0.6, 2.0, 1.2, 3.0, 2.5],
    [2.0, 0.0, 0.5, 0.5, 2.0, 0.5, 1.0],
    [1.5, 0.0, 0.3, 1.5, 0.5, 2.0, 1.5],
    [2.5, 0.0, 0.5, 0.5, 2.0, 1.0, 2.0],
    [3.0, 0.0, 0.5, 1.0, 0.3, 1.5, 1.0],
    [1.5, 0.0, 1.0, 1.5, 1.0, 0.3, 3.0],
    [3.5, 0.0, 1.0, 1.0, 1.0, 1.5, 0.3],
];

/// The same for major-type keys: I, IV, V and vi carry most of the weight.
const MAJOR_CHAIN: [[f64; 7]; 7] = [
    [0.8, 1.0, 0.6, 3.0, 2.0, 2.5, 0.0],
    [1.0, 0.3, 0.5, 1.0, 3.0, 0.5, 0.0],
    [0.8, 0.5, 0.3, 2.0, 0.5, 2.5, 0.0],
    [2.5, 1.0, 0.5, 0.3, 2.5, 1.0, 0.0],
    [3.0, 0.5, 0.5, 1.5, 0.3, 2.0, 0.0],
    [1.0, 1.5, 0.5, 2.5, 1.5, 0.3, 0.0],
    [3.0, 0.0, 0.5, 0.5, 0.5, 1.0, 0.3],
];

fn is_minor_type(scale: Scale) -> bool {
    matches!(
        scale,
        Scale::Minor
            | Scale::Dorian
            | Scale::Phrygian
            | Scale::HarmonicMinor
            | Scale::MinorPentatonic
            | Scale::Blues
    )
}

/// A chord progression with one scale degree per bar.
///
/// It starts on the tonic and changes chord every two bars (every bar for
/// parts of two bars or less), moving by a Markov chain that favours the
/// usual house moves. The four-bar phrase repeats with its last chord
/// redrawn every other time, so long parts do not loop exactly.
pub fn progression(key: &Key, bars: u32, rng: &mut dyn RandomSource) -> Vec<i32> {
    let chain = if is_minor_type(key.scale) {
        &MINOR_CHAIN
    } else {
        &MAJOR_CHAIN
    };
    let degrees = key.degrees().clamp(1, 7) as usize;
    let step = |from: i32, rng: &mut dyn RandomSource| -> i32 {
        let row = &chain[(from as usize).min(6)];
        rng.weighted(&row[..degrees.min(7)]) as i32
    };
    // Two chords per four-bar phrase, held two bars each, plus a turnaround.
    let first = 0;
    let second = step(first, rng);
    let phrase = [first, first, second, second];
    let mut result = Vec::with_capacity(bars as usize);
    if bars <= 2 {
        result.push(first);
        if bars == 2 {
            result.push(second);
        }
        return result;
    }
    for bar in 0..bars as usize {
        let in_phrase = bar % 4;
        let phrase_index = bar / 4;
        let degree = if in_phrase == 3 && phrase_index % 2 == 1 {
            step(second, rng)
        } else {
            phrase[in_phrase]
        };
        result.push(degree);
    }
    result
}

/// The sixteenth steps of one bar a style plays, with the length of each
/// note in steps. The "cell" index picks one of the style's variants.
fn rhythm_cell(style: ChordStyle, cell: usize) -> &'static [(usize, usize)] {
    match style {
        ChordStyle::Organ => match cell % 3 {
            0 => &[(0, 1), (3, 1), (6, 1), (10, 1), (13, 1)],
            1 => &[(0, 1), (3, 1), (6, 2), (11, 1), (14, 1)],
            _ => &[(3, 1), (6, 1), (10, 1), (12, 1), (14, 2)],
        },
        ChordStyle::Stab => match cell % 2 {
            0 => &[(2, 1), (6, 1), (10, 1), (14, 1)],
            _ => &[(2, 1), (6, 1), (10, 1), (13, 1), (15, 1)],
        },
        ChordStyle::Sustained => &[(0, 16)],
        ChordStyle::Pulse => match cell % 2 {
            0 => &[(2, 2), (6, 2), (10, 2), (14, 2)],
            _ => &[(1, 1), (2, 2), (6, 2), (9, 1), (10, 2), (14, 2)],
        },
    }
}

/// Number of rhythm variants of a style.
fn cell_count(style: ChordStyle) -> usize {
    match style {
        ChordStyle::Organ => 3,
        ChordStyle::Stab | ChordStyle::Pulse => 2,
        ChordStyle::Sustained => 1,
    }
}

/// Re-voices `chord` by moving its notes in octaves so it sits as close
/// as possible to `previous`, keeping every note between 36 and 96.
/// Pitch classes, and so the key, never change.
fn voice_lead(chord: &[u8], previous: Option<&[u8]>) -> Vec<u8> {
    let Some(previous) = previous else {
        return chord.to_vec();
    };
    let centre = previous.iter().map(|p| f64::from(*p)).sum::<f64>() / previous.len() as f64;
    let mut best: Option<(f64, Vec<u8>)> = None;
    // Try every inversion, each shifted by whole octaves.
    for inversion in 0..chord.len() {
        let mut voiced: Vec<i32> = chord.iter().map(|p| i32::from(*p)).collect();
        for note in voiced.iter_mut().take(inversion) {
            *note += 12;
        }
        voiced.sort_unstable();
        for shift in [-24, -12, 0, 12] {
            let candidate: Vec<i32> = voiced.iter().map(|p| p + shift).collect();
            if candidate.iter().any(|p| !(36..=96).contains(p)) {
                continue;
            }
            let mean =
                candidate.iter().map(|p| f64::from(*p)).sum::<f64>() / candidate.len() as f64;
            let top_move = (f64::from(*candidate.last().unwrap_or(&0))
                - f64::from(*previous.last().unwrap_or(&0)))
            .abs();
            let cost = (mean - centre).abs() + 0.5 * top_move;
            if best.as_ref().is_none_or(|(score, _)| cost < *score) {
                best = Some((cost, candidate.iter().map(|p| *p as u8).collect()));
            }
        }
    }
    best.map_or_else(|| chord.to_vec(), |(_, voiced)| voiced)
}

/// Generates a chord part.
///
/// Rules that always hold: every pitch is in the key; every note starts on
/// a sixteenth step inside the part; notes of one pitch never overlap; a
/// bar's notes all belong to that bar's diatonic chord. The rhythm moves
/// between the style's variants by a Markov chain that favours repeating,
/// so the same seed always gives the same part.
pub fn chord_part(
    options: &ChordOptions,
    rng: &mut dyn RandomSource,
) -> Result<ChordPart, GenerateError> {
    if !(1..=MAX_BARS).contains(&options.bars) {
        return Err(GenerateError::Bars(options.bars));
    }
    if !(2..=6).contains(&options.octave) {
        return Err(GenerateError::Option {
            field: "octave",
            reason: format!("{} is outside 2 to 6", options.octave),
        });
    }
    if !(3..=5).contains(&options.voices) {
        return Err(GenerateError::Option {
            field: "voices",
            reason: format!("{} is outside 3 to 5", options.voices),
        });
    }
    let density = clamp_unit(options.density);
    let key = options.key;
    let degrees = if options.progression.is_empty() {
        progression(&key, options.bars, rng)
    } else {
        (0..options.bars as usize)
            .map(|bar| options.progression[bar % options.progression.len()])
            .collect()
    };

    let cells = cell_count(options.style);
    let stay = 0.4 + 0.5 * (1.0 - density);
    let mut cell = 0usize;
    let mut previous: Option<Vec<u8>> = None;
    let mut notes = Vec::new();
    for (bar, degree) in degrees.iter().enumerate() {
        if bar > 0 && cells > 1 && !rng.chance(stay) {
            cell = (cell + 1 + rng.below(cells - 1)) % cells;
        }
        let chord = voice_lead(
            &key.chord(*degree, options.octave, options.voices),
            previous.as_deref(),
        );
        let bar_start = bar as i64 * BAR_TICKS;
        for (step, length) in rhythm_cell(options.style, cell) {
            let accent = if step % 4 == 0 { 8 } else { 0 };
            for pitch in &chord {
                let velocity = (92 + accent + rng.below(9) as i32 - 4).clamp(1, 127) as u8;
                notes.push(MidiNote {
                    start: Ticks::new(bar_start + *step as i64 * STEP_TICKS),
                    length: Ticks::new(*length as i64 * STEP_TICKS - 30),
                    pitch: *pitch,
                    velocity,
                });
            }
        }
        previous = Some(chord);
    }
    Ok(ChordPart {
        progression: degrees,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SeededRng;

    const STYLES: [ChordStyle; 4] = [
        ChordStyle::Organ,
        ChordStyle::Stab,
        ChordStyle::Sustained,
        ChordStyle::Pulse,
    ];

    fn part(style: ChordStyle, key: Key, seed: u64) -> ChordPart {
        let options = ChordOptions {
            bars: 8,
            key,
            style,
            ..ChordOptions::default()
        };
        chord_part(&options, &mut SeededRng::new(seed)).unwrap()
    }

    #[test]
    fn parts_are_in_key_on_grid_and_inside_the_part() {
        let keys = [
            Key::new(9, Scale::Minor),
            Key::new(0, Scale::Major),
            Key::new(2, Scale::Dorian),
            Key::new(6, Scale::Minor),
        ];
        for style in STYLES {
            for key in keys {
                for seed in 0..20 {
                    let part = part(style, key, seed);
                    assert_eq!(part.progression.len(), 8);
                    assert!(!part.notes.is_empty());
                    for note in &part.notes {
                        assert!(key.contains(note.pitch), "{style:?} {key}: {}", note.pitch);
                        assert_eq!(note.start.get() % STEP_TICKS, 0);
                        assert!(note.start.get() + note.length.get() <= 8 * BAR_TICKS);
                        assert!(note.length.get() > 0);
                        assert!((36..=96).contains(&note.pitch));
                    }
                    for pitch in 0..128u8 {
                        let mut same: Vec<_> =
                            part.notes.iter().filter(|n| n.pitch == pitch).collect();
                        same.sort_by_key(|n| n.start);
                        for pair in same.windows(2) {
                            assert!(pair[0].start + pair[0].length <= pair[1].start);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn each_bar_plays_its_diatonic_chord() {
        let key = Key::new(9, Scale::Minor);
        let part = part(ChordStyle::Organ, key, 3);
        for (bar, degree) in part.progression.iter().enumerate() {
            let classes: Vec<u8> = key.chord(*degree, 3, 4).iter().map(|p| p % 12).collect();
            for note in part
                .notes
                .iter()
                .filter(|n| n.start.get() / BAR_TICKS == bar as i64)
            {
                assert!(classes.contains(&(note.pitch % 12)), "bar {bar}");
            }
        }
    }

    #[test]
    fn progressions_start_on_the_tonic_and_are_deterministic() {
        for seed in 0..30 {
            let key = Key::default();
            let a = progression(&key, 16, &mut SeededRng::new(seed));
            let b = progression(&key, 16, &mut SeededRng::new(seed));
            assert_eq!(a, b);
            assert_eq!(a[0], 0);
            assert!(a.iter().all(|d| (0..7).contains(d)));
        }
        let distinct: std::collections::HashSet<Vec<i32>> = (0..30)
            .map(|seed| progression(&Key::default(), 8, &mut SeededRng::new(seed)))
            .collect();
        assert!(distinct.len() >= 3);
        assert_eq!(
            part(ChordStyle::Pulse, Key::default(), 8),
            part(ChordStyle::Pulse, Key::default(), 8)
        );
    }

    #[test]
    fn voice_leading_keeps_the_chords_close() {
        let key = Key::new(9, Scale::Minor);
        let part = chord_part(
            &ChordOptions {
                bars: 4,
                style: ChordStyle::Sustained,
                progression: vec![0, 5, 3, 6],
                ..ChordOptions::default()
            },
            &mut SeededRng::new(1),
        )
        .unwrap();
        let tops: Vec<u8> = (0..4)
            .map(|bar| {
                part.notes
                    .iter()
                    .filter(|n| n.start.get() / BAR_TICKS == bar)
                    .map(|n| n.pitch)
                    .max()
                    .unwrap()
            })
            .collect();
        for pair in tops.windows(2) {
            assert!(
                (i32::from(pair[0]) - i32::from(pair[1])).abs() <= 7,
                "{tops:?}"
            );
        }
        assert!(part.notes.iter().all(|n| key.contains(n.pitch)));
    }

    #[test]
    fn bad_options_are_refused() {
        let mut rng = SeededRng::new(1);
        let bars = ChordOptions {
            bars: 0,
            ..ChordOptions::default()
        };
        assert_eq!(chord_part(&bars, &mut rng), Err(GenerateError::Bars(0)));
        let voices = ChordOptions {
            voices: 9,
            ..ChordOptions::default()
        };
        assert!(chord_part(&voices, &mut rng).is_err());
        let octave = ChordOptions {
            octave: 9,
            ..ChordOptions::default()
        };
        assert!(chord_part(&octave, &mut rng).is_err());
    }
}
