//! Inter-sample (true) peak estimation for the limiter's detector.
//!
//! Each new stereo sample is written into a short history; the detector
//! then reports the largest absolute value around the sample [`DELAY`]
//! samples back: the sample itself and the signal reconstructed at a
//! quarter, half and three quarters of the way to each neighbour, with a
//! Hann-windowed sinc over [`TAPS`] samples. That is four-times
//! oversampling, the same resolution ITU-R BS.1770 true-peak meters use.
//!
//! Fixed-size arrays only: nothing here allocates.

/// Taps of the interpolation filter per phase.
pub const TAPS: usize = 16;
/// Samples between the newest input and the sample whose peak is
/// reported.
pub const DELAY: usize = TAPS / 2;
/// Fractional positions between two samples that are reconstructed.
const PHASES: [f64; 3] = [0.25, 0.5, 0.75];

/// The reconstruction filter for one fractional position: coefficient `k`
/// weights the sample at offset `k - (DELAY - 1)` from the centre.
fn phase_coefficients(fraction: f64) -> [f32; TAPS] {
    let half = DELAY as f64;
    let mut taps = [0.0f64; TAPS];
    for (k, tap) in taps.iter_mut().enumerate() {
        let offset = k as f64 - (half - 1.0);
        let t = fraction - offset;
        let sinc = if t.abs() < 1e-12 {
            1.0
        } else {
            (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
        };
        let window = 0.5 + 0.5 * (std::f64::consts::PI * t / half).cos();
        *tap = if t.abs() < half { sinc * window } else { 0.0 };
    }
    // Unity gain at DC.
    let sum: f64 = taps.iter().sum();
    taps.map(|tap| (tap / sum) as f32)
}

/// A stereo true-peak detector with [`DELAY`] samples of latency.
#[derive(Clone, Debug)]
pub struct TruePeakDetector {
    coefficients: [[f32; TAPS]; 3],
    history: [[f32; TAPS]; 2],
    /// Index of the oldest sample in `history`.
    oldest: usize,
    /// Peak of the interval that ends at the reported sample.
    previous_interval: f32,
}

impl Default for TruePeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeakDetector {
    /// A detector with silent history.
    pub fn new() -> Self {
        Self {
            coefficients: PHASES.map(phase_coefficients),
            history: [[0.0; TAPS]; 2],
            oldest: 0,
            previous_interval: 0.0,
        }
    }

    /// Forgets the history.
    pub fn reset(&mut self) {
        self.history = [[0.0; TAPS]; 2];
        self.oldest = 0;
        self.previous_interval = 0.0;
    }

    /// Takes the newest stereo sample and returns the true peak around the
    /// sample [`DELAY`] pushes back: the larger of that sample and the
    /// reconstructed signal between it and either neighbour.
    #[inline]
    pub fn push(&mut self, input: [f32; 2]) -> f32 {
        for (history, sample) in self.history.iter_mut().zip(input) {
            history[self.oldest] = sample;
        }
        self.oldest = (self.oldest + 1) % TAPS;
        let mut sample_peak = 0.0f32;
        let mut interval = 0.0f32;
        for history in &self.history {
            // The centre sample sits DELAY - 1 places after the oldest.
            let centre = history[(self.oldest + DELAY - 1) % TAPS];
            sample_peak = sample_peak.max(centre.abs());
            for phase in &self.coefficients {
                let mut sum = 0.0f32;
                for (k, coefficient) in phase.iter().enumerate() {
                    sum += coefficient * history[(self.oldest + k) % TAPS];
                }
                interval = interval.max(sum.abs());
            }
        }
        let peak = sample_peak.max(interval).max(self.previous_interval);
        self.previous_interval = interval;
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constant_reads_its_own_level() {
        let mut detector = TruePeakDetector::new();
        let mut peak = 0.0;
        for _ in 0..64 {
            peak = detector.push([0.5, -0.25]);
        }
        assert!((peak - 0.5).abs() < 1e-4, "{peak}");
    }

    #[test]
    fn an_impulse_is_reported_after_the_delay() {
        let mut detector = TruePeakDetector::new();
        let peaks: Vec<f32> = (0..40)
            .map(|n| detector.push(if n == 10 { [1.0, 0.0] } else { [0.0, 0.0] }))
            .collect();
        let first = peaks.iter().position(|p| *p > 0.5).unwrap();
        assert!(first <= 10 + DELAY && first + 2 >= 10 + DELAY, "{first}");
        assert!((peaks[10 + DELAY] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_quarter_rate_sine_between_samples_reads_its_true_peak() {
        // fs/4 at 45 degrees: every sample is at 0.707 of the amplitude,
        // but the waveform reaches 1.0 between samples.
        let mut detector = TruePeakDetector::new();
        let mut peak = 0.0f32;
        for n in 0..256 {
            let phase = std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4;
            let x = phase.sin() as f32;
            let reported = detector.push([x, x]);
            if n > 2 * TAPS {
                peak = peak.max(reported);
            }
        }
        assert!(peak > 0.98 && peak < 1.02, "{peak}");
    }
}
