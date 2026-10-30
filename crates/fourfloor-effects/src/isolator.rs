//! DJ-style three-band isolator.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::biquad::{Biquad, Coefficients, BUTTERWORTH_Q};
use crate::util::{db_to_gain, process_stereo, ParamSet, Smoothed};

const PARAM_COUNT: usize = 5;

const fn band(id: ParamId, name: &'static str) -> ParamInfo {
    ParamInfo::new(id, name, Isolator::KILL_DB, 6.0, 0.0, ParamUnit::Decibels)
}

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    band(Isolator::LOW, "Low"),
    band(Isolator::MID, "Mid"),
    band(Isolator::HIGH, "High"),
    ParamInfo::new(
        Isolator::LOW_CROSSOVER,
        "Low Crossover",
        80.0,
        1_000.0,
        250.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        Isolator::HIGH_CROSSOVER,
        "High Crossover",
        1_000.0,
        10_000.0,
        2_500.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
];

/// Filter memory for one channel.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    low: [Biquad; 2],
    low_align: Biquad,
    rest: [Biquad; 2],
    mid: [Biquad; 2],
    high: [Biquad; 2],
}

/// A three-band DJ isolator (`fx.isolator`).
///
/// The signal is split by two fourth-order Linkwitz-Riley crossovers
/// (24 dB/octave) into low, mid and high, and each band has a gain from a
/// full kill to +6 dB. With all three bands at 0 dB the bands add back to a
/// flat response (the phase is that of two all-pass filters, as in an
/// analogue isolator). Band gains glide over about 10 ms.
#[derive(Clone, Debug)]
pub struct Isolator {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f64,
    low_pass: [Coefficients; 2],
    high_pass: [Coefficients; 2],
    align: Coefficients,
    channels: [Channel; 2],
    gains: [Smoothed; 3],
}

impl Default for Isolator {
    fn default() -> Self {
        Self::new()
    }
}

impl Isolator {
    /// Low band gain in decibels.
    pub const LOW: ParamId = ParamId(0);
    /// Mid band gain in decibels.
    pub const MID: ParamId = ParamId(1);
    /// High band gain in decibels.
    pub const HIGH: ParamId = ParamId(2);
    /// Frequency where low hands over to mid, in Hz.
    pub const LOW_CROSSOVER: ParamId = ParamId(3);
    /// Frequency where mid hands over to high, in Hz.
    pub const HIGH_CROSSOVER: ParamId = ParamId(4);

    /// The bottom of a band control: this value mutes the band completely.
    pub const KILL_DB: f32 = -60.0;

    /// An isolator with all bands at 0 dB.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            low_pass: [Coefficients::IDENTITY; 2],
            high_pass: [Coefficients::IDENTITY; 2],
            align: Coefficients::IDENTITY,
            channels: [Channel::default(); 2],
            gains: [Smoothed::new(); 3],
        }
    }

    fn band_gain(&self, id: ParamId) -> f32 {
        let db = self.params.get(id);
        if db <= Self::KILL_DB {
            0.0
        } else {
            db_to_gain(db)
        }
    }

    fn update(&mut self) {
        let rate = self.sample_rate;
        let crossovers = [
            f64::from(self.params.get(Self::LOW_CROSSOVER)),
            f64::from(self.params.get(Self::HIGH_CROSSOVER)),
        ];
        for (index, frequency) in crossovers.into_iter().enumerate() {
            self.low_pass[index] = Coefficients::low_pass(rate, frequency, BUTTERWORTH_Q);
            self.high_pass[index] = Coefficients::high_pass(rate, frequency, BUTTERWORTH_Q);
        }
        // The mid and high bands pass through the upper crossover, whose
        // two halves add up to this all-pass. The low band gets the same
        // phase shift so that all three add up flat.
        self.align = Coefficients::all_pass(rate, crossovers[1], BUTTERWORTH_Q);
    }
}

impl Device for Isolator {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.as_f64();
        for gain in &mut self.gains {
            gain.set_time(0.01, self.sample_rate as f32);
        }
        self.params.mark_dirty();
        self.reset();
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        if self.params.take_dirty() {
            self.update();
        }
        let targets = [
            self.band_gain(Self::LOW),
            self.band_gain(Self::MID),
            self.band_gain(Self::HIGH),
        ];
        let (low_pass, high_pass, align) = (self.low_pass, self.high_pass, self.align);
        let (channels, gains) = (&mut self.channels, &mut self.gains);
        process_stereo(io, |frame| {
            let gain = [
                f64::from(gains[0].next(targets[0])),
                f64::from(gains[1].next(targets[1])),
                f64::from(gains[2].next(targets[2])),
            ];
            let mut out = [0.0f32; 2];
            for ((out, input), ch) in out.iter_mut().zip(frame.input).zip(channels.iter_mut()) {
                let x = f64::from(input);
                let mut low = x;
                let mut rest = x;
                for stage in 0..2 {
                    low = ch.low[stage].tick(&low_pass[0], low);
                    rest = ch.rest[stage].tick(&high_pass[0], rest);
                }
                let low = ch.low_align.tick(&align, low);
                let (mut mid, mut high) = (rest, rest);
                for stage in 0..2 {
                    mid = ch.mid[stage].tick(&low_pass[1], mid);
                    high = ch.high[stage].tick(&high_pass[1], high);
                }
                *out = (low * gain[0] + mid * gain[1] + high * gain[2]) as f32;
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
        self.channels = [Channel::default(); 2];
        for gain in &mut self.gains {
            gain.reset();
        }
    }
}
