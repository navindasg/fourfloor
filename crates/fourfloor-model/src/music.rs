//! Scales, chords and quantizing: the arithmetic behind the piano-roll
//! commands.

use fourfloor_core::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A scale, as a set of pitch classes above a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    /// Major (ionian).
    Major,
    /// Natural minor (aeolian).
    Minor,
    /// Dorian mode.
    Dorian,
    /// Phrygian mode.
    Phrygian,
    /// Lydian mode.
    Lydian,
    /// Mixolydian mode.
    Mixolydian,
    /// Harmonic minor.
    HarmonicMinor,
    /// Major pentatonic.
    MajorPentatonic,
    /// Minor pentatonic.
    MinorPentatonic,
    /// Minor blues.
    Blues,
    /// All twelve notes.
    Chromatic,
}

impl Scale {
    /// Semitones above the root of each scale degree, ascending from 0.
    pub const fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }

    /// Whether `pitch` belongs to this scale on `root` (any octave).
    pub fn contains(self, root: u8, pitch: u8) -> bool {
        let class = (i16::from(pitch) - i16::from(root % 12)).rem_euclid(12) as u8;
        self.intervals().contains(&class)
    }
}

/// Moves a pitch to the nearest note of a scale. A pitch already in the
/// scale stays; a tie between the note below and the note above goes down.
/// The result stays within 0 to 127.
pub fn snap_pitch(pitch: u8, root: u8, scale: Scale) -> u8 {
    let pitch = pitch.min(127);
    (0..=6u8)
        .flat_map(|distance| [pitch.checked_sub(distance), pitch.checked_add(distance)])
        .flatten()
        .find(|candidate| *candidate <= 127 && scale.contains(root, *candidate))
        .unwrap_or(pitch)
}

/// A chord quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChordKind {
    /// Major triad.
    Major,
    /// Minor triad.
    Minor,
    /// Diminished triad.
    Diminished,
    /// Augmented triad.
    Augmented,
    /// Suspended second.
    Sus2,
    /// Suspended fourth.
    Sus4,
    /// Major seventh.
    Major7,
    /// Minor seventh: the house chord.
    Minor7,
    /// Dominant seventh.
    Dominant7,
    /// Major ninth.
    Major9,
    /// Minor ninth.
    Minor9,
    /// Root and fifth only.
    Power,
}

impl ChordKind {
    /// Semitones above the root of each chord tone, ascending from 0.
    pub const fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Major9 => &[0, 4, 7, 11, 14],
            Self::Minor9 => &[0, 3, 7, 10, 14],
            Self::Power => &[0, 7],
        }
    }
}

/// The pitches of a chord on `root`, lowest first. Each inversion moves the
/// lowest note up an octave. Notes that would leave 0 to 127 are dropped.
pub fn chord_pitches(root: u8, chord: ChordKind, inversion: u8) -> Vec<u8> {
    let mut pitches: Vec<u16> = chord
        .intervals()
        .iter()
        .map(|interval| u16::from(root) + u16::from(*interval))
        .collect();
    for _ in 0..usize::from(inversion).min(pitches.len().saturating_sub(1)) {
        let lowest = pitches.remove(0);
        pitches.push(lowest + 12);
    }
    pitches
        .into_iter()
        .filter_map(|pitch| u8::try_from(pitch).ok().filter(|pitch| *pitch <= 127))
        .collect()
}

/// Moves a time towards the nearest multiple of `grid`. `strength` 1 lands
/// on the grid, 0 leaves the time alone and values between go part of the
/// way. A time exactly between two grid lines goes to the later one.
pub fn quantize_tick(time: Ticks, grid: Ticks, strength: f64) -> Ticks {
    let grid = grid.get();
    if grid <= 0 {
        return time;
    }
    let nearest = (time.get() + grid / 2).div_euclid(grid) * grid;
    let moved = (nearest - time.get()) as f64 * strength.clamp(0.0, 1.0);
    Ticks::new(time.get() + moved.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scale_starts_on_the_root_and_ascends() {
        for scale in [
            Scale::Major,
            Scale::Minor,
            Scale::Dorian,
            Scale::Phrygian,
            Scale::Lydian,
            Scale::Mixolydian,
            Scale::HarmonicMinor,
            Scale::MajorPentatonic,
            Scale::MinorPentatonic,
            Scale::Blues,
            Scale::Chromatic,
        ] {
            let intervals = scale.intervals();
            assert_eq!(intervals[0], 0);
            assert!(intervals.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(intervals.iter().all(|interval| *interval < 12));
        }
    }

    #[test]
    fn snapping_moves_to_the_nearest_scale_note() {
        // A minor: A B C D E F G.
        let a_minor = |pitch| snap_pitch(pitch, 9, Scale::Minor);
        assert_eq!(a_minor(69), 69, "A stays");
        assert_eq!(a_minor(70), 69, "A# is between A and B: the tie goes down");
        assert_eq!(a_minor(61), 60, "C# goes down to C");
        assert_eq!(a_minor(68), 67, "G# goes down to G");
        assert_eq!(a_minor(66), 65, "F# is between F and G: down to F");
        // C major pentatonic has gaps of three semitones.
        assert_eq!(snap_pitch(65, 0, Scale::MajorPentatonic), 64, "F to E");
        assert_eq!(snap_pitch(71, 0, Scale::MajorPentatonic), 72, "B to C");
        assert_eq!(snap_pitch(127, 0, Scale::Major), 127);
        assert_eq!(snap_pitch(0, 1, Scale::MajorPentatonic), 1);
        for pitch in 0..=127 {
            assert_eq!(snap_pitch(pitch, 3, Scale::Chromatic), pitch);
            assert!(Scale::Dorian.contains(2, snap_pitch(pitch, 2, Scale::Dorian)));
        }
    }

    #[test]
    fn chords_stack_their_intervals_and_invert() {
        assert_eq!(chord_pitches(57, ChordKind::Minor7, 0), [57, 60, 64, 67]);
        assert_eq!(chord_pitches(60, ChordKind::Major, 1), [64, 67, 72]);
        assert_eq!(chord_pitches(60, ChordKind::Major, 2), [67, 72, 76]);
        assert_eq!(chord_pitches(60, ChordKind::Major, 9), [67, 72, 76]);
        assert_eq!(
            chord_pitches(60, ChordKind::Minor9, 0),
            [60, 63, 67, 70, 74]
        );
        assert_eq!(chord_pitches(125, ChordKind::Major, 0), [125], "clipped");
        assert_eq!(chord_pitches(60, ChordKind::Power, 0), [60, 67]);
    }

    #[test]
    fn quantizing_moves_towards_the_grid_by_the_strength() {
        let q = |time, strength| quantize_tick(Ticks::new(time), Ticks::new(240), strength).get();
        assert_eq!(q(250, 1.0), 240);
        assert_eq!(q(119, 1.0), 0);
        assert_eq!(q(120, 1.0), 240, "a tie goes later");
        assert_eq!(q(300, 0.5), 270);
        assert_eq!(q(300, 0.0), 300);
        assert_eq!(q(-10, 1.0), 0);
        assert_eq!(
            quantize_tick(Ticks::new(7), Ticks::ZERO, 1.0),
            Ticks::new(7)
        );
    }
}
