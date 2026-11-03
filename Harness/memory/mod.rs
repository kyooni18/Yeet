//! Native memory persistence and policy. Model access is shared-only until
//! protected context provenance is enforced across every generation path.

mod chunking;
mod cloud;
mod embedding;
mod retrieval;
mod store;
mod types;

pub use chunking::{Chunk, chunk_text};
pub use cloud::probe_cloud_profile;
pub use embedding::{EmbeddingBatch, EmbeddingExecution, EmbeddingProfile, validate_vectors};
pub use retrieval::MemoryHit;
pub use store::MemoryStore;
pub use types::*;

#[cfg(test)]
mod tests;
