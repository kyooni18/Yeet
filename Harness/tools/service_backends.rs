//! Runtime routing for semantic services that can run inside Yeet or through MCP.

use super::*;

/// Runtime routing choice for the web semantic service.
pub(super) struct ToolServiceConfiguration {
    pub(super) web_backend: ServiceBackend,
    pub(super) web_server: Option<String>,
}

impl Default for ToolServiceConfiguration {
    fn default() -> Self {
        Self {
            web_backend: ServiceBackend::Builtin,
            web_server: Some("web".into()),
        }
    }
}

impl ToolRegistry {
    pub fn configure_web_backend(&mut self, backend: ServiceBackend, server: impl Into<String>) {
        let server = server.into();
        if self.services.web_backend != backend
            || self.services.web_server.as_deref() != Some(server.as_str())
        {
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

        self.services.web_backend = backend;
        self.services.web_server = Some(server);
    }

    pub fn configure_searxng_url(&self, url: Option<String>) {
        self.web_search.set_searxng_url(url);
    }

    pub(super) fn configured_web_tool_definitions(&self) -> Vec<ToolDefinition> {
        if self.services.web_backend != ServiceBackend::Mcp {
            return vec![web_search_tool_definition(), web_read_tool_definition()];
        }
        let Some(server) = self.services.web_server.as_deref() else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lazy_registry() -> (tempfile::TempDir, ToolRegistry) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let registry = ToolRegistry::new_with_bridge_handle(
            BridgeHandle::lazy_for_workspace(root.clone()),
            root,
            WorkerRegistry::new(Vec::new()).unwrap(),
            PermissionBroker::default(),
        )
        .unwrap();
        (directory, registry)
    }

    #[test]
    fn service_configuration_changes_reset_only_relevant_evidence_and_survive_task_finish() {
        let (_directory, mut registry) = lazy_registry();

        registry
            .evidence
            .read_cache
            .insert("source.rs".into(), Vec::new());
        registry.evidence.searches.insert("workspace query".into());
        registry.evidence.web_searches.insert("web query".into());
        registry
            .evidence
            .web_sources
            .insert("https://example.com".into());
        registry
            .evidence
            .web_reads
            .insert("https://example.com".into());

        // Reapplying the current web route preserves its evidence.
        registry.configure_web_backend(ServiceBackend::Builtin, "web");
        assert!(registry.evidence.read_cache.contains_key("source.rs"));
        assert!(registry.evidence.searches.contains("workspace query"));
        assert!(
            registry
                .evidence
                .web_sources
                .contains("https://example.com")
        );

        assert!(registry.evidence.web_searches.contains("web query"));
        assert!(registry.evidence.web_reads.contains("https://example.com"));

        // Switching the web route clears web evidence while preserving workspace evidence.
        registry.configure_web_backend(ServiceBackend::Mcp, "search-service");
        assert!(registry.evidence.read_cache.contains_key("source.rs"));
        assert!(registry.evidence.searches.contains("workspace query"));
        assert!(registry.evidence.web_searches.is_empty());
        assert!(registry.evidence.web_sources.is_empty());
        assert!(registry.evidence.web_reads.is_empty());

        registry.begin_task("task");
        registry.finish_task("task");
        assert_eq!(registry.services.web_backend, ServiceBackend::Mcp);
        assert_eq!(
            registry.services.web_server.as_deref(),
            Some("search-service")
        );
        assert!(!registry.bridge.is_started());
    }
}
