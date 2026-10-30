//! Musical keys: names, Camelot and Open Key codes, and which keys mix.

use std::fmt;
use std::str::FromStr;

/// Major or minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Mode {
    /// Major.
    Major,
    /// Natural minor.
    Minor,
}

const NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// A musical key: a tonic pitch class (0 is C, 1 is C sharp, up to 11 for
/// B) and a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Key {
    tonic: u8,
    mode: Mode,
}

/// How two keys relate for harmonic mixing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HarmonicRelation {
    /// The same key.
    Same,
    /// Relative major and minor: the same notes, the same Camelot number
    /// (8A and 8B).
    Relative,
    /// The other key is a fifth up: one step clockwise on the wheel, same
    /// letter (8A to 9A).
    FifthUp,
    /// The other key is a fifth down: one step anticlockwise (8A to 7A).
    FifthDown,
    /// Not a standard harmonic mix.
    Unrelated,
}

impl HarmonicRelation {
    /// Whether the two keys mix without clashing.
    pub fn is_compatible(self) -> bool {
        self != Self::Unrelated
    }
}

impl Key {
    /// A key from a tonic pitch class and a mode. The tonic wraps, so 12
    /// is C again.
    pub const fn new(tonic: u8, mode: Mode) -> Self {
        Self {
            tonic: tonic % 12,
            mode,
        }
    }

    /// The major key on `tonic`.
    pub const fn major(tonic: u8) -> Self {
        Self::new(tonic, Mode::Major)
    }

    /// The minor key on `tonic`.
    pub const fn minor(tonic: u8) -> Self {
        Self::new(tonic, Mode::Minor)
    }

    /// All 24 keys: the 12 major keys from C, then the 12 minor keys.
    pub fn all() -> impl Iterator<Item = Key> {
        (0..12).map(Key::major).chain((0..12).map(Key::minor))
    }

    /// Tonic pitch class, 0 (C) to 11 (B).
    pub const fn tonic(self) -> u8 {
        self.tonic
    }

    /// Major or minor.
    pub const fn mode(self) -> Mode {
        self.mode
    }

    /// Name of the tonic, such as `F#` or `Bb`.
    pub fn tonic_name(self) -> &'static str {
        NAMES[usize::from(self.tonic)]
    }

    /// Long name, such as `F# minor`.
    pub fn name(self) -> String {
        let mode = match self.mode {
            Mode::Major => "major",
            Mode::Minor => "minor",
        };
        format!("{} {mode}", self.tonic_name())
    }

    /// Short name, such as `F#m` or `C`.
    pub fn short_name(self) -> String {
        match self.mode {
            Mode::Major => self.tonic_name().to_owned(),
            Mode::Minor => format!("{}m", self.tonic_name()),
        }
    }

    /// Position on the circle of fifths, 1 to 12, as the Camelot wheel
    /// numbers it: C major and A minor are 8.
    pub const fn camelot_number(self) -> u8 {
        // The relative major of a minor key is three semitones up.
        let major_tonic = match self.mode {
            Mode::Major => self.tonic,
            Mode::Minor => (self.tonic + 3) % 12,
        };
        // Each fifth (seven semitones) is one step round the wheel.
        (major_tonic * 7 + 7) % 12 + 1
    }

    /// Camelot code, such as `8A` (A minor) or `8B` (C major).
    pub fn camelot(self) -> String {
        let letter = match self.mode {
            Mode::Major => 'B',
            Mode::Minor => 'A',
        };
        format!("{}{letter}", self.camelot_number())
    }

    /// Open Key code, such as `1m` (A minor) or `1d` (C major).
    pub fn open_key(self) -> String {
        let number = (self.camelot_number() + 4) % 12 + 1;
        let letter = match self.mode {
            Mode::Major => 'd',
            Mode::Minor => 'm',
        };
        format!("{number}{letter}")
    }

    /// The key at Camelot position `number` (1 to 12) in `mode`.
    pub fn from_camelot(number: u8, mode: Mode) -> Option<Self> {
        if !(1..=12).contains(&number) {
            return None;
        }
        Key::all().find(|key| key.mode == mode && key.camelot_number() == number)
    }

    /// The key moved by `semitones`, as when a track is pitched up or down.
    pub fn transposed(self, semitones: i32) -> Self {
        let tonic = (i32::from(self.tonic) + semitones).rem_euclid(12) as u8;
        Self::new(tonic, self.mode)
    }

    /// The relative key: A minor for C major and the reverse.
    pub fn relative(self) -> Self {
        match self.mode {
            Mode::Major => Self::minor((self.tonic + 9) % 12),
            Mode::Minor => Self::major((self.tonic + 3) % 12),
        }
    }

    /// The same mode a fifth up: one step clockwise on the wheel.
    pub fn fifth_up(self) -> Self {
        self.transposed(7)
    }

    /// The same mode a fifth down: one step anticlockwise on the wheel.
    pub fn fifth_down(self) -> Self {
        self.transposed(-7)
    }

    /// How `other` relates to this key.
    pub fn relation_to(self, other: Key) -> HarmonicRelation {
        if other == self {
            HarmonicRelation::Same
        } else if other == self.relative() {
            HarmonicRelation::Relative
        } else if other == self.fifth_up() {
            HarmonicRelation::FifthUp
        } else if other == self.fifth_down() {
            HarmonicRelation::FifthDown
        } else {
            HarmonicRelation::Unrelated
        }
    }

    /// Whether `other` mixes harmonically with this key: the same key, its
    /// relative, or a neighbour on the wheel.
    pub fn is_compatible_with(self, other: Key) -> bool {
        self.relation_to(other).is_compatible()
    }

    /// The keys that mix with this one: itself, its relative, a fifth up
    /// and a fifth down.
    pub fn compatible_keys(self) -> [Key; 4] {
        [self, self.relative(), self.fifth_up(), self.fifth_down()]
    }

    /// Steps between the two keys round the Camelot wheel, 0 to 6,
    /// ignoring the letter.
    pub fn wheel_distance(self, other: Key) -> u8 {
        let apart = (i16::from(self.camelot_number()) - i16::from(other.camelot_number()))
            .rem_euclid(12) as u8;
        apart.min(12 - apart)
    }

    /// The smallest pitch shift in semitones (-6 to 6) that moves `other`
    /// into a key compatible with this one, or 0 if it already is.
    pub fn shift_to_match(self, other: Key) -> i32 {
        [0, 1, -1, 2, -2, 3, -3, 4, -4, 5, -5, 6, -6]
            .into_iter()
            .find(|&shift| self.is_compatible_with(other.transposed(shift)))
            .unwrap_or(0)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// A key name that could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a key name, Camelot code or Open Key code")]
pub struct ParseKeyError(String);

impl FromStr for Key {
    type Err = ParseKeyError;

    /// Reads `C`, `F#m`, `Bb minor`, `a major`, Camelot `8A` and Open Key
    /// `1m`. Sharps and flats are both accepted.
    fn from_str(text: &str) -> Result<Self, ParseKeyError> {
        let trimmed = text.trim();
        parse_wheel_code(trimmed)
            .or_else(|| parse_name(trimmed))
            .ok_or_else(|| ParseKeyError(text.to_owned()))
    }
}

fn parse_wheel_code(text: &str) -> Option<Key> {
    let letter = text.chars().last()?;
    let number: u8 = text[..text.len() - letter.len_utf8()].parse().ok()?;
    if !(1..=12).contains(&number) {
        return None;
    }
    match letter {
        'A' | 'a' => Key::from_camelot(number, Mode::Minor),
        'B' | 'b' => Key::from_camelot(number, Mode::Major),
        // Open Key 1 is Camelot 8.
        'm' | 'M' => Key::from_camelot((number + 6) % 12 + 1, Mode::Minor),
        'd' | 'D' => Key::from_camelot((number + 6) % 12 + 1, Mode::Major),
        _ => None,
    }
}

fn parse_name(text: &str) -> Option<Key> {
    let mut chars = text.chars();
    let base: i32 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (accidental, rest) = if let Some(rest) = rest.strip_prefix(['#', '♯']) {
        (1, rest)
    } else if let Some(rest) = rest.strip_prefix(['b', '♭']) {
        (-1, rest)
    } else {
        (0, rest)
    };
    let mode = match rest.trim().to_ascii_lowercase().as_str() {
        "" | "maj" | "major" => Mode::Major,
        "m" | "min" | "minor" => Mode::Minor,
        _ => return None,
    };
    Some(Key::new((base + accidental).rem_euclid(12) as u8, mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_and_open_key_codes_match_the_wheel() {
        let wheel = [
            ("C", "8B", "1d"),
            ("G", "9B", "2d"),
            ("D", "10B", "3d"),
            ("A", "11B", "4d"),
            ("E", "12B", "5d"),
            ("B", "1B", "6d"),
            ("F#", "2B", "7d"),
            ("Db", "3B", "8d"),
            ("Ab", "4B", "9d"),
            ("Eb", "5B", "10d"),
            ("Bb", "6B", "11d"),
            ("F", "7B", "12d"),
            ("Am", "8A", "1m"),
            ("Em", "9A", "2m"),
            ("Bm", "10A", "3m"),
            ("F#m", "11A", "4m"),
            ("Dbm", "12A", "5m"),
            ("Abm", "1A", "6m"),
            ("Ebm", "2A", "7m"),
            ("Bbm", "3A", "8m"),
            ("Fm", "4A", "9m"),
            ("Cm", "5A", "10m"),
            ("Gm", "6A", "11m"),
            ("Dm", "7A", "12m"),
        ];
        for (name, camelot, open) in wheel {
            let key: Key = name.parse().unwrap();
            assert_eq!(key.short_name(), name);
            assert_eq!(key.camelot(), camelot, "{name}");
            assert_eq!(key.open_key(), open, "{name}");
            assert_eq!(camelot.parse::<Key>().unwrap(), key);
            assert_eq!(open.parse::<Key>().unwrap(), key);
        }
        assert_eq!(Key::all().count(), 24);
    }

    #[test]
    fn names_parse_in_several_spellings() {
        let f_sharp_minor = Key::minor(6);
        for text in ["F#m", "f# minor", "Gb min", " Gbm ", "F♯m"] {
            assert_eq!(text.parse::<Key>().unwrap(), f_sharp_minor, "{text}");
        }
        assert_eq!("Cb".parse::<Key>().unwrap(), Key::major(11));
        assert_eq!("B# major".parse::<Key>().unwrap(), Key::major(0));
        assert_eq!(f_sharp_minor.name(), "F# minor");
        assert_eq!(f_sharp_minor.to_string(), "F# minor");
        for text in ["", "H", "13A", "0B", "C dorian", "8X", "é"] {
            assert!(text.parse::<Key>().is_err(), "{text}");
        }
    }

    #[test]
    fn compatible_keys_are_neighbours_on_the_wheel() {
        let a_minor = Key::minor(9);
        assert_eq!(a_minor.relative(), Key::major(0));
        assert_eq!(Key::major(0).relative(), a_minor);
        assert_eq!(a_minor.fifth_up().camelot(), "9A");
        assert_eq!(a_minor.fifth_down().camelot(), "7A");
        assert_eq!(a_minor.relation_to(a_minor), HarmonicRelation::Same);
        assert_eq!(
            a_minor.relation_to(Key::major(0)),
            HarmonicRelation::Relative
        );
        assert_eq!(
            a_minor.relation_to(Key::minor(4)),
            HarmonicRelation::FifthUp
        );
        assert_eq!(
            a_minor.relation_to(Key::minor(2)),
            HarmonicRelation::FifthDown
        );
        assert_eq!(
            a_minor.relation_to(Key::minor(10)),
            HarmonicRelation::Unrelated
        );
        // 8A does not mix with 9B: a different number and a different letter.
        assert!(!a_minor.is_compatible_with(Key::major(7)));

        for key in Key::all() {
            let compatible: Vec<Key> = Key::all().filter(|&k| key.is_compatible_with(k)).collect();
            assert_eq!(compatible.len(), 4);
            for other in key.compatible_keys() {
                assert!(key.is_compatible_with(other));
                assert!(other.is_compatible_with(key), "compatibility is mutual");
                assert!(key.wheel_distance(other) <= 1);
            }
            assert_eq!(
                Key::from_camelot(key.camelot_number(), key.mode()),
                Some(key)
            );
        }
        assert_eq!(Key::from_camelot(13, Mode::Major), None);
    }

    #[test]
    fn transposing_moves_round_the_wheel() {
        let c = Key::major(0);
        assert_eq!(c.transposed(-1), Key::major(11));
        assert_eq!(c.transposed(14), Key::major(2));
        assert_eq!(c.wheel_distance(Key::major(6)), 6);
        assert_eq!(c.wheel_distance(Key::minor(9)), 0);
        // A track in C sharp needs to come down a semitone to sit in C.
        assert_eq!(c.shift_to_match(Key::major(1)), -1);
        assert_eq!(c.shift_to_match(Key::major(7)), 0);
        // F sharp is as far from C as it gets: two semitones reaches G.
        assert_eq!(c.shift_to_match(Key::major(6)), 1);
    }
}
