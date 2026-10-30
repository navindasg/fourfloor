//! A small seedable random source for the pattern generators.
//!
//! The generators take `&mut dyn RandomSource`, so a caller can pass any
//! generator. [`SeededRng`] is the one fourfloor ships: SplitMix64, whose
//! output for a given seed is fixed by its definition, so a seed stored in
//! a project or typed by a user gives the same pattern on every machine
//! and in every future version.

/// A source of random 64-bit numbers.
pub trait RandomSource {
    /// The next 64 random bits.
    fn next_u64(&mut self) -> u64;

    /// A number in `0.0..1.0`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// An index in `0..len`; 0 when `len` is 0.
    fn below(&mut self, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        ((self.unit() * len as f64) as usize).min(len - 1)
    }

    /// `true` with the given probability (clamped to 0 to 1).
    fn chance(&mut self, probability: f64) -> bool {
        self.unit() < probability
    }

    /// An index drawn in proportion to `weights`. Negative and non-finite
    /// weights count as zero; if every weight is zero the result is 0.
    fn weighted(&mut self, weights: &[f64]) -> usize {
        let clean = |weight: &f64| {
            if weight.is_finite() && *weight > 0.0 {
                *weight
            } else {
                0.0
            }
        };
        let total: f64 = weights.iter().map(clean).sum();
        if total <= 0.0 {
            return 0;
        }
        let mut remaining = self.unit() * total;
        for (index, weight) in weights.iter().enumerate() {
            let weight = clean(weight);
            if weight > 0.0 && remaining < weight {
                return index;
            }
            remaining -= weight;
        }
        // Rounding left a sliver: the last index with weight wins.
        weights
            .iter()
            .rposition(|weight| clean(weight) > 0.0)
            .unwrap_or(0)
    }
}

/// SplitMix64: tiny, fast and identical everywhere for a given seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    /// A generator that will always produce the same sequence for `seed`.
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }
}

impl RandomSource for SeededRng {
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_the_reference_sequence() {
        // First outputs of SplitMix64 seeded with 0 and with 1234567.
        let mut zero = SeededRng::new(0);
        assert_eq!(zero.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(zero.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        let mut seeded = SeededRng::new(1_234_567);
        assert_eq!(seeded.next_u64(), 6_457_827_717_110_365_317);
        assert_eq!(seeded.next_u64(), 3_203_168_211_198_807_973);
    }

    #[test]
    fn helpers_stay_in_range() {
        let mut rng = SeededRng::new(7);
        for _ in 0..10_000 {
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit));
            assert!(rng.below(5) < 5);
        }
        assert_eq!(rng.below(0), 0);
        assert!(!rng.chance(0.0));
        assert!(rng.chance(1.0));
    }

    #[test]
    fn weighted_follows_the_weights() {
        let mut rng = SeededRng::new(99);
        let mut counts = [0usize; 4];
        for _ in 0..20_000 {
            counts[rng.weighted(&[1.0, 0.0, 3.0, f64::NAN])] += 1;
        }
        assert_eq!(counts[1], 0);
        assert_eq!(counts[3], 0);
        let ratio = counts[2] as f64 / counts[0] as f64;
        assert!((2.7..3.3).contains(&ratio), "ratio {ratio}");
        assert_eq!(rng.weighted(&[]), 0);
        assert_eq!(rng.weighted(&[0.0, -1.0]), 0);
    }
}
