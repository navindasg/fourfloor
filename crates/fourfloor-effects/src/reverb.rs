//! Algorithmic reverb: plate and hall.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::delay_line::DelayLine;
use crate::util::{process_stereo, OnePole, ParamSet, Smoothed};

const PARAM_COUNT: usize = 8;
const LINES: usize = 8;
const DIFFUSERS: usize = 4;
const MAX_PRE_DELAY_SECONDS: f32 = 0.25;
const MAX_SIZE: f32 = 2.0;

/// Delay-line lengths in milliseconds at size 1, chosen so that no two
/// share a simple ratio. Short and close together for the plate; longer and
/// more spread for the hall.
const LINE_MS: [[f32; LINES]; 2] = [
    [23.11, 26.93, 29.87, 33.71, 37.13, 41.39, 44.83, 49.31],
    [53.17, 61.73, 71.29, 79.87, 89.33, 97.07, 107.29, 113.93],
];
/// Input diffuser lengths in milliseconds, left and right.
const DIFFUSER_MS: [[f32; DIFFUSERS]; 2] = [[4.77, 3.59, 12.73, 9.31], [5.03, 3.37, 11.87, 9.79]];
/// Diffuser feedback per mode.
const DIFFUSION: [f32; 2] = [0.72, 0.6];
/// Weights that take the eight lines to left and right. Each side reads
/// four lines of its own, so the two share no signal: that is what
/// decorrelates the channels. The lines are dealt out so both sides get the
/// same mix of short and long ones and stay equally loud; the signs
/// alternate so the sums carry no offset.
const OUT_LEFT: [f32; LINES] = [1.0, 0.0, 0.0, -1.0, 0.0, 1.0, -1.0, 0.0];
const OUT_RIGHT: [f32; LINES] = [0.0, 1.0, -1.0, 0.0, 1.0, 0.0, 0.0, -1.0];
/// Level of the wet signal, set so the tail of a full-scale burst stays
/// below full scale at the default size.
const WET_GAIN: f32 = 0.42;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(Reverb::MODE, "Mode", 0.0, 1.0, 0.0, ParamUnit::Integer),
    ParamInfo::new(Reverb::DECAY, "Decay", 0.2, 20.0, 1.8, ParamUnit::Seconds).logarithmic(),
    ParamInfo::new(Reverb::SIZE, "Size", 0.5, MAX_SIZE, 1.0, ParamUnit::Generic),
    ParamInfo::new(
        Reverb::PRE_DELAY,
        "Pre-delay",
        0.0,
        MAX_PRE_DELAY_SECONDS * 1_000.0,
        10.0,
        ParamUnit::Milliseconds,
    ),
    ParamInfo::new(
        Reverb::DAMPING,
        "Damping",
        500.0,
        20_000.0,
        6_000.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        Reverb::LOW_CUT,
        "Low Cut",
        20.0,
        1_000.0,
        100.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(Reverb::MIX, "Mix", 0.0, 1.0, 0.25, ParamUnit::Generic),
    ParamInfo::new(Reverb::WIDTH, "Width", 0.0, 1.0, 1.0, ParamUnit::Generic),
];

/// A Schroeder all-pass: smears a click into a burst without colouring the
/// spectrum.
#[derive(Clone, Debug)]
struct AllPass {
    line: DelayLine,
    length: usize,
}

impl AllPass {
    fn new() -> Self {
        Self {
            line: DelayLine::new(),
            length: 1,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32, feedback: f32) -> f32 {
        let delayed = self.line.tap(self.length);
        let write = input + delayed * feedback;
        self.line.push(write);
        delayed - write * feedback
    }
}

/// In-place fast Walsh-Hadamard transform, scaled to be orthonormal: the
/// lossless feedback matrix of the network.
#[inline]
fn hadamard(values: &mut [f32; LINES]) {
    let mut step = 1;
    while step < LINES {
        let mut start = 0;
        while start < LINES {
            for index in start..start + step {
                let (a, b) = (values[index], values[index + step]);
                values[index] = a + b;
                values[index + step] = a - b;
            }
            start += step * 2;
        }
        step *= 2;
    }
    let scale = 1.0 / (LINES as f32).sqrt();
    for value in values.iter_mut() {
        *value *= scale;
    }
}

/// An algorithmic reverb (`fx.reverb`) with a plate and a hall mode.
///
/// A feedback delay network: each input channel is pre-delayed, low-cut and
/// smeared by four all-pass diffusers, then fed into eight delay lines that
/// feed back into each other through a Hadamard matrix. Each line has a
/// gain set from `Decay`, so the tail falls by 60 dB in that many seconds,
/// and a low-pass (`Damping`) that makes the highs die sooner, as in a real
/// room. Left and right are each read from four lines of their own, which
/// makes the tail wide even for a mono source.
///
/// Mode 0 (plate) uses short, closely spaced lines and strong diffusion:
/// dense and immediate. Mode 1 (hall) uses longer, more spread lines: a
/// slower build-up and a larger space. `Size` scales the line lengths.
///
/// This is the stock reverb; the build plan also names Dragonfly Reverb as
/// a hosted plugin for more elaborate halls. The lines are not modulated,
/// so very long decays can ring slightly on sustained tones.
#[derive(Clone, Debug)]
pub struct Reverb {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    pre_delay: [DelayLine; 2],
    low_cut: [OnePole; 2],
    diffusers: [[AllPass; DIFFUSERS]; 2],
    lines: [DelayLine; LINES],
    lengths: [usize; LINES],
    gains: [f32; LINES],
    damping: [OnePole; LINES],
    diffusion: f32,
    pre_delay_samples: usize,
    mix: Smoothed,
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    /// Algorithm: 0 plate, 1 hall.
    pub const MODE: ParamId = ParamId(0);
    /// Time for the tail to fall by 60 dB, in seconds.
    pub const DECAY: ParamId = ParamId(1);
    /// Scales the size of the space, 0.5 to 2.
    pub const SIZE: ParamId = ParamId(2);
    /// Gap before the reverb starts, in milliseconds.
    pub const PRE_DELAY: ParamId = ParamId(3);
    /// Frequency above which the tail dies faster, in Hz.
    pub const DAMPING: ParamId = ParamId(4);
    /// Low cut on what goes into the reverb, in Hz.
    pub const LOW_CUT: ParamId = ParamId(5);
    /// Dry/wet mix: 0 is dry only, 1 is reverb only.
    pub const MIX: ParamId = ParamId(6);
    /// Stereo width of the reverb: 0 is mono.
    pub const WIDTH: ParamId = ParamId(7);

    /// [`Reverb::MODE`] value for the plate.
    pub const PLATE: f32 = 0.0;
    /// [`Reverb::MODE`] value for the hall.
    pub const HALL: f32 = 1.0;

    /// A plate with a 1.8 second decay.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            pre_delay: [DelayLine::new(), DelayLine::new()],
            low_cut: [OnePole::default(); 2],
            diffusers: [
                [
                    AllPass::new(),
                    AllPass::new(),
                    AllPass::new(),
                    AllPass::new(),
                ],
                [
                    AllPass::new(),
                    AllPass::new(),
                    AllPass::new(),
                    AllPass::new(),
                ],
            ],
            lines: std::array::from_fn(|_| DelayLine::new()),
            lengths: [1; LINES],
            gains: [0.0; LINES],
            damping: [OnePole::default(); LINES],
            diffusion: DIFFUSION[0],
            pre_delay_samples: 1,
            mix: Smoothed::new(),
        }
    }

    fn samples(&self, milliseconds: f32) -> usize {
        ((milliseconds * 0.001 * self.sample_rate).round() as usize).max(1)
    }

    // The loops below walk several parallel arrays by line number.
    #[allow(clippy::needless_range_loop)]
    fn update(&mut self) {
        let p = &self.params;
        let mode = p.choice(Self::MODE).min(1);
        let size = p.get(Self::SIZE);
        let decay_samples = p.get(Self::DECAY) * self.sample_rate;
        self.diffusion = DIFFUSION[mode];
        for index in 0..LINES {
            let length = self
                .samples(LINE_MS[mode][index] * size)
                .min(self.lines[index].max_delay().max(1));
            self.lengths[index] = length;
            // -60 dB after `decay_samples`, however long this line is.
            self.gains[index] = 10.0f32.powf(-3.0 * length as f32 / decay_samples);
            self.damping[index].set_cutoff(p.get(Self::DAMPING), self.sample_rate);
        }
        for filter in &mut self.low_cut {
            filter.set_cutoff(p.get(Self::LOW_CUT), self.sample_rate);
        }
        self.pre_delay_samples = self
            .samples(p.get(Self::PRE_DELAY))
            .min(self.pre_delay[0].max_delay().max(1));
    }
}

impl Device for Reverb {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        let longest = LINE_MS[1][LINES - 1] * MAX_SIZE;
        let line_capacity = self.samples(longest) + 2;
        for line in &mut self.lines {
            line.prepare(line_capacity);
        }
        let pre_delay_capacity = self.samples(MAX_PRE_DELAY_SECONDS * 1_000.0) + 2;
        for line in &mut self.pre_delay {
            line.prepare(pre_delay_capacity);
        }
        for (channel, lengths) in DIFFUSER_MS.iter().enumerate() {
            for (index, milliseconds) in lengths.iter().enumerate() {
                let length = self.samples(*milliseconds);
                let diffuser = &mut self.diffusers[channel][index];
                diffuser.line.prepare(length + 2);
                diffuser.length = length;
            }
        }
        self.mix.set_time(0.01, self.sample_rate);
        self.params.mark_dirty();
        self.reset();
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        if self.params.take_dirty() {
            self.update();
        }
        let mix_target = self.params.get(Self::MIX);
        let width = self.params.get(Self::WIDTH);
        let (lengths, gains) = (self.lengths, self.gains);
        let (diffusion, pre_delay_samples) = (self.diffusion, self.pre_delay_samples);
        let Self {
            pre_delay,
            low_cut,
            diffusers,
            lines,
            damping,
            mix,
            ..
        } = self;
        process_stereo(io, |frame| {
            let mut feed = [0.0f32; 2];
            for channel in 0..2 {
                let delayed = pre_delay[channel].tap(pre_delay_samples);
                pre_delay[channel].push(frame.input[channel]);
                let mut value = low_cut[channel].high(delayed);
                for diffuser in diffusers[channel].iter_mut() {
                    value = diffuser.tick(value, diffusion);
                }
                feed[channel] = value;
            }

            let mut taps = [0.0f32; LINES];
            let (mut left, mut right) = (0.0f32, 0.0f32);
            for index in 0..LINES {
                let tap = damping[index].low(lines[index].tap(lengths[index])) * gains[index];
                left += tap * OUT_LEFT[index];
                right += tap * OUT_RIGHT[index];
                taps[index] = tap;
            }
            hadamard(&mut taps);
            for (index, line) in lines.iter_mut().enumerate() {
                // The tiny offset keeps a dying tail out of denormal
                // numbers, which are slow on some processors.
                line.push(taps[index] + feed[index % 2] + 1.0e-20);
            }

            let mid = 0.5 * (left + right) * WET_GAIN;
            let side = 0.5 * (left - right) * WET_GAIN * width;
            let mix = mix.next(mix_target);
            [
                frame.input[0] * (1.0 - mix) + (mid + side) * mix,
                frame.input[1] * (1.0 - mix) + (mid - side) * mix,
            ]
        });
    }

    fn params(&self) -> &[ParamInfo] {
        self.params.table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        self.params.set(param, value);
    }

    fn reset(&mut self) {
        for line in self.lines.iter_mut().chain(self.pre_delay.iter_mut()) {
            line.clear();
        }
        for diffuser in self.diffusers.iter_mut().flatten() {
            diffuser.line.clear();
        }
        for filter in self.damping.iter_mut().chain(self.low_cut.iter_mut()) {
            filter.reset();
        }
        self.mix.reset();
    }
}
