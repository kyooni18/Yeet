//! Executor for activated external tools: Skills, MCP and Workers.
//!
//! Routing comes from `ToolCatalog::external_route`. Every path re-checks its
//! capability toggle at execution time, and any tool that may have changed
//! files outside the structured edit backend (non-read-only MCP, Workers)
//! advances the workspace write generation and drops cached evidence so a
//! later read cannot be suppressed as a stale duplicate.

use super::*;

impl ToolRegistry {
    pub(super) fn execute_external(
        &mut self,
        route: ExternalRoute,
        object: &Map<String, Value>,
        model: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        match route {
            ExternalRoute::SkillScript(skill) => {
                self.ensure_capability_allowed("skill", &skill)?;
                if self.catalog.deny_shell {
                    bail!("Agent execution policy prohibits Skill scripts");
                }
                self.run_skill_script(&skill, object, model, cancel)
            }
            ExternalRoute::SkillFile(skill) => {
                self.ensure_capability_allowed("skill", &skill)?;
                self.bridge_client()?.read_skill_file(
                    &skill,
                    object.get("path").and_then(Value::as_str).unwrap_or(""),
                )
            }
            ExternalRoute::Mcp {
                server,
                tool,
                read_only,
            } => {
                self.ensure_capability_allowed("mcp", &server)?;
                let result = self
                    .bridge_client()?
                    .call_mcp_tool_cancellable(&server, &tool, object, cancel)?
                    .to_string();
                if !read_only {
                    // MCP tools without an explicit readOnlyHint may have
                    // changed files behind the structured edit backend.
                    self.note_external_workspace_write();
                }
                Ok(result)
            }
            ExternalRoute::Worker { worker, tool } => {
                let result =
                    self.workers
                        .execute(&worker, &tool, object, &self.context.workspace_root)?;
                self.note_external_workspace_write();
                Ok(result)
            }
        }
    }

    fn note_external_workspace_write(&mut self) {
        self.evidence.workspace_write_generation =
            self.evidence.workspace_write_generation.wrapping_add(1);
        self.invalidate_workspace_cache();
    }
}
