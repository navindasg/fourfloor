//! The pieces of the 303 voice, ported from Open303's `rosic` classes
//! (MIT, Copyright (c) 2009 Robin Schmidt): the TB-303 ladder approximation
//! (`TeeBeeFilter`, `TB_303` mode), the one-pole and biquad helpers and the
//! envelopes. Double precision throughout, as in the original: the 7.5 Hz
//! notch and the 14 Hz allpass do not survive single precision.

use std::f64::consts::{PI, TAU};

/// `y = b0*x + b1*x1 + a1*y1`, Open303's `OnePoleFilter`.
#[derive(Clone, Copy, Debug)]
pub(super) struct OnePole {
    b0: f64,
    b1: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl OnePole {
    pub(super) const fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            a1: 0.0,
            x1: 0.0,
            y1: 0.0,
        }
    }

    pub(super) fn set_highpass(&mut self, cutoff: f64, rate: f64) {
        let x = (-TAU * cutoff / rate).exp();
        (self.b0, self.b1, self.a1) = (0.5 * (1.0 + x), -0.5 * (1.0 + x), x);
    }

    pub(super) fn set_allpass(&mut self, cutoff: f64, rate: f64) {
        let t = (PI * cutoff / rate).tan();
        let x = (t - 1.0) / (t + 1.0);
        (self.b0, self.b1, self.a1) = (x, 1.0, -x);
    }

    pub(super) fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.a1 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    pub(super) fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// RBJ biquad, transposed direct form II.
#[derive(Clone, Copy, Debug)]
pub(super) struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    pub(super) const fn new() -> Self {
        Self {
            b: [1.0, 0.0, 0.0],
            a: [0.0; 2],
            z: [0.0; 2],
        }
    }

    fn set(&mut self, b: [f64; 3], a: [f64; 3]) {
        self.b = [b[0] / a[0], b[1] / a[0], b[2] / a[0]];
        self.a = [a[1] / a[0], a[2] / a[0]];
    }

    pub(super) fn set_lowpass(&mut self, cutoff: f64, q: f64, rate: f64) {
        let w = TAU * (cutoff / rate).min(0.49);
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        self.set(
            [(1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        );
    }

    /// Band-reject with its bandwidth in octaves.
    pub(super) fn set_notch(&mut self, cutoff: f64, octaves: f64, rate: f64) {
        let w = TAU * (cutoff / rate).min(0.49);
        let (s, c) = w.sin_cos();
        let alpha = s * (0.5 * std::f64::consts::LN_2 * octaves * w / s).sinh();
        self.set([1.0, -2.0 * c, 1.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha]);
    }

    pub(super) fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    pub(super) fn reset(&mut self) {
        self.z = [0.0; 2];
    }
}

/// Open303's `TeeBeeFilter` in `TB_303` mode: four coupled one-pole stages
/// with a high-passed feedback path, the coefficients from the fitted
/// curves in `calculateCoefficientsApprox4`.
#[derive(Clone, Copy, Debug)]
pub(super) struct TeeBeeFilter {
    rate: f64,
    cutoff: f64,
    /// Resonance after Open303's skew, 0 to 1.
    resonance: f64,
    b0: f64,
    k: f64,
    g: f64,
    y: [f64; 4],
    feedback_highpass: OnePole,
}

impl TeeBeeFilter {
    /// Cutoff of the high-pass in the feedback path, in Hz.
    const FEEDBACK_HIGHPASS_HZ: f64 = 150.0;

    pub(super) const fn new() -> Self {
        Self {
            rate: 192_000.0,
            cutoff: 1_000.0,
            resonance: 0.0,
            b0: 0.0,
            k: 0.0,
            g: 1.0,
            y: [0.0; 4],
            feedback_highpass: OnePole::new(),
        }
    }

    pub(super) fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        self.feedback_highpass
            .set_highpass(Self::FEEDBACK_HIGHPASS_HZ, rate);
        self.update();
    }

    /// Sets the resonance from the panel value, 0 to 100.
    pub(super) fn set_resonance(&mut self, percent: f64) {
        let raw = 0.01 * percent;
        self.resonance = (1.0 - (-3.0 * raw).exp()) / (1.0 - (-3.0f64).exp());
        self.update();
    }

    /// Sets the cutoff in Hz, held within 200 Hz to 20 kHz.
    pub(super) fn set_cutoff(&mut self, cutoff: f64) {
        let cutoff = cutoff.clamp(200.0, 20_000.0);
        if cutoff != self.cutoff {
            self.cutoff = cutoff;
            self.update();
        }
    }

    fn update(&mut self) {
        let wc = TAU * self.cutoff / self.rate;
        let r = self.resonance;
        let fx = wc * std::f64::consts::FRAC_1_SQRT_2 / TAU;
        self.b0 = (0.000_455_223_46 + 6.192_218_9 * fx)
            / (1.0 + 12.358_354 * fx + 4.415_634_5 * (fx * fx));
        let k = fx
            * (fx
                * (fx * (fx * (fx * (fx + 7_198.699_7) - 5_837.791_7) - 476.473_08) + 614.956_11)
                + 213.871_26)
            + 16.998_792;
        let g = k * (1.0 / 17.0);
        let g = (g - 1.0) * r + 1.0;
        self.g = g * (1.0 + r);
        self.k = k * r;
    }

    pub(super) fn process(&mut self, input: f64) -> f64 {
        let b0 = self.b0;
        let [mut y1, mut y2, mut y3, mut y4] = self.y;
        let y0 = input - self.feedback_highpass.process(self.k * y4);
        y1 += 2.0 * b0 * (y0 - y1 + y2);
        y2 += b0 * (y1 - 2.0 * y2 + y3);
        y3 += b0 * (y2 - 2.0 * y3 + y4);
        y4 += b0 * (y3 - 2.0 * y4);
        self.y = [y1, y2, y3, y4];
        2.0 * self.g * y4
    }

    pub(super) fn reset(&mut self) {
        self.y = [0.0; 4];
        self.feedback_highpass.reset();
    }
}

/// Open303's `LeakyIntegrator`: a one-pole smoother with a time constant.
#[derive(Clone, Copy, Debug)]
pub(super) struct LeakyIntegrator {
    coeff: f64,
    y1: f64,
}

impl LeakyIntegrator {
    pub(super) const fn new() -> Self {
        Self {
            coeff: 0.0,
            y1: 0.0,
        }
    }

    pub(super) fn set_time_constant(&mut self, ms: f64, rate: f64) {
        self.coeff = if ms > 0.0 {
            (-1.0 / (rate * 0.001 * ms)).exp()
        } else {
            0.0
        };
    }

    pub(super) fn set_state(&mut self, value: f64) {
        self.y1 = value;
    }

    pub(super) fn process(&mut self, input: f64) -> f64 {
        self.y1 = input + self.coeff * (self.y1 - input);
        self.y1
    }
}

/// Open303's `DecayEnvelope`: jumps to 1 on a trigger and decays with a
/// time constant.
#[derive(Clone, Copy, Debug)]
pub(super) struct DecayEnvelope {
    c: f64,
    y: f64,
}

impl DecayEnvelope {
    pub(super) const fn new() -> Self {
        Self { c: 0.0, y: 0.0 }
    }

    pub(super) fn set_time_constant(&mut self, ms: f64, rate: f64) {
        self.c = (-1.0 / (0.001 * ms.max(0.001) * rate)).exp();
    }

    pub(super) fn trigger(&mut self) {
        // The first sample after a trigger is exactly 1.
        self.y = if self.c > 0.0 { 1.0 / self.c } else { 0.0 };
    }

    pub(super) fn process(&mut self) -> f64 {
        self.y *= self.c;
        self.y
    }

    pub(super) fn reset(&mut self) {
        self.y = 0.0;
    }
}

/// The amplitude envelope as Open303 configures its `AnalogEnvelope`: an
/// instant attack, a slow exponential fall towards zero while the note is
/// held and a fast one after it is released.
#[derive(Clone, Copy, Debug)]
pub(super) struct AmpEnvelope {
    decay_coeff: f64,
    release_coeff: f64,
    level: f64,
    note_on: bool,
    attack_pending: bool,
}

impl AmpEnvelope {
    /// Time constant of the fall while a note is held, in milliseconds.
    const DECAY_MS: f64 = 1_230.0;

    pub(super) const fn new() -> Self {
        Self {
            decay_coeff: 0.0,
            release_coeff: 0.0,
            level: 0.0,
            note_on: false,
            attack_pending: false,
        }
    }

    pub(super) fn set_rate(&mut self, rate: f64) {
        self.decay_coeff = 1.0 - (-1.0 / (rate * 0.001 * Self::DECAY_MS)).exp();
    }

    pub(super) fn set_release(&mut self, ms: f64, rate: f64) {
        self.release_coeff = 1.0 - (-1.0 / (rate * 0.001 * ms)).exp();
    }

    pub(super) fn note_on(&mut self) {
        self.note_on = true;
        self.attack_pending = true;
    }

    pub(super) fn note_off(&mut self) {
        self.note_on = false;
        self.attack_pending = false;
    }

    pub(super) fn is_note_on(&self) -> bool {
        self.note_on
    }

    pub(super) fn level(&self) -> f64 {
        self.level
    }

    pub(super) fn process(&mut self) -> f64 {
        if self.attack_pending {
            self.level = 1.0;
            self.attack_pending = false;
        } else if self.note_on {
            self.level -= self.decay_coeff * self.level;
        } else {
            self.level -= self.release_coeff * self.level;
        }
        self.level
    }

    pub(super) fn reset(&mut self) {
        self.level = 0.0;
        self.note_on = false;
        self.attack_pending = false;
    }
}

/// The keys held down, most recent first. Fixed size: when it is full the
/// oldest key is forgotten.
#[derive(Clone, Copy, Debug)]
pub(super) struct NoteList {
    keys: [u8; Self::CAPACITY],
    len: usize,
}

impl NoteList {
    const CAPACITY: usize = 16;

    pub(super) const fn new() -> Self {
        Self {
            keys: [0; Self::CAPACITY],
            len: 0,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn front(&self) -> Option<u8> {
        self.keys[..self.len].first().copied()
    }

    pub(super) fn push_front(&mut self, key: u8) {
        self.remove(key);
        let len = (self.len + 1).min(Self::CAPACITY);
        self.keys.copy_within(0..len - 1, 1);
        self.keys[0] = key;
        self.len = len;
    }

    pub(super) fn remove(&mut self, key: u8) {
        if let Some(index) = self.keys[..self.len].iter().position(|held| *held == key) {
            self.keys.copy_within(index + 1..self.len, index);
            self.len -= 1;
        }
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_list_keeps_most_recent_first_and_forgets_the_oldest() {
        let mut list = NoteList::new();
        assert!(list.is_empty());
        for key in 0..20u8 {
            list.push_front(key);
        }
        assert_eq!(list.front(), Some(19));
        assert_eq!(list.len, NoteList::CAPACITY);
        list.remove(19);
        assert_eq!(list.front(), Some(18));
        list.remove(0); // forgotten long ago: no effect
        assert_eq!(list.len, NoteList::CAPACITY - 1);
        list.push_front(10); // already held: moves to the front
        assert_eq!(list.front(), Some(10));
        assert_eq!(list.len, NoteList::CAPACITY - 1);
        list.clear();
        assert_eq!(list.front(), None);
    }

    #[test]
    fn decay_envelope_starts_at_one() {
        let mut env = DecayEnvelope::new();
        env.set_time_constant(100.0, 48_000.0);
        env.trigger();
        assert!((env.process() - 1.0).abs() < 1e-12);
        assert!(env.process() < 1.0);
    }
}
