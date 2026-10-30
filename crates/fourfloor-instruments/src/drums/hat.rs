//! Hi-hat: six detuned square oscillators and noise through high-pass
//! filters. Closed and open share the voice, so closed chokes open.

use fourfloor_core::{ParamId, ParamInfo, ParamUnit};

use super::{DrumVoice, NOISE_SEED};
use crate::dsp::{square, t60_coeff, Biquad, Noise, SILENCE};

/// Closed and open hi-hat in one voice.
///
/// The oscillators run freely, so a closed hit that cuts off an open one
/// does not click: only the envelope changes.
#[derive(Clone, Debug)]
pub struct HatVoice {
    tune: f32,
    closed_decay: f32,
    open_decay: f32,
    tone: f32,
    level: f32,
    rate: f32,
    closed_coeff: f32,
    open_coeff: f32,
    attack_step: f32,
    phase: [f32; 6],
    env: f32,
    attack: f32,
    coeff: f32,
    velocity: f32,
    active: bool,
    noise: Noise,
    filters: [Biquad; 2],
}

static PARAMS: [ParamInfo; 5] = [
    ParamInfo::new(HatVoice::TUNE, "Tune", 0.5, 2.0, 1.0, ParamUnit::Ratio),
    ParamInfo::new(
        HatVoice::CLOSED_DECAY,
        "Closed Decay",
        0.01,
        0.3,
        0.06,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        HatVoice::OPEN_DECAY,
        "Open Decay",
        0.1,
        2.0,
        0.6,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(
        HatVoice::TONE,
        "Tone",
        3_000.0,
        12_000.0,
        7_000.0,
        ParamUnit::Hertz,
    ),
    ParamInfo::new(HatVoice::LEVEL, "Level", 0.0, 1.0, 0.8, ParamUnit::Generic),
];

impl HatVoice {
    /// Pitch of the oscillator bank as a ratio of its normal tuning.
    pub const TUNE: ParamId = ParamId(0);
    /// Time for a closed hit to fall by 60 dB.
    pub const CLOSED_DECAY: ParamId = ParamId(1);
    /// Time for an open hit to fall by 60 dB.
    pub const OPEN_DECAY: ParamId = ParamId(2);
    /// High-pass cutoff in Hz: higher is thinner and brighter.
    pub const TONE: ParamId = ParamId(3);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(4);

    /// MIDI pitch that plays the open hat in a stand-alone hat device.
    pub const OPEN_PITCH: u8 = 46;

    /// The 808 cymbal oscillator bank, in Hz.
    const BANK: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

    fn update(&mut self) {
        self.closed_coeff = t60_coeff(self.closed_decay, self.rate);
        self.open_coeff = t60_coeff(self.open_decay, self.rate);
        self.attack_step = 1.0 / (0.0005 * self.rate).max(1.0);
        for filter in &mut self.filters {
            filter.set_highpass(self.tone, 0.8, self.rate);
        }
    }

    /// Starts a closed hit, cutting off whatever was ringing.
    pub fn trigger_closed(&mut self, velocity: f32) {
        self.start(velocity, self.closed_coeff);
    }

    /// Starts an open hit.
    pub fn trigger_open(&mut self, velocity: f32) {
        self.start(velocity, self.open_coeff);
    }

    fn start(&mut self, velocity: f32, coeff: f32) {
        // Ramp from the level already sounding so the choke is click-free.
        self.attack = (self.env * self.velocity).min(1.0);
        self.env = 1.0;
        self.coeff = coeff;
        self.velocity = velocity;
        self.active = true;
    }
}

impl Default for HatVoice {
    fn default() -> Self {
        let p = &PARAMS;
        let mut voice = Self {
            tune: p[0].default,
            closed_decay: p[1].default,
            open_decay: p[2].default,
            tone: p[3].default,
            level: p[4].default,
            rate: 48_000.0,
            closed_coeff: 0.0,
            open_coeff: 0.0,
            attack_step: 1.0,
            phase: [0.0, 0.17, 0.31, 0.47, 0.61, 0.83],
            env: 0.0,
            attack: 0.0,
            coeff: 0.0,
            velocity: 0.0,
            active: false,
            noise: Noise::new(NOISE_SEED ^ 0x4A7),
            filters: [Biquad::bypass(); 2],
        };
        voice.update();
        voice
    }
}

impl DrumVoice for HatVoice {
    fn param_table() -> &'static [ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            Self::TUNE => self.tune = value,
            Self::CLOSED_DECAY => self.closed_decay = value,
            Self::OPEN_DECAY => self.open_decay = value,
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

    fn trigger(&mut self, pitch: u8, velocity: f32) {
        if pitch == Self::OPEN_PITCH || pitch == 26 {
            self.trigger_open(velocity);
        } else {
            self.trigger_closed(velocity);
        }
    }

    fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let mut metal = 0.0;
        for (phase, freq) in self.phase.iter_mut().zip(Self::BANK) {
            let step = (freq * self.tune / self.rate).min(0.45);
            metal += square(*phase, step, 0.5);
            *phase += step;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
        }
        let mut signal = metal / 6.0 + 0.5 * self.noise.next();
        for filter in &mut self.filters {
            signal = filter.process(signal);
        }
        self.attack = (self.attack + self.attack_step).min(1.0);
        let out = 2.2 * signal * self.env * self.attack * self.velocity * self.level;
        self.env *= self.coeff;
        if self.env < SILENCE {
            self.active = false;
            self.env = 0.0;
        }
        out.clamp(-1.0, 1.0)
    }

    fn reset(&mut self) {
        self.active = false;
        self.env = 0.0;
        self.attack = 0.0;
        self.phase = [0.0, 0.17, 0.31, 0.47, 0.61, 0.83];
        self.noise = Noise::new(NOISE_SEED ^ 0x4A7);
        for filter in &mut self.filters {
            filter.reset();
        }
    }
}
