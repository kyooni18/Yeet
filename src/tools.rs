use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agents::{
        AgentGroupHandle, AgentNotification,
        group::commands::{AGENT_TOOL, LEGACY_PROPOSE_TOOL, SEND_TOOL, STOP_TOOL},
    },
    core::{BridgeClient, ToolCall, ToolDefinition, Usage},
    edit::{ApplyResult, EditClient},
    general,
    permission::PermissionBroker,
    project_settings::ServiceBackend,
    sandbox::{SandboxMode, SandboxStore},
    session_store::SessionStore,
    shell::{
        ShellExecutionRequest, ShellProgress, restricted_operation, run_shell_cancellable,
        run_shell_cancellable_with_progress,
    },
    web_search::{WebSearchBackend, WebSearchClient, WebSearchRequest},
    workers::WorkerRegistry,
};

mod agent_deploy;
mod artifact_output;
mod bridge_handle;
mod capability_runtime;
mod computer_use;
mod definitions;
mod edit_lock;
mod editing;
mod environment;
mod io;
mod paths;
pub mod repo_map;
mod service_backends;
mod session_capabilities;
mod shell_jobs;
mod shell_runtime;
mod support;
pub mod symbols;
mod syntax_edit;
mod token_efficiency;

pub(crate) use agent_deploy::deploy_agent_for_workspace;
pub(crate) use bridge_handle::BridgeHandle;
use definitions::BUILTIN_CAPABILITIES;
pub(crate) use definitions::direct_mcp_tool_definitions;
use definitions::{base_tool_definitions, web_read_tool_definition, web_search_tool_definition};
pub use definitions::{is_coding_builtin_tool, is_general_builtin_tool};
use paths::canonicalize_existing_ancestor;
pub(crate) use paths::workspace_revision_for_path;
use shell_runtime::shell_mentions_path;
pub(crate) use support::canonical_web_source_key;
use support::{
    ArtifactStore, McpServerIdentity, ReadCacheEntry, allocate_stable_tool_name,
    append_bounded_state_set, builtin_capability_for_tool, cache_read_result,
    collect_web_source_urls, coverage_complete, covered_ranges_within,
    foundation_context_from_tool_result, foundation_tool_result_text, foundation_wrapper_schema,
    merged_ranges, next_uncovered, shell_quote, string_arg, trim_cache_to_preview,
    truncate_state_value, uncovered_ranges, usize_arg, web_search_queries,
};

const DEFAULT_READ_LINES: usize = 160;
const EXPLICIT_READ_LINES: usize = 480;
const DEFAULT_INLINE_BYTES: usize = 12 * 1024;
const EXPLICIT_INLINE_BYTES: usize = 16 * 1024;
const FOUNDATION_RECALL_TOOL: &str = "project_memory_recall";
const MAX_CAPABILITY_SEARCH_RESULTS: usize = 8;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: String,
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinCapabilityDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    tools: &'static [&'static str],
}

pub fn builtin_capabilities() -> &'static [BuiltinCapabilityDescriptor] {
    BUILTIN_CAPABILITIES
}

fn capability_search_values(descriptors: &[CapabilityDescriptor], query: &str) -> Vec<Value> {
    descriptors
        .iter()
        .filter(|descriptor| {
            query.is_empty()
                || descriptor.id.to_ascii_lowercase().contains(query)
                || descriptor.description.to_ascii_lowercase().contains(query)
        })
        .take(MAX_CAPABILITY_SEARCH_RESULTS)
        .map(|descriptor| {
            json!({
                "id": descriptor.id,
                "kind": descriptor.kind,
                "description": truncate_state_value(&descriptor.description, 320)
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
struct MutationValidation {
    error_count: usize,
    changed_paths: Vec<String>,
}

impl MutationValidation {
    pub fn write_validation_passed(&self) -> bool {
        self.error_count == 0
    }
}

#[derive(Debug, Clone)]
struct FoundationToolTarget {
    server: String,
    tool: String,
}
#[derive(Debug, Clone)]
struct EditReadCoverage {
    snapshot: String,
    ranges: Vec<(usize, usize)>,
}

pub struct ToolRegistry {
    bridge: BridgeHandle,
    edit: Option<EditClient>,
    edit_generation: u64,
    workspace_root: PathBuf,
    working_directory: PathBuf,
    context_roots: Vec<PathBuf>,
    hard_access_root: Option<PathBuf>,
    workers: WorkerRegistry,
    permission: PermissionBroker,
    artifacts: ArtifactStore,
    artifacts_enabled: bool,
    descriptors: Vec<CapabilityDescriptor>,
    active_tools: BTreeMap<String, ToolDefinition>,
    skill_tool_map: HashMap<String, String>,
    skill_script_tool_map: HashMap<String, String>,
    mcp_tool_map: HashMap<String, (String, String)>,
    foundation_tool_map: HashMap<String, FoundationToolTarget>,
    worker_tool_map: HashMap<String, (String, String)>,
    active_skills: HashSet<String>,
    active_mcp: HashSet<String>,
    active_mcp_identity: HashMap<String, McpServerIdentity>,
    foundation_enabled: bool,
    foundation_backend: ServiceBackend,
    foundation_server: Option<String>,
    foundation_project: Option<String>,
    active_workers: HashSet<String>,
    skyline_handle: Option<String>,
    disabled_capabilities: HashSet<String>,
    agent_group: Option<AgentGroupHandle>,
    /// Identifies a delegated agent in permission prompts it raises.
    permission_label: Option<String>,
    read_cache: HashMap<String, Vec<ReadCacheEntry>>,
    edit_snapshots: HashMap<String, String>,
    edit_read_coverage: HashMap<String, EditReadCoverage>,
    searches: HashSet<String>,
    listings: HashSet<String>,
    shell_jobs: shell_jobs::ShellJobs,
    shell_inspections: HashSet<String>,
    permitted_shell_commands: HashSet<String>,
    auxiliary_usage: Option<Usage>,
    latest_mutation: Option<MutationValidation>,
    web_search: WebSearchClient,
    web_backend: ServiceBackend,
    web_server: Option<String>,
    web_searches: HashSet<String>,
    web_sources: HashSet<String>,
    web_reads: HashSet<String>,
    read_only_mcp_tools: HashSet<String>,
    workspace_generation: u64,
    workspace_write_generation: u64,
    protected_write_paths: Vec<PathBuf>,
    session_store: Option<SessionStore>,
    active_session_id: Option<String>,
    active_task_id: Option<String>,
}

impl ToolRegistry {
    pub fn new(
        bridge: BridgeClient,
        workspace_root: PathBuf,
        workers: WorkerRegistry,
        permission: PermissionBroker,
    ) -> Result<Self> {
        Self::new_with_bridge_handle(
            BridgeHandle::eager(bridge),
            workspace_root,
            workers,
            permission,
        )
    }

    pub(crate) fn new_with_bridge_handle(
        bridge: BridgeHandle,
        workspace_root: PathBuf,
        workers: WorkerRegistry,
        permission: PermissionBroker,
    ) -> Result<Self> {
        // External sidecars are intentionally lazy. Creating a workspace/tool
        // registry must stay in-process until a provider, MCP server, Skill, or
        // Computer Use operation actually needs the Node bridge.
        let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);
        let working_directory = workspace_root.clone();
        let context_roots = vec![workspace_root.clone()];
        let mut active_tools = BTreeMap::new();
        for tool in base_tool_definitions() {
            active_tools.insert(tool.name.clone(), tool);
        }
        Ok(Self {
            bridge,
            edit: None,
            edit_generation: 0,
            workspace_root,
            working_directory,
            context_roots,
            hard_access_root: None,
            workers,
            permission,
            artifacts: ArtifactStore::new()?,
            artifacts_enabled: true,
            descriptors: Vec::new(),
            active_tools,
            skill_tool_map: HashMap::new(),
            skill_script_tool_map: HashMap::new(),
            mcp_tool_map: HashMap::new(),
            foundation_tool_map: HashMap::new(),
            worker_tool_map: HashMap::new(),
            active_skills: HashSet::new(),
            active_mcp: HashSet::new(),
            active_mcp_identity: HashMap::new(),
            foundation_enabled: false,
            foundation_backend: ServiceBackend::Builtin,
            foundation_server: Some("foundation".into()),
            foundation_project: None,
            active_workers: HashSet::new(),
            skyline_handle: None,
            disabled_capabilities: HashSet::new(),
            agent_group: None,
            permission_label: None,
            read_cache: HashMap::new(),
            edit_snapshots: HashMap::new(),
            edit_read_coverage: HashMap::new(),
            searches: HashSet::new(),
            listings: HashSet::new(),
            shell_jobs: shell_jobs::ShellJobs::default(),
            shell_inspections: HashSet::new(),
            permitted_shell_commands: HashSet::new(),
            auxiliary_usage: None,
            latest_mutation: None,
            web_search: WebSearchClient::default(),
            web_backend: ServiceBackend::Builtin,
            web_server: Some("web".into()),
            web_searches: HashSet::new(),
            web_sources: HashSet::new(),
            web_reads: HashSet::new(),
            read_only_mcp_tools: HashSet::new(),
            workspace_generation: 0,
            workspace_write_generation: 0,
            protected_write_paths: Vec::new(),
            session_store: None,
            active_session_id: None,
            active_task_id: None,
        })
    }

    fn edit_mut(&mut self) -> Result<&mut EditClient> {
        if self.edit.is_none() {
            let edit = EditClient::start(&self.workspace_root)?;
            self.edit_generation = edit.generation();
            self.edit = Some(edit);
        }
        Ok(self.edit.as_mut().expect("edit client initialized"))
    }

    #[cfg(test)]
    pub(crate) fn is_edit_started(&self) -> bool {
        self.edit.is_some()
    }

    pub(crate) fn bridge_client(&self) -> Result<BridgeClient> {
        self.bridge.client()
    }

    pub fn set_session_runtime(&mut self, store: SessionStore, active_session_id: Option<String>) {
        self.artifacts.root = active_session_id
            .as_ref()
            .map(|id| store.directory.join(id).join("artifacts"))
            .unwrap_or_else(|| self.artifacts._directory.path().to_path_buf());
        self.session_store = Some(store);
        self.active_session_id = active_session_id;
    }

    pub fn set_artifacts_enabled(&mut self, enabled: bool) {
        self.artifacts_enabled = enabled;
    }

    pub(crate) fn bind_runtime_agent(&self, id: crate::agents::AgentId) {
        if let Some(group) = &self.agent_group {
            group.bind_primary_agent(id);
        }
    }

    pub(crate) fn set_agent_group(&mut self, group: Option<AgentGroupHandle>) {
        self.agent_group = group;
        if self.agent_group.is_some() {
            for definition in AgentGroupHandle::tool_definitions() {
                self.active_tools
                    .insert(definition.name.clone(), definition);
            }
        } else {
            for name in AgentGroupHandle::tool_names() {
                self.active_tools.remove(name);
            }
        }
    }

    /// Lets one response's foreground agent calls run concurrently.
    pub(crate) fn prepare_tool_batch(&self, calls: &[ToolCall], model: &str) {
        if let Some(group) = &self.agent_group {
            group.prestart(calls, model, self.active_session_id.clone());
        }
    }

    pub(crate) fn take_agent_notifications(&self) -> Vec<AgentNotification> {
        self.agent_group
            .as_ref()
            .map(AgentGroupHandle::take_notifications)
            .unwrap_or_default()
    }

    pub(crate) fn set_permission_label(&mut self, label: String) {
        self.permission_label = Some(label);
    }

    pub(crate) fn agent_orchestration_enabled(&self) -> bool {
        self.agent_group.is_some()
    }

    pub fn runtime_capability_snapshot(&self, visible_tools: &[ToolDefinition]) -> Value {
        let policy = SandboxStore::new(&self.workspace_root)
            .and_then(|store| store.load())
            .ok();
        json!({
            "visibleTools": visible_tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            "sandboxMode": policy.as_ref().map(|policy| match policy.mode {
                SandboxMode::Sandboxed => "sandboxed",
                SandboxMode::Unlimited => "unlimited",
            }),
            "autoApprove": policy.as_ref().map(|policy| policy.auto_approve),
            "activeSessionProtected": !self.protected_write_paths.is_empty(),
            "activeSessionId": self.active_session_id,
            "workspaceRoot": self.workspace_root,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn workspace_identity(&self) -> (String, Option<String>) {
        let root = self
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_root.clone());
        (
            root.display().to_string(),
            workspace_revision_for_path(&root),
        )
    }

    pub fn tools(&self, web_search_enabled: bool) -> Vec<ToolDefinition> {
        // Return the eligible catalog. The agent attaches schemas on demand
        // and retains loaded schemas for the rest of its turn.
        let mut tools: Vec<_> = self
            .active_tools
            .values()
            .filter(|tool| self.tool_enabled(&tool.name))
            .cloned()
            .collect();
        for tool in &mut tools {
            // Built-in descriptions already contain their execution contract.
            // Capability summaries belong in settings, not in every model tool.
            if self.foundation_tool_map.contains_key(&tool.name) {
                let tool_description = tool.description.take().unwrap_or_default();
                tool.description = Some(format!("Project memory: {tool_description}"));
            } else if let Some((server, _)) = self.mcp_tool_map.get(&tool.name) {
                let tool_description = tool.description.take().unwrap_or_default();
                tool.description = Some(format!("MCP {server}: {tool_description}"));
            } else if let Some(skill) = self.skill_tool_map.get(&tool.name) {
                let tool_description = tool.description.take().unwrap_or_default();
                tool.description = Some(format!("Skill {skill}: {tool_description}"));
            } else if let Some(skill) = self.skill_script_tool_map.get(&tool.name) {
                let tool_description = tool.description.take().unwrap_or_default();
                tool.description = Some(format!("Skill {skill}: {tool_description}"));
            } else if let Some((worker, _)) = self.worker_tool_map.get(&tool.name) {
                let tool_description = tool.description.take().unwrap_or_default();
                tool.description = Some(format!("Worker {worker}: {tool_description}"));
            }
        }
        if web_search_enabled {
            tools.extend(self.configured_web_tool_definitions());
        }
        tools
    }

    pub fn set_disabled_capabilities(&mut self, disabled: impl IntoIterator<Item = String>) {
        self.disabled_capabilities = disabled.into_iter().collect();
        self.descriptors.clear();
    }

    /// Research uses the normal registry, with the same execution guard as its schema filter.
    pub fn research_tools(&self) -> Vec<ToolDefinition> {
        self.tools(true)
            .into_iter()
            .filter(|tool| self.research_tool_allowed(&tool.name))
            .collect()
    }

    fn research_tool_allowed(&self, name: &str) -> bool {
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

    pub fn can_parallel_read_only_mcp_batch(&self, calls: &[ToolCall]) -> bool {
        (2..=4).contains(&calls.len())
            && calls.iter().all(|call| {
                self.read_only_mcp_tools.contains(&call.name)
                    && call.arguments.is_object()
                    && self
                        .mcp_tool_map
                        .get(&call.name)
                        .is_some_and(|(server, _)| !self.capability_disabled("mcp", server))
            })
    }

    /// Executes a batch only when every call is an MCP tool explicitly annotated read-only.
    /// The shared bridge supports concurrent request ids; mutation-capable tools never enter this path.
    pub fn execute_parallel_read_only_mcp_batch(
        &self,
        calls: &[ToolCall],
        cancel: &AtomicBool,
    ) -> Option<Vec<std::result::Result<String, String>>> {
        if !(2..=4).contains(&calls.len()) {
            return None;
        }
        let targets = calls
            .iter()
            .map(|call| {
                if !self.read_only_mcp_tools.contains(&call.name) {
                    return None;
                }
                let (server, tool) = self.mcp_tool_map.get(&call.name)?.clone();
                if self.capability_disabled("mcp", &server) {
                    return None;
                }
                let arguments = call.arguments.as_object()?.clone();
                Some((server, tool, arguments))
            })
            .collect::<Option<Vec<_>>>()?;
        let bridge = self.bridge_client().ok()?;
        Some(thread::scope(|scope| {
            let handles = targets
                .into_iter()
                .map(|(server, tool, arguments)| {
                    let bridge = bridge.clone();
                    scope.spawn(move || {
                        bridge
                            .call_mcp_tool_cancellable(&server, &tool, &arguments, cancel)
                            .map(|value| value.to_string())
                            .map_err(|error| error.to_string())
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("parallel read-only MCP execution panicked".into()))
                })
                .collect()
        }))
    }

    pub fn execute_research(
        &mut self,
        call: &ToolCall,
        model: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        if !self.research_tool_allowed(&call.name) {
            bail!("{} is not a read-only research tool", call.name);
        }
        if call.name == "activate_capability"
            && call
                .arguments
                .get("capability")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("worker:"))
        {
            bail!("Research can activate read-only MCP tools and Skills, not workers");
        }
        self.execute(call, model, cancel)
    }

    pub fn attach_enabled_mcp_servers(&mut self) -> Result<()> {
        let servers = self.bridge_client()?.list_mcp_servers()?;
        let configured = servers
            .iter()
            .map(|server| server.name.clone())
            .collect::<HashSet<_>>();
        let stale = self
            .active_mcp
            .iter()
            .filter(|name| !configured.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        for server in stale {
            self.deactivate_mcp(&server);
        }
        for server in servers {
            // Foundation memory owns this server through the canonical
            // project_memory_* surface. In MCP mode retry activation here so a
            // server that was unavailable at session startup can recover later;
            // in builtin mode keep it hidden to avoid duplicate memory surfaces.
            if self.foundation_server.as_deref() == Some(server.name.as_str()) {
                if self.foundation_backend == ServiceBackend::Mcp
                    && self.foundation_enabled
                    && !self.capability_disabled("mcp", &server.name)
                {
                    let _ = self.activate(&format!("mcp:{}", server.name));
                }
                continue;
            }
            if self.capability_disabled("mcp", &server.name) {
                continue;
            }
            let identity = McpServerIdentity::from(&server);
            if self.active_mcp.contains(&server.name) {
                if self.active_mcp_identity.get(&server.name) == Some(&identity) {
                    continue;
                }
                self.deactivate_mcp(&server.name);
            }
            // One unavailable optional MCP must not take the entire agent down.
            // It remains discoverable and can be retried explicitly later.
            if self.activate(&format!("mcp:{}", server.name)).is_ok() {
                self.active_mcp_identity.insert(server.name, identity);
            }
        }
        Ok(())
    }

    pub fn refresh_capabilities(&mut self) {
        let mut descriptors = Vec::new();
        if let Ok(skills) = self.bridge_client().and_then(|bridge| bridge.list_skills()) {
            descriptors.extend(
                skills
                    .into_iter()
                    .filter(|skill| {
                        skill.allow_implicit_invocation != Some(false)
                            && !self.capability_disabled("skill", &skill.name)
                    })
                    .map(|skill| CapabilityDescriptor {
                        id: format!("skill:{}", skill.name),
                        kind: "skill".into(),
                        description: skill.description,
                    }),
            );
        }
        if let Ok(servers) = self
            .bridge_client()
            .and_then(|bridge| bridge.list_mcp_servers())
        {
            descriptors.extend(
                servers
                    .into_iter()
                    .filter(|server| !self.capability_disabled("mcp", &server.name))
                    .map(|server| CapabilityDescriptor {
                        id: format!("mcp:{}", server.name),
                        kind: "mcp".into(),
                        description: format!(
                            "MCP {} server{}",
                            server.transport,
                            if server.connected { " (connected)" } else { "" }
                        ),
                    }),
            );
        }
        descriptors.extend(self.workers.descriptors().into_iter().map(|worker| {
            CapabilityDescriptor {
                id: format!("worker:{}", worker.id),
                kind: "worker".into(),
                description: format!(
                    "{} v{}: {}",
                    worker.name, worker.version, worker.description
                ),
            }
        }));
        descriptors.sort_by(|left, right| left.id.cmp(&right.id));
        self.descriptors = descriptors;
    }

    pub fn execute(&mut self, call: &ToolCall, model: &str, cancel: &AtomicBool) -> Result<String> {
        self.sync_edit_state();
        if self.shell_jobs.has_jobs() {
            // Refresh evidence without treating every poll/read as agent progress.
            let generation = self.workspace_generation;
            self.invalidate_workspace_cache();
            self.workspace_generation = generation;
        }
        if !self.tool_enabled(&call.name) {
            bail!("Tool {} is disabled for this session", call.name);
        }
        let object = call
            .arguments
            .as_object()
            .ok_or_else(|| anyhow!("tool arguments must be an object"))?
            .clone();
        match call.name.as_str() {
            "find_capabilities" => {
                self.refresh_capabilities();
                let query = object
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let values = capability_search_values(&self.descriptors, &query);
                Ok(serde_json::to_string(&values)?)
            }
            "activate_capability" => {
                let id = string_arg(&object, "capability")?;
                self.activate(id)
            }
            "skyline" => self.execute_skyline(&object),
            "deploy_agent" => self.deploy_agent(&object, cancel),
            AGENT_TOOL | SEND_TOOL | STOP_TOOL | LEGACY_PROPOSE_TOOL => self
                .agent_group
                .as_ref()
                .ok_or_else(|| anyhow!("adaptive agent orchestration is not enabled"))?
                .execute(call, &object, model, self.active_session_id.clone(), cancel),
            "read_file" => self.read_file(&object),
            // Hidden compatibility alias for restored sessions created before
            // read_file absorbed batch reads. New requests never expose this schema.
            "read_files" => self.read_files(&object),
            // Compatibility path for historical calls. list_files is no longer
            // part of the model-visible built-in catalog; use run_shell for
            // native directory and metadata inspection instead.
            "list_files" => self.list_files(&object),
            "search_workspace" => self.search_workspace(&object),
            "web_search" => self.web_search(&object, cancel),
            "web_read" => self.web_read(&object, cancel),
            "read_document" => self.read_document_tool(&object),
            "analyze_data" => self.analyze_data_tool(&object),
            "artifact_info" => {
                let id = string_arg(&object, "id")?;
                Ok(self.artifacts.info(id)?.to_string())
            }
            "read_artifact" => {
                let id = string_arg(&object, "id")?;
                let text = self.artifacts.read(
                    id,
                    usize_arg(&object, "startLine"),
                    usize_arg(&object, "endLine"),
                )?;
                artifact_output::page(&text, &object)
            }
            "search_artifact" => {
                let id = string_arg(&object, "id")?;
                let query = string_arg(&object, "query")?;
                Ok(self
                    .artifacts
                    .search(
                        id,
                        query,
                        usize_arg(&object, "maxResults").unwrap_or(20).clamp(1, 50),
                    )?
                    .to_string())
            }
            "list_sessions" => {
                let store = self
                    .session_store
                    .as_ref()
                    .ok_or_else(|| anyhow!("Session runtime is not attached"))?;
                let debate_only = object
                    .get("debateOnly")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let include_current = object
                    .get("includeCurrent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let limit = usize_arg(&object, "limit").unwrap_or(20).clamp(1, 100);
                let mut values = Vec::new();
                for session in store.list(&self.workspace_root)? {
                    let is_current = self.active_session_id.as_deref() == Some(session.id.as_str());
                    if is_current && !include_current {
                        continue;
                    }
                    let is_debate = store.is_debate_session(&session.id);
                    if debate_only && !is_debate {
                        continue;
                    }
                    values.push(json!({
                        "id": session.id,
                        "title": session.title,
                        "updatedAt": session.updated_at,
                        "model": session.model,
                        "messageCount": session.message_count,
                        "isDebate": is_debate,
                        "isCurrent": is_current,
                    }));
                    if values.len() >= limit {
                        break;
                    }
                }
                Ok(json!({
                    "sessions": values,
                    "currentSessionId": self.active_session_id,
                    "currentExcludedByDefault": !include_current,
                    "predicate": if debate_only { "debates/state.json exists and topic is non-empty" } else { "workspace session" },
                }).to_string())
            }
            "export_session" => {
                let session_id = string_arg(&object, "sessionId")?.to_owned();
                let destination = object
                    .get("destination")
                    .and_then(Value::as_str)
                    .map(|value| {
                        let path = PathBuf::from(value);
                        if path.is_absolute() {
                            path
                        } else {
                            self.workspace_root.join(path)
                        }
                    });
                if let Some(destination) = destination.as_ref() {
                    self.ensure_file_scope(&destination.to_string_lossy(), true)?;
                }
                let delete_source = object
                    .get("deleteSource")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let store = self
                    .session_store
                    .as_ref()
                    .ok_or_else(|| anyhow!("Session runtime is not attached"))?;
                let result = store.export_session(
                    &session_id,
                    destination.as_deref(),
                    delete_source,
                    self.active_session_id.as_deref(),
                )?;
                Ok(serde_json::to_string(&result)?)
            }
            "request_shell_permission" => {
                let command = string_arg(&object, "command")?.trim().to_owned();
                let reason = string_arg(&object, "reason")?.trim().to_owned();
                if command.is_empty() || reason.is_empty() {
                    bail!("request_shell_permission requires command and reason");
                }
                let restricted = restricted_operation(&command);
                let operation = restricted
                    .clone()
                    .unwrap_or_else(|| "unrestricted shell access".into());
                if restricted.is_some() && self.disabled_capabilities.contains("builtin:file-write")
                {
                    bail!(
                        "File Write is disabled for this session; mutating shell commands cannot be permitted"
                    );
                }
                let granted = self.request_approval("shell", &command, &operation, &reason)?;
                if granted {
                    self.permitted_shell_commands.insert(command.clone());
                }
                Ok(json!({"granted": granted, "permissionRequired": true, "command": command, "operation": operation, "oneTime": true}).to_string())
            }
            "run_shell" => self.run_shell_tool(&object, model, cancel),
            "shell_job" => self.shell_job_tool(&object, cancel),
            "computer_use" => self.computer_use_tool(&object, cancel),
            "computer_use_reset" => self.computer_use_reset_tool(&object, cancel),
            "apply_file_edits" => self.apply_file_edits(&call.arguments),
            other => {
                if let Some(target) = self.foundation_tool_map.get(other).cloned() {
                    if !self.foundation_enabled {
                        bail!("Project memory is disabled");
                    }
                    let result = self.call_foundation_target(&target, &object, cancel)?;
                    return Ok(foundation_tool_result_text(&result));
                }
                if let Some(skill) = self.skill_script_tool_map.get(other).cloned() {
                    if self.capability_disabled("skill", &skill) {
                        bail!("Skill {skill} is disabled for this session");
                    }
                    if self.disabled_capabilities.contains("builtin:shell") {
                        bail!("Shell is disabled for this session; Skill scripts cannot execute");
                    }
                    return self.run_skill_script(&skill, &object, model, cancel);
                }
                if let Some(skill) = self.skill_tool_map.get(other) {
                    if self.capability_disabled("skill", skill) {
                        bail!("Skill {skill} is disabled for this session");
                    }
                    return self.bridge_client()?.read_skill_file(
                        skill,
                        object.get("path").and_then(Value::as_str).unwrap_or(""),
                    );
                }
                if let Some((server, tool)) = self.mcp_tool_map.get(other).cloned() {
                    if self.capability_disabled("mcp", &server) {
                        bail!("MCP server {server} is disabled for this session");
                    }
                    let read_only = self.read_only_mcp_tools.contains(other);
                    let result = self
                        .bridge_client()?
                        .call_mcp_tool_cancellable(&server, &tool, &object, cancel)?
                        .to_string();
                    if !read_only {
                        // MCP tools without an explicit readOnlyHint may have
                        // changed files behind the structured edit backend.
                        // Drop cached source/search evidence before the next
                        // tool call so a follow-up read cannot be suppressed
                        // as a stale duplicate.
                        self.workspace_write_generation =
                            self.workspace_write_generation.wrapping_add(1);
                        self.invalidate_workspace_cache();
                    }
                    return Ok(result);
                }
                if let Some((worker, tool)) = self.worker_tool_map.get(other).cloned() {
                    let result =
                        self.workers
                            .execute(&worker, &tool, &object, &self.workspace_root)?;
                    self.workspace_write_generation =
                        self.workspace_write_generation.wrapping_add(1);
                    self.invalidate_workspace_cache();
                    return Ok(result);
                }
                bail!("Unknown direct tool: {other}")
            }
        }
    }

    pub fn activate_capability(&mut self, id: &str) -> Result<String> {
        self.activate(id)
    }

    pub fn begin_task(&mut self, task_id: &str) {
        self.active_task_id = Some(task_id.to_owned());
    }

    pub fn finish_task(&mut self, task_id: &str) {
        if self.active_task_id.as_deref() == Some(task_id) {
            self.active_task_id = None;
        }
        self.reset_model_evidence_window();
        self.edit_snapshots.clear();
        self.edit_read_coverage.clear();
        self.web_sources.clear();
        self.permitted_shell_commands.clear();
        self.auxiliary_usage = None;
        self.latest_mutation = None;
        self.workers.finish_task(task_id);
    }

    /// Starts a fresh model-visible evidence window without discarding
    /// task-wide edit snapshots, permissions, artifacts, mutations, or workers.
    /// Context rollover removes prior tool results from the request, so duplicate
    /// suppression must forget which inspection bytes were previously shown.
    pub(super) fn reset_model_evidence_window(&mut self) {
        self.read_cache.clear();
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.web_searches.clear();
        self.web_reads.clear();
    }

    /// Direct MCP calls do not reveal client-context lifetime. Clear only
    /// duplicate-suppression state between calls while preserving state required
    /// by a subsequent dependent call (fresh edit snapshots and web source grants).
    pub(super) fn reset_direct_mcp_visibility(&mut self) {
        self.read_cache.clear();
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.web_searches.clear();
        self.web_reads.clear();
    }

    pub fn consume_auxiliary_usage(&mut self) -> Option<Usage> {
        self.auxiliary_usage.take()
    }

    pub fn latest_write_validation_passed(&self) -> Option<bool> {
        self.latest_mutation
            .as_ref()
            .map(MutationValidation::write_validation_passed)
    }

    pub fn workspace_generation(&self) -> u64 {
        self.workspace_generation
    }

    pub fn workspace_write_generation(&self) -> u64 {
        self.workspace_write_generation
    }

    pub fn has_shell_jobs(&self) -> bool {
        self.shell_jobs.has_jobs()
    }

    pub fn working_state_summary(&self) -> Option<String> {
        if self.read_cache.is_empty()
            && self.listings.is_empty()
            && self.searches.is_empty()
            && self.shell_inspections.is_empty()
            && self.web_searches.is_empty()
            && self.web_reads.is_empty()
            && self.latest_mutation.is_none()
        {
            return None;
        }
        const MAX_SOURCE_PATHS: usize = 24;
        const MAX_STATE_KEYS: usize = 12;
        let mut lines = Vec::new();
        if !self.read_cache.is_empty() {
            lines.push("Source coverage:".to_owned());
            let mut paths: Vec<_> = self.read_cache.keys().cloned().collect();
            paths.sort();
            let omitted = paths.len().saturating_sub(MAX_SOURCE_PATHS);
            for path in paths.into_iter().take(MAX_SOURCE_PATHS) {
                let entries = &self.read_cache[&path];
                if let Some(first) = entries.first() {
                    let ranges = merged_ranges(
                        entries
                            .iter()
                            .map(|entry| (entry.start, entry.end))
                            .collect(),
                    );
                    lines.push(format!(
                        "{} total={} covered={}",
                        truncate_state_value(&path, 180),
                        first.total,
                        ranges
                            .iter()
                            .map(|(a, b)| if a == b {
                                a.to_string()
                            } else {
                                format!("{a}-{b}")
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
            if omitted > 0 {
                lines.push(format!("... {omitted} more source paths omitted"));
            }
        }
        append_bounded_state_set(
            &mut lines,
            "Directory listings",
            &self.listings,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(&mut lines, "Searches", &self.searches, MAX_STATE_KEYS);
        append_bounded_state_set(
            &mut lines,
            "Shell inspections",
            &self.shell_inspections,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(
            &mut lines,
            "Web searches",
            &self.web_searches,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(
            &mut lines,
            "Web source reads",
            &self.web_reads,
            MAX_STATE_KEYS,
        );
        if let Some(mutation) = &self.latest_mutation {
            lines.push(format!(
                "Latest workspace mutation: writeValidation={} files={}",
                if mutation.write_validation_passed() {
                    "passed"
                } else {
                    "failed"
                },
                mutation
                    .changed_paths
                    .iter()
                    .take(MAX_SOURCE_PATHS)
                    .map(|path| truncate_state_value(path, 180))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        Some(lines.join("\n"))
    }

    pub fn shutdown(&self) {
        self.web_search.shutdown();
        self.workers.shutdown();
    }

    fn tool_enabled(&self, tool_name: &str) -> bool {
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
            return self.foundation_memory_active();
        }
        true
    }

    fn capability_disabled(&self, kind: &str, name: &str) -> bool {
        self.disabled_capabilities
            .contains(&format!("{kind}:{name}"))
    }

    fn covered_shell_replay_reason(&self, command: &str) -> Option<String> {
        let lower = command.to_ascii_lowercase();
        let source_inspection = ["cat ", "head ", "tail ", "sed ", "awk ", "grep ", "rg "]
            .iter()
            .any(|needle| lower.contains(needle));
        if source_inspection {
            for path in self.read_cache.keys() {
                if shell_mentions_path(command, path) {
                    return Some(format!(
                        "Source for {path} is already covered by structured reads. Reuse the prior read result or request only an uncovered range with read_file."
                    ));
                }
            }
        }
        let directory_inspection = lower.contains("ls ")
            || lower.starts_with("ls")
            || lower.contains("find ")
            || lower.contains("tree ");
        if directory_inspection {
            for listing in &self.listings {
                let path = listing.split(':').next().unwrap_or(".");
                if path == "." || shell_mentions_path(command, path) {
                    return Some(format!(
                        "Directory {path} was already covered by list_files. Reuse that listing or list a different subtree with list_files instead of replaying it through the shell."
                    ));
                }
            }
        }
        None
    }

    fn request_approval(
        &self,
        kind: &str,
        target: &str,
        operation: &str,
        reason: &str,
    ) -> Result<bool> {
        let policy = SandboxStore::new(&self.workspace_root)?.load()?;
        if policy.mode == SandboxMode::Unlimited || policy.auto_approve {
            return Ok(true);
        }
        let reason = match &self.permission_label {
            Some(label) => format!("[{label}] {reason}"),
            None => reason.into(),
        };
        Ok(self
            .permission
            .request(kind.into(), target.into(), operation.into(), reason))
    }

    fn ensure_file_scope(&self, path: &str, write: bool) -> Result<()> {
        if self.path_outside_hard_access_root(path)? {
            let operation = if write { "file write" } else { "file read" };
            let root = self
                .hard_access_root
                .as_ref()
                .expect("hard access root exists when path is outside it");
            bail!(
                "MCP {operation} is restricted to {} and its descendants: {path}",
                root.display()
            );
        }
        let resolved = self.resolve_session_path(path)?;
        if self.path_in_context_roots(&resolved) {
            return Ok(());
        }
        let operation = if write {
            "outside-context file write"
        } else {
            "outside-context file read"
        };
        let reason = if write {
            "The model requested a file change outside the active session context roots."
        } else {
            "The model requested file access outside the active session context roots."
        };
        if self.request_approval("file", path, operation, reason)? {
            return Ok(());
        }
        bail!("User denied {operation}: {path}")
    }
}
