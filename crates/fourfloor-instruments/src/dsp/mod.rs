//! Small DSP building blocks shared by the instruments. Everything here is
//! allocation-free and safe to run on the audio thread.

mod envelope;
mod filter;
mod ladder;
mod osc;

pub(crate) use envelope::{Adsr, AdsrParams};
pub(crate) use filter::{Biquad, OnePole};
pub(crate) use ladder::Ladder;
pub(crate) use osc::{saw, square, Noise};

/// Per-sample multiplier for an exponential decay that falls by 60 dB in
/// `seconds`.
pub(crate) fn t60_coeff(seconds: f32, rate: f32) -> f32 {
    let samples = (seconds * rate).max(1.0);
    (-6.907_755 / samples).exp()
}

/// Per-sample multiplier for an exponential decay with time constant
/// `seconds` (falls to 1/e).
pub(crate) fn tau_coeff(seconds: f32, rate: f32) -> f32 {
    let samples = (seconds * rate).max(1.0e-3);
    (-1.0 / samples).exp()
}

/// Level below which a decaying voice is treated as finished (-100 dB).
pub(crate) const SILENCE: f32 = 1.0e-5;
