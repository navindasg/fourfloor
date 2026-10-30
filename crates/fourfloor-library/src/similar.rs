//! Similarity search: brute-force cosine similarity over the stored
//! feature vectors, or over embeddings another crate stored.

use std::collections::HashSet;

use fourfloor_analysis::FeatureVector;

use crate::blob::{cosine_similarity, decode_features};
use crate::error::LibraryError;
use crate::library::Library;
use crate::query::Query;
use crate::types::{FileId, FileRecord};

/// One result of a similarity search.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// The file.
    pub file: FileRecord,
    /// Cosine similarity, -1 to 1; higher is more alike.
    pub score: f32,
}

impl Library {
    /// The files that sound most like `file`, best first, leaving `file`
    /// itself out. `filter` narrows the candidates (its sort, limit and
    /// offset are ignored); `limit` caps the result.
    ///
    /// Similarity is the cosine of the analysis crate's scaled feature
    /// embedding ([`FeatureVector::embedding`]): timbre, spectral shape,
    /// harmony and tempo.
    pub fn similar(
        &self,
        file: FileId,
        filter: &Query,
        limit: usize,
    ) -> Result<Vec<Match>, LibraryError> {
        let features = self
            .features(file)?
            .ok_or(LibraryError::NoFeatures(file.0))?;
        self.similar_to_features(&features, filter, limit, Some(file))
    }

    /// The files whose features are most like `features` (for a sound
    /// that is not in the library), best first.
    pub fn similar_to_features(
        &self,
        features: &FeatureVector,
        filter: &Query,
        limit: usize,
        exclude: Option<FileId>,
    ) -> Result<Vec<Match>, LibraryError> {
        let target = features.embedding();
        let allowed: HashSet<i64> = self.matching_ids(filter)?.into_iter().collect();
        let mut statement = self
            .conn
            .prepare("SELECT id, features FROM files WHERE features IS NOT NULL")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut scored: Vec<(FileId, f32)> = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            if !allowed.contains(&id) || exclude.is_some_and(|skip| skip.0 == id) {
                continue;
            }
            let candidate = decode_features(&blob)?;
            scored.push((
                FileId(id),
                cosine_similarity(&target, &candidate.embedding()),
            ));
        }
        self.best(scored, limit)
    }

    /// The files whose `model` embedding is nearest `vector` by cosine,
    /// best first: text search once the ML crate has stored text-audio
    /// embeddings with [`Library::set_embedding`].
    pub fn search_embedding(
        &self,
        model: &str,
        vector: &[f32],
        filter: &Query,
        limit: usize,
    ) -> Result<Vec<Match>, LibraryError> {
        let allowed: HashSet<i64> = self.matching_ids(filter)?.into_iter().collect();
        let scored = self
            .embeddings_for(model)?
            .into_iter()
            .filter(|(id, _)| allowed.contains(&id.0))
            .map(|(id, stored)| (id, cosine_similarity(vector, &stored)))
            .collect();
        self.best(scored, limit)
    }

    fn best(
        &self,
        mut scored: Vec<(FileId, f32)>,
        limit: usize,
    ) -> Result<Vec<Match>, LibraryError> {
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(id, score)| {
                Ok(Match {
                    file: self.file(id)?,
                    score,
                })
            })
            .collect()
    }
}
