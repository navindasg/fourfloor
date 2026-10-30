//! Lookahead brickwall limiter.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::delay_line::DelayLine;
use crate::true_peak::{TruePeakDetector, DELAY as TRUE_PEAK_DELAY};
use crate::util::{db_to_gain, process_stereo, time_coefficient, ParamSet};

const PARAM_COUNT: usize = 5;

/// In true-peak mode the detector reads 0.1 dB hot. The gain changes
/// across the reconstruction filter's span, and a meter's interpolator is
/// not the detector's, so without a margin the measured true peak can land
/// a few hundredths of a dB over the ceiling.
const TRUE_PEAK_MARGIN: f32 = 1.011_579_4;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(
        Limiter::INPUT_GAIN,
        "Input Gain",
        -12.0,
        36.0,
        0.0,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(
        Limiter::CEILING,
        "Ceiling",
        -24.0,
        0.0,
        -0.3,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(
        Limiter::RELEASE,
        "Release",
        1.0,
        1_000.0,
        80.0,
        ParamUnit::Milliseconds,
    )
    .logarithmic(),
    ParamInfo::new(
        Limiter::LOOKAHEAD,
        "Lookahead",
        0.5,
        10.0,
        5.0,
        ParamUnit::Milliseconds,
    ),
    ParamInfo::toggle(Limiter::TRUE_PEAK, "True Peak", false),
];

/// The smallest value over the last `window` pushes, in constant time per
/// push: a monotonic queue kept in fixed arrays.
#[derive(Clone, Debug)]
struct SlidingMinimum {
    values: Vec<f32>,
    ages: Vec<u64>,
    head: usize,
    len: usize,
    window: u64,
    count: u64,
}

impl SlidingMinimum {
    fn new() -> Self {
        Self {
            values: vec![1.0; 2],
            ages: vec![0; 2],
            head: 0,
            len: 0,
            window: 1,
            count: 0,
        }
    }

    fn prepare(&mut self, window: usize) {
        let window = window.max(1);
        self.values = vec![1.0; window + 1];
        self.ages = vec![0; window + 1];
        self.window = window as u64;
        self.reset();
    }

    fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
        self.count = 0;
    }

    #[inline]
    fn push(&mut self, value: f32) -> f32 {
        let capacity = self.values.len();
        // Anything not smaller than the new value can never be the minimum
        // again.
        while self.len > 0 {
            let back = (self.head + self.len - 1) % capacity;
            if self.values[back] >= value {
                self.len -= 1;
            } else {
                break;
            }
        }
        // Drop the front once it is older than the window.
        while self.len > 0 && self.ages[self.head] + self.window <= self.count {
            self.head = (self.head + 1) % capacity;
            self.len -= 1;
        }
        if self.len == capacity {
            // Cannot happen (the window is shorter than the arrays); give up
            // the oldest entry rather than write out of range.
            self.head = (self.head + 1) % capacity;
            self.len -= 1;
        }
        let slot = (self.head + self.len) % capacity;
        self.values[slot] = value;
        self.ages[slot] = self.count;
        self.len += 1;
        self.count += 1;
        self.values[self.head]
    }
}

/// A lookahead brickwall limiter (`fx.limiter`).
///
/// No output sample ever exceeds the ceiling. The limiter looks ahead by
/// `Lookahead` milliseconds, so the gain is already down, smoothly, when a
/// peak arrives; it reports that delay through
/// [`latency_samples`](Device::latency_samples) so the engine can
/// compensate. Material that stays under the ceiling passes through
/// unchanged, only delayed.
///
/// The gain needed at each sample is held as a minimum over the lookahead
/// window and then averaged over the same window. The average of minima
/// that all include a given sample can never exceed the gain that sample
/// needs, which is what makes the ceiling exact.
///
/// By default the ceiling applies to sample values, and peaks between
/// samples (true peak) can read a little higher after conversion to
/// analogue. With `True Peak` on, the detector also reconstructs the signal
/// at four times the sample rate ([`crate::true_peak`]) and limits those
/// peaks too, with 0.1 dB of margin, at the cost of eight more samples of
/// latency. Sample values still never exceed the ceiling.
///
/// `Lookahead` and `True Peak` are read in `prepare`: a later change takes effect the next
/// time the device is prepared, because the reported latency must not move
/// under the engine's delay compensation.
#[derive(Clone, Debug)]
pub struct Limiter {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    lookahead: usize,
    /// Extra audio delay that lines the signal up with the true-peak
    /// detector's output; 0 when true-peak detection is off.
    detector_delay: usize,
    true_peak: Option<TruePeakDetector>,
    delay: [DelayLine; 2],
    minimum: SlidingMinimum,
    /// The last `lookahead` minima, for the moving average.
    window: Vec<f32>,
    window_at: usize,
    window_sum: f64,
    /// Gain with the release applied, before the minimum.
    released: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    /// Gain before limiting, in decibels: drive the input into the
    /// ceiling.
    pub const INPUT_GAIN: ParamId = ParamId(0);
    /// The level no output sample exceeds, in dBFS.
    pub const CEILING: ParamId = ParamId(1);
    /// How fast the gain recovers after a peak, in milliseconds.
    pub const RELEASE: ParamId = ParamId(2);
    /// Lookahead, and so latency, in milliseconds. Read in `prepare`.
    pub const LOOKAHEAD: ParamId = ParamId(3);
    /// Limit inter-sample peaks as well as sample values. Read in
    /// `prepare`, because it changes the latency.
    pub const TRUE_PEAK: ParamId = ParamId(4);

    /// A limiter with a -0.3 dB ceiling and 5 ms of lookahead.
    pub fn new() -> Self {
        let mut limiter = Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            lookahead: 1,
            detector_delay: 0,
            true_peak: None,
            delay: [DelayLine::new(), DelayLine::new()],
            minimum: SlidingMinimum::new(),
            window: vec![1.0; 1],
            window_at: 0,
            window_sum: 1.0,
            released: 1.0,
        };
        limiter.configure();
        limiter
    }

    fn lookahead_for(&self) -> usize {
        let samples = self.params.get(Self::LOOKAHEAD) * 0.001 * self.sample_rate;
        (samples.round() as usize).max(1)
    }

    /// Sizes everything for the current lookahead. Allocates.
    fn configure(&mut self) {
        self.lookahead = self.lookahead_for();
        let true_peak = self.params.get(Self::TRUE_PEAK) >= 0.5;
        self.true_peak = true_peak.then(TruePeakDetector::new);
        self.detector_delay = if true_peak { TRUE_PEAK_DELAY } else { 0 };
        for line in &mut self.delay {
            line.prepare(self.lookahead + self.detector_delay + 1);
        }
        self.minimum.prepare(self.lookahead + 1);
        self.window = vec![1.0; self.lookahead];
        self.reset();
    }
}

impl Device for Limiter {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        self.configure();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let p = &self.params;
        let input_gain = db_to_gain(p.get(Self::INPUT_GAIN));
        let ceiling = db_to_gain(p.get(Self::CEILING));
        let release = time_coefficient(p.get(Self::RELEASE) * 0.001, self.sample_rate);
        let lookahead = self.lookahead + self.detector_delay;
        let length = self.window.len().max(1);
        let Self {
            delay,
            minimum,
            window,
            window_at,
            window_sum,
            released,
            true_peak,
            ..
        } = self;
        process_stereo(io, |frame| {
            let input = [frame.input[0] * input_gain, frame.input[1] * input_gain];
            let peak = match true_peak {
                Some(detector) => detector.push(input) * TRUE_PEAK_MARGIN,
                None => input[0].abs().max(input[1].abs()),
            };
            let needed = if peak > ceiling { ceiling / peak } else { 1.0 };
            // Fall at once, recover gradually.
            *released = needed.min(*released + (1.0 - *released) * release);
            let held = minimum.push(*released);
            if let Some(slot) = window.get_mut(*window_at) {
                *window_sum += f64::from(held) - f64::from(*slot);
                *slot = held;
            }
            *window_at = (*window_at + 1) % length;
            let gain = ((*window_sum / length as f64) as f32).clamp(0.0, 1.0);

            let mut out = [0.0f32; 2];
            for ((out, input), line) in out.iter_mut().zip(input).zip(delay.iter_mut()) {
                let delayed = line.tap(lookahead);
                line.push(input);
                // The clamp only ever catches rounding in the last bit.
                *out = (delayed * gain).clamp(-ceiling, ceiling);
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
        for line in &mut self.delay {
            line.clear();
        }
        self.minimum.reset();
        if let Some(detector) = &mut self.true_peak {
            detector.reset();
        }
        self.window.fill(1.0);
        self.window_at = 0;
        self.window_sum = self.window.len() as f64;
        self.released = 1.0;
    }

    fn latency_samples(&self) -> u32 {
        (self.lookahead + self.detector_delay) as u32
    }
}
