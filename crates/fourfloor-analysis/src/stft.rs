//! Short-time Fourier transform shared by the analyzers.
//!
//! Frames are centred: frame `i` covers samples
//! `i * hop - size / 2 .. i * hop + size / 2`, zero padded at both ends, so
//! frame `i` describes the audio around sample `i * hop`.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// A periodic Hann window of `size` samples.
pub(crate) fn hann(size: usize) -> Vec<f32> {
    (0..size)
        .map(|n| {
            let phase = std::f64::consts::TAU * n as f64 / size as f64;
            (0.5 - 0.5 * phase.cos()) as f32
        })
        .collect()
}

/// The smallest power of two that is at least `value` (and at least 2).
pub(crate) fn next_pow2(value: usize) -> usize {
    value.max(2).next_power_of_two()
}

/// A power of two FFT size covering about `seconds` of audio.
pub(crate) fn fft_size_for(seconds: f64, sample_rate: f64) -> usize {
    next_pow2((seconds * sample_rate).round() as usize)
}

/// A Hann-windowed power spectrum calculator of a fixed size.
pub(crate) struct PowerSpectrum {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    scale: f32,
}

impl PowerSpectrum {
    pub(crate) fn new(size: usize) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(size);
        let window = hann(size);
        let window_energy: f64 = window.iter().map(|&w| f64::from(w) * f64::from(w)).sum();
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Self {
            fft,
            buffer: vec![Complex::new(0.0, 0.0); size],
            scratch,
            // One-sided, so that the sum over a band is the mean square of
            // the signal within it.
            scale: (2.0 / (size as f64 * window_energy)) as f32,
            window,
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.window.len()
    }

    /// Number of bins written by [`Self::power_at`]: `size / 2 + 1`.
    pub(crate) fn bins(&self) -> usize {
        self.window.len() / 2 + 1
    }

    /// Writes the power spectrum of the frame that starts at sample `start`
    /// (which may be negative or run past the end; missing samples count as
    /// zero) into `out`, which must hold [`Self::bins`] values.
    pub(crate) fn power_at(&mut self, signal: &[f32], start: isize, out: &mut [f32]) {
        for (n, (slot, &w)) in self.buffer.iter_mut().zip(&self.window).enumerate() {
            let index = start + n as isize;
            let sample = usize::try_from(index)
                .ok()
                .and_then(|index| signal.get(index))
                .copied()
                .unwrap_or(0.0);
            *slot = Complex::new(sample * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buffer, &mut self.scratch);
        let last = self.bins() - 1;
        for (bin, (value, slot)) in self.buffer.iter().zip(out.iter_mut()).enumerate() {
            let edge = bin == 0 || bin == last;
            *slot = value.norm_sqr() * if edge { self.scale * 0.5 } else { self.scale };
        }
    }
}

/// Number of centred frames for a signal of `len` samples.
pub(crate) fn frame_count(len: usize, hop: usize) -> usize {
    if len == 0 {
        0
    } else {
        len / hop + 1
    }
}

/// Calls `visit(frame index, power spectrum)` for every centred frame.
pub(crate) fn for_each_frame(
    signal: &[f32],
    size: usize,
    hop: usize,
    mut visit: impl FnMut(usize, &[f32]),
) {
    let mut spectrum = PowerSpectrum::new(size);
    let mut power = vec![0.0f32; spectrum.bins()];
    let half = (size / 2) as isize;
    for frame in 0..frame_count(signal.len(), hop) {
        spectrum.power_at(signal, (frame * hop) as isize - half, &mut power);
        visit(frame, &power);
    }
}

/// The average power spectrum over the whole signal (Welch's method) and
/// the width of one bin in Hz. The sum over a band of bins is the mean
/// square of the signal within that band. An empty signal gives all zeros.
pub(crate) fn average_power(signal: &[f32], size: usize, sample_rate: f64) -> (Vec<f64>, f64) {
    // At a hop of a quarter window the squared Hann windows of the centred
    // frames add up to a constant, so every sample counts equally and the
    // total is exact rather than an average of frame estimates.
    let hop = size / 4;
    let mut sum = vec![0.0f64; size / 2 + 1];
    for_each_frame(signal, size, hop, |_, power| {
        for (total, &value) in sum.iter_mut().zip(power) {
            *total += f64::from(value);
        }
    });
    let hops = signal.len() as f64 / hop as f64;
    if hops > 0.0 {
        for total in &mut sum {
            *total /= hops;
        }
    }
    (sum, sample_rate / size as f64)
}

/// Linear interpolation into `values` at a fractional index; zero outside.
pub(crate) fn interpolate(values: &[f32], index: f64) -> f32 {
    if index < 0.0 || values.is_empty() {
        return 0.0;
    }
    let low = index.floor() as usize;
    let fraction = (index - low as f64) as f32;
    match (values.get(low), values.get(low + 1)) {
        (Some(&a), Some(&b)) => a + (b - a) * fraction,
        (Some(&a), None) => a * (1.0 - fraction),
        _ => 0.0,
    }
}
