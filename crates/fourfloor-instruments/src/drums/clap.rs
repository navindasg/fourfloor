//! 909-style hand clap: band-passed noise, three quick bursts then a tail.

use fourfloor_core::{ParamId, ParamInfo, ParamUnit};

use super::{DrumVoice, NOISE_SEED};
use crate::dsp::{t60_coeff, Biquad, Noise, SILENCE};

/// 909-style hand clap.
#[derive(Clone, Debug)]
pub struct ClapVoice {
    tone: f32,
    decay: f32,
    spread_ms: f32,
    level: f32,
    rate: f32,
    burst_coeff: f32,
    tail_coeff: f32,
    gap: u32,
    age: u32,
    env: f32,
    velocity: f32,
    active: bool,
    noise: Noise,
    bandpass: Biquad,
    highpass: Biquad,
}

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new(
        ClapVoice::TONE,
        "Tone",
        600.0,
        3_000.0,
        1_200.0,
        ParamUnit::Hertz,
    ),
    ParamInfo::new(
        ClapVoice::DECAY,
        "Decay",
        0.05,
        1.0,
        0.3,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        ClapVoice::SPREAD,
        "Spread",
        4.0,
        20.0,
        10.0,
        ParamUnit::Milliseconds,
    ),
    ParamInfo::new(ClapVoice::LEVEL, "Level", 0.0, 1.0, 0.8, ParamUnit::Generic),
];

impl ClapVoice {
    /// Centre frequency of the noise band in Hz.
    pub const TONE: ParamId = ParamId(0);
    /// Time for the tail to fall by 60 dB.
    pub const DECAY: ParamId = ParamId(1);
    /// Gap between the opening bursts, in milliseconds.
    pub const SPREAD: ParamId = ParamId(2);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(3);

    /// Bursts before the tail.
    const BURSTS: u32 = 3;

    fn update(&mut self) {
        self.gap = ((self.spread_ms * 0.001 * self.rate) as u32).max(1);
        self.burst_coeff = t60_coeff(self.spread_ms * 0.001 * 2.5, self.rate);
        self.tail_coeff = t60_coeff(self.decay, self.rate);
        self.bandpass.set_bandpass(self.tone, 1.4, self.rate);
        self.highpass.set_highpass(500.0, 0.7, self.rate);
    }
}

impl Default for ClapVoice {
    fn default() -> Self {
        let p = &PARAMS;
        let mut voice = Self {
            tone: p[0].default,
            decay: p[1].default,
            spread_ms: p[2].default,
            level: p[3].default,
            rate: 48_000.0,
            burst_coeff: 0.0,
            tail_coeff: 0.0,
            gap: 1,
            age: 0,
            env: 0.0,
            velocity: 0.0,
            active: false,
            noise: Noise::new(NOISE_SEED ^ 0xC1A9),
            bandpass: Biquad::bypass(),
            highpass: Biquad::bypass(),
        };
        voice.update();
        voice
    }
}

impl DrumVoice for ClapVoice {
    fn param_table() -> &'static [ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TONE => self.tone = value,
            Self::DECAY => self.decay = value,
            Self::SPREAD => self.spread_ms = value,
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
        self.age = 0;
        self.env = 1.0;
        self.velocity = velocity;
        self.noise = Noise::new(NOISE_SEED ^ 0xC1A9);
        self.bandpass.reset();
        self.highpass.reset();
        self.active = true;
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let tail_start = Self::BURSTS * self.gap;
        if self.age <= tail_start && self.age.is_multiple_of(self.gap) {
            // Each burst, and then the tail, restarts the envelope.
            self.env = 1.0;
        }
        let coeff = if self.age < tail_start {
            self.burst_coeff
        } else {
            self.tail_coeff
        };
        self.age = self.age.saturating_add(1);
        let noise = self
            .highpass
            .process(self.bandpass.process(self.noise.next()));
        let out = 1.6 * noise * self.env * self.velocity * self.level;
        self.env *= coeff;
        if self.age > tail_start && self.env < SILENCE {
            self.active = false;
        }
        out.clamp(-1.0, 1.0)
    }

    fn reset(&mut self) {
        self.active = false;
        self.env = 0.0;
        self.bandpass.reset();
        self.highpass.reset();
    }
}
