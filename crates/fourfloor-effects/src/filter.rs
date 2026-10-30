//! Multimode resonant filter with a tempo-synced LFO.

use std::f64::consts::{PI, TAU};

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::clock::PhaseClock;
use crate::util::{process_stereo, ParamSet};

const PARAM_COUNT: usize = 10;
/// Frames between coefficient updates while the cutoff moves.
const CONTROL_PERIOD: usize = 16;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(Filter::MODE, "Mode", 0.0, 2.0, 0.0, ParamUnit::Integer),
    ParamInfo::new(
        Filter::CUTOFF,
        "Cutoff",
        20.0,
        20_000.0,
        1_000.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        Filter::RESONANCE,
        "Resonance",
        0.0,
        1.0,
        0.2,
        ParamUnit::Generic,
    ),
    ParamInfo::toggle(Filter::STEEP, "24 dB", false),
    ParamInfo::new(
        Filter::LFO_DEPTH,
        "LFO Depth",
        0.0,
        5.0,
        0.0,
        ParamUnit::Generic,
    ),
    ParamInfo::toggle(Filter::LFO_SYNC, "LFO Sync", true),
    ParamInfo::new(
        Filter::LFO_BEATS,
        "LFO Period",
        0.0625,
        16.0,
        1.0,
        ParamUnit::Beats,
    )
    .logarithmic(),
    ParamInfo::new(
        Filter::LFO_HZ,
        "LFO Rate",
        0.01,
        20.0,
        1.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        Filter::LFO_SHAPE,
        "LFO Shape",
        0.0,
        3.0,
        0.0,
        ParamUnit::Integer,
    ),
    ParamInfo::new(
        Filter::LFO_PHASE,
        "LFO Phase",
        0.0,
        1.0,
        0.0,
        ParamUnit::Generic,
    ),
];

/// Coefficients of one state-variable stage.
#[derive(Clone, Copy, Debug, Default)]
struct SvfCoefficients {
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
}

/// A trapezoidal state-variable filter (Andrew Simper's formulation), which
/// stays stable while its cutoff is modulated.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f64,
    ic2: f64,
}

impl Svf {
    #[inline]
    fn tick(&mut self, c: &SvfCoefficients, mode: usize, input: f64) -> f64 {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match mode {
            Filter::HIGH_PASS => input - c.k * v1 - v2,
            // Scaled so the band-pass peak is unity at any resonance.
            Filter::BAND_PASS => c.k * v1,
            _ => v2,
        }
    }
}

/// A multimode resonant filter (`fx.filter`): low-pass, high-pass or
/// band-pass, 12 or 24 dB/octave, with an LFO on the cutoff.
///
/// The LFO moves the cutoff by up to `LFO Depth` octaves either way. With
/// `LFO Sync` on, one cycle lasts `LFO Period` beats and its phase is read
/// from the timeline: phase 0 falls on beat 0 and every multiple of the
/// period, so a render is repeatable. With the transport stopped, or with
/// sync off (`LFO Rate` in Hz), it runs freely.
///
/// Shapes: 0 sine, 1 triangle, 2 falling saw, 3 square.
#[derive(Clone, Debug)]
pub struct Filter {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f64,
    coefficients: SvfCoefficients,
    stages: [[Svf; 2]; 2],
    clock: PhaseClock,
    /// Smoothed base cutoff, as log2(Hz). Negative until first used.
    pitch: f64,
    pitch_coefficient: f64,
    countdown: usize,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    /// Response: 0 low-pass, 1 high-pass, 2 band-pass.
    pub const MODE: ParamId = ParamId(0);
    /// Cutoff frequency in Hz.
    pub const CUTOFF: ParamId = ParamId(1);
    /// Resonance, 0 to 1.
    pub const RESONANCE: ParamId = ParamId(2);
    /// Two stages in series: 24 dB/octave instead of 12.
    pub const STEEP: ParamId = ParamId(3);
    /// How far the LFO moves the cutoff, in octaves each way.
    pub const LFO_DEPTH: ParamId = ParamId(4);
    /// Locks the LFO to the tempo.
    pub const LFO_SYNC: ParamId = ParamId(5);
    /// Length of one LFO cycle in beats, when synced.
    pub const LFO_BEATS: ParamId = ParamId(6);
    /// LFO rate in Hz, when not synced.
    pub const LFO_HZ: ParamId = ParamId(7);
    /// LFO shape: 0 sine, 1 triangle, 2 falling saw, 3 square.
    pub const LFO_SHAPE: ParamId = ParamId(8);
    /// LFO phase offset, in cycles.
    pub const LFO_PHASE: ParamId = ParamId(9);

    /// [`Filter::MODE`] value for a low-pass.
    pub const LOW_PASS: usize = 0;
    /// [`Filter::MODE`] value for a high-pass.
    pub const HIGH_PASS: usize = 1;
    /// [`Filter::MODE`] value for a band-pass.
    pub const BAND_PASS: usize = 2;

    /// A low-pass at 1 kHz with the LFO off.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            coefficients: SvfCoefficients::default(),
            stages: [[Svf::default(); 2]; 2],
            clock: PhaseClock::default(),
            pitch: -1.0,
            pitch_coefficient: 1.0,
            countdown: 0,
        }
    }

    fn lfo(shape: usize, phase: f64) -> f64 {
        match shape {
            1 => 1.0 - 4.0 * ((phase + 0.25).rem_euclid(1.0) - 0.5).abs(),
            2 => 1.0 - 2.0 * phase,
            3 => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => (TAU * phase).sin(),
        }
    }

    fn coefficients(&self, pitch: f64, resonance: f64) -> SvfCoefficients {
        let frequency = pitch.exp2().clamp(20.0, self.sample_rate * 0.45);
        let g = (PI * frequency / self.sample_rate).tan();
        // k is 1/Q: 2 is no resonance, 0 would self-oscillate.
        let k = 2.0 - 1.96 * resonance;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        SvfCoefficients {
            k,
            a1,
            a2,
            a3: g * a2,
        }
    }
}

impl Device for Filter {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.as_f64();
        // About 5 ms of glide on cutoff changes, at the control rate.
        let updates = 0.005 * self.sample_rate / CONTROL_PERIOD as f64;
        self.pitch_coefficient = 1.0 - (-1.0 / updates.max(1.0e-3)).exp();
        self.reset();
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let p = &self.params;
        let mode = p.choice(Self::MODE);
        let steep = p.on(Self::STEEP);
        let target_pitch = f64::from(p.get(Self::CUTOFF)).log2();
        let resonance = f64::from(p.get(Self::RESONANCE));
        let depth = f64::from(p.get(Self::LFO_DEPTH));
        let shape = p.choice(Self::LFO_SHAPE);
        let offset = f64::from(p.get(Self::LFO_PHASE));
        let frames = io.frames();
        let phase = if p.on(Self::LFO_SYNC) {
            self.clock
                .synced(ctx, f64::from(p.get(Self::LFO_BEATS)), frames)
        } else {
            self.clock
                .free(f64::from(p.get(Self::LFO_HZ)), self.sample_rate, frames)
        };
        if self.pitch < 0.0 {
            self.pitch = target_pitch;
        }

        let mut index = 0usize;
        let mut pitch = self.pitch;
        let mut countdown = self.countdown;
        let mut coefficients = self.coefficients;
        let pitch_coefficient = self.pitch_coefficient;
        let this = &*self;
        let mut stages = this.stages;
        process_stereo(io, |frame| {
            if countdown == 0 {
                countdown = CONTROL_PERIOD;
                pitch += (target_pitch - pitch) * pitch_coefficient;
                if (target_pitch - pitch).abs() < 1.0e-5 {
                    pitch = target_pitch;
                }
                let modulation = if depth > 0.0 {
                    depth * Self::lfo(shape, phase.at(index, offset))
                } else {
                    0.0
                };
                coefficients = this.coefficients(pitch + modulation, resonance);
            }
            countdown -= 1;
            index += 1;
            let mut out = [0.0f32; 2];
            for ((out, input), stage) in out.iter_mut().zip(frame.input).zip(stages.iter_mut()) {
                let mut value = stage[0].tick(&coefficients, mode, f64::from(input));
                if steep {
                    value = stage[1].tick(&coefficients, mode, value);
                }
                *out = value as f32;
            }
            out
        });
        self.stages = stages;
        self.pitch = pitch;
        self.countdown = countdown;
        self.coefficients = coefficients;
    }

    fn params(&self) -> &[ParamInfo] {
        self.params.table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        self.params.set(param, value);
    }

    fn reset(&mut self) {
        self.stages = [[Svf::default(); 2]; 2];
        self.clock.reset();
        self.pitch = -1.0;
        self.countdown = 0;
    }
}
