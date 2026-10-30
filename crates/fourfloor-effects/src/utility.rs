//! Utility: gain, pan, width, mono.

use std::f32::consts::FRAC_PI_2;

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::util::{db_to_gain, process_stereo, ParamSet, Smoothed};

const PARAM_COUNT: usize = 6;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(
        Utility::GAIN,
        "Gain",
        Utility::MIN_DB,
        24.0,
        0.0,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(Utility::PAN, "Pan", -1.0, 1.0, 0.0, ParamUnit::Generic),
    ParamInfo::new(Utility::WIDTH, "Width", 0.0, 2.0, 1.0, ParamUnit::Generic),
    ParamInfo::toggle(Utility::MONO, "Mono", false),
    ParamInfo::toggle(Utility::INVERT_LEFT, "Invert Left", false),
    ParamInfo::toggle(Utility::INVERT_RIGHT, "Invert Right", false),
];

/// A utility (`fx.utility`): gain, balance, stereo width, mono and polarity.
///
/// Order: polarity, then width (or mono), then balance, then gain. Width
/// scales the difference between the channels: 0 is mono, 1 leaves the
/// signal alone, 2 doubles the side signal. The balance law is the track
/// fader's: unity at centre, and panning one way turns the other channel
/// down along a cosine. Changes glide over about 5 ms.
#[derive(Clone, Debug)]
pub struct Utility {
    params: ParamSet<PARAM_COUNT>,
    gains: [Smoothed; 2],
    width: Smoothed,
}

impl Default for Utility {
    fn default() -> Self {
        Self::new()
    }
}

impl Utility {
    /// Gain in decibels. The minimum is silence.
    pub const GAIN: ParamId = ParamId(0);
    /// Balance, -1 (left) to 1 (right).
    pub const PAN: ParamId = ParamId(1);
    /// Stereo width, 0 (mono) to 2.
    pub const WIDTH: ParamId = ParamId(2);
    /// Sums left and right to mono.
    pub const MONO: ParamId = ParamId(3);
    /// Flips the polarity of the left channel.
    pub const INVERT_LEFT: ParamId = ParamId(4);
    /// Flips the polarity of the right channel.
    pub const INVERT_RIGHT: ParamId = ParamId(5);

    /// The bottom of the gain control; this value is silence.
    pub const MIN_DB: f32 = -96.0;

    /// A utility that changes nothing.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            gains: [Smoothed::new(); 2],
            width: Smoothed::new(),
        }
    }
}

impl Device for Utility {
    fn prepare(&mut self, config: &PrepareConfig) {
        let rate = config.sample_rate.hz() as f32;
        for gain in &mut self.gains {
            gain.set_time(0.005, rate);
        }
        self.width.set_time(0.005, rate);
        self.reset();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let p = &self.params;
        let db = p.get(Self::GAIN);
        let gain = if db <= Self::MIN_DB {
            0.0
        } else {
            db_to_gain(db)
        };
        let pan = p.get(Self::PAN);
        let targets = [
            gain * (pan.max(0.0) * FRAC_PI_2).cos(),
            gain * ((-pan).max(0.0) * FRAC_PI_2).cos(),
        ];
        let width_target = if p.on(Self::MONO) {
            0.0
        } else {
            p.get(Self::WIDTH)
        };
        let polarity = [
            if p.on(Self::INVERT_LEFT) { -1.0 } else { 1.0 },
            if p.on(Self::INVERT_RIGHT) { -1.0 } else { 1.0 },
        ];
        let (gains, width) = (&mut self.gains, &mut self.width);
        process_stereo(io, |frame| {
            let left = frame.input[0] * polarity[0];
            let right = frame.input[1] * polarity[1];
            let width = width.next(width_target);
            let (left, right) = if width == 1.0 {
                (left, right)
            } else {
                let mid = 0.5 * (left + right);
                let side = 0.5 * (left - right) * width;
                (mid + side, mid - side)
            };
            [
                left * gains[0].next(targets[0]),
                right * gains[1].next(targets[1]),
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
        for gain in &mut self.gains {
            gain.reset();
        }
        self.width.reset();
    }
}
