//! The one-knob DJ filter: low-pass to the left of centre, high-pass to
//! the right, bypassed in the middle. Built on `fourfloor-effects`'
//! [`Filter`].

use fourfloor_core::{
    Device, EventBuffer, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};
use fourfloor_effects::Filter;

use crate::inner::{at, process_into};

/// Cutoff of the low-pass with the knob fully left, in Hz.
pub const LOW_PASS_FLOOR_HZ: f32 = 60.0;
/// Cutoff of the high-pass with the knob fully right, in Hz.
pub const HIGH_PASS_CEILING_HZ: f32 = 10_000.0;
/// Knob travel from the centre over which the filter fades in.
const DEAD_ZONE: f32 = 0.05;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new(
        DjFilter::AMOUNT,
        "Filter",
        -1.0,
        1.0,
        0.0,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        DjFilter::RESONANCE,
        "Resonance",
        0.0,
        1.0,
        0.3,
        ParamUnit::Generic,
    ),
];

/// The DJ filter (see the [module docs](self)).
pub struct DjFilter {
    low_pass: Filter,
    high_pass: Filter,
    amount: f32,
    resonance: f32,
    wet: f32,
    glide: f32,
    stride: usize,
    low: Vec<f32>,
    high: Vec<f32>,
    events: EventBuffer,
}

impl Default for DjFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Cutoffs of the low-pass and high-pass for a knob position.
pub fn filter_cutoffs(amount: f32) -> (f32, f32) {
    let amount = if amount.is_finite() {
        amount.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let low = 20_000.0 * (LOW_PASS_FLOOR_HZ / 20_000.0).powf((-amount).max(0.0));
    let high = 20.0 * (HIGH_PASS_CEILING_HZ / 20.0).powf(amount.max(0.0));
    (low, high)
}

impl DjFilter {
    /// Knob position: -1 full low-pass, 0 off, 1 full high-pass.
    pub const AMOUNT: ParamId = ParamId(0);
    /// Resonance, 0 to 1.
    pub const RESONANCE: ParamId = ParamId(1);

    /// A filter with the knob at the centre.
    pub fn new() -> Self {
        let mut low_pass = Filter::new();
        let mut high_pass = Filter::new();
        low_pass.set_param(Filter::MODE, Filter::LOW_PASS as f32);
        high_pass.set_param(Filter::MODE, Filter::HIGH_PASS as f32);
        for filter in [&mut low_pass, &mut high_pass] {
            filter.set_param(Filter::STEEP, 1.0);
        }
        let mut this = Self {
            low_pass,
            high_pass,
            amount: 0.0,
            resonance: PARAMS[1].default,
            wet: 0.0,
            glide: 1.0,
            stride: 0,
            low: Vec::new(),
            high: Vec::new(),
            events: EventBuffer::with_capacity(0),
        };
        this.apply();
        this
    }

    fn apply(&mut self) {
        let (low, high) = filter_cutoffs(self.amount);
        self.low_pass.set_param(Filter::CUTOFF, low);
        self.high_pass.set_param(Filter::CUTOFF, high);
        self.low_pass.set_param(Filter::RESONANCE, self.resonance);
        self.high_pass.set_param(Filter::RESONANCE, self.resonance);
    }

    fn target_wet(&self) -> f32 {
        let x = (self.amount.abs() / DEAD_ZONE).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    }
}

impl Device for DjFilter {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.stride = config.max_block;
        let size = config.max_block * config.channels.max(2);
        self.low = vec![0.0; size];
        self.high = vec![0.0; size];
        self.low_pass.prepare(config);
        self.high_pass.prepare(config);
        self.glide = 1.0 - (-1.0 / (0.005 * config.sample_rate.as_f64() as f32)).exp();
        self.wet = self.target_wet();
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        if self.stride < frames || io.outputs.len() < 2 || io.inputs.len() < 2 {
            io.pass_through();
            return;
        }
        let channels = io.outputs.len().min(self.low.len() / self.stride);
        let inputs = &io.inputs[..io.inputs.len().min(channels)];
        let stride = self.stride;
        process_into(
            &mut self.low_pass,
            ctx,
            inputs,
            &mut self.low[..channels * stride],
            stride,
            frames,
            &mut self.events,
        );
        process_into(
            &mut self.high_pass,
            ctx,
            inputs,
            &mut self.high[..channels * stride],
            stride,
            frames,
            &mut self.events,
        );
        let target = self.target_wet();
        let filtered = if self.amount < 0.0 {
            &self.low
        } else {
            &self.high
        };
        for index in 0..frames {
            self.wet += (target - self.wet) * self.glide;
            let wet = self.wet;
            for channel in 0..2 {
                let dry = io.inputs[channel][index];
                let value = at(filtered, stride, channel, index);
                io.outputs[channel][index] = dry + (value - dry) * wet;
            }
        }
        for channel in io.outputs.iter_mut().skip(2) {
            channel.fill(0.0);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        match param {
            Self::AMOUNT => self.amount = PARAMS[0].clamp(value),
            Self::RESONANCE => self.resonance = PARAMS[1].clamp(value),
            _ => return,
        }
        self.apply();
    }

    fn reset(&mut self) {
        self.low_pass.reset();
        self.high_pass.reset();
        self.wet = self.target_wet();
    }
}
