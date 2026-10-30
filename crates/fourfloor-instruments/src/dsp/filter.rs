//! One-pole and biquad filters.

use std::f32::consts::TAU;

/// First-order filter: `y = b0*x + b1*x1 + a1*y1`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OnePole {
    b0: f32,
    b1: f32,
    a1: f32,
    x1: f32,
    y1: f32,
}

impl OnePole {
    /// A filter that passes its input through.
    pub(crate) const fn bypass() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            a1: 0.0,
            x1: 0.0,
            y1: 0.0,
        }
    }

    pub(crate) fn set_lowpass(&mut self, cutoff: f32, rate: f32) {
        let x = (-TAU * cutoff / rate).exp();
        (self.b0, self.b1, self.a1) = (1.0 - x, 0.0, x);
    }

    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.a1 * self.y1;
        self.x1 = x;
        self.y1 = if y.abs() < 1.0e-20 { 0.0 } else { y };
        y
    }

    pub(crate) fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// Second-order filter (RBJ cookbook designs, transposed direct form II).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A filter that passes its input through.
    pub(crate) const fn bypass() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn set(&mut self, b: [f32; 3], a: [f32; 3]) {
        let inv = 1.0 / a[0];
        self.b0 = b[0] * inv;
        self.b1 = b[1] * inv;
        self.b2 = b[2] * inv;
        self.a1 = a[1] * inv;
        self.a2 = a[2] * inv;
    }

    fn omega(cutoff: f32, rate: f32) -> (f32, f32) {
        let w = TAU * (cutoff / rate).clamp(1.0e-5, 0.49);
        (w.sin(), w.cos())
    }

    pub(crate) fn set_lowpass(&mut self, cutoff: f32, q: f32, rate: f32) {
        let (s, c) = Self::omega(cutoff, rate);
        let alpha = s / (2.0 * q);
        self.set(
            [(1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        );
    }

    pub(crate) fn set_highpass(&mut self, cutoff: f32, q: f32, rate: f32) {
        let (s, c) = Self::omega(cutoff, rate);
        let alpha = s / (2.0 * q);
        self.set(
            [(1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        );
    }

    /// Band-pass with 0 dB gain at the centre frequency.
    pub(crate) fn set_bandpass(&mut self, cutoff: f32, q: f32, rate: f32) {
        let (s, c) = Self::omega(cutoff, rate);
        let alpha = s / (2.0 * q);
        self.set([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * c, 1.0 - alpha]);
    }

    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        if self.z1.abs() < 1.0e-20 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1.0e-20 {
            self.z2 = 0.0;
        }
        y
    }

    pub(crate) fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}
