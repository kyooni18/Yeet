//! Cloud profile validation sends only an explicit bundled non-sensitive probe.
//! Real memory dispatch is gated until durable egress/job fencing is integrated.

use super::*;
use crate::core::BridgeClient;
use anyhow::{Result, ensure};
use serde_json::json;
use std::sync::atomic::AtomicBool;

/// Caller must obtain permission to contact the selected endpoint. No user
/// history, document or management-search query is accepted by this API.
pub fn probe_cloud_profile(
    bridge: &BridgeClient,
    id: String,
    model: String,
    cancel: &AtomicBool,
) -> Result<EmbeddingProfile> {
    ensure!(
        !id.is_empty() && model.contains('/'),
        "profile ID and provider/model are required"
    );
    let input = vec![
        "Yeet embedding compatibility probe: document.",
        "Yeet embedding compatibility probe: query.",
    ];
    let fields = json!({"model":model,"input":input})
        .as_object()
        .unwrap()
        .clone();
    let frame = bridge.request_cancellable("embed", fields, cancel)?;
    let batch: EmbeddingBatch = serde_json::from_value(
        frame
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing embedding response"))?,
    )?;
    let dimensions = batch.vectors.first().map_or(0, Vec::len);
    validate_vectors(&batch.vectors, 2, dimensions)?;
    ensure!(batch.model == model, "embedding model mismatch");
    let profile = EmbeddingProfile {
        id,
        execution: EmbeddingExecution::Cloud,
        model,
        source: batch.source,
        resolved_model: batch.resolved_model,
        dimensions,
        preprocessing_revision: "plain-text-v1".into(),
    };
    profile.fingerprint()?;
    Ok(profile)
}
