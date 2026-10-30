//! Humanizing MIDI drums: a rule- and statistics-based stand-in for
//! GrooVAE.
//!
//! GrooVAE (Magenta; Apache-2.0 weights trained on the CC BY 4.0 Groove
//! MIDI Dataset) learns how drummers place and weight notes. This module
//! models the parts of that behaviour that are well documented, with
//! hand-set numbers rather than learned ones:
//!
//! - **Timing** is a slow random walk (AR(1), so neighbouring notes drift
//!   together like a real player's time) plus a little per-note jitter,
//!   with a per-instrument spread (hats loosest, kick tightest) and an
//!   optional "laid back" backbeat.
//! - **Velocity** follows the metrical position (downbeats strongest, then
//!   beats, eighths, sixteenths), plus noise, with alternating hand
//!   strength on hats.
//! - **Swing** delays every second sixteenth.
//!
//! [`learn_groove`] goes the other way: it averages the timing and velocity
//! of a played part per grid step into a [`GrooveTemplate`] the edit model
//! can apply. Running the trained GrooVAE model is future work.

use fourfloor_core::{Tempo, Ticks, TICKS_PER_BEAT};
use fourfloor_model::{GrooveStep, GrooveTemplate, MidiNote};
use serde::{Deserialize, Serialize};

use crate::error::MlError;

const SIXTEENTH: i64 = TICKS_PER_BEAT / 4;

/// Settings of [`humanize`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HumanizeOptions {
    /// Overall strength, 0 (no change) to 1.
    pub amount: f64,
    /// Standard deviation of timing for the loosest instrument, in ms.
    pub timing_ms: f64,
    /// How strongly the timing error carries over from note to note, 0 to
    /// below 1. Higher drifts more slowly.
    pub drift: f64,
    /// Standard deviation of velocity noise, in MIDI steps.
    pub velocity_spread: f64,
    /// Depth of the metrical accents, in MIDI steps between a downbeat and
    /// an off-sixteenth.
    pub accent: f64,
    /// Delay of snares and claps on beats 2 and 4, in ms (positive is late).
    pub laid_back_ms: f64,
    /// Swing: where the second sixteenth of each pair falls, 0.5 (straight)
    /// to 0.75.
    pub swing: f64,
    /// Seed of the random generator; the same seed gives the same result.
    pub seed: u64,
}

impl Default for HumanizeOptions {
    fn default() -> Self {
        Self {
            amount: 1.0,
            timing_ms: 8.0,
            drift: 0.6,
            velocity_spread: 6.0,
            accent: 18.0,
            laid_back_ms: 6.0,
            swing: 0.5,
            seed: 0x5eed,
        }
    }
}

impl HumanizeOptions {
    fn validate(&self) -> Result<(), MlError> {
        let ok =
            |value: f64, low: f64, high: f64| value.is_finite() && (low..=high).contains(&value);
        if ok(self.amount, 0.0, 1.0)
            && ok(self.timing_ms, 0.0, 60.0)
            && ok(self.drift, 0.0, 0.99)
            && ok(self.velocity_spread, 0.0, 40.0)
            && ok(self.accent, 0.0, 60.0)
            && ok(self.laid_back_ms, -40.0, 40.0)
            && ok(self.swing, 0.5, 0.75)
        {
            Ok(())
        } else {
            Err(MlError::InvalidInput(
                "humanize options out of range (amount 0..1, timing 0..60 ms, drift 0..0.99, \
                 velocity spread 0..40, accent 0..60, laid back -40..40 ms, swing 0.5..0.75)"
                    .to_owned(),
            ))
        }
    }
}

/// Instrument families, by General MIDI drum pitch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Kick,
    Snare,
    Hat,
    Other,
}

impl Family {
    fn of(pitch: u8) -> Self {
        match pitch {
            35 | 36 => Self::Kick,
            37..=40 => Self::Snare,
            42 | 44 | 46 | 51 | 53 | 59 => Self::Hat,
            _ => Self::Other,
        }
    }

    /// Timing spread relative to the loosest family.
    fn looseness(self) -> f64 {
        match self {
            Self::Kick => 0.45,
            Self::Snare => 0.65,
            Self::Hat => 1.0,
            Self::Other => 0.8,
        }
    }
}

/// A small deterministic generator (SplitMix64) with Gaussian samples.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (core::f64::consts::TAU * v).cos()
    }
}

/// Accent, -1 (weakest) to 1, of a position in sixteenths within the bar.
fn metrical_weight(sixteenth_in_bar: i64) -> f64 {
    match sixteenth_in_bar.rem_euclid(16) {
        0 => 1.0,
        8 => 0.6,
        4 | 12 => 0.3,
        2 | 6 | 10 | 14 => -0.2,
        _ => -1.0,
    }
}

/// Humanizes `notes` (clip notes, starts relative to the clip) at `tempo`.
/// The result is sorted by start then pitch; no note starts before 0 and
/// every velocity stays in 1..=127.
pub fn humanize(
    notes: &[MidiNote],
    tempo: Tempo,
    options: &HumanizeOptions,
) -> Result<Vec<MidiNote>, MlError> {
    options.validate()?;
    let mut sorted = notes.to_vec();
    sorted.sort_by_key(|note| (note.start, note.pitch));
    if options.amount == 0.0 {
        return Ok(sorted);
    }
    let ticks_per_ms = TICKS_PER_BEAT as f64 / (tempo.seconds_per_beat() * 1000.0);
    let mut rng = Rng(options.seed);
    let innovation = (1.0 - options.drift * options.drift).sqrt();
    let mut walk = 0.0f64;
    let mut hat_hand = 0usize;
    let mut previous_start: Option<Ticks> = None;
    let mut out = Vec::with_capacity(sorted.len());
    for note in &sorted {
        let family = Family::of(note.pitch);
        let start = note.start.get();
        // One walk step per distinct onset, so a chord moves together.
        if previous_start != Some(note.start) {
            walk = options.drift * walk + innovation * rng.gaussian();
            previous_start = Some(note.start);
        }
        let sixteenth = (start + SIXTEENTH / 2).div_euclid(SIXTEENTH);
        let on_grid = (start - sixteenth * SIXTEENTH).abs() <= SIXTEENTH / 8;
        let in_bar = sixteenth.rem_euclid(16);

        let mut shift_ms =
            options.timing_ms * family.looseness() * (0.8 * walk + 0.6 * rng.gaussian());
        if family == Family::Snare && on_grid && (in_bar == 4 || in_bar == 12) {
            shift_ms += options.laid_back_ms;
        }
        let mut shift = shift_ms * ticks_per_ms;
        if on_grid && sixteenth.rem_euclid(2) == 1 {
            shift += (options.swing - 0.5) * 2.0 * SIXTEENTH as f64;
        }
        let new_start = (start as f64 + options.amount * shift).round().max(0.0) as i64;

        let mut velocity_change = options.accent / 2.0 * metrical_weight(in_bar)
            + options.velocity_spread * rng.gaussian();
        if family == Family::Hat {
            velocity_change += if hat_hand.is_multiple_of(2) {
                4.0
            } else {
                -4.0
            };
            hat_hand += 1;
        }
        let velocity = (f64::from(note.velocity) + options.amount * velocity_change)
            .round()
            .clamp(1.0, 127.0) as u8;
        out.push(MidiNote {
            start: Ticks::new(new_start),
            velocity,
            ..*note
        });
    }
    out.sort_by_key(|note| (note.start, note.pitch));
    Ok(out)
}

/// Learns a groove from a played part: for each step of a `steps`-step
/// cycle of `step_ticks`, the mean timing offset of the notes nearest that
/// step (as a fraction of a step) and their mean velocity relative to the
/// whole part. Steps with no notes stay neutral.
pub fn learn_groove(
    name: &str,
    notes: &[MidiNote],
    step_ticks: i64,
    steps: usize,
) -> Result<GrooveTemplate, MlError> {
    if step_ticks <= 0 || steps == 0 || steps > fourfloor_model::groove::MAX_GROOVE_STEPS {
        return Err(MlError::InvalidInput(format!(
            "a groove needs a positive step and 1 to {} steps",
            fourfloor_model::groove::MAX_GROOVE_STEPS
        )));
    }
    if notes.is_empty() {
        return Err(MlError::InvalidInput("no notes to learn from".to_owned()));
    }
    let mean_velocity = notes
        .iter()
        .map(|note| f64::from(note.velocity))
        .sum::<f64>()
        / notes.len() as f64;
    let mut timing = vec![(0.0f64, 0usize); steps];
    let mut velocity = vec![(0.0f64, 0usize); steps];
    for note in notes {
        let start = note.start.get();
        let index = (start + step_ticks / 2).div_euclid(step_ticks);
        let offset = (start - index * step_ticks) as f64 / step_ticks as f64;
        let step = index.rem_euclid(steps as i64) as usize;
        timing[step].0 += offset;
        timing[step].1 += 1;
        velocity[step].0 += f64::from(note.velocity);
        velocity[step].1 += 1;
    }
    let template = GrooveTemplate {
        name: name.to_owned(),
        step_ticks,
        steps: timing
            .iter()
            .zip(&velocity)
            .map(|(&(t, n), &(v, _))| {
                if n == 0 {
                    GrooveStep::NEUTRAL
                } else {
                    GrooveStep {
                        timing: (t / n as f64).clamp(-0.5, 0.5),
                        velocity: (v / n as f64 / mean_velocity.max(1.0)).clamp(0.0, 2.0),
                    }
                }
            })
            .collect(),
    };
    template
        .validate()
        .map_err(|error| MlError::InvalidInput(error.to_string()))?;
    Ok(template)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic_and_roughly_normal() {
        let mut a = Rng(1);
        let mut b = Rng(1);
        assert_eq!(a.next_u64(), b.next_u64());
        let mut rng = Rng(42);
        let samples: Vec<f64> = (0..20_000).map(|_| rng.gaussian()).collect();
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
        assert!(mean.abs() < 0.03, "mean {mean}");
        assert!((var - 1.0).abs() < 0.05, "variance {var}");
    }

    #[test]
    fn metrical_weights_rank_downbeat_beat_eighth_sixteenth() {
        assert!(metrical_weight(0) > metrical_weight(8));
        assert!(metrical_weight(8) > metrical_weight(4));
        assert!(metrical_weight(4) > metrical_weight(2));
        assert!(metrical_weight(2) > metrical_weight(1));
        assert_eq!(metrical_weight(16), metrical_weight(0));
    }

    #[test]
    fn bad_options_are_refused() {
        let tempo = Tempo::new(124.0).expect("tempo");
        let bad = HumanizeOptions {
            swing: 0.9,
            ..HumanizeOptions::default()
        };
        assert!(humanize(&[], tempo, &bad).is_err());
        let nan = HumanizeOptions {
            amount: f64::NAN,
            ..HumanizeOptions::default()
        };
        assert!(humanize(&[], tempo, &nan).is_err());
        assert!(learn_groove("x", &[], 240, 16).is_err());
    }
}
