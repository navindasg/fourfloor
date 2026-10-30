//! Oscillator helpers: band-limited steps and a noise source.

/// Deterministic white noise from a xorshift generator, so renders repeat.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Noise(u32);

impl Noise {
    pub(crate) const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    /// Next sample in -1.0..1.0.
    pub(crate) fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// PolyBLEP residual: the correction that band-limits a unit step at phase
/// 0. `t` is the phase in cycles (0..1), `dt` the phase step per sample.
pub(crate) fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Band-limited sawtooth in -1..1 at phase `t`, rising, stepping down at
/// the wrap.
pub(crate) fn saw(t: f32, dt: f32) -> f32 {
    2.0 * t - 1.0 - poly_blep(t, dt)
}

/// Band-limited pulse in -1..1 at phase `t`: high for the first `width`
/// of the cycle.
pub(crate) fn square(t: f32, dt: f32, width: f32) -> f32 {
    let mut shifted = t - width;
    if shifted < 0.0 {
        shifted += 1.0;
    }
    let naive = if t < width { 1.0 } else { -1.0 };
    naive + poly_blep(t, dt) - poly_blep(shifted, dt)
}
