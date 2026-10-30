//! Sample embeddings for search: "dusty 909 open hat" by text, or "sounds
//! like this one" by audio.
//!
//! - [`ClapEmbedder`] (feature `clap`) runs LAION CLAP `larger_clap_general`
//!   (Apache-2.0 weights; 8-bit ONNX export by Xenova) with tract. Text and
//!   audio land in the same 512-dimensional space, so a text query ranks
//!   samples directly.
//! - [`DspEmbedder`] is the **fallback**: the hand-made timbre, brightness,
//!   tonality and tempo features of `fourfloor-analysis`, audio only. It
//!   finds similar sounds but cannot answer text queries.
//!
//! An [`Embedding`] records which model made it, is L2-normalised, and
//! converts to and from little-endian bytes for storage in the sample
//! library's database. Embeddings of different models are never compared.

#[cfg(feature = "clap")]
mod clap;
pub mod mel;

use fourfloor_audio::AudioBuffer;
use serde::{Deserialize, Serialize};

#[cfg(feature = "clap")]
pub use clap::ClapEmbedder;

use crate::error::MlError;

/// A unit-length embedding vector and the model that made it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    /// Name of the model (or fallback) that made the vector.
    pub model: String,
    /// The vector, L2-normalised (all zeros only for silence).
    pub vector: Vec<f32>,
}

impl Embedding {
    /// An embedding from a raw vector, normalised to unit length.
    /// Non-finite values are replaced by zero.
    pub fn new(model: &str, mut vector: Vec<f32>) -> Self {
        for value in &mut vector {
            if !value.is_finite() {
                *value = 0.0;
            }
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            vector.iter_mut().for_each(|v| *v /= norm);
        }
        Self {
            model: model.to_owned(),
            vector,
        }
    }

    /// Number of dimensions.
    pub fn len(&self) -> usize {
        self.vector.len()
    }

    /// Whether the vector is empty.
    pub fn is_empty(&self) -> bool {
        self.vector.is_empty()
    }

    /// Cosine similarity, -1 to 1. Refuses embeddings of different models
    /// or sizes.
    pub fn similarity(&self, other: &Self) -> Result<f32, MlError> {
        if self.model != other.model || self.len() != other.len() {
            return Err(MlError::InvalidInput(format!(
                "cannot compare a `{}` embedding ({} dims) with a `{}` one ({} dims)",
                self.model,
                self.len(),
                other.model,
                other.len()
            )));
        }
        Ok(self
            .vector
            .iter()
            .zip(&other.vector)
            .map(|(a, b)| a * b)
            .sum())
    }

    /// The vector as little-endian `f32` bytes, for a database blob.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        self.vector.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// An embedding from bytes written by [`Embedding::to_le_bytes`].
    pub fn from_le_bytes(model: &str, bytes: &[u8]) -> Result<Self, MlError> {
        if !bytes.len().is_multiple_of(4) {
            return Err(MlError::InvalidInput(format!(
                "{} bytes is not a whole number of f32 values",
                bytes.len()
            )));
        }
        Ok(Self {
            model: model.to_owned(),
            vector: bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect(),
        })
    }
}

/// Embeds audio.
pub trait AudioEmbedder: Send + Sync {
    /// Name written into every [`Embedding`] this embedder makes.
    fn model_name(&self) -> &str;
    /// Whether this is the DSP fallback rather than a trained model.
    fn is_fallback(&self) -> bool;
    /// Number of dimensions.
    fn dimensions(&self) -> usize;
    /// Embeds a sample (any rate, any channel count).
    fn embed_audio(&self, audio: &AudioBuffer) -> Result<Embedding, MlError>;
    /// Embeds a text query into the same space, if the model has one.
    fn embed_text(&self, text: &str) -> Result<Embedding, MlError> {
        let _ = text;
        Err(MlError::Unsupported(format!(
            "`{}` has no text encoder",
            self.model_name()
        )))
    }
}

/// Ranks `candidates` by similarity to `query`, best first. Candidates made
/// by another model are skipped.
pub fn rank<'a, T>(
    query: &Embedding,
    candidates: impl IntoIterator<Item = (T, &'a Embedding)>,
) -> Vec<(T, f32)> {
    let mut scored: Vec<(T, f32)> = candidates
        .into_iter()
        .filter_map(|(id, embedding)| query.similarity(embedding).ok().map(|score| (id, score)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
}

/// Name of the fallback's embeddings.
pub const DSP_EMBEDDING: &str = "fallback-dsp-features-v1";

/// The fallback embedder: `fourfloor-analysis` features (MFCC mean and
/// spread, spectral centroid, roll-off, flatness, zero-crossing rate,
/// chroma, tempo), 41 dimensions. Audio only.
#[derive(Clone, Copy, Debug, Default)]
pub struct DspEmbedder;

impl AudioEmbedder for DspEmbedder {
    fn model_name(&self) -> &str {
        DSP_EMBEDDING
    }

    fn is_fallback(&self) -> bool {
        true
    }

    fn dimensions(&self) -> usize {
        fourfloor_analysis::EMBEDDING_LEN
    }

    fn embed_audio(&self, audio: &AudioBuffer) -> Result<Embedding, MlError> {
        if audio.frames() == 0 || audio.channel_count() == 0 {
            return Err(MlError::InvalidInput("audio is empty".to_owned()));
        }
        let features = fourfloor_analysis::extract_features(&audio.to_mono(), audio.sample_rate)
            .map_err(|error| MlError::InvalidInput(error.to_string()))?;
        Ok(Embedding::new(DSP_EMBEDDING, features.embedding()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddings_are_normalised_and_round_trip_through_bytes() {
        let e = Embedding::new("m", vec![3.0, 4.0, f32::NAN]);
        assert_eq!(e.vector, vec![0.6, 0.8, 0.0]);
        let back = Embedding::from_le_bytes("m", &e.to_le_bytes()).expect("bytes");
        assert_eq!(back, e);
        assert!(Embedding::from_le_bytes("m", &[0, 1, 2]).is_err());
        assert!((e.similarity(&e).expect("same") - 1.0).abs() < 1e-6);
        assert!(e
            .similarity(&Embedding::new("other", vec![1.0, 0.0, 0.0]))
            .is_err());
        assert!(Embedding::new("m", vec![0.0, 0.0])
            .vector
            .iter()
            .all(|v| *v == 0.0));
    }

    #[test]
    fn ranking_puts_the_closest_first_and_skips_other_models() {
        let query = Embedding::new("m", vec![1.0, 0.0]);
        let near = Embedding::new("m", vec![0.9, 0.1]);
        let far = Embedding::new("m", vec![-1.0, 0.2]);
        let alien = Embedding::new("x", vec![1.0, 0.0]);
        let ranked = rank(&query, [("far", &far), ("alien", &alien), ("near", &near)]);
        let names: Vec<&str> = ranked.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec!["near", "far"]);
    }

    #[test]
    fn the_fallback_has_no_text_encoder() {
        assert!(matches!(
            DspEmbedder.embed_text("dusty 909 open hat"),
            Err(MlError::Unsupported(_))
        ));
        assert!(DspEmbedder.is_fallback());
    }
}
