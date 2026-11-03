//! CPU-native managed embeddings using pinned safetensors and tokenizer data.
//! No Python, chat-provider daemon, downloaded executable or inference network.

use super::*;
use anyhow::{Context, Result, ensure};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use std::{
    fs::{self, File},
    sync::atomic::{AtomicBool, Ordering},
};
use tokenizers::Tokenizer;

/// One loaded encoder, with a shared installation lease. Hosts should serialize
/// requests and reuse this handle; dropping it releases weights and the lease.
pub struct LocalEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    profile: EmbeddingProfile,
    max_tokens: usize,
    _lease: File,
}

impl LocalEmbedder {
    pub fn load(models: &ManagedModels, model_id: &str, profile_id: String) -> Result<Self> {
        let manifest = models.manifest(model_id)?;
        let lease = models.read_lease(model_id)?;
        let directory = models.directory(model_id)?;
        let config: Config = serde_json::from_slice(&fs::read(directory.join("config.json"))?)?;
        let tokenizer = Tokenizer::from_file(directory.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
        let tokenizer = prepare_tokenizer(tokenizer)?;
        let device = Device::Cpu;
        let weights = fs::read(directory.join("model.safetensors"))?;
        let variables = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)?;
        let model = BertModel::load(variables, &config)?;
        let profile = EmbeddingProfile {
            id: profile_id,
            execution: EmbeddingExecution::ManagedLocal,
            model: manifest.repository.into(),
            source: format!("{}@{}", manifest.repository, manifest.revision),
            resolved_model: manifest.revision.into(),
            dimensions: manifest.dimensions,
            preprocessing_revision: format!(
                "{}:{}:tokens{}:unpadded:leaves240o24:v1",
                manifest.runtime, manifest.pooling, manifest.max_tokens
            ),
        };
        profile.fingerprint()?;
        Ok(Self {
            model,
            tokenizer,
            profile,
            max_tokens: manifest.max_tokens,
            _lease: lease,
        })
    }

    pub fn profile(&self) -> &EmbeddingProfile {
        &self.profile
    }

    /// Reject oversized inputs instead of silently truncating the tail. Call
    /// `chunks` before indexing long documents. Query/document formatting is
    /// identical for this particular encoder and is pinned in its fingerprint.
    pub fn embed(&self, input: &[String], cancel: &AtomicBool) -> Result<Vec<Vec<f32>>> {
        ensure!(
            !input.is_empty() && input.len() <= 64,
            "invalid embedding batch size"
        );
        let mut vectors = Vec::new();
        for text in input {
            ensure!(!cancel.load(Ordering::Acquire), "cancelled");
            ensure!(
                !text.trim().is_empty() && text.len() <= 128 * 1024,
                "invalid embedding input"
            );
            let tokens = self
                .tokenizer
                .encode(text.as_str(), true)
                .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
            ensure!(
                tokens.len() <= self.max_tokens,
                "embedding_input_too_long: split into model-aware chunks"
            );
            let token_ids = Tensor::new(tokens.get_ids(), &Device::Cpu)?.unsqueeze(0)?;
            let type_ids = token_ids.zeros_like()?;
            let embeddings = self.model.forward(&token_ids, &type_ids, None)?;
            let pooled = (embeddings.sum(1)? / tokens.len() as f64)?;
            let mut vector = pooled.squeeze(0)?.to_vec1::<f32>()?;
            let norm = vector
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                .sqrt();
            ensure!(norm > 0.0 && norm.is_finite(), "invalid embedding norm");
            for value in &mut vector {
                *value = (f64::from(*value) / norm) as f32;
            }
            vectors.push(vector);
        }
        validate_vectors(&vectors, input.len(), self.profile.dimensions)?;
        Ok(vectors)
    }

    /// Stable byte ranges with token overlap, leaving room for special tokens.
    pub fn chunks(&self, text: &str) -> Result<Vec<Chunk>> {
        ensure!(text.len() <= 128 * 1024, "input exceeds 128 KiB");
        let tokens = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
        let offsets = tokens.get_offsets();
        let capacity = self.max_tokens - 16;
        let mut start = 0;
        let mut chunks = Vec::new();
        while start < offsets.len() {
            let end = (start + capacity).min(offsets.len());
            let byte_start = if start == 0 { 0 } else { offsets[start].0 };
            let byte_end = if end == offsets.len() {
                text.len()
            } else {
                offsets[end].0
            };
            let content = text
                .get(byte_start..byte_end)
                .context("invalid tokenizer source span")?;
            chunks.push(Chunk {
                start: byte_start,
                end: byte_end,
                text: content.into(),
            });
            if end == offsets.len() {
                break;
            }
            start = end.saturating_sub(24);
        }
        Ok(chunks)
    }
}

fn prepare_tokenizer(mut tokenizer: Tokenizer) -> Result<Tokenizer> {
    // The published tokenizer enables fixed padding/truncation. We run one
    // unpadded sequence and reject overflow ourselves; do not pool padding or
    // silently discard long text before checking its token count.
    tokenizer.with_padding(None);
    tokenizer
        .with_truncation(None)
        .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
    Ok(tokenizer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::{
        PaddingParams, PaddingStrategy, TruncationParams, models::wordlevel::WordLevel,
        pre_tokenizers::whitespace::Whitespace,
    };

    #[test]
    fn serialized_padding_and_truncation_cannot_corrupt_native_inputs() {
        let model = WordLevel::builder()
            .vocab(
                [("[UNK]".into(), 0), ("word".into(), 1)]
                    .into_iter()
                    .collect(),
            )
            .unk_token("[UNK]".into())
            .build()
            .unwrap();
        let mut tokenizer = Tokenizer::new(model);
        tokenizer.with_pre_tokenizer(Some(Whitespace));
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::Fixed(128),
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 128,
                ..Default::default()
            }))
            .unwrap();
        let tokenizer = prepare_tokenizer(tokenizer).unwrap();
        assert_eq!(tokenizer.encode("word", false).unwrap().len(), 1);
        assert_eq!(
            tokenizer.encode("word ".repeat(300), false).unwrap().len(),
            300
        );
    }
}
