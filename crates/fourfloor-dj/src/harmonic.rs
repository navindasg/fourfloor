//! Harmonic mixing helper: ranks library tracks by how well they mix with
//! the playing one, by key (Camelot wheel) and tempo.

use fourfloor_analysis::{HarmonicRelation, Key};
use serde::{Deserialize, Serialize};

/// A track the helper can suggest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryTrack {
    /// Caller's identifier (a path, a database id).
    pub id: String,
    /// Display title.
    pub title: String,
    /// Tempo, when known.
    pub bpm: Option<f64>,
    /// Key name or code (`"A minor"`, `"Am"`, `"8A"`), when known.
    pub key: Option<String>,
}

impl LibraryTrack {
    /// A library entry.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        bpm: Option<f64>,
        key: Option<Key>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            bpm,
            key: key.map(|key| key.name()),
        }
    }
}

/// How the ranking weighs key against tempo.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchOptions {
    /// Largest tempo difference, in percent, that still counts as a match
    /// (after half/double-time folding). Default 6.
    pub max_tempo_percent: f64,
    /// Weight of the key score (0 to 1); the tempo score gets the rest.
    /// Default 0.6.
    pub key_weight: f64,
    /// Whether 64 and 128 BPM count as the same tempo. Default true.
    pub fold_double_time: bool,
    /// Drop tracks whose key does not mix. Default false.
    pub compatible_only: bool,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            max_tempo_percent: 6.0,
            key_weight: 0.6,
            fold_double_time: true,
            compatible_only: false,
        }
    }
}

/// How a key relates, for the ranking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyMatch {
    /// Same key.
    Same,
    /// Relative major/minor.
    Relative,
    /// One step clockwise on the wheel.
    FifthUp,
    /// One step anticlockwise.
    FifthDown,
    /// Two steps round the wheel: an energy shift, usable with care.
    TwoSteps,
    /// Clashes.
    Clash,
    /// One of the keys is unknown.
    Unknown,
}

impl KeyMatch {
    fn of(playing: Option<Key>, candidate: Option<Key>) -> Self {
        let (Some(playing), Some(candidate)) = (playing, candidate) else {
            return Self::Unknown;
        };
        match playing.relation_to(candidate) {
            HarmonicRelation::Same => Self::Same,
            HarmonicRelation::Relative => Self::Relative,
            HarmonicRelation::FifthUp => Self::FifthUp,
            HarmonicRelation::FifthDown => Self::FifthDown,
            HarmonicRelation::Unrelated
                if playing.mode() == candidate.mode() && playing.wheel_distance(candidate) == 2 =>
            {
                Self::TwoSteps
            }
            HarmonicRelation::Unrelated => Self::Clash,
        }
    }

    /// Score from 0 (clash) to 1 (same key).
    pub fn score(self) -> f64 {
        match self {
            Self::Same => 1.0,
            Self::Relative => 0.9,
            Self::FifthUp | Self::FifthDown => 0.85,
            Self::TwoSteps => 0.4,
            Self::Unknown => 0.3,
            Self::Clash => 0.0,
        }
    }

    /// Whether the keys mix without clashing.
    pub fn is_compatible(self) -> bool {
        matches!(
            self,
            Self::Same | Self::Relative | Self::FifthUp | Self::FifthDown
        )
    }
}

/// One ranked suggestion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Index into the library slice.
    pub index: usize,
    /// The track's id.
    pub id: String,
    /// Overall score, 0 to 1 (higher mixes better).
    pub score: f64,
    /// How the keys relate.
    pub key_match: KeyMatch,
    /// Pitch change in percent that brings the candidate to the playing
    /// tempo (after folding), when both tempos are known.
    pub tempo_change_percent: Option<f64>,
    /// Semitones to shift the candidate so its key mixes (0 when it
    /// already does), when both keys are known.
    pub key_shift_semitones: Option<i32>,
}

/// Folds `candidate` by factors of two towards `reference`.
fn fold(reference: f64, candidate: f64) -> f64 {
    let mut folded = candidate;
    for _ in 0..4 {
        if folded / reference > 1.5 {
            folded /= 2.0;
        } else if reference / folded > 1.5 {
            folded *= 2.0;
        }
    }
    folded
}

/// Ranks `library` against a playing track of tempo `bpm` and `key`,
/// best first. Ties keep library order.
pub fn rank_compatible(
    bpm: Option<f64>,
    key: Option<Key>,
    library: &[LibraryTrack],
    options: &MatchOptions,
) -> Vec<Suggestion> {
    let key_weight = options.key_weight.clamp(0.0, 1.0);
    let limit = options.max_tempo_percent.max(0.01);
    let mut suggestions: Vec<Suggestion> = library
        .iter()
        .enumerate()
        .filter_map(|(index, track)| {
            let candidate_key = track
                .key
                .as_deref()
                .and_then(|name| name.parse::<Key>().ok());
            let key_match = KeyMatch::of(key, candidate_key);
            if options.compatible_only && !key_match.is_compatible() {
                return None;
            }
            let change = match (bpm, track.bpm) {
                (Some(reference), Some(candidate))
                    if reference > 0.0 && candidate > 0.0 && candidate.is_finite() =>
                {
                    let candidate = if options.fold_double_time {
                        fold(reference, candidate)
                    } else {
                        candidate
                    };
                    Some((reference / candidate - 1.0) * 100.0)
                }
                _ => None,
            };
            let tempo_score = change.map_or(0.3, |change| (1.0 - change.abs() / limit).max(0.0));
            let shift = key.zip(candidate_key).map(|(k, c)| k.shift_to_match(c));
            Some(Suggestion {
                index,
                id: track.id.clone(),
                score: key_weight * key_match.score() + (1.0 - key_weight) * tempo_score,
                key_match,
                tempo_change_percent: change,
                key_shift_semitones: shift,
            })
        })
        .collect();
    suggestions.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.index.cmp(&b.index)));
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, bpm: f64, key: &str) -> LibraryTrack {
        LibraryTrack {
            id: id.into(),
            title: id.into(),
            bpm: Some(bpm),
            key: Some(key.into()),
        }
    }

    #[test]
    fn ranks_same_and_neighbouring_keys_at_near_tempos_first() {
        let library = [
            track("clash", 124.0, "F# major"),
            track("relative", 124.0, "C major"),
            track("same-far", 140.0, "A minor"),
            track("same", 125.0, "8A"),
            track("fifth", 123.0, "Em"),
            track("double", 62.0, "A minor"),
        ];
        let ranked = rank_compatible(
            Some(124.0),
            Some(Key::minor(9)),
            &library,
            &MatchOptions::default(),
        );
        let order: Vec<&str> = ranked.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(order[0], "double", "{order:?}");
        assert_eq!(order[1], "same");
        assert_eq!(order[2], "relative");
        assert_eq!(order[3], "fifth");
        assert_eq!(*order.last().unwrap(), "clash");
        assert_eq!(ranked[0].tempo_change_percent, Some(0.0));
        let same = &ranked[1];
        assert!((same.tempo_change_percent.unwrap() + 0.8).abs() < 1e-9);
        assert_eq!(same.key_match, KeyMatch::Same);
        let clash = ranked.iter().find(|s| s.id == "clash").unwrap();
        assert_eq!(clash.key_match, KeyMatch::Clash);
        assert_ne!(clash.key_shift_semitones, Some(0));
        let only = rank_compatible(
            Some(124.0),
            Some(Key::minor(9)),
            &library,
            &MatchOptions {
                compatible_only: true,
                ..MatchOptions::default()
            },
        );
        assert!(only.iter().all(|s| s.key_match.is_compatible()));
        assert_eq!(only.len(), 5);
    }

    #[test]
    fn unknown_metadata_scores_low_but_is_kept() {
        let library = [LibraryTrack::new("x", "x", None, None)];
        let ranked = rank_compatible(Some(120.0), None, &library, &MatchOptions::default());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].key_match, KeyMatch::Unknown);
        assert!(ranked[0].score < 0.5);
    }
}
