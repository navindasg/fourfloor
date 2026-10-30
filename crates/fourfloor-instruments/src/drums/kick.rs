//! Kick drums: a swept sine with a click (909) and a long, soft boom (808).

use std::f32::consts::TAU;

use fourfloor_core::{ParamId, ParamInfo, ParamUnit};

use super::{DrumVoice, NOISE_SEED};
use crate::dsp::{t60_coeff, tau_coeff, Noise, OnePole, SILENCE};

/// 909-style kick: a sine whose pitch falls from `Tune + Sweep` to `Tune`,
/// lightly saturated, with a short noise click on the attack.
#[derive(Clone, Debug)]
pub struct Kick909Voice {
    tune: f32,
    decay: f32,
    sweep: f32,
    sweep_ms: f32,
    click: f32,
    drive: f32,
    level: f32,
    rate: f32,
    amp_coeff: f32,
    pitch_coeff: f32,
    click_coeff: f32,
    drive_gain: f32,
    drive_norm: f32,
    phase: f32,
    amp: f32,
    pitch_env: f32,
    click_env: f32,
    velocity: f32,
    active: bool,
    noise: Noise,
    click_filter: OnePole,
}

static KICK909_PARAMS: [ParamInfo; 7] = [
    ParamInfo::new(
        Kick909Voice::TUNE,
        "Tune",
        30.0,
        100.0,
        48.0,
        ParamUnit::Hertz,
    ),
    ParamInfo::new(
        Kick909Voice::DECAY,
        "Decay",
        0.05,
        1.5,
        0.45,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        Kick909Voice::SWEEP,
        "Sweep",
        0.0,
        600.0,
        220.0,
        ParamUnit::Hertz,
    ),
    ParamInfo::new(
        Kick909Voice::SWEEP_TIME,
        "Sweep Time",
        5.0,
        120.0,
        30.0,
        ParamUnit::Milliseconds,
    ),
    ParamInfo::new(
        Kick909Voice::CLICK,
        "Click",
        0.0,
        1.0,
        0.3,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Kick909Voice::DRIVE,
        "Drive",
        0.0,
        1.0,
        0.25,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Kick909Voice::LEVEL,
        "Level",
        0.0,
        1.0,
        0.8,
        ParamUnit::Generic,
    ),
];

impl Kick909Voice {
    /// Resting pitch of the drum in Hz: its fundamental once the sweep is
    /// over.
    pub const TUNE: ParamId = ParamId(0);
    /// Time for the body to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(1);
    /// How far above `Tune` the pitch starts, in Hz.
    pub const SWEEP: ParamId = ParamId(2);
    /// Time constant of the pitch fall, in milliseconds.
    pub const SWEEP_TIME: ParamId = ParamId(3);
    /// Level of the attack click, 0 to 1.
    pub const CLICK: ParamId = ParamId(4);
    /// Saturation of the body, 0 to 1.
    pub const DRIVE: ParamId = ParamId(5);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(6);

    fn update(&mut self) {
        self.amp_coeff = t60_coeff(self.decay, self.rate);
        self.pitch_coeff = tau_coeff(self.sweep_ms * 0.001, self.rate);
        self.click_coeff = t60_coeff(0.012, self.rate);
        self.drive_gain = 1.0 + 4.0 * self.drive;
        self.drive_norm = 1.0 / self.drive_gain.tanh();
        self.click_filter.set_lowpass(3_500.0, self.rate);
    }
}

impl Default for Kick909Voice {
    fn default() -> Self {
        let p = &KICK909_PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            decay: p[1].default,
            sweep: p[2].default,
            sweep_ms: p[3].default,
            click: p[4].default,
            drive: p[5].default,
            level: p[6].default,
            rate: 48_000.0,
            amp_coeff: 0.0,
            pitch_coeff: 0.0,
            click_coeff: 0.0,
            drive_gain: 1.0,
            drive_norm: 1.0,
            phase: 0.0,
            amp: 0.0,
            pitch_env: 0.0,
            click_env: 0.0,
            velocity: 0.0,
            active: false,
            noise: Noise::new(NOISE_SEED),
            click_filter: OnePole::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for Kick909Voice {
    fn param_table() -> &'static [ParamInfo] {
        &KICK909_PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
            Self::DECAY => self.decay = value,
            Self::SWEEP => self.sweep = value,
            Self::SWEEP_TIME => self.sweep_ms = value,
            Self::CLICK => self.click = value,
            Self::DRIVE => self.drive = value,
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
        self.phase = 0.0;
        self.amp = 1.0;
        self.pitch_env = 1.0;
        self.click_env = 1.0;
        self.velocity = velocity;
        self.noise = Noise::new(NOISE_SEED);
        self.click_filter.reset();
        self.active = true;
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let freq = self.tune + self.sweep * self.pitch_env;
        self.pitch_env *= self.pitch_coeff;
        let body = ((TAU * self.phase).sin() * self.drive_gain).tanh() * self.drive_norm;
        self.phase += freq / self.rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let click = self.click_filter.process(self.noise.next()) * self.click_env * self.click;
        self.click_env *= self.click_coeff;
        let out = (body * self.amp + 0.5 * click) * self.velocity * self.level;
        self.amp *= self.amp_coeff;
        if self.amp < SILENCE {
            self.active = false;
        }
        out
    }

    fn reset(&mut self) {
        self.active = false;
        self.amp = 0.0;
        self.click_filter.reset();
    }
}

/// 808-style kick: a pure, long sine boom with a brief pitch blip and a
/// soft filtered pulse on the attack.
#[derive(Clone, Debug)]
pub struct Kick808Voice {
    tune: f32,
    decay: f32,
    tone: f32,
    level: f32,
    rate: f32,
    amp_coeff: f32,
    pitch_coeff: f32,
    pulse_len: u32,
    phase: f32,
    amp: f32,
    pitch_env: f32,
    age: u32,
    velocity: f32,
    active: bool,
    pulse_filter: OnePole,
}

static KICK808_PARAMS: [ParamInfo; 4] = [
    ParamInfo::new(
        Kick808Voice::TUNE,
        "Tune",
        30.0,
        90.0,
        50.0,
        ParamUnit::Hertz,
    ),
    ParamInfo::new(
        Kick808Voice::DECAY,
        "Decay",
        0.1,
        4.0,
        1.0,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        Kick808Voice::TONE,
        "Tone",
        0.0,
        1.0,
        0.4,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Kick808Voice::LEVEL,
        "Level",
        0.0,
        1.0,
        0.8,
        ParamUnit::Generic,
    ),
];

impl Kick808Voice {
    /// Pitch of the boom in Hz.
    pub const TUNE: ParamId = ParamId(0);
    /// Time for the boom to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(1);
    /// Brightness and level of the attack pulse, 0 to 1.
    pub const TONE: ParamId = ParamId(2);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(3);

    /// How far above `Tune` the pitch starts, as a fraction of `Tune`.
    const BLIP: f32 = 1.2;

    fn update(&mut self) {
        self.amp_coeff = t60_coeff(self.decay, self.rate);
        self.pitch_coeff = tau_coeff(0.006, self.rate);
        self.pulse_len = (0.001 * self.rate) as u32;
        self.pulse_filter
            .set_lowpass(300.0 + 2_700.0 * self.tone, self.rate);
    }
}

impl Default for Kick808Voice {
    fn default() -> Self {
        let p = &KICK808_PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            decay: p[1].default,
            tone: p[2].default,
            level: p[3].default,
            rate: 48_000.0,
            amp_coeff: 0.0,
            pitch_coeff: 0.0,
            pulse_len: 0,
            phase: 0.0,
            amp: 0.0,
            pitch_env: 0.0,
            age: 0,
            velocity: 0.0,
            active: false,
            pulse_filter: OnePole::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for Kick808Voice {
    fn param_table() -> &'static [ParamInfo] {
        &KICK808_PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
            Self::DECAY => self.decay = value,
            Self::TONE => self.tone = value,
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
        self.phase = 0.0;
        self.amp = 1.0;
        self.pitch_env = 1.0;
        self.age = 0;
        self.velocity = velocity;
        self.pulse_filter.reset();
        self.active = true;
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let freq = self.tune * (1.0 + Self::BLIP * self.pitch_env);
        self.pitch_env *= self.pitch_coeff;
        let body = (TAU * self.phase).sin();
        self.phase += freq / self.rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let pulse_in = if self.age < self.pulse_len { 1.0 } else { 0.0 };
        self.age = self.age.saturating_add(1);
        let pulse = self.pulse_filter.process(pulse_in) * self.tone;
        let out = (body * self.amp + 0.4 * pulse) * self.velocity * self.level;
        self.amp *= self.amp_coeff;
        if self.amp < SILENCE {
            self.active = false;
        }
        out
    }

    fn reset(&mut self) {
        self.active = false;
        self.amp = 0.0;
        self.pulse_filter.reset();
    }
}
