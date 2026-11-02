//! Cross-tool token-efficiency policy for output bounds and deferred extension surfaces.

use crate::core::ToolCall;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;

use super::{ToolRegistry, artifact_output};

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
        &mut self,
        tool_name: &str,
        content: String,
    ) -> Result<(String, bool)> {
        if !self.artifacts_enabled {
            return Ok((content, false));
        }
        if matches!(tool_name, "read_file" | "read_files")
            && content.len() > artifact_output::MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES
        {
            // This legacy boundary has no request paths. Conservatively revoke
            // source evidence rather than credit content the model never saw.
            self.evidence.read_cache.clear();
            self.evidence.edit_snapshots.clear();
            self.evidence.edit_read_coverage.clear();
        }
        artifact_output::externalize_model_visible_tool_output(&self.artifacts, tool_name, content)
    }

    /// Applies both the historical per-call boundary and the aggregate budget
    /// shared by all results produced before the next model inference.
    pub(crate) fn bound_round_output(
        &mut self,
        call: &ToolCall,
        content: String,
        budget: &mut ModelVisibleToolOutputRoundBudget,
    ) -> Result<(String, bool)> {
        if !self.artifacts_enabled {
            budget.observe(content.len());
            return Ok((content, false));
        }
        let limit = budget.next_limit();
        if content.len() > limit && matches!(call.name.as_str(), "read_file" | "read_files") {
            self.revoke_shortened_read_evidence(&call.arguments);
        }
        let result = artifact_output::externalize_model_visible_tool_output_with_limit(
            &self.artifacts,
            &call.name,
            content,
            limit,
        )?;
        budget.observe(result.0.len());
        Ok(result)
    }

    /// The formatter can shorten a read after its handler recorded coverage.
    /// A saved artifact is recoverable evidence, not evidence already viewed.
    /// Revoke all evidence for the requested files; the next narrow read will
    /// restore only delivered ranges and rehydrate the backend accordingly.
    fn revoke_shortened_read_evidence(&mut self, arguments: &Value) {
        fn collect_paths(value: &Value, paths: &mut Vec<String>) {
            if let Some(path) = value.get("path").and_then(Value::as_str) {
                paths.push(path.to_owned());
            }
            if let Some(requests) = value.get("requests").and_then(Value::as_array) {
                for request in requests {
                    collect_paths(request, paths);
                }
            }
        }
        let mut requested = Vec::new();
        collect_paths(arguments, &mut requested);
        let paths = requested
            .iter()
            .filter_map(|requested| {
                let path = self.context.resolve_session_path(requested).ok()?;
                super::paths::stable_workspace_path_key(
                    &self.context.workspace_root,
                    &path.to_string_lossy(),
                )
                .ok()
            })
            .collect::<HashSet<_>>();
        for path in paths {
            self.evidence.read_cache.remove(&path);
            self.evidence.edit_snapshots.remove(&path);
            self.evidence.edit_read_coverage.remove(&path);
        }
    }

    pub fn is_read_only_extension_tool(&self, name: &str) -> bool {
        self.catalog.skill_tool_map.contains_key(name)
            || self.catalog.read_only_mcp_tools.contains(name)
    }

    pub fn is_provider_defer_candidate(&self, name: &str) -> bool {
        self.catalog.mcp_tool_map.contains_key(name)
            || self.catalog.skill_tool_map.contains_key(name)
            || self.catalog.skill_script_tool_map.contains_key(name)
            || self.catalog.worker_tool_map.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::super::BridgeHandle;
    use super::*;
    use crate::{permission::PermissionBroker, workers::WorkerRegistry};
    use serde_json::json;

    #[test]
    fn final_read_truncation_revokes_only_requested_source_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(
            root.join("large.txt"),
            format!("{}\n", "line".repeat(100)).repeat(12),
        )
        .unwrap();
        std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
        let mut registry = ToolRegistry::new_with_bridge_handle(
            BridgeHandle::lazy_for_workspace(root.clone()),
            root,
            WorkerRegistry::new(Vec::new()).unwrap(),
            PermissionBroker::default(),
        )
        .unwrap();
        registry
            .read_file(json!({"path":"keep.txt"}).as_object().unwrap())
            .unwrap();
        let args = json!({"requests":[{"path":"large.txt"}]});
        let content = registry.read_file(args.as_object().unwrap()).unwrap();
        assert!(
            registry
                .evidence
                .edit_read_coverage
                .contains_key("large.txt")
        );
        registry
            .evidence
            .edit_snapshots
            .insert("large.txt".into(), "old-handle".into());
        let call = ToolCall {
            id: "read".into(),
            name: "read_file".into(),
            arguments: args,
        };
        let (rendered, shortened) = registry
            .bound_round_output(&call, content, &mut ToolRegistry::round_output_budget(8))
            .unwrap();
        assert!(
            shortened && rendered.contains("read_artifact") && rendered.contains("before editing")
        );
        assert!(!registry.evidence.read_cache.contains_key("large.txt"));
        assert!(!registry.evidence.edit_snapshots.contains_key("large.txt"));
        assert!(
            !registry
                .evidence
                .edit_read_coverage
                .contains_key("large.txt")
        );
        assert!(
            registry
                .evidence
                .edit_read_coverage
                .contains_key("keep.txt")
        );
        // A narrow reread restores exactly the requested evidence, not the
        // unseen remainder retained in the artifact.
        registry
            .read_file(
                json!({"path":"large.txt","startLine":2,"endLine":2})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            registry.evidence.edit_read_coverage["large.txt"].ranges,
            vec![(2, 2)]
        );
    }
}
