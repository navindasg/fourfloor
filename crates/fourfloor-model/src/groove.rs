//! Groove templates: per-step timing and velocity offsets that repeat along
//! a grid, the way a drum machine's shuffle or a sampled groove does.

use fourfloor_core::{Ticks, TICKS_PER_BEAT};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::MidiNote;
use crate::error::ModelError;

/// Most steps one template may hold.
pub const MAX_GROOVE_STEPS: usize = 64;

/// What a groove does to notes on one step of its cycle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrooveStep {
    /// How far the step moves, as a fraction of one step: -0.5 (early) to
    /// 0.5 (late). 0 leaves it on the grid.
    pub timing: f64,
    /// Velocity multiplier, 0 to 2. 1 leaves the velocity alone.
    pub velocity: f64,
}

impl GrooveStep {
    /// A step the groove does not touch.
    pub const NEUTRAL: Self = Self {
        timing: 0.0,
        velocity: 1.0,
    };
}

/// A repeating pattern of per-step timing and velocity offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrooveTemplate {
    /// Display name.
    pub name: String,
    /// Length of one step in ticks, 1 to 3840. 240 is a sixteenth note.
    pub step_ticks: i64,
    /// The cycle, 1 to 64 steps. It repeats from the start of the clip.
    pub steps: Vec<GrooveStep>,
}

/// Names [`GrooveTemplate::builtin`] knows.
pub const BUILTIN_GROOVES: &[&str] = &[
    "straight",
    "swing_16_54",
    "swing_16_58",
    "swing_16_62",
    "swing_16_66",
    "swing_8_62",
    "house_push",
];

impl GrooveTemplate {
    /// A two-step swing: the second step of each pair is delayed so that it
    /// falls `percent` of the way through the pair (50 is straight).
    pub fn swing(name: &str, step_ticks: i64, percent: f64) -> Self {
        Self {
            name: name.to_owned(),
            step_ticks,
            steps: vec![
                GrooveStep::NEUTRAL,
                GrooveStep {
                    timing: (percent / 50.0 - 1.0).clamp(-0.5, 0.5),
                    velocity: 1.0,
                },
            ],
        }
    }

    /// A template that ships with fourfloor, by name (see
    /// [`BUILTIN_GROOVES`]).
    ///
    /// - `straight`: no change.
    /// - `swing_16_NN`: drum-machine swing on sixteenths at NN percent.
    /// - `swing_8_62`: swing on eighths.
    /// - `house_push`: sixteenths with strong downbeats, a late and softer
    ///   second and fourth sixteenth and a slightly early offbeat.
    pub fn builtin(name: &str) -> Option<Self> {
        let sixteenth = TICKS_PER_BEAT / 4;
        let step = |timing, velocity| GrooveStep { timing, velocity };
        match name {
            "straight" => Some(Self {
                name: name.to_owned(),
                step_ticks: sixteenth,
                steps: vec![GrooveStep::NEUTRAL],
            }),
            "swing_16_54" => Some(Self::swing(name, sixteenth, 54.0)),
            "swing_16_58" => Some(Self::swing(name, sixteenth, 58.0)),
            "swing_16_62" => Some(Self::swing(name, sixteenth, 62.0)),
            "swing_16_66" => Some(Self::swing(name, sixteenth, 66.0)),
            "swing_8_62" => Some(Self::swing(name, TICKS_PER_BEAT / 2, 62.0)),
            "house_push" => Some(Self {
                name: name.to_owned(),
                step_ticks: sixteenth,
                steps: vec![
                    step(0.0, 1.1),
                    step(0.12, 0.8),
                    step(-0.04, 1.0),
                    step(0.12, 0.85),
                ],
            }),
            _ => None,
        }
    }

    /// Checks the ranges documented on the fields.
    pub fn validate(&self) -> Result<(), ModelError> {
        let invalid = |reason: String| ModelError::InvalidValue {
            field: "groove",
            reason,
        };
        if !(1..=TICKS_PER_BEAT * 4).contains(&self.step_ticks) {
            return Err(invalid(format!(
                "step_ticks {} is outside 1 to {}",
                self.step_ticks,
                TICKS_PER_BEAT * 4
            )));
        }
        if !(1..=MAX_GROOVE_STEPS).contains(&self.steps.len()) {
            return Err(invalid(format!(
                "a groove has 1 to {MAX_GROOVE_STEPS} steps"
            )));
        }
        self.steps.iter().enumerate().try_for_each(|(index, step)| {
            let timing_ok = step.timing.is_finite() && (-0.5..=0.5).contains(&step.timing);
            let velocity_ok = step.velocity.is_finite() && (0.0..=2.0).contains(&step.velocity);
            if timing_ok && velocity_ok {
                Ok(())
            } else {
                Err(invalid(format!(
                    "step {index} needs timing in -0.5..=0.5 and velocity in 0..=2"
                )))
            }
        })
    }

    /// The step of the cycle a time belongs to: the nearest grid line.
    /// The velocity multiplier the template gives a note at `time`.
    pub fn velocity_at(&self, time: Ticks) -> f64 {
        self.step_at(time).velocity
    }

    fn step_at(&self, time: Ticks) -> GrooveStep {
        if self.steps.is_empty() || self.step_ticks <= 0 {
            return GrooveStep::NEUTRAL;
        }
        let index = (time.get() + self.step_ticks / 2).div_euclid(self.step_ticks);
        self.steps[index.rem_euclid(self.steps.len() as i64) as usize]
    }

    /// How far a note at `time` moves, in ticks, at full strength.
    pub fn shift_ticks(&self, time: Ticks) -> i64 {
        (self.step_at(time).timing * self.step_ticks as f64).round() as i64
    }

    /// Applies the groove to one note. `amount` scales both the timing and
    /// the velocity change: 0 does nothing, 1 is the full template.
    /// The start never goes below zero and the velocity stays in 1 to 127.
    pub fn apply(&self, note: &MidiNote, amount: f64) -> MidiNote {
        let amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let step = self.step_at(note.start);
        let shift = (step.timing * self.step_ticks as f64 * amount).round() as i64;
        let scale = 1.0 + (step.velocity - 1.0) * amount;
        let velocity = (f64::from(note.velocity) * scale).round().clamp(1.0, 127.0) as u8;
        MidiNote {
            start: Ticks::new((note.start.get() + shift).max(0)),
            velocity,
            ..*note
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, velocity: u8) -> MidiNote {
        MidiNote {
            start: Ticks::new(start),
            length: Ticks::new(120),
            pitch: 42,
            velocity,
        }
    }

    #[test]
    fn every_builtin_exists_and_is_valid() {
        for name in BUILTIN_GROOVES {
            let groove = GrooveTemplate::builtin(name).unwrap();
            assert_eq!(groove.name, *name);
            assert_eq!(groove.validate(), Ok(()));
        }
        assert_eq!(GrooveTemplate::builtin("nope"), None);
    }

    #[test]
    fn swing_delays_the_off_step_by_the_percentage() {
        // 58% of a 480-tick pair is 278.4: the off sixteenth moves from 240
        // to 278 (0.16 of a step is 38.4 ticks).
        let groove = GrooveTemplate::builtin("swing_16_58").unwrap();
        assert_eq!(groove.shift_ticks(Ticks::new(0)), 0);
        assert_eq!(groove.shift_ticks(Ticks::new(240)), 38);
        assert_eq!(groove.shift_ticks(Ticks::new(480)), 0);
        assert_eq!(groove.shift_ticks(Ticks::new(720)), 38);
        assert_eq!(groove.apply(&note(240, 100), 1.0).start, Ticks::new(278));
        assert_eq!(groove.apply(&note(240, 100), 0.5).start, Ticks::new(259));
        assert_eq!(groove.apply(&note(240, 100), 0.0), note(240, 100));
        // A note a little off the grid keeps its own offset.
        assert_eq!(groove.apply(&note(250, 100), 1.0).start, Ticks::new(288));
        let eighths = GrooveTemplate::builtin("swing_8_62").unwrap();
        assert_eq!(eighths.shift_ticks(Ticks::new(480)), 115);
    }

    #[test]
    fn velocity_is_scaled_and_kept_in_range() {
        let groove = GrooveTemplate::builtin("house_push").unwrap();
        assert_eq!(groove.apply(&note(0, 100), 1.0).velocity, 110);
        assert_eq!(groove.apply(&note(240, 100), 1.0).velocity, 80);
        assert_eq!(groove.apply(&note(240, 100), 0.5).velocity, 90);
        assert_eq!(groove.apply(&note(0, 127), 1.0).velocity, 127);
        assert_eq!(groove.apply(&note(480, 100), 1.0).start, Ticks::new(470));
        let silent = GrooveTemplate {
            steps: vec![GrooveStep {
                timing: -0.5,
                velocity: 0.0,
            }],
            ..groove
        };
        let applied = silent.apply(&note(0, 100), 1.0);
        assert_eq!((applied.start, applied.velocity), (Ticks::ZERO, 1));
    }

    #[test]
    fn out_of_range_templates_are_refused() {
        let good = GrooveTemplate::builtin("swing_16_58").unwrap();
        let bad = [
            GrooveTemplate {
                step_ticks: 0,
                ..good.clone()
            },
            GrooveTemplate {
                steps: Vec::new(),
                ..good.clone()
            },
            GrooveTemplate {
                steps: vec![GrooveStep {
                    timing: 0.75,
                    velocity: 1.0,
                }],
                ..good.clone()
            },
            GrooveTemplate {
                steps: vec![GrooveStep {
                    timing: 0.0,
                    velocity: f64::NAN,
                }],
                ..good.clone()
            },
        ];
        for groove in bad {
            assert!(matches!(
                groove.validate(),
                Err(ModelError::InvalidValue {
                    field: "groove",
                    ..
                })
            ));
        }
    }
}
