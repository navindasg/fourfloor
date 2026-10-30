//! Step sequencing and swing.
//!
//! Swing is baked into note positions when a pattern is written. The engine
//! plays notes where they are stored, so swing is never applied twice.
//!
//! The grid is in 4/4: sixteen sixteenth-note steps per bar.

use fourfloor_core::{ClipId, Ticks, TrackId, TICKS_PER_BEAT};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::bus::CommandBus;
use crate::clip::MidiNote;
use crate::command::{CreateTrack, WriteMidiClip};
use crate::error::ModelError;
use crate::project::{Groove, SwingGrid};
use crate::track::TrackKind;
use crate::validate::{self, MAX_SWING_PERCENT, MIN_SWING_PERCENT};

/// Steps in one 4/4 bar.
pub const STEPS_PER_BAR: usize = 16;
/// Ticks in one sixteenth-note step.
pub const STEP_TICKS: i64 = TICKS_PER_BEAT / 4;
/// Ticks in one 4/4 bar.
pub const BAR_TICKS: i64 = TICKS_PER_BEAT * 4;
/// Longest pattern [`write_house_pattern`] writes, in bars.
pub const MAX_PATTERN_BARS: u32 = 64;

/// General MIDI bass drum 1.
pub const GM_KICK: u8 = 36;
/// General MIDI closed hi-hat.
pub const GM_CLOSED_HAT: u8 = 42;
/// General MIDI open hi-hat.
pub const GM_OPEN_HAT: u8 = 46;

impl SwingGrid {
    /// Length of one step of this grid, in ticks.
    pub const fn step_ticks(self) -> i64 {
        match self {
            Self::Eighth => TICKS_PER_BEAT / 2,
            Self::Sixteenth => TICKS_PER_BEAT / 4,
        }
    }
}

/// Where the off step of a swung pair lands, in ticks from the pair's start.
///
/// A pair is two grid steps. At 50% the off step sits halfway (straight
/// time); at 66.7% it sits two thirds of the way (triplet feel). The result
/// is rounded to the nearest tick. Percentages outside 50 to 75 are clamped.
pub fn swing_offset_ticks(groove: &Groove) -> i64 {
    let pair = groove.grid.step_ticks() * 2;
    let percent = if groove.swing_percent.is_finite() {
        groove
            .swing_percent
            .clamp(MIN_SWING_PERCENT, MAX_SWING_PERCENT)
    } else {
        MIN_SWING_PERCENT
    };
    // Bounded by `pair`, so the cast cannot truncate.
    (pair as f64 * percent / 100.0).round() as i64
}

/// Moves a straight-time position to its swung position.
///
/// Positions on the first step of a pair stay put and the off step moves to
/// [`swing_offset_ticks`]. Positions in between are stretched in proportion,
/// so order is preserved.
pub fn apply_swing(time: Ticks, groove: &Groove) -> Ticks {
    let step = groove.grid.step_ticks();
    let pair = step * 2;
    let offset = swing_offset_ticks(groove);
    let pair_start = time.get().div_euclid(pair) * pair;
    let within = time.get().rem_euclid(pair);
    let swung = if within < step {
        scale(within, offset, step)
    } else {
        offset + scale(within - step, pair - offset, step)
    };
    Ticks::new(pair_start + swung)
}

/// `value * numerator / denominator`, rounded to nearest. All inputs are
/// small non-negative tick counts.
fn scale(value: i64, numerator: i64, denominator: i64) -> i64 {
    (value * numerator + denominator / 2) / denominator
}

/// One row of a step sequencer: a pitch and which sixteenth steps play it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepLane {
    /// MIDI pitch, 0 to 127.
    pub pitch: u8,
    /// MIDI velocity, 1 to 127.
    pub velocity: u8,
    /// Length of each note in ticks.
    #[schemars(with = "i64")]
    pub note_length: Ticks,
    /// One flag per sixteenth step; `true` plays a note on that step.
    pub steps: Vec<bool>,
    /// Per-step velocities, 1 to 127, for accents. A step with no entry
    /// here, or with 0, uses `velocity`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub velocities: Vec<u8>,
}

impl StepLane {
    /// The velocity of one step: its accent if it has one, else the lane's.
    pub fn step_velocity(&self, step: usize) -> u8 {
        match self.velocities.get(step).copied() {
            Some(accent) if accent > 0 => accent,
            _ => self.velocity,
        }
    }

    /// The lane's notes with `groove` applied to their start times.
    pub fn notes(&self, groove: &Groove) -> Vec<MidiNote> {
        self.notes_for(self.steps.len(), groove)
    }

    /// The notes of `total_steps` steps, repeating the lane's pattern (and
    /// its accents) as often as needed, with `groove` applied.
    pub fn notes_for(&self, total_steps: usize, groove: &Groove) -> Vec<MidiNote> {
        if self.steps.is_empty() {
            return Vec::new();
        }
        (0..total_steps)
            .filter(|step| self.steps[step % self.steps.len()])
            .map(|step| MidiNote {
                start: apply_swing(Ticks::new(step as i64 * STEP_TICKS), groove),
                length: self.note_length,
                pitch: self.pitch,
                velocity: self.step_velocity(step % self.steps.len()),
            })
            .collect()
    }
}

/// A basic house drum pattern: four-on-the-floor kick and offbeat hats.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HousePattern {
    /// Pattern length in 4/4 bars, 1 to 64.
    pub bars: u32,
    /// Where the clips start, in ticks from the project start.
    #[schemars(with = "i64")]
    pub start: Ticks,
    /// Pitch of the kick, played on every beat.
    pub kick_pitch: u8,
    /// Velocity of the kick.
    pub kick_velocity: u8,
    /// Pitch of the open hat, played on every offbeat eighth.
    pub open_hat_pitch: u8,
    /// Velocity of the open hat.
    pub open_hat_velocity: u8,
    /// Pitch of closed hats on the remaining sixteenths, or null for none.
    pub closed_hat_pitch: Option<u8>,
    /// Velocity of the closed hats.
    pub closed_hat_velocity: u8,
    /// Swing for this pattern. Null uses the project's groove setting.
    pub swing: Option<Groove>,
    /// MIDI track for the kick clip. Null creates a track named "Kick".
    #[schemars(with = "Option<u64>")]
    pub kick_track: Option<TrackId>,
    /// MIDI track for the hat clip. Null creates a track named "Hats".
    #[schemars(with = "Option<u64>")]
    pub hat_track: Option<TrackId>,
}

impl Default for HousePattern {
    fn default() -> Self {
        Self {
            bars: 1,
            start: Ticks::ZERO,
            kick_pitch: GM_KICK,
            kick_velocity: 120,
            open_hat_pitch: GM_OPEN_HAT,
            open_hat_velocity: 100,
            closed_hat_pitch: Some(GM_CLOSED_HAT),
            closed_hat_velocity: 70,
            swing: None,
            kick_track: None,
            hat_track: None,
        }
    }
}

impl HousePattern {
    fn step_count(&self) -> usize {
        self.bars as usize * STEPS_PER_BAR
    }

    fn lane(&self, pitch: u8, velocity: u8, plays: impl Fn(usize) -> bool) -> StepLane {
        StepLane {
            pitch,
            velocity,
            note_length: Ticks::new(STEP_TICKS),
            steps: (0..self.step_count()).map(plays).collect(),
            velocities: Vec::new(),
        }
    }

    /// Kick notes: one on every beat.
    pub fn kick_notes(&self, groove: &Groove) -> Vec<MidiNote> {
        self.lane(self.kick_pitch, self.kick_velocity, |step| step % 4 == 0)
            .notes(groove)
    }

    /// Hat notes: an open hat on every offbeat eighth, plus closed hats on
    /// the odd sixteenths when `closed_hat_pitch` is set. Sorted by start.
    pub fn hat_notes(&self, groove: &Groove) -> Vec<MidiNote> {
        let open = self.lane(self.open_hat_pitch, self.open_hat_velocity, |step| {
            step % 4 == 2
        });
        let closed = self
            .closed_hat_pitch
            .map(|pitch| self.lane(pitch, self.closed_hat_velocity, |step| step % 2 == 1));
        let notes: Vec<MidiNote> = open
            .notes(groove)
            .into_iter()
            .chain(closed.iter().flat_map(|lane| lane.notes(groove)))
            .collect();
        validate::sorted_notes(&notes)
    }
}

/// The tracks and clips [`write_house_pattern`] wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct HousePatternWritten {
    /// Track holding the kick clip.
    pub kick_track: TrackId,
    /// The new kick clip.
    pub kick_clip: ClipId,
    /// Track holding the hat clip.
    pub hat_track: TrackId,
    /// The new hat clip.
    pub hat_clip: ClipId,
}

/// Writes a house pattern into two new MIDI clips through the command bus.
///
/// Everything happens in one transaction, so a single undo removes the
/// clips and any tracks that were created for them.
pub fn write_house_pattern(
    bus: &mut CommandBus,
    pattern: &HousePattern,
) -> Result<HousePatternWritten, ModelError> {
    if !(1..=MAX_PATTERN_BARS).contains(&pattern.bars) {
        return Err(ModelError::InvalidValue {
            field: "bars",
            reason: format!("{} is outside 1 to {MAX_PATTERN_BARS}", pattern.bars),
        });
    }
    let groove = pattern.swing.unwrap_or(bus.project().groove);
    validate::groove(&groove)?;
    let length = Ticks::new(i64::from(pattern.bars) * BAR_TICKS);
    let kick_notes = pattern.kick_notes(&groove);
    let hat_notes = pattern.hat_notes(&groove);

    bus.transaction("write_house_pattern", |tx| {
        let mut voice = |existing: Option<TrackId>, name: &str, notes: Vec<MidiNote>| {
            let track = match existing {
                Some(track) => track,
                None => tx
                    .execute(CreateTrack {
                        name: name.to_owned(),
                        kind: TrackKind::Midi,
                        index: None,
                    })?
                    .track()
                    .ok_or(ModelError::IdExhausted)?,
            };
            let clip = tx
                .execute(WriteMidiClip {
                    track,
                    clip: None,
                    name: name.to_owned(),
                    start: pattern.start,
                    length,
                    notes,
                })?
                .clip()
                .ok_or(ModelError::IdExhausted)?;
            Ok::<_, ModelError>((track, clip))
        };
        let (kick_track, kick_clip) = voice(pattern.kick_track, "Kick", kick_notes)?;
        let (hat_track, hat_clip) = voice(pattern.hat_track, "Hats", hat_notes)?;
        Ok(HousePatternWritten {
            kick_track,
            kick_clip,
            hat_track,
            hat_clip,
        })
    })
}

#[cfg(test)]
mod tests {
    use fourfloor_core::{SampleRate, Tempo};

    use super::*;
    use crate::command::SetSwing;
    use crate::project::Project;

    fn groove(swing_percent: f64, grid: SwingGrid) -> Groove {
        Groove {
            swing_percent,
            grid,
        }
    }

    fn starts(notes: &[MidiNote], pitch: u8) -> Vec<i64> {
        notes
            .iter()
            .filter(|note| note.pitch == pitch)
            .map(|note| note.start.get())
            .collect()
    }

    #[test]
    fn straight_swing_is_the_identity() {
        for grid in [SwingGrid::Eighth, SwingGrid::Sixteenth] {
            let straight = groove(50.0, grid);
            assert_eq!(swing_offset_ticks(&straight), grid.step_ticks());
            for tick in -2000..4000 {
                assert_eq!(apply_swing(Ticks::new(tick), &straight), Ticks::new(tick));
            }
        }
    }

    #[test]
    fn swing_offset_matches_the_percentage() {
        // Sixteenth pairs are 480 ticks long, eighth pairs 960.
        assert_eq!(swing_offset_ticks(&groove(56.0, SwingGrid::Sixteenth)), 269);
        assert_eq!(
            swing_offset_ticks(&groove(200.0 / 3.0, SwingGrid::Sixteenth)),
            320
        );
        assert_eq!(swing_offset_ticks(&groove(75.0, SwingGrid::Sixteenth)), 360);
        assert_eq!(swing_offset_ticks(&groove(60.0, SwingGrid::Eighth)), 576);
        assert_eq!(
            swing_offset_ticks(&groove(200.0 / 3.0, SwingGrid::Eighth)),
            640
        );
    }

    #[test]
    fn out_of_range_swing_is_clamped() {
        assert_eq!(swing_offset_ticks(&groove(10.0, SwingGrid::Sixteenth)), 240);
        assert_eq!(swing_offset_ticks(&groove(99.0, SwingGrid::Sixteenth)), 360);
        assert_eq!(
            swing_offset_ticks(&groove(f64::NAN, SwingGrid::Sixteenth)),
            240
        );
    }

    #[test]
    fn sixteenth_swing_moves_only_the_odd_sixteenths() {
        let shuffle = groove(60.0, SwingGrid::Sixteenth);
        let swung: Vec<i64> = (0..8)
            .map(|step| apply_swing(Ticks::new(step * STEP_TICKS), &shuffle).get())
            .collect();
        // Off steps land 288 ticks into each 480-tick pair instead of 240.
        assert_eq!(swung, [0, 288, 480, 768, 960, 1248, 1440, 1728]);
    }

    #[test]
    fn eighth_swing_moves_the_offbeat_and_keeps_the_beat() {
        let shuffle = groove(62.5, SwingGrid::Eighth);
        assert_eq!(apply_swing(Ticks::new(0), &shuffle), Ticks::new(0));
        assert_eq!(apply_swing(Ticks::new(480), &shuffle), Ticks::new(600));
        assert_eq!(apply_swing(Ticks::new(960), &shuffle), Ticks::new(960));
        assert_eq!(apply_swing(Ticks::new(1440), &shuffle), Ticks::new(1560));
        // Sixteenths in between are stretched in proportion.
        assert_eq!(apply_swing(Ticks::new(240), &shuffle), Ticks::new(300));
        assert_eq!(apply_swing(Ticks::new(720), &shuffle), Ticks::new(780));
    }

    #[test]
    fn swing_preserves_order_and_handles_negative_time() {
        let shuffle = groove(75.0, SwingGrid::Sixteenth);
        let swung: Vec<Ticks> = (-960..1920)
            .map(|tick| apply_swing(Ticks::new(tick), &shuffle))
            .collect();
        assert!(swung.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(apply_swing(Ticks::new(-480), &shuffle), Ticks::new(-480));
        assert_eq!(apply_swing(Ticks::new(-240), &shuffle), Ticks::new(-120));
    }

    #[test]
    fn swung_sixteenth_lands_on_the_expected_sample() {
        // 120 BPM at 48 kHz: one beat is 24000 frames, one tick 25 frames.
        let tempo = Tempo::new(120.0).unwrap();
        let rate = SampleRate::HZ_48000;
        let shuffle = groove(60.0, SwingGrid::Sixteenth);
        let straight = Ticks::new(STEP_TICKS).to_sample_pos(tempo, rate);
        let swung = apply_swing(Ticks::new(STEP_TICKS), &shuffle).to_sample_pos(tempo, rate);
        assert_eq!(straight, 6_000);
        assert_eq!(swung, 7_200);
        // 60% of an eighth note (12000 frames) is 7200 frames.
        assert_eq!(swung, (12_000.0_f64 * 0.60).round() as u64);
    }

    #[test]
    fn step_lane_writes_one_note_per_active_step() {
        let lane = StepLane {
            pitch: 60,
            velocity: 90,
            note_length: Ticks::new(120),
            steps: vec![true, false, false, true],
            velocities: Vec::new(),
        };
        let notes = lane.notes(&groove(60.0, SwingGrid::Sixteenth));
        assert_eq!(starts(&notes, 60), [0, 768]);
        assert!(notes.iter().all(|note| note.length == Ticks::new(120)));
        assert!(notes.iter().all(|note| note.velocity == 90));
    }

    #[test]
    fn step_lane_repeats_its_pattern_and_accents() {
        let lane = StepLane {
            pitch: 42,
            velocity: 70,
            note_length: Ticks::new(120),
            steps: vec![true, true, false],
            velocities: vec![110, 0],
        };
        let notes = lane.notes_for(7, &Groove::STRAIGHT);
        assert_eq!(starts(&notes, 42), [0, 240, 720, 960, 1440]);
        let velocities: Vec<u8> = notes.iter().map(|note| note.velocity).collect();
        assert_eq!(velocities, [110, 70, 110, 70, 110]);
        let empty = StepLane {
            steps: Vec::new(),
            ..lane
        };
        assert!(empty.notes_for(16, &Groove::STRAIGHT).is_empty());
    }

    #[test]
    fn kick_is_four_on_the_floor() {
        let pattern = HousePattern {
            bars: 2,
            ..HousePattern::default()
        };
        let notes = pattern.kick_notes(&groove(62.0, SwingGrid::Sixteenth));
        let expected: Vec<i64> = (0..8).map(|beat| beat * TICKS_PER_BEAT).collect();
        assert_eq!(starts(&notes, GM_KICK), expected);
    }

    #[test]
    fn straight_hats_sit_on_the_offbeats() {
        let pattern = HousePattern::default();
        let notes = pattern.hat_notes(&Groove::STRAIGHT);
        assert_eq!(starts(&notes, GM_OPEN_HAT), [480, 1440, 2400, 3360]);
        assert_eq!(
            starts(&notes, GM_CLOSED_HAT),
            [240, 720, 1200, 1680, 2160, 2640, 3120, 3600]
        );
    }

    #[test]
    fn eighth_swing_delays_the_offbeat_hats() {
        let pattern = HousePattern {
            closed_hat_pitch: None,
            ..HousePattern::default()
        };
        let notes = pattern.hat_notes(&groove(60.0, SwingGrid::Eighth));
        // 60% of a 960-tick beat is 576.
        assert_eq!(starts(&notes, GM_OPEN_HAT), [576, 1536, 2496, 3456]);
        assert_eq!(notes.len(), 4);
    }

    #[test]
    fn sixteenth_swing_shuffles_closed_hats_but_not_offbeat_hats() {
        let pattern = HousePattern::default();
        let notes = pattern.hat_notes(&groove(56.0, SwingGrid::Sixteenth));
        assert_eq!(starts(&notes, GM_OPEN_HAT), [480, 1440, 2400, 3360]);
        assert_eq!(
            starts(&notes, GM_CLOSED_HAT),
            [269, 749, 1229, 1709, 2189, 2669, 3149, 3629]
        );
        assert!(notes.windows(2).all(|pair| pair[0].start <= pair[1].start));
    }

    #[test]
    fn write_house_pattern_creates_tracks_and_clips_in_one_undo_step() {
        let mut bus = CommandBus::new(Project::new("House"));
        bus.execute(SetSwing {
            percent: 60.0,
            grid: Some(SwingGrid::Eighth),
        })
        .unwrap();
        let before = bus.project().clone();
        let pattern = HousePattern {
            bars: 4,
            ..HousePattern::default()
        };
        let written = write_house_pattern(&mut bus, &pattern).unwrap();

        let project = bus.project();
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.track(written.kick_track).unwrap().name, "Kick");
        assert_eq!(project.track(written.hat_track).unwrap().name, "Hats");
        let (_, kick) = project.find_clip(written.kick_clip).unwrap();
        let (_, hats) = project.find_clip(written.hat_clip).unwrap();
        assert_eq!(kick.length, Ticks::new(4 * BAR_TICKS));
        assert_eq!(kick.notes().len(), 16);
        // The project groove (60% on eighths) was used: first offbeat at 576.
        assert_eq!(starts(hats.notes(), GM_OPEN_HAT)[0], 576);
        assert_eq!(project.validate(), Ok(()));

        assert_eq!(bus.undo().unwrap().label, "write_house_pattern");
        assert_eq!(bus.project(), &before);
        bus.redo().unwrap();
        assert!(bus.project().find_clip(written.hat_clip).is_some());
    }

    #[test]
    fn write_house_pattern_reuses_given_tracks_and_explicit_swing() {
        let mut bus = CommandBus::new(Project::new("House"));
        let drums = bus
            .execute(CreateTrack {
                name: "Drums".to_owned(),
                kind: TrackKind::Midi,
                index: None,
            })
            .unwrap()
            .track()
            .unwrap();
        let pattern = HousePattern {
            start: Ticks::new(BAR_TICKS),
            swing: Some(groove(75.0, SwingGrid::Sixteenth)),
            kick_track: Some(drums),
            hat_track: Some(drums),
            ..HousePattern::default()
        };
        let written = write_house_pattern(&mut bus, &pattern).unwrap();
        assert_eq!((written.kick_track, written.hat_track), (drums, drums));
        let track = bus.project().track(drums).unwrap();
        assert_eq!(track.clips.len(), 2);
        assert!(track
            .clips
            .iter()
            .all(|clip| clip.start == Ticks::new(BAR_TICKS)));
        let (_, hats) = bus.project().find_clip(written.hat_clip).unwrap();
        assert_eq!(starts(hats.notes(), GM_CLOSED_HAT)[0], 360);
    }

    #[test]
    fn write_house_pattern_rejects_bad_input_without_changing_the_project() {
        let mut bus = CommandBus::new(Project::new("House"));
        let before = bus.project().clone();

        let no_bars = HousePattern {
            bars: 0,
            ..HousePattern::default()
        };
        assert!(matches!(
            write_house_pattern(&mut bus, &no_bars),
            Err(ModelError::InvalidValue { field: "bars", .. })
        ));

        let wild_swing = HousePattern {
            swing: Some(groove(90.0, SwingGrid::Sixteenth)),
            ..HousePattern::default()
        };
        assert_eq!(
            write_house_pattern(&mut bus, &wild_swing),
            Err(ModelError::InvalidSwing { percent: 90.0 })
        );

        // The kick track is created first, then the missing hat track fails:
        // the transaction must roll the new kick track back.
        let missing_track = HousePattern {
            hat_track: Some(TrackId::new(404)),
            ..HousePattern::default()
        };
        assert_eq!(
            write_house_pattern(&mut bus, &missing_track),
            Err(ModelError::TrackNotFound(TrackId::new(404)))
        );

        let bad_pitch = HousePattern {
            kick_pitch: 200,
            ..HousePattern::default()
        };
        assert!(matches!(
            write_house_pattern(&mut bus, &bad_pitch),
            Err(ModelError::InvalidNote { index: 0, .. })
        ));

        assert_eq!(bus.project(), &before);
        assert!(!bus.can_undo());
    }
}
