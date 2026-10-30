//! Stem separation.
//!
//! Every separator works the same way: an STFT of each channel, one soft
//! mask per stem estimated from the magnitudes, the masks applied to the
//! complex spectrum, and an inverse STFT per stem. The mask estimate is
//! the only part that differs, so it sits behind [`MaskEstimator`]:
//!
//! - [`SpleeterOnnx`] runs Deezer's Spleeter 2-stem model (MIT code and
//!   weights) through tract: vocals and accompaniment.
//! - [`HpssEstimator`] is the **DSP fallback**, no model needed: median
//!   filter harmonic/percussive separation followed by a band split, giving
//!   drums, bass and other. It is clearly worse than a trained model and
//!   reports itself as a fallback.
//!
//! [`StemCache`] keeps finished stems on disk keyed by a BLAKE3 hash of the
//! audio content and the separator, so a track is separated once.

mod cache;
mod hpss;
mod spleeter;

use fourfloor_audio::{resample, AudioBuffer};
use fourfloor_core::SampleRate;
use serde::{Deserialize, Serialize};

pub use cache::StemCache;
pub use hpss::{HpssEstimator, HpssOptions};
pub use spleeter::SpleeterOnnx;

use crate::error::MlError;
use crate::stft::{Stft, Window};

/// One separated part of the mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Stem {
    /// What the stem holds: `vocals`, `accompaniment`, `drums`, `bass`,
    /// `other`.
    pub name: String,
    /// The audio, at the rate and channel count of the input.
    pub audio: AudioBuffer,
}

/// The result of a separation.
#[derive(Clone, Debug, PartialEq)]
pub struct Stems {
    /// Name of the separator that made them.
    pub separator: String,
    /// Whether the separator is the DSP fallback rather than a model.
    pub fallback: bool,
    /// The stems, in the separator's order.
    pub stems: Vec<Stem>,
}

impl Stems {
    /// The stem called `name`.
    pub fn get(&self, name: &str) -> Option<&Stem> {
        self.stems.iter().find(|stem| stem.name == name)
    }

    /// Names of the stems in order.
    pub fn names(&self) -> Vec<&str> {
        self.stems.iter().map(|stem| stem.name.as_str()).collect()
    }
}

/// Something that splits a mix into stems.
pub trait Separator: Send + Sync {
    /// Stable name, used in cache keys (bump it when the output changes).
    fn name(&self) -> &str;
    /// Whether this is a DSP stand-in rather than a trained model.
    fn is_fallback(&self) -> bool;
    /// Names of the stems [`Separator::separate`] returns, in order.
    fn stem_names(&self) -> Vec<String>;
    /// Separates `audio` (any rate, one or more channels).
    fn separate(&self, audio: &AudioBuffer) -> Result<Stems, MlError>;
}

/// STFT layout a mask estimator works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StftLayout {
    /// Sample rate the estimator expects; input is resampled to it.
    /// `None` works at the rate of the input.
    pub sample_rate: Option<u32>,
    /// FFT size.
    pub fft_size: usize,
    /// Hop size.
    pub hop: usize,
}

/// Magnitude spectrograms of every channel, `frames * bins` each.
#[derive(Clone, Debug, PartialEq)]
pub struct Magnitudes {
    /// Frames per channel.
    pub frames: usize,
    /// Bins per frame.
    pub bins: usize,
    /// Bin spacing in hertz.
    pub bin_hz: f64,
    /// One `frames * bins` plane per channel.
    pub channels: Vec<Vec<f32>>,
}

/// Estimates one soft mask per stem from magnitude spectrograms.
pub trait MaskEstimator: Send + Sync {
    /// Stable name.
    fn name(&self) -> &str;
    /// Whether this is a DSP stand-in rather than a trained model.
    fn is_fallback(&self) -> bool;
    /// The STFT the masks are estimated on.
    fn layout(&self) -> StftLayout;
    /// Stem names, in mask order.
    fn stem_names(&self) -> Vec<String>;
    /// `masks[stem][channel]`, each `frames * bins` values in 0..=1.
    fn masks(&self, magnitudes: &Magnitudes) -> Result<Vec<Vec<Vec<f32>>>, MlError>;
}

/// A [`Separator`] made of a mask estimator and the STFT pipeline.
#[derive(Debug)]
pub struct MaskSeparator<E> {
    estimator: E,
    window: Window,
}

impl<E: MaskEstimator> MaskSeparator<E> {
    /// A separator around `estimator`, using a Hann STFT.
    pub fn new(estimator: E) -> Self {
        Self {
            estimator,
            window: Window::Hann,
        }
    }

    /// The estimator.
    pub fn estimator(&self) -> &E {
        &self.estimator
    }
}

impl<E: MaskEstimator> Separator for MaskSeparator<E> {
    fn name(&self) -> &str {
        self.estimator.name()
    }

    fn is_fallback(&self) -> bool {
        self.estimator.is_fallback()
    }

    fn stem_names(&self) -> Vec<String> {
        self.estimator.stem_names()
    }

    fn separate(&self, audio: &AudioBuffer) -> Result<Stems, MlError> {
        if audio.frames() == 0 || audio.channel_count() == 0 {
            return Err(MlError::InvalidInput("audio is empty".to_owned()));
        }
        let layout = self.estimator.layout();
        let rate = match layout.sample_rate {
            Some(hz) => {
                SampleRate::new(hz).map_err(|error| MlError::InvalidInput(error.to_string()))?
            }
            None => audio.sample_rate,
        };
        let stft = Stft::new(layout.fft_size, layout.hop, self.window)?;
        let channels: Vec<Vec<f32>> = audio
            .channels
            .iter()
            .map(|channel| {
                let clean: Vec<f32> = channel
                    .iter()
                    .map(|s| if s.is_finite() { *s } else { 0.0 })
                    .collect();
                if audio.sample_rate == rate {
                    clean
                } else {
                    resample(&clean, audio.sample_rate, rate)
                }
            })
            .collect();
        let len = channels[0].len();
        let spectra: Vec<_> = channels
            .iter()
            .map(|channel| stft.forward(channel))
            .collect();
        let magnitudes = Magnitudes {
            frames: spectra[0].frames,
            bins: spectra[0].bins,
            bin_hz: rate.as_f64() / layout.fft_size as f64,
            channels: spectra
                .iter()
                .map(|spectrum| spectrum.magnitudes())
                .collect(),
        };
        let masks = self.estimator.masks(&magnitudes)?;
        let names = self.estimator.stem_names();
        if masks.len() != names.len()
            || masks.iter().any(|stem| {
                stem.len() != spectra.len()
                    || stem
                        .iter()
                        .any(|mask| mask.len() != magnitudes.frames * magnitudes.bins)
            })
        {
            return Err(MlError::inference(
                self.estimator.name(),
                "mask estimator returned masks of the wrong shape",
            ));
        }
        let mut stems = Vec::with_capacity(names.len());
        for (name, stem_masks) in names.into_iter().zip(&masks) {
            let mut planes = Vec::with_capacity(spectra.len());
            for (spectrum, mask) in spectra.iter().zip(stem_masks) {
                let signal = stft.inverse(&spectrum.masked(mask)?, len)?;
                planes.push(if audio.sample_rate == rate {
                    signal
                } else {
                    let mut back = resample(&signal, rate, audio.sample_rate);
                    back.resize(audio.frames(), 0.0);
                    back
                });
            }
            stems.push(Stem {
                name,
                audio: AudioBuffer::new(audio.sample_rate, planes),
            });
        }
        Ok(Stems {
            separator: self.estimator.name().to_owned(),
            fallback: self.estimator.is_fallback(),
            stems,
        })
    }
}

/// Wiener-style soft masks from per-stem magnitude estimates:
/// `m_i = (e_i^p + eps / n) / (sum_j e_j^p + eps)`, so the masks always sum
/// to one and silent cells are shared evenly.
pub fn soft_masks(estimates: &[&[f32]], power: f32) -> Vec<Vec<f32>> {
    const EPS: f32 = 1e-10;
    let stems = estimates.len();
    let len = estimates.first().map_or(0, |e| e.len());
    let mut masks = vec![vec![0.0f32; len]; stems];
    for cell in 0..len {
        let powers: Vec<f32> = estimates
            .iter()
            .map(|e| e[cell].abs().powf(power))
            .collect();
        let total: f32 = powers.iter().sum::<f32>() + EPS;
        for (mask, value) in masks.iter_mut().zip(&powers) {
            mask[cell] = (value + EPS / stems as f32) / total;
        }
    }
    masks
}
