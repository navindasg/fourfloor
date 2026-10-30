//! LAION CLAP `larger_clap_general` through tract.
//!
//! Weights: Apache-2.0 (LAION), 8-bit dynamically quantized ONNX export of
//! the audio and text towers by Xenova (`Xenova/larger_clap_general`).
//! Audio goes through [`super::mel`] and the HTSAT audio tower; text goes
//! through the RoBERTa byte-level BPE tokenizer and the text tower. Both
//! outputs are 512-dimensional projections, L2-normalised here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use fourfloor_audio::{resample, AudioBuffer};
use fourfloor_core::SampleRate;
use tract_onnx::prelude::InferenceModel;

use super::mel::{clap_log_mel, fit_to_ten_seconds, CLAP_FRAMES, CLAP_MELS, CLAP_SAMPLE_RATE};
use super::{AudioEmbedder, Embedding};
use crate::cache::ModelCache;
use crate::error::MlError;
use crate::onnx::{parse, tensor_f32, tensor_i64, FlatOutput, InputShape, OnnxModel};
use crate::registry::{ModelRegistry, CLAP_GENERAL};

/// Name written into CLAP embeddings.
pub const CLAP_EMBEDDING: &str = "clap-larger-general-q8-v1";
/// Dimensions of a CLAP embedding.
pub const CLAP_DIMENSIONS: usize = 512;
/// Longest token sequence fed to the text tower (CLAP's training length).
const MAX_TOKENS: usize = 77;

/// The CLAP audio and text encoders.
pub struct ClapEmbedder {
    audio: OnnxModel,
    text_graph: InferenceModel,
    tokenizer: tokenizers::Tokenizer,
    /// The text tower optimised for each token count seen so far; tract
    /// optimises for fixed shapes and the model has no attention mask, so
    /// queries are never padded.
    text_plans: Mutex<HashMap<usize, OnnxModel>>,
}

impl core::fmt::Debug for ClapEmbedder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClapEmbedder").finish_non_exhaustive()
    }
}

impl ClapEmbedder {
    /// Downloads the model on first use (about 207 MB, checksums verified)
    /// and loads it.
    pub fn load(cache: &ModelCache, registry: &ModelRegistry) -> Result<Self, MlError> {
        let paths = cache.ensure(registry.get(CLAP_GENERAL)?)?;
        Self::from_files(
            paths.file("audio_model_quantized.onnx")?,
            paths.file("text_model_quantized.onnx")?,
            paths.file("tokenizer.json")?,
        )
    }

    /// Loads the encoders from their files.
    pub fn from_files(audio: &Path, text: &Path, tokenizer: &Path) -> Result<Self, MlError> {
        let audio = OnnxModel::load(
            CLAP_GENERAL,
            audio,
            &[InputShape::f32(&[1, 1, CLAP_FRAMES, CLAP_MELS])],
        )?;
        let tokenizer = tokenizers::Tokenizer::from_file(tokenizer)
            .map_err(|error| MlError::inference(CLAP_GENERAL, error))?;
        Ok(Self {
            audio,
            text_graph: parse(CLAP_GENERAL, text)?,
            tokenizer,
            text_plans: Mutex::new(HashMap::new()),
        })
    }

    /// The token ids of a query, with `<s>` and `</s>`, at most 77.
    pub fn tokens(&self, text: &str) -> Result<Vec<i64>, MlError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|error| MlError::inference(CLAP_GENERAL, error))?;
        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|id| i64::from(*id)).collect();
        if ids.len() > MAX_TOKENS {
            let end = ids[ids.len() - 1];
            ids.truncate(MAX_TOKENS - 1);
            ids.push(end);
        }
        Ok(ids)
    }

    fn text_plan(&self, tokens: usize) -> Result<OnnxModel, MlError> {
        let mut plans = self
            .text_plans
            .lock()
            .map_err(|_| MlError::inference(CLAP_GENERAL, "text plan cache poisoned"))?;
        if let Some(plan) = plans.get(&tokens) {
            return Ok(plan.clone());
        }
        let plan = OnnxModel::from_parsed(
            CLAP_GENERAL,
            self.text_graph.clone(),
            &[InputShape::i64(&[1, tokens])],
        )?;
        plans.insert(tokens, plan.clone());
        Ok(plan)
    }

    fn single_output(outputs: Vec<FlatOutput>) -> Result<Vec<f32>, MlError> {
        let (shape, data) = outputs
            .into_iter()
            .next()
            .ok_or_else(|| MlError::inference(CLAP_GENERAL, "model has no output"))?;
        if data.len() != CLAP_DIMENSIONS {
            return Err(MlError::inference(
                CLAP_GENERAL,
                format!("unexpected output shape {shape:?}"),
            ));
        }
        Ok(data)
    }
}

impl AudioEmbedder for ClapEmbedder {
    fn model_name(&self) -> &str {
        CLAP_EMBEDDING
    }

    fn is_fallback(&self) -> bool {
        false
    }

    fn dimensions(&self) -> usize {
        CLAP_DIMENSIONS
    }

    fn embed_audio(&self, audio: &AudioBuffer) -> Result<Embedding, MlError> {
        if audio.frames() == 0 || audio.channel_count() == 0 {
            return Err(MlError::InvalidInput("audio is empty".to_owned()));
        }
        let rate = SampleRate::new(CLAP_SAMPLE_RATE)
            .map_err(|error| MlError::InvalidInput(error.to_string()))?;
        let mono: Vec<f32> = audio
            .to_mono()
            .into_iter()
            .map(|s| if s.is_finite() { s } else { 0.0 })
            .collect();
        let signal = if audio.sample_rate == rate {
            mono
        } else {
            resample(&mono, audio.sample_rate, rate)
        };
        let features = clap_log_mel(&fit_to_ten_seconds(&signal));
        let input = tensor_f32(&[1, 1, CLAP_FRAMES, CLAP_MELS], &features)?;
        let vector = Self::single_output(self.audio.run(vec![input])?)?;
        Ok(Embedding::new(CLAP_EMBEDDING, vector))
    }

    fn embed_text(&self, text: &str) -> Result<Embedding, MlError> {
        let tokens = self.tokens(text)?;
        let plan = self.text_plan(tokens.len())?;
        let input = tensor_i64(&[1, tokens.len()], &tokens)?;
        let vector = Self::single_output(plan.run(vec![input])?)?;
        Ok(Embedding::new(CLAP_EMBEDDING, vector))
    }
}
