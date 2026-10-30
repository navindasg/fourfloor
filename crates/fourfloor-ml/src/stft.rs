//! Short-time Fourier transform and its inverse, for the separators and the
//! feature extractors.
//!
//! The same window is used for analysis and synthesis and the inverse
//! divides by the summed squared window, so `inverse(forward(x))` returns
//! `x` to within float rounding for any window and any hop up to half the
//! FFT size, including the first and last samples.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::error::MlError;

/// Window shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// Periodic Hann. What Spleeter and most models were trained with.
    Hann,
    /// Square root of the periodic Hann: the product of analysis and
    /// synthesis windows is a Hann, which keeps masking artefacts low.
    SqrtHann,
}

/// A complex spectrogram: `frames` rows of `bins` values.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrogram {
    /// FFT size the spectrogram was made with.
    pub fft_size: usize,
    /// Samples between frames.
    pub hop: usize,
    /// Number of frames.
    pub frames: usize,
    /// Number of frequency bins, `fft_size / 2 + 1`.
    pub bins: usize,
    /// `frames * bins` values, frame by frame.
    pub data: Vec<Complex32>,
}

impl Spectrogram {
    /// One frame.
    pub fn frame(&self, frame: usize) -> &[Complex32] {
        &self.data[frame * self.bins..(frame + 1) * self.bins]
    }

    /// One frame, mutable.
    pub fn frame_mut(&mut self, frame: usize) -> &mut [Complex32] {
        &mut self.data[frame * self.bins..(frame + 1) * self.bins]
    }

    /// Magnitudes, in the same layout as `data`.
    pub fn magnitudes(&self) -> Vec<f32> {
        self.data.iter().map(|value| value.norm()).collect()
    }

    /// Centre frequency of `bin` in hertz.
    pub fn bin_hz(&self, bin: usize, sample_rate: f64) -> f64 {
        bin as f64 * sample_rate / self.fft_size as f64
    }

    /// A copy with every value multiplied by the matching entry of `mask`
    /// (same layout as `data`).
    pub fn masked(&self, mask: &[f32]) -> Result<Self, MlError> {
        if mask.len() != self.data.len() {
            return Err(MlError::InvalidInput(format!(
                "mask has {} values, spectrogram has {}",
                mask.len(),
                self.data.len()
            )));
        }
        let mut out = self.clone();
        for (value, gain) in out.data.iter_mut().zip(mask) {
            *value *= *gain;
        }
        Ok(out)
    }
}

/// A reusable STFT of one size.
#[derive(Clone)]
pub struct Stft {
    fft_size: usize,
    hop: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

impl core::fmt::Debug for Stft {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Stft")
            .field("fft_size", &self.fft_size)
            .field("hop", &self.hop)
            .finish_non_exhaustive()
    }
}

impl Stft {
    /// An STFT with `fft_size` samples per frame, advancing `hop` samples.
    ///
    /// `fft_size` must be even and at least 4; `hop` must be between 1 and
    /// `fft_size / 2`.
    pub fn new(fft_size: usize, hop: usize, window: Window) -> Result<Self, MlError> {
        if fft_size < 4 || !fft_size.is_multiple_of(2) {
            return Err(MlError::InvalidInput(format!(
                "FFT size {fft_size} must be even and at least 4"
            )));
        }
        if hop == 0 || hop > fft_size / 2 {
            return Err(MlError::InvalidInput(format!(
                "hop {hop} must be between 1 and half the FFT size ({})",
                fft_size / 2
            )));
        }
        let window = (0..fft_size)
            .map(|n| {
                let hann = 0.5 - 0.5 * (core::f64::consts::TAU * n as f64 / fft_size as f64).cos();
                match window {
                    Window::Hann => hann as f32,
                    Window::SqrtHann => hann.sqrt() as f32,
                }
            })
            .collect();
        let mut planner = FftPlanner::new();
        Ok(Self {
            fft_size,
            hop,
            window,
            forward: planner.plan_fft_forward(fft_size),
            inverse: planner.plan_fft_inverse(fft_size),
        })
    }

    /// FFT size.
    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Hop size.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Number of bins per frame.
    pub fn bins(&self) -> usize {
        self.fft_size / 2 + 1
    }

    /// Zeros put in front of the signal so its first samples are covered by
    /// as many frames as the rest.
    fn lead(&self) -> usize {
        self.fft_size - self.hop
    }

    /// Number of frames [`Stft::forward`] produces for `len` samples.
    pub fn frames_for(&self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            (self.lead() + len).div_ceil(self.hop)
        }
    }

    /// Position in the signal, in samples, of the centre of `frame`. It is
    /// negative for the first few frames, which mostly cover the lead-in.
    pub fn frame_center(&self, frame: usize) -> f64 {
        (frame * self.hop) as f64 + self.fft_size as f64 / 2.0 - self.lead() as f64
    }

    /// Transforms `signal`.
    pub fn forward(&self, signal: &[f32]) -> Spectrogram {
        let frames = self.frames_for(signal.len());
        let bins = self.bins();
        let lead = self.lead();
        let mut data = Vec::with_capacity(frames * bins);
        let mut buffer = vec![Complex32::ZERO; self.fft_size];
        let mut scratch = vec![Complex32::ZERO; self.forward.get_inplace_scratch_len()];
        for frame in 0..frames {
            let start = frame * self.hop;
            for (n, slot) in buffer.iter_mut().enumerate() {
                let sample = (start + n)
                    .checked_sub(lead)
                    .and_then(|index| signal.get(index))
                    .copied()
                    .unwrap_or(0.0);
                *slot = Complex32::new(sample * self.window[n], 0.0);
            }
            self.forward.process_with_scratch(&mut buffer, &mut scratch);
            data.extend_from_slice(&buffer[..bins]);
        }
        Spectrogram {
            fft_size: self.fft_size,
            hop: self.hop,
            frames,
            bins,
            data,
        }
    }

    /// Rebuilds `len` samples from a spectrogram made by [`Stft::forward`]
    /// (possibly modified since).
    pub fn inverse(&self, spectrogram: &Spectrogram, len: usize) -> Result<Vec<f32>, MlError> {
        if spectrogram.fft_size != self.fft_size
            || spectrogram.hop != self.hop
            || spectrogram.bins != self.bins()
            || spectrogram.data.len() != spectrogram.frames * spectrogram.bins
        {
            return Err(MlError::InvalidInput(
                "spectrogram does not match this STFT".to_owned(),
            ));
        }
        let lead = self.lead();
        let total = lead + len + self.fft_size;
        let mut sum = vec![0.0f32; total];
        let mut weight = vec![0.0f32; total];
        let mut buffer = vec![Complex32::ZERO; self.fft_size];
        let mut scratch = vec![Complex32::ZERO; self.inverse.get_inplace_scratch_len()];
        let scale = 1.0 / self.fft_size as f32;
        let bins = self.bins();
        for frame in 0..spectrogram.frames {
            let start = frame * self.hop;
            if start >= lead + len {
                break;
            }
            let row = spectrogram.frame(frame);
            buffer[..bins].copy_from_slice(row);
            for bin in 1..bins - 1 {
                buffer[self.fft_size - bin] = row[bin].conj();
            }
            self.inverse.process_with_scratch(&mut buffer, &mut scratch);
            for n in 0..self.fft_size {
                let w = self.window[n];
                sum[start + n] += buffer[n].re * scale * w;
                weight[start + n] += w * w;
            }
        }
        Ok((0..len)
            .map(|n| {
                let w = weight[lead + n];
                if w > 1e-9 {
                    sum[lead + n] / w
                } else {
                    0.0
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_sizes_are_refused() {
        assert!(Stft::new(0, 1, Window::Hann).is_err());
        assert!(Stft::new(1023, 256, Window::Hann).is_err());
        assert!(Stft::new(1024, 0, Window::Hann).is_err());
        assert!(Stft::new(1024, 768, Window::Hann).is_err());
        assert!(Stft::new(1024, 512, Window::Hann).is_ok());
    }

    #[test]
    fn empty_input_gives_no_frames() {
        let stft = Stft::new(512, 128, Window::SqrtHann).unwrap();
        let spec = stft.forward(&[]);
        assert_eq!(spec.frames, 0);
        assert!(stft.inverse(&spec, 0).unwrap().is_empty());
    }

    #[test]
    fn a_mismatched_spectrogram_is_refused() {
        let a = Stft::new(512, 128, Window::Hann).unwrap();
        let b = Stft::new(1024, 256, Window::Hann).unwrap();
        let spec = a.forward(&[0.0; 2000]);
        assert!(b.inverse(&spec, 2000).is_err());
        assert!(spec.masked(&[1.0; 3]).is_err());
    }
}
