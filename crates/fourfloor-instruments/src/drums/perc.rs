//! Rim shot, tom and cowbell.

use std::f32::consts::TAU;

use fourfloor_core::{ParamId, ParamInfo, ParamUnit};

use super::{DrumVoice, NOISE_SEED};
use crate::dsp::{square, t60_coeff, tau_coeff, Biquad, Noise, SILENCE};

/// 909-style rim shot: three short, inharmonic partials driven hard.
#[derive(Clone, Debug)]
pub struct RimVoice {
    tune: f32,
    decay: f32,
    level: f32,
    rate: f32,
    coeff: f32,
    phase: [f32; 3],
    env: f32,
    velocity: f32,
    active: bool,
    highpass: Biquad,
}

static RIM_PARAMS: [ParamInfo; 3] = [
    ParamInfo::new(RimVoice::TUNE, "Tune", 0.5, 2.0, 1.0, ParamUnit::Ratio),
    ParamInfo::new(
        RimVoice::DECAY,
        "Decay",
        0.01,
        0.2,
        0.045,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(RimVoice::LEVEL, "Level", 0.0, 1.0, 0.8, ParamUnit::Generic),
];

impl RimVoice {
    /// Pitch as a ratio of the normal tuning.
    pub const TUNE: ParamId = ParamId(0);
    /// Time to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(1);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(2);

    /// Partial frequencies in Hz and their levels.
    const PARTIALS: [(f32, f32); 3] = [(455.0, 1.0), (1_120.0, 0.7), (1_667.0, 0.5)];

    fn update(&mut self) {
        self.coeff = t60_coeff(self.decay, self.rate);
        self.highpass.set_highpass(300.0, 0.7, self.rate);
    }
}

impl Default for RimVoice {
    fn default() -> Self {
        let p = &RIM_PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            decay: p[1].default,
            level: p[2].default,
            rate: 48_000.0,
            coeff: 0.0,
            phase: [0.0; 3],
            env: 0.0,
            velocity: 0.0,
            active: false,
            highpass: Biquad::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for RimVoice {
    fn param_table() -> &'static [ParamInfo] {
        &RIM_PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
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
        self.phase = [0.0; 3];
        self.env = 1.0;
        self.velocity = velocity;
        self.highpass.reset();
        self.active = true;
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let mut sum = 0.0;
        for (phase, (freq, gain)) in self.phase.iter_mut().zip(Self::PARTIALS) {
            sum += gain * (TAU * *phase).sin();
            *phase = (*phase + freq * self.tune / self.rate).fract();
        }
        let shaped = (1.5 * sum * self.env).tanh();
        let out = 0.8 * self.highpass.process(shaped) * self.velocity * self.level;
        self.env *= self.coeff;
        if self.env < SILENCE {
            self.active = false;
        }
        out
    }

    fn reset(&mut self) {
        self.active = false;
        self.env = 0.0;
        self.highpass.reset();
    }
}

/// 909-style tom: a sine that bends down onto its pitch, with a touch of
/// noise on the attack.
#[derive(Clone, Debug)]
pub struct TomVoice {
    tune: f32,
    decay: f32,
    bend: f32,
    level: f32,
    rate: f32,
    coeff: f32,
    pitch_coeff: f32,
    noise_coeff: f32,
    ratio: f32,
    phase: f32,
    env: f32,
    pitch_env: f32,
    noise_env: f32,
    velocity: f32,
    active: bool,
    noise: Noise,
    noise_filter: Biquad,
}

static TOM_PARAMS: [ParamInfo; 4] = [
    ParamInfo::new(TomVoice::TUNE, "Tune", 50.0, 400.0, 130.0, ParamUnit::Hertz),
    ParamInfo::new(TomVoice::DECAY, "Decay", 0.05, 1.5, 0.4, ParamUnit::Seconds),
    ParamInfo::new(TomVoice::BEND, "Bend", 0.0, 1.0, 0.5, ParamUnit::Generic),
    ParamInfo::new(TomVoice::LEVEL, "Level", 0.0, 1.0, 0.8, ParamUnit::Generic),
];

impl TomVoice {
    /// Resting pitch in Hz.
    pub const TUNE: ParamId = ParamId(0);
    /// Time to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(1);
    /// How far the pitch starts above `Tune`, 0 to 1 (1 is an octave).
    pub const BEND: ParamId = ParamId(2);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(3);

    fn update(&mut self) {
        self.coeff = t60_coeff(self.decay, self.rate);
        self.pitch_coeff = tau_coeff(0.03, self.rate);
        self.noise_coeff = t60_coeff(0.03, self.rate);
        self.noise_filter.set_bandpass(1_500.0, 0.8, self.rate);
    }

    /// Starts the tom at `ratio` times its set tuning.
    pub fn trigger_scaled(&mut self, ratio: f32, velocity: f32) {
        self.ratio = ratio;
        self.phase = 0.0;
        self.env = 1.0;
        self.pitch_env = 1.0;
        self.noise_env = 1.0;
        self.velocity = velocity;
        self.noise = Noise::new(NOISE_SEED ^ 0x70);
        self.noise_filter.reset();
        self.active = true;
    }

    /// Tuning ratio a stand-alone tom uses for a MIDI pitch: the six GM
    /// tom pitches spread over an octave, everything else at 1.
    pub fn gm_ratio(pitch: u8) -> f32 {
        match pitch {
            41 => 0.667,
            43 => 0.75,
            45 => 0.891,
            47 => 1.0,
            48 => 1.189,
            50 => 1.335,
            _ => 1.0,
        }
    }
}

impl Default for TomVoice {
    fn default() -> Self {
        let p = &TOM_PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            decay: p[1].default,
            bend: p[2].default,
            level: p[3].default,
            rate: 48_000.0,
            coeff: 0.0,
            pitch_coeff: 0.0,
            noise_coeff: 0.0,
            ratio: 1.0,
            phase: 0.0,
            env: 0.0,
            pitch_env: 0.0,
            noise_env: 0.0,
            velocity: 0.0,
            active: false,
            noise: Noise::new(NOISE_SEED ^ 0x70),
            noise_filter: Biquad::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for TomVoice {
    fn param_table() -> &'static [ParamInfo] {
        &TOM_PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
            Self::DECAY => self.decay = value,
            Self::BEND => self.bend = value,
            Self::LEVEL => self.level = value,
            _ => return,
        }
        self.update();
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sample_rate;
        self.update();
    }

    fn trigger(&mut self, pitch: u8, velocity: f32) {
        self.trigger_scaled(Self::gm_ratio(pitch), velocity);
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let freq = self.tune * self.ratio * (1.0 + self.bend * self.pitch_env);
        self.pitch_env *= self.pitch_coeff;
        let body = (TAU * self.phase).sin();
        self.phase = (self.phase + freq / self.rate).fract();
        let noise = self.noise_filter.process(self.noise.next()) * self.noise_env;
        self.noise_env *= self.noise_coeff;
        let out = (0.9 * body * self.env + 0.15 * noise) * self.velocity * self.level;
        self.env *= self.coeff;
        if self.env < SILENCE {
            self.active = false;
        }
        out
    }

    fn reset(&mut self) {
        self.active = false;
        self.env = 0.0;
        self.noise_filter.reset();
    }
}

/// 808-style cowbell: two square oscillators (540 Hz and 800 Hz) through a
/// band-pass, with a fast knock and a slower ring.
#[derive(Clone, Debug)]
pub struct Cowbell808Voice {
    tune: f32,
    decay: f32,
    level: f32,
    rate: f32,
    knock_coeff: f32,
    ring_coeff: f32,
    phase: [f32; 2],
    knock: f32,
    ring: f32,
    velocity: f32,
    active: bool,
    bandpass: Biquad,
}

static COWBELL_PARAMS: [ParamInfo; 3] = [
    ParamInfo::new(
        Cowbell808Voice::TUNE,
        "Tune",
        0.5,
        2.0,
        1.0,
        ParamUnit::Ratio,
    ),
    ParamInfo::new(
        Cowbell808Voice::DECAY,
        "Decay",
        0.05,
        1.5,
        0.4,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        Cowbell808Voice::LEVEL,
        "Level",
        0.0,
        1.0,
        0.8,
        ParamUnit::Generic,
    ),
];

impl Cowbell808Voice {
    /// Pitch as a ratio of the normal tuning.
    pub const TUNE: ParamId = ParamId(0);
    /// Time for the ring to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(1);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(2);

    /// The two oscillator frequencies in Hz at a tuning of 1.
    pub const FREQUENCIES: [f32; 2] = [540.0, 800.0];

    fn update(&mut self) {
        self.knock_coeff = t60_coeff(0.05, self.rate);
        self.ring_coeff = t60_coeff(self.decay, self.rate);
        self.bandpass
            .set_bandpass(1_000.0 * self.tune, 0.6, self.rate);
    }
}

impl Default for Cowbell808Voice {
    fn default() -> Self {
        let p = &COWBELL_PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            decay: p[1].default,
            level: p[2].default,
            rate: 48_000.0,
            knock_coeff: 0.0,
            ring_coeff: 0.0,
            phase: [0.0; 2],
            knock: 0.0,
            ring: 0.0,
            velocity: 0.0,
            active: false,
            bandpass: Biquad::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for Cowbell808Voice {
    fn param_table() -> &'static [ParamInfo] {
        &COWBELL_PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
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
        self.knock = 1.0;
        self.ring = 1.0;
        self.velocity = velocity;
        self.bandpass.reset();
        self.active = true;
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let mut sum = 0.0;
        for (phase, freq) in self.phase.iter_mut().zip(Self::FREQUENCIES) {
            let step = freq * self.tune / self.rate;
            sum += square(*phase, step, 0.5);
            *phase += step;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
        }
        let env = 0.6 * self.knock + 0.4 * self.ring;
        let out = 0.6 * self.bandpass.process(0.5 * sum) * env * self.velocity * self.level;
        self.knock *= self.knock_coeff;
        self.ring *= self.ring_coeff;
        if self.ring < SILENCE {
            self.active = false;
        }
        out
    }

    fn reset(&mut self) {
        self.active = false;
        self.knock = 0.0;
        self.ring = 0.0;
        self.bandpass.reset();
    }
}
