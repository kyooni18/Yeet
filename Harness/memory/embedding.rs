//! Embedding identity and response validation. Privacy checks happen before
//! transport; compatibility is stricter than matching dimensions.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmbeddingExecution {
    Cloud,
    ManagedLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbeddingProfile {
    pub id: String,
    pub execution: EmbeddingExecution,
    pub model: String,
    /// Endpoint identity returned by transport, not a secret URL.
    pub source: String,
    pub resolved_model: String,
    pub dimensions: usize,
    pub preprocessing_revision: String,
}

impl EmbeddingProfile {
    pub fn fingerprint(&self) -> Result<String> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 128
                && self
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
                && !self.source.is_empty()
                && !self.model.is_empty()
                && !self.resolved_model.is_empty()
                && !self.preprocessing_revision.is_empty()
                && (1..=65536).contains(&self.dimensions),
            "invalid embedding profile"
        );
        let identity = (
            self.execution,
            &self.model,
            &self.source,
            &self.resolved_model,
            self.dimensions,
            &self.preprocessing_revision,
        );
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&identity)?)
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingBatch {
    pub model: String,
    pub resolved_model: String,
    pub source: String,
    pub vectors: Vec<Vec<f32>>,
}

pub fn validate_vectors(vectors: &[Vec<f32>], count: usize, dimensions: usize) -> Result<()> {
    ensure!(
        count > 0 && count <= 64 && (1..=65536).contains(&dimensions) && vectors.len() == count,
        "invalid embedding shape"
    );
    ensure!(
        vectors.iter().all(|v| v.len() == dimensions
            && v.iter().all(|n| n.is_finite())
            && v.iter().any(|n| *n != 0.0)),
        "invalid embedding values"
    );
    Ok(())
}
