//! A linear four-pole lowpass ladder, solved without a unit delay in the
//! feedback path (the topology-preserving form), so it stays in tune and
//! stable up to self-oscillation.

use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ladder {
    stage: [f32; 4],
    /// `g / (1 + g)` for the one-pole stages.
    big_g: f32,
    /// `1 / (1 + g)`.
    inv: f32,
    /// Feedback amount, 0 to just under 4.
    k: f32,
}

impl Ladder {
    pub(crate) const fn new() -> Self {
        Self {
            stage: [0.0; 4],
            big_g: 0.5,
            inv: 0.5,
            k: 0.0,
        }
    }

    /// Sets the cutoff in Hz and the resonance, 0 to 1.
    pub(crate) fn set(&mut self, cutoff: f32, resonance: f32, rate: f32) {
        let g = (PI * (cutoff / rate).clamp(1.0e-4, 0.49)).tan();
        self.inv = 1.0 / (1.0 + g);
        self.big_g = g * self.inv;
        self.k = 3.9 * resonance.clamp(0.0, 1.0);
    }

    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let g = self.big_g;
        let [s0, s1, s2, s3] = self.stage;
        let feedback = (g * g * g * s0 + g * g * s1 + g * s2 + s3) * self.inv;
        let mut u = (x - self.k * feedback) / (1.0 + self.k * g * g * g * g);
        for state in &mut self.stage {
            let v = (u - *state) * g;
            let y = v + *state;
            *state = if (y + v).abs() < 1.0e-20 { 0.0 } else { y + v };
            u = y;
        }
        // Make up some of the passband level the feedback takes away.
        u * (1.0 + 0.5 * self.k)
    }

    pub(crate) fn reset(&mut self) {
        self.stage = [0.0; 4];
    }
}
