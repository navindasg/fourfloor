//! House drum patterns: fixed rules for the parts that define the genre
//! (four-on-the-floor kick, backbeat clap, offbeat open hat) and Markov
//! chains for the parts that vary (closed hats, percussion, fills).

use fourfloor_core::Ticks;
use fourfloor_model::sequencer::{StepLane, STEPS_PER_BAR, STEP_TICKS};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{clamp_unit, GenerateError, MAX_BARS};
use crate::rng::RandomSource;

/// One drum sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DrumVoice {
    /// Bass drum.
    Kick,
    /// Hand clap.
    Clap,
    /// Snare drum.
    Snare,
    /// Closed hi-hat.
    ClosedHat,
    /// Open hi-hat.
    OpenHat,
    /// Rim shot.
    Rim,
    /// Tom.
    Tom,
    /// Cowbell.
    Cowbell,
}

impl DrumVoice {
    /// Every voice, in mixer order.
    pub const ALL: [Self; 8] = [
        Self::Kick,
        Self::Clap,
        Self::Snare,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Rim,
        Self::Tom,
        Self::Cowbell,
    ];

    /// The General MIDI pitch of the voice, which is also the pitch the
    /// fourfloor drum kit and hi-hat devices answer to.
    pub const fn gm_pitch(self) -> u8 {
        match self {
            Self::Kick => 36,
            Self::Rim => 37,
            Self::Snare => 38,
            Self::Clap => 39,
            Self::ClosedHat => 42,
            Self::Tom => 45,
            Self::OpenHat => 46,
            Self::Cowbell => 56,
        }
    }

    /// Display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kick => "Kick",
            Self::Clap => "Clap",
            Self::Snare => "Snare",
            Self::ClosedHat => "Closed Hat",
            Self::OpenHat => "Open Hat",
            Self::Rim => "Rim",
            Self::Tom => "Tom",
            Self::Cowbell => "Cowbell",
        }
    }
}

/// The flavour of house the pattern aims for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DrumStyle {
    /// Chicago-style: kick, clap, offbeat open hat, shuffling closed hats.
    #[default]
    Classic,
    /// Sparse and soft, with rim-shot syncopation.
    Deep,
    /// Busy sixteenth hats and tom syncopation.
    Tech,
    /// Classic plus ghost snares and cowbell.
    Jackin,
}

/// What to generate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrumOptions {
    /// Length in 4/4 bars, 1 to 64.
    pub bars: u32,
    /// The flavour.
    pub style: DrumStyle,
    /// How busy the varying parts are, 0 to 1. 0.5 is the style's default.
    pub density: f64,
    /// Whether the last bar of each eight-bar phrase (and the last bar of
    /// the pattern, if it has at least four) gets a fill.
    pub fills: bool,
}

impl Default for DrumOptions {
    fn default() -> Self {
        Self {
            bars: 4,
            style: DrumStyle::Classic,
            density: 0.5,
            fills: true,
        }
    }
}

/// One voice of a pattern: a velocity per sixteenth step, 0 for a rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrumLane {
    /// The sound.
    pub voice: DrumVoice,
    /// One entry per sixteenth step; 0 is a rest, 1 to 127 a hit.
    pub velocities: Vec<u8>,
}

impl DrumLane {
    /// The steps that play.
    pub fn hits(&self) -> Vec<usize> {
        self.velocities
            .iter()
            .enumerate()
            .filter(|(_, velocity)| **velocity > 0)
            .map(|(step, _)| step)
            .collect()
    }

    /// The lane as a step-sequencer row for
    /// [`fourfloor_model::command::WriteStepPattern`]. `pitch` overrides
    /// the voice's General MIDI pitch.
    pub fn to_step_lane(&self, pitch: Option<u8>) -> StepLane {
        StepLane {
            pitch: pitch.unwrap_or(self.voice.gm_pitch()),
            velocity: 100,
            note_length: Ticks::new(STEP_TICKS / 2),
            steps: self
                .velocities
                .iter()
                .map(|velocity| *velocity > 0)
                .collect(),
            velocities: self.velocities.clone(),
        }
    }
}

/// A generated drum pattern on the straight sixteenth grid. Swing is added
/// when the pattern is written into a clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrumPattern {
    /// Length in bars.
    pub bars: u32,
    /// The voices that play, in [`DrumVoice::ALL`] order. Voices with no
    /// hits are left out.
    pub lanes: Vec<DrumLane>,
}

impl DrumPattern {
    /// Number of sixteenth steps.
    pub fn steps(&self) -> usize {
        self.bars as usize * STEPS_PER_BAR
    }

    /// The lane of one voice, if it plays.
    pub fn lane(&self, voice: DrumVoice) -> Option<&DrumLane> {
        self.lanes.iter().find(|lane| lane.voice == voice)
    }

    /// The steps one voice plays; empty if it does not play.
    pub fn hits(&self, voice: DrumVoice) -> Vec<usize> {
        self.lane(voice).map(DrumLane::hits).unwrap_or_default()
    }
}

/// Per-style numbers for the varying parts.
struct StyleTable {
    clap_velocity: u8,
    open_hat_velocity: u8,
    /// Which offbeats (0 to 3) carry the open hat.
    open_hat_beats: [bool; 4],
    /// Closed hat chain: chance of a hit after a hit, and after a rest.
    hat_after_hit: f64,
    hat_after_rest: f64,
    /// Percussion voice and its base chance on a syncopated step.
    perc: Option<(DrumVoice, f64)>,
    /// Chance of a ghost snare on the last sixteenth before a backbeat.
    ghost_snare: f64,
}

fn style_table(style: DrumStyle) -> StyleTable {
    match style {
        DrumStyle::Classic => StyleTable {
            clap_velocity: 110,
            open_hat_velocity: 100,
            open_hat_beats: [true; 4],
            hat_after_hit: 0.5,
            hat_after_rest: 0.9,
            perc: Some((DrumVoice::Rim, 0.08)),
            ghost_snare: 0.0,
        },
        DrumStyle::Deep => StyleTable {
            clap_velocity: 94,
            open_hat_velocity: 84,
            open_hat_beats: [true; 4],
            hat_after_hit: 0.2,
            hat_after_rest: 0.7,
            perc: Some((DrumVoice::Rim, 0.3)),
            ghost_snare: 0.0,
        },
        DrumStyle::Tech => StyleTable {
            clap_velocity: 106,
            open_hat_velocity: 92,
            open_hat_beats: [true, false, true, true],
            hat_after_hit: 0.85,
            hat_after_rest: 0.95,
            perc: Some((DrumVoice::Tom, 0.3)),
            ghost_snare: 0.0,
        },
        DrumStyle::Jackin => StyleTable {
            clap_velocity: 112,
            open_hat_velocity: 100,
            open_hat_beats: [true; 4],
            hat_after_hit: 0.6,
            hat_after_rest: 0.85,
            perc: Some((DrumVoice::Cowbell, 0.2)),
            ghost_snare: 0.6,
        },
    }
}

/// How much a syncopated percussion hit suits each sixteenth of the bar.
/// Beats are left to the kick; the "a" of a beat and the push before the
/// bar line are the favourite spots.
const PERC_WEIGHTS: [f64; STEPS_PER_BAR] = [
    0.0, 0.3, 0.0, 1.0, 0.0, 0.2, 0.6, 1.0, 0.0, 0.5, 0.0, 1.0, 0.0, 0.4, 0.6, 0.8,
];

type Bar = [u8; STEPS_PER_BAR];

fn jitter(base: u8, spread: u8, rng: &mut dyn RandomSource) -> u8 {
    let offset = rng.below(usize::from(spread) * 2 + 1) as i32 - i32::from(spread);
    (i32::from(base) + offset).clamp(1, 127) as u8
}

fn closed_hat_bar(table: &StyleTable, open: &Bar, density: f64, rng: &mut dyn RandomSource) -> Bar {
    let scale = 0.5 + density;
    let mut bar = [0u8; STEPS_PER_BAR];
    let mut previous_hit = false;
    for step in 0..STEPS_PER_BAR {
        if open[step] > 0 {
            // The open hat owns its step: a closed hat there would choke it.
            previous_hit = false;
            continue;
        }
        let chance = if previous_hit {
            table.hat_after_hit
        } else {
            table.hat_after_rest
        };
        let hit = rng.chance((chance * scale).clamp(0.0, 1.0));
        if hit {
            let base = if step % 4 == 0 { 80 } else { 62 };
            bar[step] = jitter(base, 6, rng);
        }
        previous_hit = hit;
    }
    // A bar with almost no hats sounds like a mistake: fall back to the
    // odd sixteenths, the classic shuffle positions.
    if bar.iter().filter(|velocity| **velocity > 0).count() < 4 {
        for step in (1..STEPS_PER_BAR).step_by(2) {
            if open[step] == 0 && bar[step] == 0 {
                bar[step] = 62;
            }
        }
    }
    bar
}

fn perc_bar(base_chance: f64, density: f64, rng: &mut dyn RandomSource) -> Bar {
    let scale = 0.5 + density;
    let mut bar = [0u8; STEPS_PER_BAR];
    let mut previous_hit = false;
    for step in 0..STEPS_PER_BAR {
        // Two hits in a row are rare: the chain mostly alternates.
        let follow = if previous_hit { 0.2 } else { 1.0 };
        let chance = (base_chance * scale * PERC_WEIGHTS[step] * follow * 2.0).clamp(0.0, 1.0);
        let hit = rng.chance(chance);
        if hit {
            bar[step] = jitter(82, 8, rng);
        }
        previous_hit = hit;
    }
    bar
}

/// A copy of `bar` with a couple of steps flipped, for the fourth bar of a
/// phrase. Steps in `locked` are never turned on.
fn vary(bar: &Bar, locked: &Bar, velocity: u8, rng: &mut dyn RandomSource) -> Bar {
    let mut varied = *bar;
    for _ in 0..2 {
        let step = rng.below(STEPS_PER_BAR);
        if step.is_multiple_of(4) || locked[step] > 0 {
            continue;
        }
        varied[step] = if varied[step] > 0 { 0 } else { velocity };
    }
    varied
}

/// A snare roll into the next phrase: rising hits over the last beat.
fn fill_bar(snare: &Bar, density: f64, rng: &mut dyn RandomSource) -> Bar {
    let mut bar = *snare;
    let steps = [(12usize, 84u8), (13, 92), (14, 104), (15, 116)];
    for (step, velocity) in steps {
        // The last two hits always play, so every fill ends with a push.
        if step >= 14 || rng.chance(0.5 + density * 0.5) {
            bar[step] = velocity;
        }
    }
    if rng.chance(0.5) {
        bar[10] = 76;
    }
    bar
}

/// Generates a house drum pattern.
///
/// Rules that always hold: the kick plays on every beat and nowhere else;
/// the clap plays on beats two and four; open hats only fall on offbeat
/// eighths; a closed hat never shares a step with an open hat. The closed
/// hats and the percussion come from Markov chains seeded by `rng`, so the
/// same options and seed always give the same pattern.
pub fn house_drums(
    options: &DrumOptions,
    rng: &mut dyn RandomSource,
) -> Result<DrumPattern, GenerateError> {
    if !(1..=MAX_BARS).contains(&options.bars) {
        return Err(GenerateError::Bars(options.bars));
    }
    let density = clamp_unit(options.density);
    let table = style_table(options.style);
    let bars = options.bars as usize;

    let mut kick = [0u8; STEPS_PER_BAR];
    let mut clap = [0u8; STEPS_PER_BAR];
    let mut open = [0u8; STEPS_PER_BAR];
    for beat in 0..4 {
        kick[beat * 4] = 120;
        if beat % 2 == 1 {
            clap[beat * 4] = table.clap_velocity;
        }
        if table.open_hat_beats[beat] {
            open[beat * 4 + 2] = table.open_hat_velocity;
        }
    }
    let closed = closed_hat_bar(&table, &open, density, rng);
    let perc = table
        .perc
        .map(|(voice, chance)| (voice, perc_bar(chance, density, rng)));
    let mut snare = [0u8; STEPS_PER_BAR];
    for step in [3usize, 11] {
        if rng.chance(table.ghost_snare * (0.5 + density)) {
            snare[step] = jitter(58, 6, rng);
        }
    }

    let mut lanes: Vec<(DrumVoice, Vec<u8>)> = DrumVoice::ALL
        .iter()
        .map(|voice| (*voice, Vec::with_capacity(bars * STEPS_PER_BAR)))
        .collect();
    let mut push = |voice: DrumVoice, bar: &Bar| {
        if let Some((_, steps)) = lanes.iter_mut().find(|(lane, _)| *lane == voice) {
            steps.extend_from_slice(bar);
        }
    };
    let silent = [0u8; STEPS_PER_BAR];
    for index in 0..bars {
        let phrase_end = index % 4 == 3;
        let fill = options.fills && bars >= 4 && (index % 8 == 7 || index == bars - 1);
        let closed_now = if phrase_end {
            vary(&closed, &open, 62, rng)
        } else {
            closed
        };
        let snare_now = if fill {
            fill_bar(&snare, density, rng)
        } else {
            snare
        };
        push(DrumVoice::Kick, &kick);
        push(DrumVoice::Clap, &clap);
        push(DrumVoice::OpenHat, &open);
        push(DrumVoice::ClosedHat, &closed_now);
        push(DrumVoice::Snare, &snare_now);
        for voice in [DrumVoice::Rim, DrumVoice::Tom, DrumVoice::Cowbell] {
            match &perc {
                Some((perc_voice, bar)) if *perc_voice == voice => {
                    let bar_now = if phrase_end {
                        vary(bar, &silent, 80, rng)
                    } else {
                        *bar
                    };
                    push(voice, &bar_now);
                }
                _ => push(voice, &silent),
            }
        }
    }

    Ok(DrumPattern {
        bars: options.bars,
        lanes: lanes
            .into_iter()
            .filter(|(_, velocities)| velocities.iter().any(|velocity| *velocity > 0))
            .map(|(voice, velocities)| DrumLane { voice, velocities })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SeededRng;

    const STYLES: [DrumStyle; 4] = [
        DrumStyle::Classic,
        DrumStyle::Deep,
        DrumStyle::Tech,
        DrumStyle::Jackin,
    ];

    fn pattern(style: DrumStyle, bars: u32, seed: u64) -> DrumPattern {
        let options = DrumOptions {
            bars,
            style,
            ..DrumOptions::default()
        };
        house_drums(&options, &mut SeededRng::new(seed)).unwrap()
    }

    #[test]
    fn the_house_rules_hold_for_every_style_and_seed() {
        for style in STYLES {
            for seed in 0..40 {
                let pattern = pattern(style, 8, seed);
                let steps = pattern.steps();
                assert_eq!(steps, 128);
                let beats: Vec<usize> = (0..steps).step_by(4).collect();
                assert_eq!(pattern.hits(DrumVoice::Kick), beats, "{style:?} {seed}");
                let backbeats: Vec<usize> = (4..steps).step_by(8).collect();
                assert_eq!(pattern.hits(DrumVoice::Clap), backbeats);
                let open = pattern.hits(DrumVoice::OpenHat);
                assert!(!open.is_empty());
                assert!(open.iter().all(|step| step % 4 == 2));
                let closed = pattern.hits(DrumVoice::ClosedHat);
                assert!(closed.iter().all(|step| !open.contains(step)));
                for bar in 0..8 {
                    let in_bar = closed.iter().filter(|step| *step / 16 == bar).count();
                    assert!(in_bar >= 3, "{style:?} {seed}: bar {bar} has {in_bar} hats");
                }
                for lane in &pattern.lanes {
                    assert_eq!(lane.velocities.len(), steps);
                    assert!(lane.velocities.iter().all(|velocity| *velocity <= 127));
                }
            }
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_pattern_and_seeds_differ() {
        for style in STYLES {
            assert_eq!(pattern(style, 8, 11), pattern(style, 8, 11));
        }
        let distinct: std::collections::HashSet<Vec<u8>> = (0..20)
            .filter_map(|seed| {
                pattern(DrumStyle::Classic, 4, seed)
                    .lane(DrumVoice::ClosedHat)
                    .map(|lane| lane.velocities.clone())
            })
            .collect();
        assert!(distinct.len() > 10, "only {} hat patterns", distinct.len());
    }

    #[test]
    fn fills_land_in_the_last_bar_of_a_phrase_only() {
        let options = DrumOptions {
            bars: 16,
            style: DrumStyle::Classic,
            density: 0.5,
            fills: true,
        };
        let pattern = house_drums(&options, &mut SeededRng::new(3)).unwrap();
        let snare = pattern.hits(DrumVoice::Snare);
        assert!(!snare.is_empty());
        assert!(snare.iter().all(|step| [7, 15].contains(&(step / 16))));
        assert!(snare.contains(&(7 * 16 + 15)) && snare.contains(&(15 * 16 + 15)));

        let plain = house_drums(
            &DrumOptions {
                fills: false,
                ..options
            },
            &mut SeededRng::new(3),
        )
        .unwrap();
        assert!(plain.lane(DrumVoice::Snare).is_none());
    }

    #[test]
    fn density_changes_how_busy_the_hats_are() {
        let count = |density: f64| -> usize {
            (0..30)
                .map(|seed| {
                    let options = DrumOptions {
                        bars: 4,
                        style: DrumStyle::Deep,
                        density,
                        fills: false,
                    };
                    house_drums(&options, &mut SeededRng::new(seed))
                        .unwrap()
                        .hits(DrumVoice::ClosedHat)
                        .len()
                })
                .sum()
        };
        assert!(count(1.0) > count(0.0) + 100);
    }

    #[test]
    fn step_lanes_carry_the_velocities_and_bad_lengths_are_refused() {
        let pattern = pattern(DrumStyle::Classic, 2, 5);
        let kick = pattern.lane(DrumVoice::Kick).unwrap().to_step_lane(None);
        assert_eq!(kick.pitch, 36);
        assert_eq!(kick.steps.len(), 32);
        assert_eq!(kick.step_velocity(0), 120);
        assert!(kick.steps[4] && !kick.steps[5]);
        let custom = pattern
            .lane(DrumVoice::Kick)
            .unwrap()
            .to_step_lane(Some(60));
        assert_eq!(custom.pitch, 60);

        for bars in [0, 65] {
            let options = DrumOptions {
                bars,
                ..DrumOptions::default()
            };
            assert_eq!(
                house_drums(&options, &mut SeededRng::new(1)),
                Err(GenerateError::Bars(bars))
            );
        }
    }
}
