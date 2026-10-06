//! Tool catalog: model-facing schemas, activation state and routing lookup.
//!
//! The catalog answers "which tools exist, which are enabled, and which
//! domain executes a given name" without executing anything. Activation code
//! (`capability_runtime`, `session_capabilities`, `service_backends`) mutates
//! it; `ToolRegistry::execute` only reads it to route a call.

use super::*;

/// Schemas and activation maps for every tool the current session can see.
pub(super) struct ToolCatalog {
    pub(super) active_tools: BTreeMap<String, ToolDefinition>,
    pub(super) descriptors: Vec<CapabilityDescriptor>,
    pub(super) skill_tool_map: HashMap<String, String>,
    pub(super) skill_script_tool_map: HashMap<String, String>,
    pub(super) mcp_tool_map: HashMap<String, (String, String)>,
    pub(super) foundation_tool_map: HashMap<String, FoundationToolTarget>,
    pub(super) worker_tool_map: HashMap<String, (String, String)>,
    pub(super) active_skills: HashSet<String>,
    pub(super) active_mcp: HashSet<String>,
    pub(super) active_mcp_identity: HashMap<String, McpServerIdentity>,
    pub(super) active_workers: HashSet<String>,
    /// MCP tools explicitly annotated read-only; only these may run in a
    /// parallel batch or inside research.
    pub(super) read_only_mcp_tools: HashSet<String>,
    pub(super) disabled_capabilities: HashSet<String>,
}

/// Where a non-built-in tool name is executed.
pub(super) enum ExternalRoute {
    Foundation(FoundationToolTarget),
    SkillScript(String),
    SkillFile(String),
    Mcp {
        server: String,
        tool: String,
        read_only: bool,
    },
    Worker {
        worker: String,
        tool: String,
    },
}

impl ToolCatalog {
    pub(super) fn with_builtin_tools() -> Self {
        Self {
            active_tools: base_tool_definitions()
                .into_iter()
                .map(|tool| (tool.name.clone(), tool))
                .collect(),
            descriptors: Vec::new(),
            skill_tool_map: HashMap::new(),
            skill_script_tool_map: HashMap::new(),
            mcp_tool_map: HashMap::new(),
            foundation_tool_map: HashMap::new(),
            worker_tool_map: HashMap::new(),
            active_skills: HashSet::new(),
            active_mcp: HashSet::new(),
            active_mcp_identity: HashMap::new(),
            active_workers: HashSet::new(),
            read_only_mcp_tools: HashSet::new(),
            disabled_capabilities: HashSet::new(),
        }
    }

    pub(super) fn capability_disabled(&self, kind: &str, name: &str) -> bool {
        self.disabled_capabilities
            .contains(&format!("{kind}:{name}"))
    }

    /// Whether `tool_name` may be listed or executed. Project-memory tools
    /// additionally depend on the memory service being active.
    pub(super) fn tool_enabled(&self, tool_name: &str, foundation_active: bool) -> bool {
        if builtin_capability_for_tool(tool_name)
            .is_some_and(|capability| self.disabled_capabilities.contains(capability.id))
        {
            return false;
        }
        if let Some(skill) = self.skill_tool_map.get(tool_name) {
            return !self.capability_disabled("skill", skill);
        }
        if let Some((server, _)) = self.mcp_tool_map.get(tool_name) {
            return !self.capability_disabled("mcp", server);
        }
        if self.foundation_tool_map.contains_key(tool_name) {
            return foundation_active;
        }
        true
    }

    /// Enabled active schemas, with descriptions labelled by their source.
    pub(super) fn enabled_definitions(&self, foundation_active: bool) -> Vec<ToolDefinition> {
        let mut tools: Vec<_> = self
            .active_tools
            .values()
            .filter(|tool| self.tool_enabled(&tool.name, foundation_active))
            .cloned()
            .collect();
        for tool in &mut tools {
            // Built-in descriptions already contain their execution contract.
            // Capability summaries belong in settings, not in every model tool.
            let label = if self.foundation_tool_map.contains_key(&tool.name) {
                Some("Project memory".to_owned())
            } else if let Some((server, _)) = self.mcp_tool_map.get(&tool.name) {
                Some(format!("MCP {server}"))
            } else if let Some(skill) = self
                .skill_tool_map
                .get(&tool.name)
                .or_else(|| self.skill_script_tool_map.get(&tool.name))
            {
                Some(format!("Skill {skill}"))
            } else {
                self.worker_tool_map
                    .get(&tool.name)
                    .map(|(worker, _)| format!("Worker {worker}"))
            };
            if let Some(label) = label {
                let tool_description = tool.description.take().unwrap_or_default();
                tool.description = Some(format!("{label}: {tool_description}"));
            }
        }
        tools
    }

    /// Tools research may call: read-only built-ins, Skill reads and MCP
    /// tools explicitly annotated read-only.
    pub(super) fn research_tool_allowed(&self, name: &str) -> bool {
        matches!(
            name,
            "find_capabilities"
                | "activate_capability"
                | "read_file"
                | "list_files"
                | "search_workspace"
                | "web_search"
                | "web_read"
                | "read_document"
                | "analyze_data"
                | "artifact_info"
                | "read_artifact"
                | "search_artifact"
                | FOUNDATION_RECALL_TOOL
                | "project_memory_get"
                | "project_memory_connections"
        ) || self.skill_tool_map.contains_key(name)
            || self.read_only_mcp_tools.contains(name)
    }

    /// The enabled read-only MCP target for `call`, if it may join a
    /// parallel read-only batch.
    pub(super) fn read_only_mcp_target(&self, call: &ToolCall) -> Option<(String, String)> {
        if !self.read_only_mcp_tools.contains(&call.name) || !call.arguments.is_object() {
            return None;
        }
        let (server, tool) = self.mcp_tool_map.get(&call.name)?;
        (!self.capability_disabled("mcp", server)).then(|| (server.clone(), tool.clone()))
    }

    /// Resolves an activated external tool name. Lookup order matters when
    /// stable-name allocation ever collides: project memory, Skill scripts,
    /// Skill files, MCP, then Workers.
    pub(super) fn external_route(&self, name: &str) -> Option<ExternalRoute> {
        if let Some(target) = self.foundation_tool_map.get(name) {
            return Some(ExternalRoute::Foundation(target.clone()));
        }
        if let Some(skill) = self.skill_script_tool_map.get(name) {
            return Some(ExternalRoute::SkillScript(skill.clone()));
        }
        if let Some(skill) = self.skill_tool_map.get(name) {
            return Some(ExternalRoute::SkillFile(skill.clone()));
        }
        if let Some((server, tool)) = self.mcp_tool_map.get(name) {
            return Some(ExternalRoute::Mcp {
                server: server.clone(),
                tool: tool.clone(),
                read_only: self.read_only_mcp_tools.contains(name),
            });
        }
        self.worker_tool_map
            .get(name)
            .map(|(worker, tool)| ExternalRoute::Worker {
                worker: worker.clone(),
                tool: tool.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_mcp_server_hides_tools_and_parallel_eligibility() {
        let mut catalog = ToolCatalog::with_builtin_tools();
        catalog
            .mcp_tool_map
            .insert("mcp_docs_search".into(), ("docs".into(), "search".into()));
        catalog.read_only_mcp_tools.insert("mcp_docs_search".into());
        let call = ToolCall {
            id: "1".into(),
            name: "mcp_docs_search".into(),
            arguments: json!({}),
        };
        assert!(catalog.tool_enabled("mcp_docs_search", false));
        assert!(catalog.read_only_mcp_target(&call).is_some());
        assert!(catalog.research_tool_allowed("mcp_docs_search"));

        catalog.disabled_capabilities.insert("mcp:docs".into());
        assert!(!catalog.tool_enabled("mcp_docs_search", false));
        assert!(catalog.read_only_mcp_target(&call).is_none());
    }
}
