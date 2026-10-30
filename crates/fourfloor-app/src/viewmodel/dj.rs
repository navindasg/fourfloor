//! DJ view logic: what deck gestures mean as [`DjCommand`]s, and the
//! "what mixes next" list (key and tempo from the harmonic helper, blended
//! with sound similarity from the library).

use fourfloor_analysis::Key;
use fourfloor_dj::{rank_compatible, DeckState, DjCommand, KeyMatch, LibraryTrack, MatchOptions};
use fourfloor_library::{FileId, FileRecord};

/// Pitch fader ranges, in percent.
pub const PITCH_RANGES: [f64; 3] = [8.0, 16.0, 50.0];
/// Loop lengths offered, in beats.
pub const LOOP_BEATS: [f64; 6] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
/// Beat jump sizes offered, in beats.
pub const JUMP_BEATS: [f64; 3] = [4.0, 16.0, 32.0];
/// Share of a suggestion's score that comes from sound similarity when the
/// library has features for both files.
pub const SIMILARITY_WEIGHT: f64 = 0.3;

/// The pitch fader position (-1 top to 1 bottom, as on a CDJ where down is
/// faster) for `percent` within `range`.
pub fn pitch_fader_position(percent: f64, range: f64) -> f64 {
    if range <= 0.0 {
        return 0.0;
    }
    (percent / range).clamp(-1.0, 1.0)
}

/// The pitch command for fader `position` (-1..1) within `range` percent.
pub fn pitch_command(deck: usize, position: f64, range: f64) -> DjCommand {
    let percent = (position.clamp(-1.0, 1.0) * range).clamp(-50.0, 100.0);
    // Round to a hundredth of a percent so a fader at rest is exactly 0.
    DjCommand::SetPitch {
        deck,
        percent: (percent * 100.0).round() / 100.0,
    }
}

/// A hot cue pad press: jump to (or set) the cue, or clear it with
/// `clear` (shift-click).
pub fn hot_cue_command(deck: usize, slot: usize, clear: bool) -> DjCommand {
    if clear {
        DjCommand::ClearHotCue { deck, slot }
    } else {
        DjCommand::HotCue { deck, slot }
    }
}

/// Length of the active loop in beats, if any and the tempo is known.
pub fn active_loop_beats(state: &DeckState) -> Option<f64> {
    let active = state.loop_state?;
    let bpm = state.track.as_ref()?.bpm?;
    Some((active.end_seconds - active.start_seconds) * bpm / 60.0)
}

/// A loop button: sets a loop of `beats`, or exits when that loop is on.
pub fn loop_command(state: &DeckState, beats: f64) -> DjCommand {
    let deck = state.index;
    match active_loop_beats(state) {
        Some(current) if (current - beats).abs() < 1e-3 => DjCommand::LoopExit { deck },
        _ => DjCommand::BeatLoop { deck, beats },
    }
}

/// `m:ss.t` for a time in seconds.
pub fn clock(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0).floor() as u64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// One row of the "what mixes next" list.
#[derive(Clone, Debug, PartialEq)]
pub struct NextTrack {
    /// The library file.
    pub file: FileId,
    /// Its title.
    pub title: String,
    /// Its tempo.
    pub bpm: Option<f64>,
    /// Its key.
    pub key: Option<Key>,
    /// Combined score, 0 to 1.
    pub score: f64,
    /// How the keys relate.
    pub key_match: KeyMatch,
    /// Pitch change that matches the tempo, percent.
    pub tempo_change_percent: Option<f64>,
    /// Sound similarity, when the library has it.
    pub similarity: Option<f32>,
}

/// Ranks `library` as follow-ups to `state`'s track: key (Camelot) and
/// tempo through the DJ crate's helper, blended with sound similarity
/// (`similarity`, by file) when present. The deck's own file is left out.
pub fn rank_next(
    state: &DeckState,
    own: Option<FileId>,
    library: &[FileRecord],
    similarity: &[(FileId, f32)],
    limit: usize,
) -> Vec<NextTrack> {
    let Some(track) = &state.track else {
        return Vec::new();
    };
    let key = track
        .key
        .as_deref()
        .and_then(|name| name.parse::<Key>().ok());
    let bpm = state.bpm.or(track.bpm);
    let candidates: Vec<&FileRecord> = library
        .iter()
        .filter(|record| Some(record.id) != own && !record.missing)
        .collect();
    let tracks: Vec<LibraryTrack> = candidates
        .iter()
        .map(|record| {
            LibraryTrack::new(
                record.id.0.to_string(),
                &record.title,
                record.bpm,
                record.key,
            )
        })
        .collect();
    let mut ranked: Vec<NextTrack> = rank_compatible(bpm, key, &tracks, &MatchOptions::default())
        .into_iter()
        .filter_map(|suggestion| {
            let record = candidates.get(suggestion.index)?;
            let similar = similarity
                .iter()
                .find(|(id, _)| *id == record.id)
                .map(|(_, score)| *score);
            let score = match similar {
                Some(similar) => {
                    (1.0 - SIMILARITY_WEIGHT) * suggestion.score
                        + SIMILARITY_WEIGHT * f64::from(similar.clamp(0.0, 1.0))
                }
                None => suggestion.score,
            };
            Some(NextTrack {
                file: record.id,
                title: record.title.clone(),
                bpm: record.bpm,
                key: record.key,
                score,
                key_match: suggestion.key_match,
                tempo_change_percent: suggestion.tempo_change_percent,
                similarity: similar,
            })
        })
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_dj::{CrossfaderSide, LoopState, SyncMode, TrackSummary};
    use fourfloor_library::{FileKind, Tag};
    use std::path::PathBuf;

    fn deck(bpm: Option<f64>, key: Option<&str>) -> DeckState {
        DeckState {
            index: 1,
            track: Some(TrackSummary {
                name: "Playing".to_owned(),
                path: None,
                bpm,
                key: key.map(str::to_owned),
                camelot: None,
                duration_seconds: 300.0,
            }),
            playing: true,
            at_end: false,
            position_seconds: 10.0,
            beat: Some(20.0),
            pitch_percent: 0.0,
            rate: 1.0,
            bpm,
            key_lock: false,
            sync: SyncMode::Off,
            quantize: true,
            cue_seconds: 0.0,
            hot_cues: vec![None; 8],
            loop_state: None,
            eq_db: [0.0; 3],
            eq_kill: [false; 3],
            filter: 0.0,
            fader: 1.0,
            trim_db: 0.0,
            cue_monitor: false,
            crossfader_side: CrossfaderSide::B,
            effects: Vec::new(),
            stems: Vec::new(),
        }
    }

    fn record(id: i64, title: &str, bpm: f64, key: Key) -> FileRecord {
        FileRecord {
            id: FileId(id),
            path: PathBuf::from(format!("/music/{title}.wav")),
            filename: format!("{title}.wav"),
            title: title.to_owned(),
            content_hash: String::new(),
            size_bytes: 1,
            mtime_ns: 0,
            kind: FileKind::Track,
            duration_seconds: 300.0,
            sample_rate: 44_100,
            channels: 2,
            bpm: Some(bpm),
            bpm_confidence: None,
            first_beat_seconds: None,
            key: Some(key),
            key_confidence: None,
            loudness_lufs: None,
            true_peak_dbtp: None,
            tags: Vec::<Tag>::new(),
            rating: 0,
            play_count: 0,
            last_played_at: None,
            missing: false,
            locked: false,
            added_at: 0,
            analyzed_at: 0,
        }
    }

    #[test]
    fn the_pitch_fader_maps_both_ways_and_rests_at_zero() {
        assert_eq!(pitch_fader_position(4.0, 8.0), 0.5);
        assert_eq!(pitch_fader_position(40.0, 8.0), 1.0);
        assert_eq!(
            pitch_command(0, 0.5, 16.0),
            DjCommand::SetPitch {
                deck: 0,
                percent: 8.0
            }
        );
        assert_eq!(
            pitch_command(0, 1e-6, 8.0),
            DjCommand::SetPitch {
                deck: 0,
                percent: 0.0
            }
        );
        assert_eq!(
            pitch_command(2, -1.0, 50.0),
            DjCommand::SetPitch {
                deck: 2,
                percent: -50.0
            }
        );
    }

    #[test]
    fn loop_buttons_toggle_their_own_length() {
        let mut state = deck(Some(120.0), None);
        assert_eq!(
            loop_command(&state, 4.0),
            DjCommand::BeatLoop {
                deck: 1,
                beats: 4.0
            }
        );
        state.loop_state = Some(LoopState {
            start_seconds: 10.0,
            end_seconds: 12.0,
            roll: false,
        });
        assert_eq!(active_loop_beats(&state), Some(4.0));
        assert_eq!(loop_command(&state, 4.0), DjCommand::LoopExit { deck: 1 });
        assert_eq!(
            loop_command(&state, 8.0),
            DjCommand::BeatLoop {
                deck: 1,
                beats: 8.0
            }
        );
        assert_eq!(
            hot_cue_command(1, 3, true),
            DjCommand::ClearHotCue { deck: 1, slot: 3 }
        );
    }

    #[test]
    fn clocks_read_minutes_seconds_and_tenths() {
        assert_eq!(clock(0.0), "0:00.0");
        assert_eq!(clock(65.37), "1:05.3");
        assert_eq!(clock(-3.0), "0:00.0");
    }

    #[test]
    fn suggestions_prefer_compatible_keys_and_near_tempos() {
        let state = deck(Some(124.0), Some("A minor"));
        let library = vec![
            record(1, "Clash", 124.0, Key::minor(3)),
            record(2, "Same key", 124.0, Key::minor(9)),
            record(3, "Relative far tempo", 100.0, Key::major(0)),
            record(4, "Fifth up", 125.0, Key::minor(4)),
            record(5, "Itself", 124.0, Key::minor(9)),
        ];
        let ranked = rank_next(&state, Some(FileId(5)), &library, &[], 10);
        assert_eq!(ranked.len(), 4, "the playing file is left out");
        assert_eq!(ranked[0].title, "Same key");
        assert_eq!(ranked[1].title, "Fifth up");
        assert_eq!(ranked.last().map(|t| t.title.as_str()), Some("Clash"));
        // Similarity lifts an otherwise equal candidate.
        let with_sound = rank_next(
            &state,
            None,
            &library[1..2]
                .iter()
                .cloned()
                .chain([record(6, "Twin", 124.0, Key::minor(9))])
                .collect::<Vec<_>>(),
            &[(FileId(6), 0.95), (FileId(2), 0.1)],
            10,
        );
        assert_eq!(with_sound[0].title, "Twin");
        assert_eq!(with_sound[0].similarity, Some(0.95));
        assert!(rank_next(&deck(None, None), None, &library, &[], 3).len() <= 3);
    }
}
