//! Compressor with an external sidechain input, and its glue (bus)
//! variant.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::biquad::{Biquad, Coefficients, BUTTERWORTH_Q};
use crate::util::{db_to_gain, process_stereo, time_coefficient, ParamSet, Smoothed};

const PARAM_COUNT: usize = 10;
/// Averaging time of the RMS detector.
const RMS_SECONDS: f32 = 0.01;
/// Key high-pass settings at or below this are "off".
const KEY_FILTER_OFF_HZ: f32 = 20.5;

/// Starting values that differ between the two registered kinds.
struct Defaults {
    threshold: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    knee: f32,
    key_high_pass: f32,
    rms: bool,
}

const fn table(d: Defaults) -> [ParamInfo; PARAM_COUNT] {
    [
        ParamInfo::new(
            Compressor::THRESHOLD,
            "Threshold",
            -60.0,
            0.0,
            d.threshold,
            ParamUnit::Decibels,
        ),
        ParamInfo::new(
            Compressor::RATIO,
            "Ratio",
            1.0,
            20.0,
            d.ratio,
            ParamUnit::Ratio,
        )
        .logarithmic(),
        ParamInfo::new(
            Compressor::ATTACK,
            "Attack",
            0.05,
            250.0,
            d.attack,
            ParamUnit::Milliseconds,
        )
        .logarithmic(),
        ParamInfo::new(
            Compressor::RELEASE,
            "Release",
            5.0,
            2_000.0,
            d.release,
            ParamUnit::Milliseconds,
        )
        .logarithmic(),
        ParamInfo::new(
            Compressor::KNEE,
            "Knee",
            0.0,
            24.0,
            d.knee,
            ParamUnit::Decibels,
        ),
        ParamInfo::new(
            Compressor::MAKEUP,
            "Makeup",
            -12.0,
            36.0,
            0.0,
            ParamUnit::Decibels,
        ),
        ParamInfo::new(Compressor::MIX, "Mix", 0.0, 1.0, 1.0, ParamUnit::Generic),
        ParamInfo::toggle(Compressor::EXTERNAL_KEY, "External Key", true),
        ParamInfo::new(
            Compressor::KEY_HIGH_PASS,
            "Key High Pass",
            20.0,
            500.0,
            d.key_high_pass,
            ParamUnit::Hertz,
        )
        .logarithmic(),
        ParamInfo::toggle(Compressor::RMS, "RMS Detector", d.rms),
    ]
}

static COMPRESSOR_PARAMS: [ParamInfo; PARAM_COUNT] = table(Defaults {
    threshold: -18.0,
    ratio: 4.0,
    attack: 10.0,
    release: 120.0,
    knee: 6.0,
    key_high_pass: 20.0,
    rms: false,
});

static GLUE_PARAMS: [ParamInfo; PARAM_COUNT] = table(Defaults {
    threshold: -16.0,
    ratio: 2.0,
    attack: 30.0,
    release: 300.0,
    knee: 12.0,
    key_high_pass: 90.0,
    rms: true,
});

/// A feed-forward compressor (`fx.compressor`, and `fx.glue` with bus
/// settings).
///
/// The detector listens to the sidechain input when one is connected and
/// `External Key` is on, and to the main input otherwise. Left and right
/// share one gain, so the stereo image does not move. The detector can be
/// high-passed so that bass does not drive the compression.
///
/// The level detector is the "smooth, decoupled" log-domain design from
/// Giannoulis, Massberg and Reiss (2012): `Attack` is the time constant with
/// which gain reduction sets in and `Release` the one with which it lets
/// go.
///
/// [`Compressor::glue`] is the same processor with the settings of a mix-bus
/// compressor: RMS detection, 2:1, a wide knee, slow attack and a 90 Hz key
/// high-pass, so a kick does not pump the whole mix.
#[derive(Clone, Debug)]
pub struct Compressor {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    key_filter: Coefficients,
    key_state: [Biquad; 2],
    mean_square: f32,
    /// Release-smoothed gain reduction in dB (positive).
    held: f32,
    /// Gain reduction applied, in dB (positive).
    reduction: f32,
    attack: f32,
    release: f32,
    rms_coefficient: f32,
    makeup: Smoothed,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    /// Level above which gain is reduced, in dBFS.
    pub const THRESHOLD: ParamId = ParamId(0);
    /// Compression ratio: 4 means 4:1.
    pub const RATIO: ParamId = ParamId(1);
    /// Attack time constant in milliseconds.
    pub const ATTACK: ParamId = ParamId(2);
    /// Release time constant in milliseconds.
    pub const RELEASE: ParamId = ParamId(3);
    /// Width of the soft knee in decibels; 0 is a hard knee.
    pub const KNEE: ParamId = ParamId(4);
    /// Gain after compression, in decibels.
    pub const MAKEUP: ParamId = ParamId(5);
    /// Dry/wet mix: 1 is fully compressed.
    pub const MIX: ParamId = ParamId(6);
    /// Key from the sidechain input when one is connected.
    pub const EXTERNAL_KEY: ParamId = ParamId(7);
    /// High-pass on the key signal, in Hz. 20 is off.
    pub const KEY_HIGH_PASS: ParamId = ParamId(8);
    /// Detect the RMS level instead of the peak level.
    pub const RMS: ParamId = ParamId(9);

    /// A general-purpose compressor.
    pub fn new() -> Self {
        Self::with_table(&COMPRESSOR_PARAMS)
    }

    /// The glue (bus) compressor: the same processor with mix-bus
    /// defaults.
    pub fn glue() -> Self {
        Self::with_table(&GLUE_PARAMS)
    }

    fn with_table(table: &'static [ParamInfo; PARAM_COUNT]) -> Self {
        Self {
            params: ParamSet::new(table),
            sample_rate: 48_000.0,
            key_filter: Coefficients::IDENTITY,
            key_state: [Biquad::default(); 2],
            mean_square: 0.0,
            held: 0.0,
            reduction: 0.0,
            attack: 1.0,
            release: 1.0,
            rms_coefficient: 1.0,
            makeup: Smoothed::new(),
        }
    }

    /// Gain reduction being applied right now, in decibels (0 or more).
    /// For meters.
    pub fn gain_reduction_db(&self) -> f32 {
        self.reduction
    }

    fn update(&mut self) {
        let rate = self.sample_rate;
        self.attack = time_coefficient(self.params.get(Self::ATTACK) * 0.001, rate);
        self.release = time_coefficient(self.params.get(Self::RELEASE) * 0.001, rate);
        self.rms_coefficient = time_coefficient(RMS_SECONDS, rate);
        self.key_filter = Coefficients::high_pass(
            f64::from(rate),
            f64::from(self.params.get(Self::KEY_HIGH_PASS)),
            BUTTERWORTH_Q,
        );
    }
}

/// Static curve: decibels of gain reduction (0 or more) for a detector
/// level `level_db`.
#[inline]
fn reduction_for(level_db: f32, threshold: f32, slope: f32, knee: f32) -> f32 {
    let over = level_db - threshold;
    if 2.0 * over <= -knee {
        0.0
    } else if knee > 0.0 && 2.0 * over < knee {
        let into = over + knee * 0.5;
        slope * into * into / (2.0 * knee)
    } else {
        slope * over
    }
}

impl Device for Compressor {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        self.makeup.set_time(0.01, self.sample_rate);
        self.params.mark_dirty();
        self.reset();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        if self.params.take_dirty() {
            self.update();
        }
        let p = &self.params;
        let threshold = p.get(Self::THRESHOLD);
        let slope = 1.0 - 1.0 / p.get(Self::RATIO).max(1.0);
        let knee = p.get(Self::KNEE);
        let mix = p.get(Self::MIX);
        let makeup_target = db_to_gain(p.get(Self::MAKEUP));
        let external = p.on(Self::EXTERNAL_KEY) && io.sidechain.is_some();
        let filter_key = p.get(Self::KEY_HIGH_PASS) > KEY_FILTER_OFF_HZ;
        let rms = p.on(Self::RMS);
        let (attack, release, rms_coefficient) = (self.attack, self.release, self.rms_coefficient);
        let key_filter = self.key_filter;

        let mut key_state = self.key_state;
        let mut mean_square = self.mean_square;
        let mut held = self.held;
        let mut reduction = self.reduction;
        let makeup = &mut self.makeup;
        process_stereo(io, |frame| {
            let mut key = if external { frame.key } else { frame.input };
            if filter_key {
                for (sample, state) in key.iter_mut().zip(key_state.iter_mut()) {
                    *sample = state.tick(&key_filter, f64::from(*sample)) as f32;
                }
            }
            let level = if rms {
                let square = 0.5 * (key[0] * key[0] + key[1] * key[1]);
                mean_square += (square - mean_square) * rms_coefficient;
                mean_square.max(0.0).sqrt()
            } else {
                key[0].abs().max(key[1].abs())
            };
            let level_db = 20.0 * level.max(1.0e-9).log10();
            let wanted = reduction_for(level_db, threshold, slope, knee);
            held = wanted.max(held + (wanted - held) * release);
            reduction += (held - reduction) * attack;
            let gain = db_to_gain(-reduction) * makeup.next(makeup_target);
            let wet = mix * gain + (1.0 - mix);
            [frame.input[0] * wet, frame.input[1] * wet]
        });
        // Let the state fall to exactly zero instead of lingering in
        // denormals.
        self.key_state = key_state;
        self.mean_square = if mean_square < 1.0e-20 {
            0.0
        } else {
            mean_square
        };
        self.held = if held < 1.0e-9 { 0.0 } else { held };
        self.reduction = if reduction < 1.0e-9 { 0.0 } else { reduction };
    }

    fn params(&self) -> &[ParamInfo] {
        self.params.table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        self.params.set(param, value);
    }

    fn reset(&mut self) {
        self.key_state = [Biquad::default(); 2];
        self.mean_square = 0.0;
        self.held = 0.0;
        self.reduction = 0.0;
        self.makeup.reset();
    }
}
