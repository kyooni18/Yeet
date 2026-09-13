//! Cross-tool token-efficiency policy for output bounds and deferred extension surfaces.

use crate::core::ToolCall;
use anyhow::Result;

use super::{FOUNDATION_RECALL_TOOL, ToolRegistry, artifact_output};

/// a1b3's two widest research rounds returned four independent web results each,
/// totaling about 17 KiB of model-visible text. A 12 KiB aggregate budget leaves
/// roughly 3 KiB per result in a four-call round (comfortably above web_read's
/// 2K minimum evidence request after wrapper overhead) while removing the
/// otherwise multiplicative tail. Single-call rounds retain the 20 KiB boundary.
const MULTI_CALL_ROUND_MODEL_VISIBLE_TOOL_OUTPUT_BYTES: usize = 12 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ModelVisibleToolOutputRoundBudget {
    remaining_bytes: usize,
    remaining_calls: usize,
}

impl ModelVisibleToolOutputRoundBudget {
    pub(crate) fn new(call_count: usize) -> Self {
        let total_budget_bytes = if call_count <= 1 {
            artifact_output::MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES
        } else {
            MULTI_CALL_ROUND_MODEL_VISIBLE_TOOL_OUTPUT_BYTES
        };
        Self {
            remaining_bytes: total_budget_bytes,
            remaining_calls: call_count.max(1),
        }
    }

    /// Reserves a fair share for every still-pending call. Small early results
    /// return their unused capacity to later calls, so the cap does not punish a
    /// useful large result merely because it was ordered after tiny metadata.
    fn next_limit(&self) -> usize {
        if self.remaining_calls == 0 {
            return 0;
        }
        (self.remaining_bytes / self.remaining_calls)
            .min(artifact_output::MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES)
    }

    fn observe(&mut self, rendered_bytes: usize) {
        self.remaining_bytes = self.remaining_bytes.saturating_sub(rendered_bytes);
        self.remaining_calls = self.remaining_calls.saturating_sub(1);
    }
}

impl ToolRegistry {
    pub(crate) fn round_output_budget(call_count: usize) -> ModelVisibleToolOutputRoundBudget {
        ModelVisibleToolOutputRoundBudget::new(call_count)
    }

    /// Final model-visible output boundary shared by built-ins, Skills, Workers,
    /// and MCP tools. Tool-specific handlers may externalize earlier; this is the
    /// fallback that prevents an extension result from dominating every later
    /// request in the same turn.
    pub fn bound_model_visible_tool_output(
        &self,
        tool_name: &str,
        content: String,
    ) -> Result<(String, bool)> {
        artifact_output::externalize_model_visible_tool_output(&self.artifacts, tool_name, content)
    }

    /// Applies both the historical per-call boundary and the aggregate budget
    /// shared by all results produced before the next model inference.
    pub(crate) fn bound_round_output(
        &self,
        call: &ToolCall,
        content: String,
        budget: &mut ModelVisibleToolOutputRoundBudget,
    ) -> Result<(String, bool)> {
        let limit = budget.next_limit();
        let result = artifact_output::externalize_model_visible_tool_output_with_limit(
            &self.artifacts,
            &call.name,
            content,
            limit,
        )?;
        budget.observe(result.0.len());
        Ok(result)
    }

    pub fn is_read_only_extension_tool(&self, name: &str) -> bool {
        self.skill_tool_map.contains_key(name)
            || self.read_only_mcp_tools.contains(name)
            || name == FOUNDATION_RECALL_TOOL
    }

    pub fn is_provider_defer_candidate(&self, name: &str) -> bool {
        self.mcp_tool_map.contains_key(name)
            || self.skill_tool_map.contains_key(name)
            || self.skill_script_tool_map.contains_key(name)
            || self.worker_tool_map.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn bound_reference_round(sizes: &[usize]) -> (usize, usize) {
        let store = super::super::ArtifactStore::new().unwrap();
        let mut budget = ModelVisibleToolOutputRoundBudget::new(sizes.len());
        let mut visible = 0usize;
        let mut externalized = 0usize;
        for (index, size) in sizes.iter().copied().enumerate() {
            let source = format!("result-{index}:{}", "x".repeat(size.saturating_sub(9)));
            let limit = budget.next_limit();
            let (bounded, was_externalized) =
                artifact_output::externalize_model_visible_tool_output_with_limit(
                    &store,
                    "web_evidence",
                    source.clone(),
                    limit,
                )
                .unwrap();
            assert!(bounded.len() <= limit);
            if was_externalized {
                externalized += 1;
                let payload: Value = serde_json::from_str(&bounded).unwrap();
                let id = payload["artifactId"].as_str().unwrap();
                assert_eq!(store.read(id, None, None).unwrap(), source);
                assert!(
                    payload["preview"]
                        .as_str()
                        .is_some_and(|preview| !preview.is_empty())
                );
            } else {
                assert_eq!(bounded, source);
            }
            visible += bounded.len();
            budget.observe(bounded.len());
        }
        (visible, externalized)
    }

    #[test]
    fn a1b3_four_result_rounds_are_aggregate_bounded_and_recoverable() {
        // Exact UTF-8 byte sizes measured from the persisted a1b3 model-history rounds.
        let read_round = [4_527, 4_577, 3_339, 4_582];
        let search_round = [4_450, 4_323, 4_206, 4_143];
        let (read_visible, read_externalized) = bound_reference_round(&read_round);
        let (search_visible, search_externalized) = bound_reference_round(&search_round);

        assert!(read_visible <= MULTI_CALL_ROUND_MODEL_VISIBLE_TOOL_OUTPUT_BYTES);
        assert!(search_visible <= MULTI_CALL_ROUND_MODEL_VISIBLE_TOOL_OUTPUT_BYTES);
        assert_eq!(read_externalized, 4);
        assert_eq!(search_externalized, 4);
        let original = read_round.iter().sum::<usize>() + search_round.iter().sum::<usize>();
        let bounded = read_visible + search_visible;
        assert!(original.saturating_sub(bounded) >= 9 * 1024);
    }

    #[test]
    fn small_parallel_results_keep_exact_content_and_return_unused_capacity() {
        let (visible, externalized) = bound_reference_round(&[1_744, 219]);
        assert_eq!(externalized, 0);
        assert_eq!(visible, 1_963);
    }

    #[test]
    fn single_call_round_retains_the_existing_twenty_kib_boundary() {
        let store = super::super::ArtifactStore::new().unwrap();
        let budget = ModelVisibleToolOutputRoundBudget::new(1);
        assert_eq!(
            budget.next_limit(),
            artifact_output::MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES
        );
        let source = "x".repeat(16 * 1024);
        let limit = budget.next_limit();
        let (bounded, externalized) =
            artifact_output::externalize_model_visible_tool_output_with_limit(
                &store,
                "single",
                source.clone(),
                limit,
            )
            .unwrap();
        assert!(!externalized);
        assert_eq!(bounded, source);
    }

    #[test]
    fn pathological_parallel_batch_still_obeys_the_hard_round_ceiling() {
        let store = super::super::ArtifactStore::new().unwrap();
        let mut budget = ModelVisibleToolOutputRoundBudget::new(256);
        let mut visible = 0usize;
        for index in 0..256 {
            let limit = budget.next_limit();
            let source = format!("result-{index}:{}", "x".repeat(4 * 1024));
            let (bounded, externalized) =
                artifact_output::externalize_model_visible_tool_output_with_limit(
                    &store,
                    "pathological_parallel_evidence",
                    source,
                    limit,
                )
                .unwrap();
            assert!(externalized);
            assert!(bounded.len() <= limit);
            visible += bounded.len();
            budget.observe(bounded.len());
        }
        assert!(visible <= MULTI_CALL_ROUND_MODEL_VISIBLE_TOOL_OUTPUT_BYTES);
    }

    #[test]
    fn budget_reallocates_unused_early_result_space_to_later_calls() {
        let mut budget = ModelVisibleToolOutputRoundBudget::new(4);
        assert_eq!(MULTI_CALL_ROUND_MODEL_VISIBLE_TOOL_OUTPUT_BYTES, 12 * 1024);
        assert_eq!(budget.next_limit(), 3 * 1024);
        budget.observe(512);
        assert!(budget.next_limit() > 3 * 1024);
    }
}
