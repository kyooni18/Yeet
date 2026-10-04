//! Runtime routing for semantic services that can run inside Yeet or through MCP.

use super::*;

impl ToolRegistry {
    pub fn configure_foundation_memory(
        &mut self,
        enabled: bool,
        backend: ServiceBackend,
        server: impl Into<String>,
        project: impl Into<String>,
    ) {
        let server = server.into();
        let project = project.into();
        let changed = self.foundation_backend != backend
            || self.foundation_server.as_deref() != Some(server.as_str())
            || self.foundation_project.as_deref() != Some(project.as_str())
            || self.foundation_enabled != enabled;
        if changed {
            self.deactivate_foundation();
        }
        self.foundation_enabled = enabled;
        self.foundation_backend = backend;
        self.foundation_server = Some(server);
        self.foundation_project = Some(project);
    }

    pub fn configure_web_backend(&mut self, backend: ServiceBackend, server: impl Into<String>) {
        let server = server.into();
        if self.web_backend != backend || self.web_server.as_deref() != Some(server.as_str()) {
            self.evidence.web_searches.clear();
            self.evidence.web_sources.clear();
            self.evidence.web_reads.clear();
        }

        // web_search/web_read are reserved semantic tools. MCP servers can
        // provide their implementation through the selected Web backend, but
        // must not also expose raw aliases that bypass Yeet's provenance guard.
        let raw_web_aliases = self
            .catalog
            .mcp_tool_map
            .iter()
            .filter(|(_, (_, tool))| matches!(tool.as_str(), "web_search" | "web_read"))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in raw_web_aliases {
            self.catalog.mcp_tool_map.remove(&name);
            self.catalog.active_tools.remove(&name);
            self.catalog.read_only_mcp_tools.remove(&name);
        }

        self.web_backend = backend;
        self.web_server = Some(server);
    }

    pub(super) fn foundation_memory_active(&self) -> bool {
        let backend_available = match self.foundation_backend {
            ServiceBackend::Builtin => true,
            ServiceBackend::Mcp => self
                .foundation_server
                .as_deref()
                .is_some_and(|server| !self.catalog.capability_disabled("mcp", server)),
        };
        self.foundation_enabled
            && backend_available
            && self.foundation_project.is_some()
            && self
                .catalog
                .foundation_tool_map
                .contains_key(FOUNDATION_RECALL_TOOL)
    }

    pub fn foundation_memory_guidance(&self) -> Option<&'static str> {
        self.foundation_memory_active().then_some(
            "Yeet project memory uses the configured Foundation backend alongside session-local task_notes and context_history. Keep task progress and exact evidence in those local stores; promote only durable knowledge to project memory. Available memory operations depend on the selected backend. Give mutable current-state facts a stable key so newer values supersede older ones while preserving history. Do not store secrets, credentials, raw transcripts, transient progress chatter, build output, or facts that are cheap to rediscover from the repository. Fresh source evidence overrides stale memory.",
        )
    }

    pub(super) fn call_foundation_target(
        &self,
        target: &FoundationToolTarget,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let project = self
            .foundation_project
            .as_deref()
            .ok_or_else(|| anyhow!("Project identity is unavailable"))?;
        if target.server != crate::foundation_backend::BUILTIN_FOUNDATION_SERVER
            && self.catalog.capability_disabled("mcp", &target.server)
        {
            bail!("MCP server {} is disabled for this session", target.server);
        }
        let mut arguments = object.clone();
        arguments.insert("project".into(), json!(project));
        let result = self.bridge_client()?.call_mcp_tool_cancellable(
            &target.server,
            &target.tool,
            &arguments,
            cancel,
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!(
                "Foundation {}/{}: {}",
                target.server,
                target.tool,
                foundation_tool_result_text(&result)
            );
        }
        Ok(result)
    }

    pub fn recall_foundation_memory(
        &self,
        query: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<String>> {
        if !self.foundation_memory_active() || query.trim().is_empty() {
            return Ok(None);
        }
        let target = self
            .catalog
            .foundation_tool_map
            .get(FOUNDATION_RECALL_TOOL)
            .ok_or_else(|| anyhow!("Project memory recall is unavailable"))?;
        let query = query.chars().take(20_000).collect::<String>();
        let value = self.call_foundation_target(
            target,
            &Map::from_iter([("query".into(), json!(query)), ("limit".into(), json!(8))]),
            cancel,
        )?;
        Ok(foundation_context_from_tool_result(&value))
    }

    pub(super) fn configured_web_tool_definitions(&self) -> Vec<ToolDefinition> {
        if self.web_backend == ServiceBackend::Builtin {
            return vec![web_search_tool_definition(), web_read_tool_definition()];
        }
        let Some(server) = self.web_server.as_deref() else {
            return Vec::new();
        };
        if self.catalog.capability_disabled("mcp", server) {
            return Vec::new();
        }
        let Ok(mut tools) = self
            .bridge_client()
            .and_then(|bridge| bridge.list_mcp_tools(Some(server)))
        else {
            return Vec::new();
        };
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        let has_search = tools.iter().any(|tool| tool.name == "web_search");
        let has_read = tools.iter().any(|tool| tool.name == "web_read");
        if !has_search || !has_read {
            return Vec::new();
        }
        tools
            .into_iter()
            .filter(|tool| matches!(tool.name.as_str(), "web_search" | "web_read"))
            .map(|tool| ToolDefinition {
                name: tool.name,
                description: Some(format!(
                    "Web backend MCP {server}: {}",
                    tool.description.unwrap_or_default()
                )),
                input_schema: tool.input_schema,
            })
            .collect()
    }
}
