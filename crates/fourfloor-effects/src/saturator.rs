//! Saturation and console-style drive.

use std::f32::consts::FRAC_PI_2;

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::util::{db_to_gain, process_stereo, OnePole, ParamSet, Smoothed};

const PARAM_COUNT: usize = 5;
/// Where the Spiral curve stops: sqrt(pi / 2), as in the original. The
/// curve peaks at 0.85 a little before that and ends at 0.8.
const SPIRAL_LIMIT: f32 = 1.253_314_1;
/// Corner of the filter that removes the offset asymmetry creates.
const DC_BLOCK_HZ: f32 = 10.0;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(
        Saturator::DRIVE,
        "Drive",
        0.0,
        36.0,
        6.0,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(Saturator::MODE, "Mode", 0.0, 3.0, 0.0, ParamUnit::Integer),
    ParamInfo::new(
        Saturator::ASYMMETRY,
        "Asymmetry",
        0.0,
        1.0,
        0.0,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Saturator::OUTPUT,
        "Output",
        -24.0,
        12.0,
        0.0,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(Saturator::MIX, "Mix", 0.0, 1.0, 1.0, ParamUnit::Generic),
];

/// The transfer curve of each mode. Every curve has unity gain for small
/// signals and never exceeds 1.
#[inline]
fn shape(mode: usize, x: f32) -> f32 {
    match mode {
        Saturator::SPIRAL => {
            let x = x.clamp(-SPIRAL_LIMIT, SPIRAL_LIMIT);
            let magnitude = x.abs();
            if magnitude < 1.0e-6 {
                x
            } else {
                (x * magnitude).sin() / magnitude
            }
        }
        Saturator::CONSOLE => x.clamp(-FRAC_PI_2, FRAC_PI_2).sin(),
        Saturator::HARD => x.clamp(-1.0, 1.0),
        _ => x.tanh(),
    }
}

/// A saturator (`fx.saturator`): drive into a soft curve.
///
/// Modes:
/// - 0, tape: `tanh`, the classic soft clip.
/// - 1, spiral: `sin(x * |x|) / |x|`, the transfer curve of Airwindows
///   Spiral. Cleaner than `tanh` at low levels, with a smooth top that
///   reaches 0.85 rather than full scale.
/// - 2, console: `sin(x)` up to its peak, the channel curve of Airwindows
///   PurestConsole. Gentle; meant for small amounts on many tracks.
/// - 3, hard: a plain clip.
///
/// The spiral and console curves are written from the formulas in Chris
/// Johnson's MIT-licensed Airwindows plugins; they are the transfer curves
/// only, not ports of the whole plugins.
///
/// `Asymmetry` shifts the signal on the curve so the two halves of the wave
/// are shaped differently, which adds even harmonics (the shift itself is
/// filtered out again). There is no oversampling: heavy drive on bright
/// material aliases.
#[derive(Clone, Debug)]
pub struct Saturator {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    dc: [OnePole; 2],
    drive: Smoothed,
    output: Smoothed,
}

impl Default for Saturator {
    fn default() -> Self {
        Self::new()
    }
}

impl Saturator {
    /// Gain into the curve, in decibels.
    pub const DRIVE: ParamId = ParamId(0);
    /// Curve: 0 tape, 1 spiral, 2 console, 3 hard clip.
    pub const MODE: ParamId = ParamId(1);
    /// Amount of even harmonics, 0 to 1.
    pub const ASYMMETRY: ParamId = ParamId(2);
    /// Gain after the curve, in decibels.
    pub const OUTPUT: ParamId = ParamId(3);
    /// Dry/wet mix.
    pub const MIX: ParamId = ParamId(4);

    /// [`Saturator::MODE`] value for `tanh`.
    pub const TAPE: usize = 0;
    /// [`Saturator::MODE`] value for the Spiral curve.
    pub const SPIRAL: usize = 1;
    /// [`Saturator::MODE`] value for the console curve.
    pub const CONSOLE: usize = 2;
    /// [`Saturator::MODE`] value for the hard clip.
    pub const HARD: usize = 3;

    /// Tape saturation with 6 dB of drive.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            dc: [OnePole::default(); 2],
            drive: Smoothed::new(),
            output: Smoothed::new(),
        }
    }
}

impl Device for Saturator {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        for filter in &mut self.dc {
            filter.set_cutoff(DC_BLOCK_HZ, self.sample_rate);
        }
        self.drive.set_time(0.01, self.sample_rate);
        self.output.set_time(0.01, self.sample_rate);
        self.reset();
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let p = &self.params;
        let mode = p.choice(Self::MODE);
        let drive_target = db_to_gain(p.get(Self::DRIVE));
        let output_target = db_to_gain(p.get(Self::OUTPUT));
        let bias = 0.5 * p.get(Self::ASYMMETRY);
        let mix = p.get(Self::MIX);
        let offset = shape(mode, bias);
        let Self {
            dc, drive, output, ..
        } = self;
        process_stereo(io, |frame| {
            let drive = drive.next(drive_target);
            let output = output.next(output_target);
            let mut out = [0.0f32; 2];
            for channel in 0..2 {
                let dry = frame.input[channel];
                let mut wet = shape(mode, dry * drive + bias) - offset;
                if bias > 0.0 {
                    wet = dc[channel].high(wet);
                }
                out[channel] = (dry * (1.0 - mix) + wet * mix) * output;
            }
            out
        });
    }

    fn params(&self) -> &[ParamInfo] {
        self.params.table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        self.params.set(param, value);
    }

    fn reset(&mut self) {
        for filter in &mut self.dc {
            filter.reset();
        }
        self.drive.reset();
        self.output.reset();
    }
}
