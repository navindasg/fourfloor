//! Block-rate parameter control: automation lanes and modulators.
//!
//! A [`ControlPlan`] lists the parameters the graph drives by itself. At the
//! start of every block the graph works out each one's value and hands it to
//! the device with `set_param`:
//!
//! ```text
//! value = clamp(lane(playhead) or base + sum(depth * (max - min) * modulator), min, max)
//! ```
//!
//! - An automation *lane* is a list of breakpoints on the timeline; the
//!   value between two points is interpolated linearly, and held before the
//!   first and after the last.
//! - A *modulator* is an LFO (-1 to 1) or a note-triggered envelope (0 to
//!   1). One modulator can move any number of parameters.
//!
//! Values change once per block, so the block size is the time resolution:
//! 512 frames at 48 kHz is about 10.7 ms. Devices smooth their own
//! parameters where a step would click.

use std::f64::consts::TAU;

use fourfloor_core::{DeviceId, ParamId, Ticks, TransportContext, TICKS_PER_BEAT};

use super::{GraphError, Node};

/// The waveform of an LFO. Every shape swings between -1 and 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LfoShape {
    /// A sine wave, starting at zero and rising.
    Sine,
    /// A triangle wave, starting at zero and rising.
    Triangle,
    /// A ramp from -1 up to 1.
    SawUp,
    /// A ramp from 1 down to -1.
    SawDown,
    /// 1 for the first half of the cycle, -1 for the second.
    Square,
}

impl LfoShape {
    /// The shape's value at `phase` (any real number; one cycle per 1.0).
    pub fn value(self, phase: f64) -> f32 {
        let p = phase.rem_euclid(1.0);
        let value = match self {
            Self::Sine => (p * TAU).sin(),
            Self::Triangle => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            Self::SawUp => 2.0 * p - 1.0,
            Self::SawDown => 1.0 - 2.0 * p,
            Self::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        value as f32
    }
}

/// How fast an LFO runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LfoRate {
    /// Cycles per second. Runs freely from the moment the graph starts.
    Hertz(f64),
    /// Length of one cycle in beats. Locked to the playhead.
    Beats(f64),
}

/// One modulator of a [`ControlPlan`].
#[derive(Clone, Debug, PartialEq)]
pub enum ModulatorSpec {
    /// A low-frequency oscillator.
    Lfo {
        /// The waveform.
        shape: LfoShape,
        /// The speed.
        rate: LfoRate,
        /// Where in the cycle the LFO starts, 0 to 1.
        phase: f64,
    },
    /// An attack-decay-sustain-release envelope driven by notes on the
    /// timeline. It rises on every note start and releases when no note is
    /// sounding. It follows the playhead and rests while the transport is
    /// stopped.
    Envelope {
        /// `(start, end)` of every note that triggers it, in ticks.
        notes: Vec<(Ticks, Ticks)>,
        /// Milliseconds to rise from 0 to 1.
        attack_ms: f64,
        /// Milliseconds to fall from 1 to `sustain`.
        decay_ms: f64,
        /// Level held while a note sounds, 0 to 1.
        sustain: f64,
        /// Milliseconds to fall from 1 to 0 after the last note ends.
        release_ms: f64,
    },
}

/// One breakpoint of an automation lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlPoint {
    /// Timeline position.
    pub time: Ticks,
    /// Parameter value there, in the parameter's plain units.
    pub value: f32,
}

/// One parameter the graph drives.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlTarget {
    /// The node that owns the parameter.
    pub device: DeviceId,
    /// The parameter.
    pub param: ParamId,
    /// The value modulation is centred on when there is no lane.
    pub base: f32,
    /// Lowest value ever sent.
    pub min: f32,
    /// Highest value ever sent.
    pub max: f32,
    /// Automation breakpoints sorted by time; empty for none.
    pub lane: Vec<ControlPoint>,
    /// `(index into ControlPlan::modulators, depth)`; depth is a fraction
    /// of `max - min`, from -1 to 1.
    pub modulation: Vec<(usize, f32)>,
}

impl ControlTarget {
    /// A target with no lane and no modulation yet.
    pub fn new(device: DeviceId, param: ParamId, base: f32, min: f32, max: f32) -> Self {
        Self {
            device,
            param,
            base,
            min,
            max,
            lane: Vec::new(),
            modulation: Vec::new(),
        }
    }
}

/// Everything the graph drives by itself. Give it to
/// [`GraphBuilder::set_control`](super::GraphBuilder::set_control).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlPlan {
    /// LFOs and envelopes.
    pub modulators: Vec<ModulatorSpec>,
    /// The driven parameters.
    pub targets: Vec<ControlTarget>,
}

impl ControlPlan {
    /// Whether the plan drives nothing.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// The value of a lane at `ticks`: linear between points, held outside.
pub fn lane_value(lane: &[ControlPoint], ticks: f64) -> Option<f32> {
    let (first, last) = (lane.first()?, lane.last()?);
    if ticks <= first.time.get() as f64 {
        return Some(first.value);
    }
    if ticks >= last.time.get() as f64 {
        return Some(last.value);
    }
    // First point strictly after `ticks`; at least 1 and below len here.
    let next = lane.partition_point(|point| point.time.get() as f64 <= ticks);
    let (a, b) = (lane[next - 1], lane[next]);
    let span = (b.time.get() - a.time.get()) as f64;
    if span <= 0.0 {
        return Some(b.value);
    }
    let t = ((ticks - a.time.get() as f64) / span) as f32;
    Some(a.value + (b.value - a.value) * t)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

enum ModState {
    Lfo {
        shape: LfoShape,
        rate: LfoRate,
        phase: f64,
    },
    Envelope {
        /// Note starts in ticks, sorted.
        starts: Vec<i64>,
        /// Non-overlapping `(start, end)` spans in which a note sounds.
        spans: Vec<(i64, i64)>,
        attack_ms: f64,
        decay_ms: f64,
        sustain: f64,
        release_ms: f64,
        level: f64,
        stage: Stage,
    },
}

struct Target {
    node: usize,
    device: DeviceId,
    param: ParamId,
    base: f32,
    low: f32,
    high: f32,
    lane: Vec<ControlPoint>,
    modulation: Vec<(usize, f32)>,
    /// The value last sent, as bits, so a steady value is sent once.
    sent: Option<u32>,
}

/// The running form of a [`ControlPlan`], owned by the compiled graph.
pub(super) struct Control {
    modulators: Vec<ModState>,
    values: Vec<f32>,
    targets: Vec<Target>,
    /// Frames processed since the graph started or was reset.
    elapsed: u64,
}

fn merged_spans(notes: &[(Ticks, Ticks)]) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = notes
        .iter()
        .map(|(start, end)| (start.get(), end.get().max(start.get() + 1)))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

impl Control {
    /// Resolves a plan against the compiled nodes.
    pub(super) fn new(plan: ControlPlan, nodes: &[Node]) -> Result<Self, GraphError> {
        let modulators: Vec<ModState> = plan
            .modulators
            .into_iter()
            .map(|spec| match spec {
                ModulatorSpec::Lfo { shape, rate, phase } => ModState::Lfo { shape, rate, phase },
                ModulatorSpec::Envelope {
                    notes,
                    attack_ms,
                    decay_ms,
                    sustain,
                    release_ms,
                } => {
                    let mut starts: Vec<i64> = notes.iter().map(|(start, _)| start.get()).collect();
                    starts.sort_unstable();
                    ModState::Envelope {
                        starts,
                        spans: merged_spans(&notes),
                        attack_ms: attack_ms.max(0.0),
                        decay_ms: decay_ms.max(0.0),
                        sustain: sustain.clamp(0.0, 1.0),
                        release_ms: release_ms.max(0.0),
                        level: 0.0,
                        stage: Stage::Idle,
                    }
                }
            })
            .collect();
        let count = modulators.len();
        let targets = plan
            .targets
            .into_iter()
            .map(|target| {
                let node = nodes
                    .iter()
                    .position(|node| node.id == target.device)
                    .ok_or(GraphError::UnknownNode(target.device))?;
                Ok(Target {
                    node,
                    device: target.device,
                    param: target.param,
                    base: target.base,
                    low: target.min.min(target.max),
                    high: target.max.max(target.min),
                    lane: target.lane,
                    modulation: target
                        .modulation
                        .into_iter()
                        .filter(|(index, _)| *index < count)
                        .collect(),
                    sent: None,
                })
            })
            .collect::<Result<Vec<_>, GraphError>>()?;
        Ok(Self {
            values: vec![0.0; count],
            modulators,
            targets,
            elapsed: 0,
        })
    }

    /// Whether anything is driven.
    pub(super) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// The current output of one modulator.
    pub(super) fn modulator_value(&self, index: usize) -> Option<f32> {
        self.values.get(index).copied()
    }

    /// Restarts free-running LFOs and envelopes and resends every value.
    pub(super) fn reset(&mut self) {
        self.elapsed = 0;
        for modulator in &mut self.modulators {
            if let ModState::Envelope { level, stage, .. } = modulator {
                *level = 0.0;
                *stage = Stage::Idle;
            }
        }
        for target in &mut self.targets {
            target.sent = None;
        }
    }

    /// A live parameter change moves the centre of a driven parameter.
    pub(super) fn set_base(&mut self, device: DeviceId, param: ParamId, value: f32) {
        for target in &mut self.targets {
            if target.device == device && target.param == param {
                target.base = value;
                target.sent = None;
            }
        }
    }

    /// Audio thread. Works out every driven value for the block that starts
    /// at `ctx` and sends the ones that changed.
    pub(super) fn apply(&mut self, ctx: &TransportContext, frames: usize, nodes: &mut [Node]) {
        let rate = ctx.sample_rate.as_f64();
        let beats = ctx.position_beats.get();
        let ticks = beats * TICKS_PER_BEAT as f64;
        let block_beats = frames as f64 / ctx.tempo.samples_per_beat(ctx.sample_rate);
        let ticks_end = ticks + block_beats * TICKS_PER_BEAT as f64;
        let block_ms = frames as f64 / rate * 1_000.0;
        let seconds = self.elapsed as f64 / rate;

        for (state, value) in self.modulators.iter_mut().zip(self.values.iter_mut()) {
            *value = match state {
                ModState::Lfo { shape, rate, phase } => {
                    let cycles = match *rate {
                        LfoRate::Hertz(hz) => seconds * hz,
                        LfoRate::Beats(length) if length > 0.0 => beats / length,
                        LfoRate::Beats(_) => 0.0,
                    };
                    shape.value(cycles + *phase)
                }
                ModState::Envelope {
                    starts,
                    spans,
                    attack_ms,
                    decay_ms,
                    sustain,
                    release_ms,
                    level,
                    stage,
                } => {
                    let first = starts.partition_point(|start| (*start as f64) < ticks);
                    let triggered = ctx.playing
                        && starts
                            .get(first)
                            .is_some_and(|start| (*start as f64) < ticks_end);
                    let span = spans.partition_point(|(_, end)| (*end as f64) <= ticks);
                    let gate = ctx.playing
                        && spans
                            .get(span)
                            .is_some_and(|(start, _)| (*start as f64) <= ticks);
                    if triggered {
                        *stage = Stage::Attack;
                    } else if !gate && *stage != Stage::Idle {
                        *stage = Stage::Release;
                    }
                    match *stage {
                        Stage::Attack => {
                            *level = if *attack_ms > 0.0 {
                                (*level + block_ms / *attack_ms).min(1.0)
                            } else {
                                1.0
                            };
                            if *level >= 1.0 {
                                *stage = Stage::Decay;
                            }
                        }
                        Stage::Decay => {
                            *level = if *decay_ms > 0.0 {
                                (*level - (1.0 - *sustain) * block_ms / *decay_ms).max(*sustain)
                            } else {
                                *sustain
                            };
                        }
                        Stage::Release => {
                            *level = if *release_ms > 0.0 {
                                (*level - block_ms / *release_ms).max(0.0)
                            } else {
                                0.0
                            };
                            if *level <= 0.0 {
                                *stage = Stage::Idle;
                            }
                        }
                        Stage::Idle => *level = 0.0,
                    }
                    *level as f32
                }
            };
        }

        for target in &mut self.targets {
            let centre = lane_value(&target.lane, ticks).unwrap_or(target.base);
            let range = target.high - target.low;
            let offset: f32 = target
                .modulation
                .iter()
                .map(|&(index, depth)| depth * range * self.values[index])
                .sum();
            let value = (centre + offset).clamp(target.low, target.high);
            if value.is_finite() && target.sent != Some(value.to_bits()) {
                nodes[target.node].device.set_param(target.param, value);
                target.sent = Some(value.to_bits());
            }
        }
        self.elapsed = self.elapsed.saturating_add(frames as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(time: i64, value: f32) -> ControlPoint {
        ControlPoint {
            time: Ticks::new(time),
            value,
        }
    }

    #[test]
    fn a_lane_interpolates_between_points_and_holds_outside() {
        let lane = [
            point(960, 0.0),
            point(1_920, 1.0),
            point(1_920, 5.0),
            point(2_880, 3.0),
        ];
        assert_eq!(lane_value(&[], 0.0), None);
        assert_eq!(lane_value(&lane, 0.0), Some(0.0));
        assert_eq!(lane_value(&lane, 1_440.0), Some(0.5));
        assert_eq!(
            lane_value(&lane, 1_920.0),
            Some(5.0),
            "a step takes the later point"
        );
        assert_eq!(lane_value(&lane, 2_400.0), Some(4.0));
        assert_eq!(lane_value(&lane, 99_999.0), Some(3.0));
    }

    #[test]
    fn shapes_swing_between_minus_one_and_one() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(LfoShape::Sine.value(0.25), 1.0));
        assert!(close(LfoShape::Sine.value(1.75), -1.0));
        assert!(close(LfoShape::Triangle.value(0.0), 0.0));
        assert!(close(LfoShape::Triangle.value(0.25), 1.0));
        assert!(close(LfoShape::Triangle.value(0.5), 0.0));
        assert!(close(LfoShape::Triangle.value(0.75), -1.0));
        assert!(close(LfoShape::SawUp.value(0.0), -1.0));
        assert!(close(LfoShape::SawUp.value(0.75), 0.5));
        assert!(close(LfoShape::SawDown.value(0.25), 0.5));
        assert!(close(LfoShape::Square.value(0.1), 1.0));
        assert!(close(LfoShape::Square.value(-0.4), -1.0));
    }

    #[test]
    fn overlapping_notes_merge_into_one_span() {
        let t = Ticks::new;
        let spans = merged_spans(&[
            (t(100), t(200)),
            (t(0), t(50)),
            (t(150), t(300)),
            (t(300), t(310)),
        ]);
        assert_eq!(spans, [(0, 50), (100, 310)]);
        assert_eq!(
            merged_spans(&[(t(5), t(5))]),
            [(5, 6)],
            "an empty note still gates"
        );
    }
}
