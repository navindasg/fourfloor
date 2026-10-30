//! Step sequencer editing: a MIDI clip read as rows of sixteenth steps,
//! edited, and written back with `write_step_pattern` (one undo step per
//! edit, with the pattern's swing and groove applied by the model).
//!
//! Reading back is approximate by design: the model bakes swing and
//! grooves into note times, so each note is rounded to its nearest step
//! (a swung off-sixteenth is at most half a step late, and rounds back).

use fourfloor_core::{ClipId, Ticks, TrackId};
use fourfloor_model::command::{GrooveSource, WriteStepPattern};
use fourfloor_model::sequencer::StepLane;
use fourfloor_model::{Clip, Groove, GrooveTemplate, SwingGrid};

use crate::geometry::{BAR_TICKS, SIXTEENTH_TICKS};

/// Steps per bar (sixteenths).
pub const STEPS_PER_BAR: usize = 16;
/// Lanes a new drum pattern starts with: kick, clap, closed and open hat.
pub const DEFAULT_LANES: [u8; 4] = [36, 39, 42, 46];
/// Every sound the built-in drum kit plays, in the order the lane menu
/// offers them (one pitch per voice).
pub const KIT_PITCHES: [u8; 12] = [36, 35, 38, 39, 37, 42, 44, 46, 41, 45, 48, 56];
/// Velocity of a newly set step.
pub const DEFAULT_VELOCITY: u8 = 100;
/// Most bars a pattern may have (the model's limit).
pub const MAX_BARS: u32 = 64;

/// A drum-style name for a General MIDI / 909 kit pitch.
pub fn drum_name(pitch: u8) -> &'static str {
    match pitch {
        35 => "Kick 808",
        36 => "Kick",
        37 => "Rim",
        38 | 40 => "Snare",
        39 => "Clap",
        42 => "Closed hat",
        44 => "Pedal hat",
        46 => "Open hat",
        41 | 43 => "Tom low",
        45 | 47 => "Tom mid",
        48 | 50 => "Tom high",
        56 => "Cowbell",
        _ => "",
    }
}

/// The step a note starting at `start` ticks belongs to (half a step late
/// still counts as this step).
pub fn step_of(start: i64) -> usize {
    ((start.max(0) + SIXTEENTH_TICKS / 2 - 1) / SIXTEENTH_TICKS) as usize
}

/// A clip as a step pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct StepPattern {
    /// Length in bars.
    pub bars: u32,
    /// One lane per pitch, lowest pitch first.
    pub lanes: Vec<StepLane>,
}

impl StepPattern {
    /// Total steps.
    pub fn steps(&self) -> usize {
        self.bars as usize * STEPS_PER_BAR
    }

    /// An empty pattern of `bars` with the default drum lanes.
    pub fn empty(bars: u32) -> Self {
        let bars = bars.clamp(1, MAX_BARS);
        let steps = bars as usize * STEPS_PER_BAR;
        Self {
            bars,
            lanes: DEFAULT_LANES
                .iter()
                .map(|pitch| new_lane(*pitch, steps))
                .collect(),
        }
    }

    /// Reads a clip. Every pitch in it gets a lane; the default drum lanes
    /// are added when missing.
    pub fn from_clip(clip: &Clip) -> Self {
        let bars = ((clip.length.get() + BAR_TICKS - 1) / BAR_TICKS).clamp(1, i64::from(MAX_BARS));
        let mut pattern = Self::empty(bars as u32);
        let steps = pattern.steps();
        for note in clip.notes() {
            let step = step_of(note.start.get());
            if step >= steps {
                continue;
            }
            let lane = match pattern
                .lanes
                .iter()
                .position(|lane| lane.pitch == note.pitch)
            {
                Some(index) => index,
                None => {
                    pattern.lanes.push(new_lane(note.pitch, steps));
                    pattern.lanes.len() - 1
                }
            };
            let lane = &mut pattern.lanes[lane];
            lane.steps[step] = true;
            lane.velocities[step] = note.velocity.max(1);
            lane.note_length = note.length.max(Ticks::new(1));
        }
        pattern.lanes.sort_by_key(|lane| lane.pitch);
        pattern
    }

    /// Undoes `groove`'s velocity scaling. The model bakes the groove into
    /// the notes, so a pattern read back with [`Self::from_clip`] already
    /// carries it; rewriting that pattern with the same groove would apply
    /// it again on every edit and drag the scaled steps towards silence.
    pub fn without_groove(&self, groove: &GrooveTemplate) -> Self {
        let lanes = self
            .lanes
            .iter()
            .map(|lane| {
                let velocities = lane
                    .velocities
                    .iter()
                    .enumerate()
                    .map(|(step, &velocity)| {
                        let at = Ticks::new(step as i64 * SIXTEENTH_TICKS);
                        let scale = groove.velocity_at(at);
                        if velocity == 0 || scale <= 0.0 {
                            velocity
                        } else {
                            (f64::from(velocity) / scale).round().clamp(1.0, 127.0) as u8
                        }
                    })
                    .collect();
                StepLane {
                    velocities,
                    ..lane.clone()
                }
            })
            .collect();
        Self {
            lanes,
            ..self.clone()
        }
    }

    /// Whether a step is on.
    pub fn is_on(&self, lane: usize, step: usize) -> bool {
        self.lanes
            .get(lane)
            .and_then(|lane| lane.steps.get(step))
            .copied()
            .unwrap_or(false)
    }

    /// The velocity a step plays at (0 when off).
    pub fn velocity(&self, lane: usize, step: usize) -> u8 {
        if !self.is_on(lane, step) {
            return 0;
        }
        self.lanes[lane].step_velocity(step)
    }

    /// Flips a step. Returns its new state.
    pub fn toggle(&mut self, lane: usize, step: usize) -> bool {
        let Some(row) = self.lanes.get_mut(lane) else {
            return false;
        };
        let Some(cell) = row.steps.get_mut(step) else {
            return false;
        };
        *cell = !*cell;
        if *cell && row.velocities[step] == 0 {
            row.velocities[step] = DEFAULT_VELOCITY;
        }
        *cell
    }

    /// Sets a step's velocity (1 to 127), turning it on.
    pub fn set_velocity(&mut self, lane: usize, step: usize, velocity: u8) {
        if let Some(row) = self.lanes.get_mut(lane) {
            if step < row.steps.len() {
                row.steps[step] = true;
                row.velocities[step] = velocity.clamp(1, 127);
            }
        }
    }

    /// This pattern with an (empty) lane for each of `pitches` it lacks.
    pub fn with_lanes(&self, pitches: &[u8]) -> Self {
        let mut grown = self.clone();
        for pitch in pitches {
            grown.add_lane(*pitch);
        }
        grown
    }

    /// Adds a lane for `pitch` if there is none. Returns its index.
    pub fn add_lane(&mut self, pitch: u8) -> usize {
        if let Some(index) = self.lanes.iter().position(|lane| lane.pitch == pitch) {
            return index;
        }
        let steps = self.steps();
        self.lanes.push(new_lane(pitch, steps));
        self.lanes.sort_by_key(|lane| lane.pitch);
        self.lanes
            .iter()
            .position(|lane| lane.pitch == pitch)
            .unwrap_or(0)
    }

    /// The command that writes this pattern. `clip` replaces that clip;
    /// `swing` overrides the project swing; `groove` names a built-in
    /// groove template.
    pub fn to_command(
        &self,
        track: TrackId,
        clip: Option<ClipId>,
        name: &str,
        start: Ticks,
        swing: Option<f64>,
        groove: Option<&str>,
    ) -> WriteStepPattern {
        WriteStepPattern {
            track,
            clip,
            name: name.to_owned(),
            start,
            bars: self.bars,
            lanes: self.lanes.clone(),
            swing: swing.map(|percent| Groove {
                swing_percent: percent.clamp(50.0, 75.0),
                grid: SwingGrid::Sixteenth,
            }),
            groove: groove.map(|name| GrooveSource::Builtin(name.to_owned())),
            groove_amount: 1.0,
        }
    }
}

fn new_lane(pitch: u8, steps: usize) -> StepLane {
    StepLane {
        pitch,
        velocity: DEFAULT_VELOCITY,
        note_length: Ticks::new(SIXTEENTH_TICKS - 40),
        steps: vec![false; steps],
        velocities: vec![0; steps],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::CreateTrack;
    use fourfloor_model::{CommandBus, Project, SwingGrid, TrackKind};

    fn bus_with_track() -> (CommandBus, TrackId) {
        let mut bus = CommandBus::new(Project::new("Steps"));
        let track = bus
            .execute(CreateTrack {
                name: "Drums".to_owned(),
                kind: TrackKind::Midi,
                index: None,
            })
            .expect("track")
            .track()
            .expect("id");
        (bus, track)
    }

    #[test]
    fn swung_steps_read_back_onto_their_own_step() {
        assert_eq!(step_of(0), 0);
        assert_eq!(step_of(240), 1);
        assert_eq!(step_of(240 + 119), 1, "75% swing is half a step late");
        assert_eq!(step_of(240 + 121), 2);
        assert_eq!(step_of(-30), 0);
    }

    #[test]
    fn toggling_writes_a_pattern_that_reads_back_the_same() {
        let (mut bus, track) = bus_with_track();
        let mut pattern = StepPattern::empty(1);
        for step in [0, 4, 8, 12] {
            assert!(pattern.toggle(0, step));
        }
        pattern.toggle(2, 2);
        pattern.set_velocity(2, 6, 60);
        // Write with heavy swing: off-sixteenths move late.
        let command = pattern.to_command(track, None, "Beat", Ticks::ZERO, Some(66.0), None);
        let clip = bus.execute(command).expect("written").clip().expect("clip");
        let (_, written) = bus.project().find_clip(clip).expect("clip exists");
        assert_eq!(written.notes().len(), 6);
        let back = StepPattern::from_clip(written);
        assert_eq!(back.bars, 1);
        for lane in 0..pattern.lanes.len() {
            for step in 0..16 {
                assert_eq!(
                    back.is_on(lane, step),
                    pattern.is_on(lane, step),
                    "{lane} {step}"
                );
            }
        }
        assert_eq!(back.velocity(2, 6), 60);
        assert_eq!(back.velocity(0, 0), DEFAULT_VELOCITY);
        // Rewriting the same clip replaces it in place.
        let mut edited = back.clone();
        assert!(!edited.toggle(0, 4));
        bus.execute(edited.to_command(
            track,
            Some(clip),
            "Beat",
            Ticks::ZERO,
            None,
            Some("house_push"),
        ))
        .expect("rewritten");
        let (_, rewritten) = bus.project().find_clip(clip).expect("still there");
        assert_eq!(rewritten.notes().len(), 5);
        assert_eq!(bus.project().tracks[0].clips.len(), 1);
    }

    #[test]
    fn repeated_edits_with_a_groove_do_not_shrink_velocities() {
        let (mut bus, track) = bus_with_track();
        let groove = GrooveTemplate::builtin("house_push").expect("builtin");
        let mut pattern = StepPattern::empty(1);
        for step in 0..16 {
            pattern.toggle(0, step);
        }
        let write = |bus: &mut CommandBus, pattern: &StepPattern, clip| {
            bus.execute(pattern.to_command(
                track,
                clip,
                "Beat",
                Ticks::ZERO,
                None,
                Some("house_push"),
            ))
            .expect("written")
            .clip()
            .expect("clip")
        };
        let clip = write(&mut bus, &pattern, None);
        let first: Vec<u8> = {
            let (_, written) = bus.project().find_clip(clip).expect("clip");
            written.notes().iter().map(|n| n.velocity).collect()
        };
        assert!(
            first.iter().any(|&v| v < DEFAULT_VELOCITY),
            "the groove softens some steps"
        );
        // Each edit reads the clip back the way the sequencer does and
        // rewrites it unchanged, like clicking an unrelated lane.
        for _ in 0..10 {
            let (_, written) = bus.project().find_clip(clip).expect("clip");
            let back = StepPattern::from_clip(written).without_groove(&groove);
            write(&mut bus, &back, Some(clip));
        }
        let (_, written) = bus.project().find_clip(clip).expect("clip");
        let last: Vec<u8> = written.notes().iter().map(|n| n.velocity).collect();
        assert_eq!(last, first, "velocities stay put across edits");
    }

    #[test]
    fn added_lanes_survive_a_read_back_and_every_kit_sound_is_named() {
        let (mut bus, track) = bus_with_track();
        let pattern = StepPattern::empty(1).with_lanes(&[38, 56]);
        assert_eq!(pattern.lanes.len(), DEFAULT_LANES.len() + 2);
        let command = pattern.to_command(track, None, "Beat", Ticks::ZERO, None, None);
        let clip = bus.execute(command).expect("written").clip().expect("clip");
        let (_, written) = bus.project().find_clip(clip).expect("clip");
        // The empty lanes have no notes, so the clip alone loses them...
        assert_eq!(
            StepPattern::from_clip(written).lanes.len(),
            DEFAULT_LANES.len()
        );
        // ...and the sequencer's remembered lanes put them back.
        let back = StepPattern::from_clip(written).with_lanes(&[38, 56]);
        assert!(back.lanes.iter().any(|l| l.pitch == 38));
        assert!(back.lanes.iter().any(|l| l.pitch == 56));
        assert_eq!(
            back.with_lanes(&[38]).lanes.len(),
            back.lanes.len(),
            "no duplicates"
        );
        for pitch in KIT_PITCHES {
            assert!(!drum_name(pitch).is_empty(), "{pitch} has a name");
        }
    }

    #[test]
    fn unknown_pitches_get_their_own_lane_and_names() {
        let mut pattern = StepPattern::empty(2);
        assert_eq!(pattern.steps(), 32);
        let index = pattern.add_lane(56);
        assert_eq!(pattern.lanes[index].pitch, 56);
        assert_eq!(pattern.add_lane(56), index, "no duplicate lanes");
        assert_eq!(drum_name(56), "Cowbell");
        assert_eq!(drum_name(42), "Closed hat");
        assert!(!pattern.toggle(99, 0), "out of range is ignored");
        let command = pattern.to_command(TrackId(1), None, "x", Ticks::ZERO, Some(90.0), None);
        let swing = command.swing.expect("swing");
        assert_eq!(swing.swing_percent, 75.0);
        assert_eq!(swing.grid, SwingGrid::Sixteenth);
    }
}
