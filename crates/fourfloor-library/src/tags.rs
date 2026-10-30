//! Automatic tags: what a file name says about a sound, and what its
//! features suggest when the name says nothing.

use std::collections::BTreeSet;

use fourfloor_analysis::{FeatureVector, Key};

use crate::types::FileKind;

/// Tags that name percussion without a pitch worth reporting. A file with
/// one of these gets no key from analysis.
pub const UNPITCHED_TAGS: [&str; 10] = [
    "kick", "snare", "clap", "hat", "rim", "tom", "perc", "cymbal", "drums", "top",
];

/// Tags that say which instrument a sound is. When the file name supplies
/// one, the features are not asked for a guess.
pub const INSTRUMENT_TAGS: [&str; 21] = [
    "kick", "snare", "clap", "hat", "rim", "tom", "cowbell", "perc", "cymbal", "drums", "top",
    "bass", "vocal", "pad", "stab", "chord", "lead", "synth", "organ", "keys", "fx",
];

/// What each word of a file name stands for.
const KEYWORDS: &[(&[&str], &[&str])] = &[
    (
        &["kick", "kicks", "kik", "bd", "bassdrum"],
        &["kick", "drums"],
    ),
    (&["snare", "snares", "sd", "snr"], &["snare", "drums"]),
    (&["clap", "claps", "cp", "clp"], &["clap", "drums"]),
    (
        &["hat", "hats", "hh", "hihat", "hihats", "chh", "ohh"],
        &["hat", "drums"],
    ),
    (&["rim", "rimshot"], &["rim", "perc", "drums"]),
    (&["tom", "toms"], &["tom", "drums"]),
    (&["cowbell"], &["cowbell", "perc", "drums"]),
    (
        &[
            "perc",
            "percussion",
            "conga",
            "bongo",
            "shaker",
            "tamb",
            "tambourine",
            "clave",
        ],
        &["perc", "drums"],
    ),
    (&["ride", "crash", "cymbal", "cym"], &["cymbal", "drums"]),
    (&["drum", "drums", "beat", "break"], &["drums"]),
    (&["top", "tops"], &["top", "drums"]),
    (&["bass", "sub", "reese"], &["bass"]),
    (&["acid", "303"], &["acid", "bass"]),
    (
        &[
            "vocal",
            "vocals",
            "vox",
            "voice",
            "acapella",
            "acappella",
            "chant",
        ],
        &["vocal"],
    ),
    (&["pad", "pads", "strings", "atmos", "atmosphere"], &["pad"]),
    (&["stab", "stabs"], &["stab"]),
    (&["chord", "chords"], &["chord"]),
    (&["lead", "leads"], &["lead"]),
    (&["synth", "synths"], &["synth"]),
    (&["organ"], &["organ", "keys"]),
    (&["piano", "rhodes", "keys"], &["keys"]),
    (
        &[
            "fx",
            "sfx",
            "riser",
            "sweep",
            "impact",
            "uplifter",
            "downlifter",
        ],
        &["fx"],
    ),
    (&["loop", "loops"], &["loop"]),
];

/// What a file name tells about its contents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameHints {
    /// Tags for the words recognised, sorted, without repeats.
    pub tags: Vec<&'static str>,
    /// A tempo written in the name: `124bpm`, `bpm 124`, or a lone number
    /// from 70 to 180 in a name that also says `loop`.
    pub bpm: Option<f64>,
    /// A key written in the name with its mode: `Am`, `F#min`, `Cmaj`.
    /// A bare note name (`A`, `F#`) is not taken as a key.
    pub key: Option<Key>,
    /// The name says `loop`.
    pub looped: bool,
    /// The name says `one shot`, `oneshot` or `shot`.
    pub one_shot: bool,
}

/// The words of a file name, lowercased: split at anything that is not a
/// letter or digit, between a lowercase and an uppercase letter (`HiHat`),
/// and between letters and digits (`124bpm`). The extension is dropped.
pub fn tokens(filename: &str) -> Vec<String> {
    let stem = strip_extension(filename);
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for ch in stem.chars() {
        if !ch.is_alphanumeric() {
            flush(&mut current, &mut out);
            previous = None;
            continue;
        }
        if let Some(before) = previous {
            let case_edge = before.is_lowercase() && ch.is_uppercase();
            let digit_edge = before.is_ascii_digit() != ch.is_ascii_digit();
            if case_edge || digit_edge {
                flush(&mut current, &mut out);
            }
        }
        current.extend(ch.to_lowercase());
        previous = Some(ch);
    }
    flush(&mut current, &mut out);
    out
}

fn flush(current: &mut String, out: &mut Vec<String>) {
    if !current.is_empty() {
        out.push(std::mem::take(current));
    }
}

fn strip_extension(filename: &str) -> &str {
    match filename.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty()
                && (1..=5).contains(&extension.len())
                && extension.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            stem
        }
        _ => filename,
    }
}

/// Reads a file name for tags, tempo, key and loop or one-shot wording.
pub fn name_hints(filename: &str) -> NameHints {
    let words = tokens(filename);
    let mut tags: BTreeSet<&'static str> = BTreeSet::new();
    for (index, word) in words.iter().enumerate() {
        // "bass drum" is a kick, not a bass.
        let bass_drum = word == "bass"
            && words
                .get(index + 1)
                .is_some_and(|next| next == "drum" || next == "drums");
        if bass_drum {
            tags.extend(["kick", "drums"]);
            continue;
        }
        for (names, add) in KEYWORDS {
            if names.contains(&word.as_str()) {
                tags.extend(add.iter().copied());
            }
        }
    }
    let looped = tags.contains("loop");
    let one_shot = words.iter().any(|word| word == "oneshot" || word == "shot");
    NameHints {
        bpm: name_bpm(&words, looped),
        key: name_key(strip_extension(filename)),
        tags: tags.into_iter().collect(),
        looped,
        one_shot,
    }
}

fn plausible_bpm(word: &str) -> Option<f64> {
    let value: u32 = word.parse().ok()?;
    (40..=250).contains(&value).then_some(f64::from(value))
}

fn name_bpm(words: &[String], looped: bool) -> Option<f64> {
    let marked = words.windows(2).find_map(|pair| {
        if pair[1] == "bpm" {
            plausible_bpm(&pair[0])
        } else if pair[0] == "bpm" {
            plausible_bpm(&pair[1])
        } else {
            None
        }
    });
    if marked.is_some() || !looped {
        return marked;
    }
    // "Drum Loop 124": a lone number in dance-music range.
    let mut candidates = words
        .iter()
        .filter_map(|word| plausible_bpm(word))
        .filter(|bpm| (70.0..=180.0).contains(bpm));
    match (candidates.next(), candidates.next()) {
        (Some(bpm), None) => Some(bpm),
        _ => None,
    }
}

fn name_key(stem: &str) -> Option<Key> {
    stem.split(|c: char| c.is_whitespace() || "_-.()[],".contains(c))
        .find_map(key_word)
}

/// `Am`, `F#min`, `Bbmaj`, `Cminor`: an uppercase note, an optional
/// accidental and a mode.
fn key_word(word: &str) -> Option<Key> {
    let mut chars = word.chars();
    let note = chars.next().filter(|c| ('A'..='G').contains(c))?;
    let rest = chars.as_str();
    let (accidental, mode) = match rest.chars().next() {
        Some('#') => ("#", &rest[1..]),
        Some('b') => ("b", &rest[1..]),
        _ => ("", rest),
    };
    let mode = match mode {
        "m" | "min" | "minor" => "minor",
        "maj" | "major" => "major",
        _ => return None,
    };
    format!("{note}{accidental} {mode}").parse().ok()
}

/// Brightness above which a sound is tagged `bright`, in Hz of spectral
/// centroid.
const BRIGHT_HZ: f32 = 5_000.0;
/// Centroid below which a sound is tagged `dark`.
const DARK_HZ: f32 = 400.0;
/// Spectral flatness above which a sound counts as noise rather than tone.
const NOISY_FLATNESS: f32 = 0.015;

/// Whether the features look like noise (hats, claps, snares) rather than
/// something with a pitch.
pub fn is_noisy(features: &FeatureVector) -> bool {
    features.flatness > NOISY_FLATNESS
}

/// A guess at the instrument of a one-shot from its features alone. The
/// thresholds were set on fourfloor's own drum and synth voices, so this
/// is a rough sorter, not a classifier: toms land in `bass`, and a rim
/// shot or cowbell gets no guess.
pub fn guess_instrument(features: &FeatureVector) -> Option<&'static str> {
    let centroid = features.centroid_hz;
    if is_noisy(features) {
        if centroid > 8_000.0 {
            Some("hat")
        } else if centroid > 1_000.0 {
            // A clap is noise all the way through; a snare has a pitched
            // body under its noise, so its spectrum is less flat.
            Some(if features.flatness >= 0.028 {
                "clap"
            } else {
                "snare"
            })
        } else {
            None
        }
    } else if centroid < 110.0 && features.duration_seconds < 1.5 {
        Some("kick")
    } else if centroid < 300.0 {
        Some("bass")
    } else if features.duration_seconds >= 2.0 {
        // Long, tonal and above the bass: a held chord or texture.
        Some("pad")
    } else {
        None
    }
}

/// A guess at what a loop is from its features: noise-like loops are
/// drums, low ones are bass. Anything else gets no guess.
pub fn guess_loop(features: &FeatureVector) -> Option<&'static str> {
    if is_noisy(features) {
        Some("drums")
    } else if features.centroid_hz < 300.0 {
        Some("bass")
    } else {
        None
    }
}

/// Tags that describe the sound whatever it is: `bright`, `dark`, `noisy`.
pub fn character_tags(features: &FeatureVector) -> Vec<&'static str> {
    let mut tags = Vec::new();
    if features.centroid_hz > BRIGHT_HZ {
        tags.push("bright");
    } else if features.centroid_hz < DARK_HZ {
        tags.push("dark");
    }
    if is_noisy(features) {
        tags.push("noisy");
    }
    tags
}

/// The tags the scanner gives a file: the words of its name, an
/// instrument guess from the features when a one-shot's name has none,
/// and the character tags. Sorted, lowercase, without repeats.
pub fn auto_tags(filename: &str, features: &FeatureVector, kind: FileKind) -> Vec<String> {
    let hints = name_hints(filename);
    let mut tags: BTreeSet<&'static str> = hints.tags.iter().copied().collect();
    let named = tags.iter().any(|tag| INSTRUMENT_TAGS.contains(tag));
    if !named && kind == FileKind::OneShot {
        if let Some(guess) = guess_instrument(features) {
            tags.insert(guess);
            if UNPITCHED_TAGS.contains(&guess) {
                tags.insert("drums");
            }
        }
    }
    if !named && kind == FileKind::Loop {
        if let Some(guess) = guess_loop(features) {
            tags.insert(guess);
        }
    }
    if kind == FileKind::Loop {
        tags.insert("loop");
    } else {
        // A long file called "... loop" is still not a loop.
        tags.remove("loop");
    }
    tags.extend(character_tags(features));
    tags.into_iter().map(str::to_owned).collect()
}

/// Whether any of `tags` names unpitched percussion.
pub fn is_unpitched<S: AsRef<str>>(tags: &[S]) -> bool {
    tags.iter()
        .any(|tag| UNPITCHED_TAGS.contains(&tag.as_ref()))
}

/// A tag name as stored: trimmed and lowercased. `None` if nothing is
/// left.
pub fn normalise_tag(name: &str) -> Option<String> {
    let cleaned = name.trim().to_lowercase();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_at_punctuation_case_and_digits() {
        assert_eq!(
            tokens("FF Acid Bass Loop 124bpm Am.wav"),
            ["ff", "acid", "bass", "loop", "124", "bpm", "am"]
        );
        assert_eq!(
            tokens("HiHat_Closed-01.flac"),
            ["hi", "hat", "closed", "01"]
        );
        assert_eq!(tokens("KICK.WAV"), ["kick"]);
        assert_eq!(tokens("no extension here"), ["no", "extension", "here"]);
        assert_eq!(
            tokens("v1.2 final mix.aiff"),
            ["v", "1", "2", "final", "mix"]
        );
    }

    #[test]
    fn instrument_words_become_tags() {
        assert_eq!(name_hints("Kick_909.wav").tags, ["drums", "kick"]);
        assert_eq!(name_hints("HiHat Open.wav").tags, ["drums", "hat"]);
        assert_eq!(name_hints("BD 01.wav").tags, ["drums", "kick"]);
        assert_eq!(name_hints("Bass Drum 3.wav").tags, ["drums", "kick"]);
        assert_eq!(name_hints("Sub Bass C.wav").tags, ["bass"]);
        assert_eq!(name_hints("Vox Chop 7.wav").tags, ["vocal"]);
        assert_eq!(name_hints("clap.wav").tags, ["clap", "drums"]);
        assert_eq!(name_hints("Organ Stab.wav").tags, ["keys", "organ", "stab"]);
        // Words inside other words are not matched.
        assert!(name_hints("Subway Ambience.wav").tags.is_empty());
        assert!(name_hints("Chatter.wav").tags.is_empty());
    }

    #[test]
    fn tempo_is_read_only_where_it_is_marked() {
        assert_eq!(name_hints("Groove 124bpm.wav").bpm, Some(124.0));
        assert_eq!(name_hints("groove_BPM_128.wav").bpm, Some(128.0));
        assert_eq!(name_hints("Drum Loop 126.wav").bpm, Some(126.0));
        // No "loop" and no "bpm": a number is just a number.
        assert_eq!(name_hints("Kick 124.wav").bpm, None);
        // Two candidates: no guess.
        assert_eq!(name_hints("Loop 120 128.wav").bpm, None);
        assert_eq!(name_hints("Loop 909.wav").bpm, None);
        assert_eq!(name_hints("999bpm.wav").bpm, None);
    }

    #[test]
    fn keys_need_a_mode_to_be_believed() {
        assert_eq!(name_hints("Stab Loop Am.wav").key, Some(Key::minor(9)));
        assert_eq!(name_hints("Pad_F#min_long.wav").key, Some(Key::minor(6)));
        assert_eq!(name_hints("Chords Bbmaj.wav").key, Some(Key::major(10)));
        assert_eq!(name_hints("Bass (Cminor).wav").key, Some(Key::minor(0)));
        assert_eq!(name_hints("Bass Note A.wav").key, None);
        assert_eq!(name_hints("Take A1.wav").key, None);
        assert_eq!(name_hints("Dm7 Chord.wav").key, None);
        assert_eq!(name_hints("am radio.wav").key, None);
    }

    #[test]
    fn loop_and_one_shot_wording_is_noticed() {
        assert!(name_hints("Top Loop 01.wav").looped);
        assert!(!name_hints("Loopy.wav").looped);
        assert!(name_hints("Snare OneShot.wav").one_shot);
        assert!(name_hints("Snare One Shot.wav").one_shot);
    }

    fn features(centroid_hz: f32, flatness: f32, duration_seconds: f64) -> FeatureVector {
        FeatureVector {
            tempo_bpm: None,
            duration_seconds,
            centroid_hz,
            rolloff_hz: centroid_hz * 1.5,
            flatness,
            zero_crossing_rate: centroid_hz,
            mfcc_mean: [0.0; 12],
            mfcc_std: [0.0; 12],
            chroma: [0.0; 12],
        }
    }

    #[test]
    fn features_fill_in_when_the_name_is_silent() {
        let kick = features(80.0, 0.0, 0.6);
        let hat = features(15_000.0, 0.1, 0.3);
        assert_eq!(
            auto_tags("sample_001.wav", &kick, FileKind::OneShot),
            ["dark", "drums", "kick"]
        );
        assert_eq!(
            auto_tags("sample_002.wav", &hat, FileKind::OneShot),
            ["bright", "drums", "hat", "noisy"]
        );
        // The name wins over the features.
        assert_eq!(
            auto_tags("Sub Bass.wav", &kick, FileKind::OneShot),
            ["bass", "dark"]
        );
        // Loops get a coarse guess; tracks get none.
        assert_eq!(
            auto_tags("x.wav", &kick, FileKind::Loop),
            ["bass", "dark", "loop"]
        );
        assert_eq!(
            auto_tags("x.wav", &hat, FileKind::Loop),
            ["bright", "drums", "loop", "noisy"]
        );
        assert_eq!(auto_tags("x loop.wav", &kick, FileKind::Track), ["dark"]);
    }

    #[test]
    fn tag_names_are_normalised() {
        assert_eq!(
            normalise_tag("  Deep House "),
            Some("deep house".to_owned())
        );
        assert_eq!(normalise_tag("   "), None);
        assert!(is_unpitched(&["bass", "hat"]));
        assert!(!is_unpitched(&["bass", "pad"]));
    }
}
