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
