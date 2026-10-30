//! Keys: a root pitch class with a scale, and scale-degree arithmetic.

use std::fmt;
use std::str::FromStr;

use fourfloor_model::Scale;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Names of the twelve pitch classes, sharps only.
pub const PITCH_CLASS_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// A musical key: where the generators find their notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Key {
    /// Root as a pitch class: 0 is C, 9 is A.
    pub root: u8,
    /// The scale built on the root.
    pub scale: Scale,
}

impl Default for Key {
    /// A minor: the house default.
    fn default() -> Self {
        Self::new(9, Scale::Minor)
    }
}

/// Why a key name could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot read \"{0}\" as a key; try \"A minor\", \"F#m\", \"C major\" or \"D dorian\"")]
pub struct KeyParseError(pub String);

impl Key {
    /// A key on `root` (taken modulo 12).
    pub const fn new(root: u8, scale: Scale) -> Self {
        Self {
            root: root % 12,
            scale,
        }
    }

    /// Whether `pitch` belongs to the key, in any octave.
    pub fn contains(&self, pitch: u8) -> bool {
        self.scale.contains(self.root, pitch)
    }

    /// Number of notes in one octave of the scale.
    pub fn degrees(&self) -> i32 {
        self.scale.intervals().len() as i32
    }

    /// The pitch of a scale degree. Degree 0 is the root at or above
    /// `base`'s octave start; each step of `degree` moves one scale note,
    /// wrapping into higher or lower octaves. The result is clamped to
    /// 0 to 127 by octave, so it always stays in the key.
    pub fn degree_pitch(&self, degree: i32, base_octave: i32) -> u8 {
        let intervals = self.scale.intervals();
        let count = intervals.len() as i32;
        let octave = degree.div_euclid(count);
        let index = degree.rem_euclid(count) as usize;
        let mut pitch =
            12 * (base_octave + 1 + octave) + i32::from(self.root) + i32::from(intervals[index]);
        while pitch > 127 {
            pitch -= 12;
        }
        while pitch < 0 {
            pitch += 12;
        }
        pitch as u8
    }

    /// The diatonic chord on a scale degree: `voices` notes stacked in
    /// scale thirds (3 gives a triad, 4 a seventh chord), lowest first.
    /// Every note is in the key.
    pub fn chord(&self, degree: i32, base_octave: i32, voices: usize) -> Vec<u8> {
        (0..voices as i32)
            .map(|voice| self.degree_pitch(degree + 2 * voice, base_octave))
            .collect()
    }

    /// A short name such as `A minor`.
    pub fn name(&self) -> String {
        format!(
            "{} {}",
            PITCH_CLASS_NAMES[usize::from(self.root % 12)],
            scale_name(self.scale)
        )
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// The lowercase name of a scale, as [`Key::from_str`] reads it.
pub const fn scale_name(scale: Scale) -> &'static str {
    match scale {
        Scale::Major => "major",
        Scale::Minor => "minor",
        Scale::Dorian => "dorian",
        Scale::Phrygian => "phrygian",
        Scale::Lydian => "lydian",
        Scale::Mixolydian => "mixolydian",
        Scale::HarmonicMinor => "harmonic minor",
        Scale::MajorPentatonic => "major pentatonic",
        Scale::MinorPentatonic => "minor pentatonic",
        Scale::Blues => "blues",
        Scale::Chromatic => "chromatic",
    }
}

fn parse_scale(text: &str) -> Option<Scale> {
    let squeezed: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .collect();
    Some(match squeezed.as_str() {
        "" | "maj" | "major" | "ionian" => Scale::Major,
        "m" | "min" | "minor" | "aeolian" | "naturalminor" => Scale::Minor,
        "dorian" | "dor" => Scale::Dorian,
        "phrygian" => Scale::Phrygian,
        "lydian" => Scale::Lydian,
        "mixolydian" | "mixo" => Scale::Mixolydian,
        "harmonicminor" => Scale::HarmonicMinor,
        "majorpentatonic" | "majpent" => Scale::MajorPentatonic,
        "minorpentatonic" | "minpent" | "pentatonic" => Scale::MinorPentatonic,
        "blues" => Scale::Blues,
        "chromatic" => Scale::Chromatic,
        _ => return None,
    })
}

/// Reads a pitch-class name (`C`, `F#`, `Bb`, `E♭`) from the start of
/// `text` and returns it with the rest of the string.
pub fn parse_pitch_class(text: &str) -> Option<(u8, &str)> {
    let mut chars = text.char_indices();
    let (_, letter) = chars.next()?;
    let base: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = &text[letter.len_utf8()..];
    let (shift, rest) = match rest.chars().next() {
        Some('#') | Some('♯') => (1, &rest[rest.chars().next().map_or(0, char::len_utf8)..]),
        Some('b') | Some('♭') => {
            // "b" is a flat unless it starts the word "blues" ("Ab blues"
            // still works: there the flat comes first).
            let after = &rest[rest.chars().next().map_or(0, char::len_utf8)..];
            if rest.to_lowercase().starts_with("blues") {
                (0, rest)
            } else {
                (-1, after)
            }
        }
        _ => (0, rest),
    };
    Some(((base + shift).rem_euclid(12) as u8, rest))
}

impl FromStr for Key {
    type Err = KeyParseError;

    /// Reads `A minor`, `F#m`, `Bb major`, `C`, `D dorian`, `a min`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let trimmed = text.trim();
        let error = || KeyParseError(text.to_owned());
        let (root, rest) = parse_pitch_class(trimmed).ok_or_else(error)?;
        let rest = rest.trim();
        // "Am" and "Amin" are minor; a bare capital "A" is major. Only the
        // scale word is lowercased, so "M" would not be confused with "m",
        // but nobody writes that: treat both as given.
        let scale = if rest == "M" {
            Scale::Major
        } else {
            parse_scale(&rest.to_lowercase()).ok_or_else(error)?
        };
        Ok(Self::new(root, scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_from_common_spellings() {
        assert_eq!("A minor".parse(), Ok(Key::new(9, Scale::Minor)));
        assert_eq!("Am".parse(), Ok(Key::new(9, Scale::Minor)));
        assert_eq!("f#m".parse(), Ok(Key::new(6, Scale::Minor)));
        assert_eq!("Bb major".parse(), Ok(Key::new(10, Scale::Major)));
        assert_eq!("C".parse(), Ok(Key::new(0, Scale::Major)));
        assert_eq!("D dorian".parse(), Ok(Key::new(2, Scale::Dorian)));
        assert_eq!("E♭ min".parse(), Ok(Key::new(3, Scale::Minor)));
        assert_eq!("B blues".parse(), Ok(Key::new(11, Scale::Blues)));
        assert_eq!("Ab blues".parse(), Ok(Key::new(8, Scale::Blues)));
        assert_eq!(
            "G harmonic minor".parse(),
            Ok(Key::new(7, Scale::HarmonicMinor))
        );
        assert!("H minor".parse::<Key>().is_err());
        assert!("A wobble".parse::<Key>().is_err());
        assert!("".parse::<Key>().is_err());
    }

    #[test]
    fn names_round_trip() {
        for root in 0..12 {
            for scale in [Scale::Major, Scale::Minor, Scale::Dorian, Scale::Blues] {
                let key = Key::new(root, scale);
                assert_eq!(key.name().parse(), Ok(key));
            }
        }
    }

    #[test]
    fn degrees_walk_the_scale_across_octaves() {
        let a_minor = Key::new(9, Scale::Minor);
        // Octave 2 starts at MIDI 36 (C2); A2 is 45.
        assert_eq!(a_minor.degree_pitch(0, 2), 45);
        assert_eq!(a_minor.degree_pitch(2, 2), 48);
        assert_eq!(a_minor.degree_pitch(7, 2), 57);
        assert_eq!(a_minor.degree_pitch(-1, 2), 43);
        assert_eq!(a_minor.degree_pitch(-7, 2), 33);
        for degree in -40..60 {
            assert!(a_minor.contains(a_minor.degree_pitch(degree, 3)));
        }
    }

    #[test]
    fn diatonic_chords_stay_in_the_key() {
        let a_minor = Key::new(9, Scale::Minor);
        assert_eq!(a_minor.chord(0, 3, 3), [57, 60, 64]);
        assert_eq!(a_minor.chord(0, 3, 4), [57, 60, 64, 67]);
        // VI in A minor is F major.
        assert_eq!(a_minor.chord(5, 3, 3), [65, 69, 72]);
        for key in [Key::new(6, Scale::Dorian), Key::new(1, Scale::Major)] {
            for degree in 0..7 {
                assert!(key.chord(degree, 3, 4).iter().all(|p| key.contains(*p)));
            }
        }
    }
}
