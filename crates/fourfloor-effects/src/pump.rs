//! Volume shaper: tempo-synced ducking without a sidechain.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::clock::PhaseClock;
use crate::util::{process_stereo, time_coefficient, ParamSet, Smoothed};

const PARAM_COUNT: usize = 7;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(Pump::RATE, "Rate", 0.125, 8.0, 1.0, ParamUnit::Beats).logarithmic(),
    ParamInfo::new(Pump::DEPTH, "Depth", 0.0, 1.0, 0.8, ParamUnit::Generic),
    ParamInfo::new(Pump::HOLD, "Hold", 0.0, 0.5, 0.0, ParamUnit::Generic),
    ParamInfo::new(Pump::RELEASE, "Release", 0.05, 1.0, 0.6, ParamUnit::Generic),
    ParamInfo::new(Pump::CURVE, "Curve", 0.2, 5.0, 1.5, ParamUnit::Generic).logarithmic(),
    ParamInfo::new(Pump::PHASE, "Phase", 0.0, 1.0, 0.0, ParamUnit::Generic),
    ParamInfo::new(
        Pump::SMOOTH,
        "Smooth",
        0.1,
        50.0,
        3.0,
        ParamUnit::Milliseconds,
    )
    .logarithmic(),
];

/// A volume shaper (`fx.pump`): the sidechain "pump" of house music without
/// a sidechain.
///
/// Every `Rate` beats the gain drops by `Depth` and then recovers. The cycle
/// is read from the timeline, so the dip sits exactly on the beat (move it
/// with `Phase`) and a render is repeatable; with the transport stopped it
/// runs freely at the same speed.
///
/// One cycle, as fractions of its length: the gain stays down for `Hold`,
/// then rises over `Release` and stays at unity for the rest. `Curve` bends
/// the rise: 1 is a straight line, more than 1 comes back quickly at first
/// and less than 1 stays down longer. `Smooth` rounds the drop so it does
/// not click.
#[derive(Clone, Debug)]
pub struct Pump {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    clock: PhaseClock,
    gain: Smoothed,
}

impl Default for Pump {
    fn default() -> Self {
        Self::new()
    }
}

impl Pump {
    /// Length of one cycle in beats: 1 pumps on every quarter note.
    pub const RATE: ParamId = ParamId(0);
    /// How far the gain drops: 0 is none, 1 is to silence.
    pub const DEPTH: ParamId = ParamId(1);
    /// Share of the cycle the gain stays fully down.
    pub const HOLD: ParamId = ParamId(2);
    /// Share of the cycle the gain takes to come back.
    pub const RELEASE: ParamId = ParamId(3);
    /// Shape of the recovery; 1 is linear.
    pub const CURVE: ParamId = ParamId(4);
    /// Moves the dip, in cycles.
    pub const PHASE: ParamId = ParamId(5);
    /// Smoothing of the gain, in milliseconds.
    pub const SMOOTH: ParamId = ParamId(6);

    /// A quarter-note pump.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            clock: PhaseClock::default(),
            gain: Smoothed::new(),
        }
    }

    /// The gain at `phase` (0 to 1) of a cycle, before smoothing.
    pub fn shape(phase: f32, depth: f32, hold: f32, release: f32, curve: f32) -> f32 {
        let duck = if phase < hold {
            1.0
        } else if phase < hold + release {
            (1.0 - (phase - hold) / release.max(1.0e-6)).powf(curve)
        } else {
            0.0
        };
        1.0 - depth * duck
    }
}

impl Device for Pump {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        self.reset();
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let p = &self.params;
        let depth = p.get(Self::DEPTH);
        let hold = p.get(Self::HOLD);
        let release = p.get(Self::RELEASE);
        let curve = p.get(Self::CURVE);
        let offset = f64::from(p.get(Self::PHASE));
        let coefficient = time_coefficient(p.get(Self::SMOOTH) * 0.001, self.sample_rate);
        let phase = self
            .clock
            .synced(ctx, f64::from(p.get(Self::RATE)), io.frames());
        self.gain.set_coefficient(coefficient);
        let gain = &mut self.gain;
        let mut index = 0usize;
        process_stereo(io, |frame| {
            let position = phase.at(index, offset) as f32;
            index += 1;
            let g = gain.next(Self::shape(position, depth, hold, release, curve));
            [frame.input[0] * g, frame.input[1] * g]
        });
    }

    fn params(&self) -> &[ParamInfo] {
        self.params.table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        self.params.set(param, value);
    }

    fn reset(&mut self) {
        self.clock.reset();
        self.gain.reset();
    }
}
