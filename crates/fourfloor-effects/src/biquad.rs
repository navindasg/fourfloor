//! Second-order filter sections with the Audio EQ Cookbook (Robert
//! Bristow-Johnson) coefficient formulas. Double precision, so low
//! frequencies stay accurate at high sample rates.

use std::f64::consts::TAU;

/// Butterworth Q: maximally flat second-order response.
pub(crate) const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Normalised coefficients (`a0` is 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

struct Prototype {
    cos: f64,
    alpha: f64,
}

fn prototype(sample_rate: f64, frequency: f64, q: f64) -> Prototype {
    let frequency = frequency.clamp(5.0, sample_rate * 0.49);
    let w0 = TAU * frequency / sample_rate;
    Prototype {
        cos: w0.cos(),
        alpha: w0.sin() / (2.0 * q.max(0.01)),
    }
}

impl Coefficients {
    /// Passes the signal unchanged.
    pub(crate) const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalised(b: [f64; 3], a: [f64; 3]) -> Self {
        let inverse = 1.0 / a[0];
        Self {
            b0: b[0] * inverse,
            b1: b[1] * inverse,
            b2: b[2] * inverse,
            a1: a[1] * inverse,
            a2: a[2] * inverse,
        }
    }

    pub(crate) fn low_pass(sample_rate: f64, frequency: f64, q: f64) -> Self {
        let Prototype { cos, alpha } = prototype(sample_rate, frequency, q);
        let b1 = 1.0 - cos;
        Self::normalised(
            [b1 * 0.5, b1, b1 * 0.5],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    pub(crate) fn high_pass(sample_rate: f64, frequency: f64, q: f64) -> Self {
        let Prototype { cos, alpha } = prototype(sample_rate, frequency, q);
        let b1 = 1.0 + cos;
        Self::normalised(
            [b1 * 0.5, -b1, b1 * 0.5],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    pub(crate) fn all_pass(sample_rate: f64, frequency: f64, q: f64) -> Self {
        let Prototype { cos, alpha } = prototype(sample_rate, frequency, q);
        Self::normalised(
            [1.0 - alpha, -2.0 * cos, 1.0 + alpha],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// A bell: `gain_db` at `frequency`, unity far from it.
    pub(crate) fn peaking(sample_rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let Prototype { cos, alpha } = prototype(sample_rate, frequency, q);
        let a = 10.0f64.powf(gain_db / 40.0);
        Self::normalised(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        )
    }

    /// `gain_db` below `frequency`, unity above.
    pub(crate) fn low_shelf(sample_rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let Prototype { cos, alpha } = prototype(sample_rate, frequency, q);
        let a = 10.0f64.powf(gain_db / 40.0);
        let root = 2.0 * a.sqrt() * alpha;
        Self::normalised(
            [
                a * ((a + 1.0) - (a - 1.0) * cos + root),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - root),
            ],
            [
                (a + 1.0) + (a - 1.0) * cos + root,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - root,
            ],
        )
    }

    /// `gain_db` above `frequency`, unity below.
    pub(crate) fn high_shelf(sample_rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let Prototype { cos, alpha } = prototype(sample_rate, frequency, q);
        let a = 10.0f64.powf(gain_db / 40.0);
        let root = 2.0 * a.sqrt() * alpha;
        Self::normalised(
            [
                a * ((a + 1.0) + (a - 1.0) * cos + root),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - root),
            ],
            [
                (a + 1.0) - (a - 1.0) * cos + root,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - root,
            ],
        )
    }
}

impl Coefficients {
    /// Moves every coefficient `amount` (0 to 1) of the way to `target` and
    /// returns whether it has arrived. Both feedback pairs being stable
    /// keeps every step in between stable, because the stable region of
    /// `(a1, a2)` is convex.
    #[inline]
    pub(crate) fn glide_to(&mut self, target: &Self, amount: f64) -> bool {
        let mut distance = 0.0f64;
        let mut step = |value: &mut f64, target: f64| {
            *value += (target - *value) * amount;
            distance = distance.max((target - *value).abs());
        };
        step(&mut self.b0, target.b0);
        step(&mut self.b1, target.b1);
        step(&mut self.b2, target.b2);
        step(&mut self.a1, target.a1);
        step(&mut self.a2, target.a2);
        let arrived = distance < 1.0e-9;
        if arrived {
            *self = *target;
        }
        arrived
    }
}

/// The memory of one section (transposed direct form II).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Biquad {
    z1: f64,
    z2: f64,
}

impl Biquad {
    #[inline]
    pub(crate) fn tick(&mut self, c: &Coefficients, input: f64) -> f64 {
        let output = c.b0 * input + self.z1;
        self.z1 = c.b1 * input - c.a1 * output + self.z2;
        self.z2 = c.b2 * input - c.a2 * output;
        output
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}
