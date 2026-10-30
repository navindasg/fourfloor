//! 909-style snare: two pitched shells plus filtered noise for the snares.

use std::f32::consts::TAU;

use fourfloor_core::{ParamId, ParamInfo, ParamUnit};

use super::{DrumVoice, NOISE_SEED};
use crate::dsp::{t60_coeff, tau_coeff, Biquad, Noise, SILENCE};

/// 909-style snare drum.
#[derive(Clone, Debug)]
pub struct Snare909Voice {
    tune: f32,
    tone: f32,
    snappy: f32,
    decay: f32,
    level: f32,
    rate: f32,
    body_coeff: f32,
    noise_coeff: f32,
    pitch_coeff: f32,
    phase: [f32; 2],
    body_env: f32,
    noise_env: f32,
    pitch_env: f32,
    velocity: f32,
    active: bool,
    noise: Noise,
    highpass: Biquad,
    lowpass: Biquad,
}

static PARAMS: [ParamInfo; 5] = [
    ParamInfo::new(
        Snare909Voice::TUNE,
        "Tune",
        120.0,
        300.0,
        185.0,
        ParamUnit::Hertz,
    ),
    ParamInfo::new(
        Snare909Voice::TONE,
        "Tone",
        0.0,
        1.0,
        0.5,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Snare909Voice::SNAPPY,
        "Snappy",
        0.0,
        1.0,
        0.6,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Snare909Voice::DECAY,
        "Decay",
        0.05,
        0.8,
        0.25,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        Snare909Voice::LEVEL,
        "Level",
        0.0,
        1.0,
        0.8,
        ParamUnit::Generic,
    ),
];

impl Snare909Voice {
    /// Pitch of the lower shell in Hz.
    pub const TUNE: ParamId = ParamId(0);
    /// Brightness of the snare noise, 0 to 1.
    pub const TONE: ParamId = ParamId(1);
    /// Level of the snare noise, 0 to 1.
    pub const SNAPPY: ParamId = ParamId(2);
    /// Time for the snare noise to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(3);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(4);

    /// Frequency of the upper shell relative to the lower one.
    const SHELL_RATIO: f32 = 1.78;

    fn update(&mut self) {
        self.noise_coeff = t60_coeff(self.decay, self.rate);
        self.body_coeff = t60_coeff(self.decay * 0.6, self.rate);
        self.pitch_coeff = tau_coeff(0.008, self.rate);
        self.highpass.set_highpass(900.0, 0.7, self.rate);
        self.lowpass
            .set_lowpass(4_000.0 + 8_000.0 * self.tone, 0.7, self.rate);
    }
}

impl Default for Snare909Voice {
    fn default() -> Self {
        let p = &PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            tone: p[1].default,
            snappy: p[2].default,
            decay: p[3].default,
            level: p[4].default,
            rate: 48_000.0,
            body_coeff: 0.0,
            noise_coeff: 0.0,
            pitch_coeff: 0.0,
            phase: [0.0; 2],
            body_env: 0.0,
            noise_env: 0.0,
            pitch_env: 0.0,
            velocity: 0.0,
            active: false,
            noise: Noise::new(NOISE_SEED ^ 0x51),
            highpass: Biquad::bypass(),
            lowpass: Biquad::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for Snare909Voice {
    fn param_table() -> &'static [ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
            Self::TONE => self.tone = value,
            Self::SNAPPY => self.snappy = value,
            Self::DECAY => self.decay = value,
            Self::LEVEL => self.level = value,
            _ => return,
        }
        self.update();
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sample_rate;
        self.update();
    }

    fn trigger(&mut self, _pitch: u8, velocity: f32) {
        self.phase = [0.0; 2];
        self.body_env = 1.0;
        self.noise_env = 1.0;
        self.pitch_env = 1.0;
        self.velocity = velocity;
        self.noise = Noise::new(NOISE_SEED ^ 0x51);
        self.highpass.reset();
        self.lowpass.reset();
        self.active = true;
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let bend = 1.0 + 0.6 * self.pitch_env;
        self.pitch_env *= self.pitch_coeff;
        let low = (TAU * self.phase[0]).sin();
        let high = (TAU * self.phase[1]).sin();
        self.phase[0] = (self.phase[0] + self.tune * bend / self.rate).fract();
        self.phase[1] = (self.phase[1] + self.tune * Self::SHELL_RATIO * bend / self.rate).fract();
        let body = (0.6 * low + 0.35 * high) * self.body_env;
        let rattle = self
            .lowpass
            .process(self.highpass.process(self.noise.next()))
            * self.noise_env
            * self.snappy;
        let out = (0.7 * body + 0.6 * rattle) * self.velocity * self.level;
        self.body_env *= self.body_coeff;
        self.noise_env *= self.noise_coeff;
        if self.noise_env < SILENCE {
            self.active = false;
        }
        out
    }

    fn reset(&mut self) {
        self.active = false;
        self.body_env = 0.0;
        self.noise_env = 0.0;
        self.highpass.reset();
        self.lowpass.reset();
    }
}
