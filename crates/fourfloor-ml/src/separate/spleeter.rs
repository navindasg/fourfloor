//! Deezer Spleeter, 2 stems (vocals / accompaniment), through tract.
//!
//! The weights are Deezer's MIT-licensed `2stems` checkpoint, exported to
//! ONNX by the sherpa-onnx project (Apache-2.0 export code). The pipeline
//! follows sherpa-onnx's `offline-source-separation-spleeter-impl.h`:
//! 44.1 kHz, a 4096-point Hann STFT with a 1024 hop, the magnitudes of the
//! lowest 1024 bins in chunks of 512 frames go into each stem's network,
//! and the two estimates become Wiener masks `e_i^2 / (e_v^2 + e_a^2)`.
//!
//! One departure: sherpa-onnx zeroes the masks above bin 1024 (11 kHz),
//! which drops everything above 11 kHz from both stems. Here those bins
//! take the mean mask of the frame (Spleeter's own `average` mask
//! extension), so the stems still sum to the mix.

use std::path::Path;

use super::{soft_masks, Magnitudes, MaskEstimator, MaskSeparator, StftLayout};
use crate::cache::ModelCache;
use crate::error::MlError;
use crate::onnx::{tensor_f32, InputShape, OnnxModel};
use crate::registry::{ModelRegistry, SPLEETER_2STEMS};

const MODEL_BINS: usize = 1024;
const CHUNK_FRAMES: usize = 512;
const NAME: &str = "spleeter-2stems-v1";

/// The Spleeter 2-stem mask estimator.
#[derive(Clone, Debug)]
pub struct SpleeterOnnx {
    vocals: OnnxModel,
    accompaniment: OnnxModel,
}

impl SpleeterOnnx {
    /// Downloads the two networks on first use (about 79 MB, checksums
    /// verified) and loads them.
    pub fn load(cache: &ModelCache, registry: &ModelRegistry) -> Result<Self, MlError> {
        let paths = cache.ensure(registry.get(SPLEETER_2STEMS)?)?;
        Self::from_files(
            paths.file("vocals.onnx")?,
            paths.file("accompaniment.onnx")?,
        )
    }

    /// Loads the networks from their ONNX files.
    pub fn from_files(vocals: &Path, accompaniment: &Path) -> Result<Self, MlError> {
        let shape = [InputShape::f32(&[2, 1, CHUNK_FRAMES, MODEL_BINS])];
        Ok(Self {
            vocals: OnnxModel::load(SPLEETER_2STEMS, vocals, &shape)?,
            accompaniment: OnnxModel::load(SPLEETER_2STEMS, accompaniment, &shape)?,
        })
    }

    /// A separator around the loaded model.
    pub fn separator(self) -> MaskSeparator<Self> {
        MaskSeparator::new(self)
    }

    fn run(&self, model: &OnnxModel, input: &[f32]) -> Result<Vec<f32>, MlError> {
        let tensor = tensor_f32(&[2, 1, CHUNK_FRAMES, MODEL_BINS], input)?;
        let mut outputs = model.run(vec![tensor])?;
        let (_, data) = outputs
            .pop()
            .ok_or_else(|| MlError::inference(SPLEETER_2STEMS, "model has no output"))?;
        if data.len() != input.len() {
            return Err(MlError::inference(
                SPLEETER_2STEMS,
                format!("output has {} values, expected {}", data.len(), input.len()),
            ));
        }
        Ok(data)
    }
}

impl MaskEstimator for SpleeterOnnx {
    fn name(&self) -> &str {
        NAME
    }

    fn is_fallback(&self) -> bool {
        false
    }

    fn layout(&self) -> StftLayout {
        StftLayout {
            sample_rate: Some(44_100),
            fft_size: 4096,
            hop: 1024,
        }
    }

    fn stem_names(&self) -> Vec<String> {
        vec!["vocals".to_owned(), "accompaniment".to_owned()]
    }

    fn masks(&self, input: &Magnitudes) -> Result<Vec<Vec<Vec<f32>>>, MlError> {
        let (frames, bins) = (input.frames, input.bins);
        if bins <= MODEL_BINS || input.channels.is_empty() {
            return Err(MlError::InvalidInput(
                "spectrogram too narrow for Spleeter".to_owned(),
            ));
        }
        let channels = input.channels.len();
        // The network is stereo; a mono mix is fed to both inputs.
        let source = |model_channel: usize| &input.channels[model_channel.min(channels - 1)];
        let mut masks = vec![vec![vec![0.0f32; frames * bins]; channels]; 2];
        let plane = CHUNK_FRAMES * MODEL_BINS;
        let mut chunk = vec![0.0f32; 2 * plane];
        for first in (0..frames).step_by(CHUNK_FRAMES) {
            let count = CHUNK_FRAMES.min(frames - first);
            chunk.fill(0.0);
            for model_channel in 0..2 {
                let magnitudes = source(model_channel);
                for frame in 0..count {
                    let from = (first + frame) * bins;
                    let to = model_channel * plane + frame * MODEL_BINS;
                    chunk[to..to + MODEL_BINS]
                        .copy_from_slice(&magnitudes[from..from + MODEL_BINS]);
                }
            }
            let vocals = self.run(&self.vocals, &chunk)?;
            let accompaniment = self.run(&self.accompaniment, &chunk)?;
            let chunk_masks = soft_masks(&[&vocals, &accompaniment], 2.0);
            for (stem, stem_mask) in chunk_masks.iter().enumerate() {
                for (channel, out) in masks[stem].iter_mut().enumerate() {
                    let model_channel = channel.min(1);
                    for frame in 0..count {
                        let from = model_channel * plane + frame * MODEL_BINS;
                        let row = &stem_mask[from..from + MODEL_BINS];
                        let to = (first + frame) * bins;
                        out[to..to + MODEL_BINS].copy_from_slice(row);
                        let mean = row.iter().sum::<f32>() / MODEL_BINS as f32;
                        out[to + MODEL_BINS..to + bins].fill(mean);
                    }
                }
            }
        }
        Ok(masks)
    }
}
