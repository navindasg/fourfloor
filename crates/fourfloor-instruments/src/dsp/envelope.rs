//! Attack / decay / sustain / release envelope.

use super::t60_coeff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// An ADSR with a linear attack and exponential decay and release. The
/// decay and release times are the time to fall by 60 dB. Once the release
/// passes -80 dB the envelope snaps to zero and reports itself idle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Adsr {
    stage: Stage,
    level: f32,
    attack_step: f32,
    decay_coeff: f32,
    sustain: f32,
    release_coeff: f32,
}

/// Times and level of an [`Adsr`], in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AdsrParams {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Adsr {
    const FLOOR: f32 = 1.0e-4;

    pub(crate) const fn new() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            attack_step: 1.0,
            decay_coeff: 0.0,
            sustain: 1.0,
            release_coeff: 0.0,
        }
    }

    pub(crate) fn set(&mut self, params: &AdsrParams, rate: f32) {
        let attack_samples = params.attack * rate;
        self.attack_step = if attack_samples < 1.0 {
            1.0
        } else {
            1.0 / attack_samples
        };
        self.decay_coeff = t60_coeff(params.decay, rate);
        self.sustain = params.sustain.clamp(0.0, 1.0);
        self.release_coeff = t60_coeff(params.release, rate);
    }

    /// Starts the attack from the current level (no click on a retrigger).
    pub(crate) fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub(crate) fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    pub(crate) fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    pub(crate) fn next(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => 0.0,
            Stage::Attack => {
                self.level += self.attack_step;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
                self.level
            }
            Stage::Decay => {
                self.level = self.sustain + (self.level - self.sustain) * self.decay_coeff;
                self.level
            }
            Stage::Release => {
                self.level *= self.release_coeff;
                if self.level < Self::FLOOR {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
                self.level
            }
        }
    }
}
