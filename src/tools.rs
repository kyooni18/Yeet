use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    core::{BridgeClient, CallRequest, McpServerStatus, Message, ToolCall, ToolDefinition, Usage},
    edit::{ApplyResult, EditClient},
    general,
    permission::PermissionBroker,
    sandbox::{SandboxMode, SandboxStore},
    session_store::SessionStore,
    shell::{
        ShellExecutionRequest, restricted_operation, run_shell_cancellable_with_protected_paths,
    },
    web_search::{WebSearchBackend, WebSearchClient, WebSearchRequest},
    workers::WorkerRegistry,
};

mod artifact_output;
mod definitions;
mod editing;
mod io;
mod paths;
mod shell_jobs;
mod shell_runtime;
mod support;

#[cfg(test)]
use crate::edit::ReadResult;
use definitions::{base_tool_definitions, web_read_tool_definition, web_search_tool_definition};
pub use definitions::{is_coding_builtin_tool, is_general_builtin_tool};
pub(crate) use paths::workspace_revision_for_path;
use paths::{canonicalize_existing_ancestor, path_outside_workspace};
use shell_runtime::shell_mentions_path;
#[cfg(test)]
use shell_runtime::{
    bounded_shell_evaluation_input, shell_is_inspection, shell_output_needs_model_evaluation,
};
#[cfg(test)]
use std::fs;
#[cfg(test)]
use support::foundation_wrapper_schema;
use support::{
    ArtifactStore, ReadCacheEntry, allocate_tool_name, append_bounded_state_set,
    builtin_capability_for_tool, cache_read_result, collect_web_source_urls, coverage_complete,
    covered_ranges_within, foundation_context_from_tool_result, foundation_tool_result_text,
    merged_ranges, next_uncovered, shell_quote, string_arg, truncate_state_value, uncovered_ranges,
    usize_arg, web_search_queries,
};

const DEFAULT_READ_LINES: usize = 160;
const EXPLICIT_READ_LINES: usize = 480;
const DEFAULT_INLINE_BYTES: usize = 12 * 1024;
const EXPLICIT_INLINE_BYTES: usize = 64 * 1024;
const FOUNDATION_RECALL_TOOL: &str = "project_memory_recall";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: String,
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpServerIdentity {
    transport: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    url: Option<String>,
    headers: BTreeMap<String, String>,
}

impl From<&McpServerStatus> for McpServerIdentity {
    fn from(server: &McpServerStatus) -> Self {
        Self {
            transport: server.transport.clone(),
            command: server.command.clone(),
            args: server.args.clone(),
            env: server.env.clone().unwrap_or_default().into_iter().collect(),
            cwd: server.cwd.clone(),
            url: server.url.clone(),
            headers: server
                .headers
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinCapabilityDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    tools: &'static [&'static str],
}

const BUILTIN_CAPABILITIES: &[BuiltinCapabilityDescriptor] = &[
    BuiltinCapabilityDescriptor {
        id: "builtin:file-read",
        name: "File Read",
        description: "Read one or several UTF-8 files with read_file. Outside-project paths trigger project approval unless unlimited or auto-approval mode is enabled. Reads return snapshot-safe line anchors for later edits.",
        // read_files is a hidden legacy alias so old restored calls inherit
        // the same disable/approval policy without re-exposing a second schema.
        tools: &["read_file", "read_files"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:file-write",
        name: "File Write",
        description: "Create, modify, move, or delete files with snapshot-safe apply_file_edits. Outside-project paths trigger project approval unless unlimited or auto-approval mode is enabled. Existing files require a snapshot from File Read.",
        tools: &["apply_file_edits"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:file-list",
        name: "File Listing",
        description: "List workspace files and directories with list_files. Use it for structure discovery without reading file contents.",
        tools: &["list_files"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:workspace-search",
        name: "Workspace Search",
        description: "Search workspace source with search_workspace. Matching is literal by default; narrow the path and use regex only when needed.",
        tools: &["search_workspace"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:shell",
        name: "Shell",
        description: "Run commands with run_shell. In sandboxed mode, mutating or outside-project execution asks for approval; unlimited mode runs without sandbox restrictions. Builds/tests/noisy commands can use actor mode. Use run_shell background=true to detach and shell_job to check, list, stop, or forget jobs.",
        tools: &["run_shell", "shell_job", "request_shell_permission"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:artifacts",
        name: "Artifacts",
        description: "Inspect large tool results externalized as artifacts with read_artifact/search_artifact instead of replaying the original expensive command or read.",
        tools: &["artifact_info", "read_artifact", "search_artifact"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:document-read",
        name: "Document Read",
        description: "Extract readable content from documents without treating them as source code. Supports PDF, DOCX, spreadsheets, CSV/TSV, JSON, Markdown, and UTF-8 text, with large content stored as typed artifacts.",
        tools: &["read_document"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:data-analysis",
        name: "Data Analysis",
        description: "Analyze structured local data directly. Supports CSV, TSV, JSON arrays of objects, Excel workbooks, and ODS with summaries, value counts, grouped aggregates, and Pearson correlation.",
        tools: &["analyze_data"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:sessions",
        name: "Session Management",
        description: "List Yeet sessions structurally and export a verified session archive without shell-based session discovery or deleting the active runtime state.",
        tools: &["list_sessions", "export_session"],
    },
];

pub fn builtin_capabilities() -> &'static [BuiltinCapabilityDescriptor] {
    BUILTIN_CAPABILITIES
}

#[derive(Debug, Clone)]
struct MutationValidation {
    error_count: usize,
}

impl MutationValidation {
    pub fn write_validation_passed(&self) -> bool {
        self.error_count == 0
    }
}

pub struct ToolRegistry {
    bridge: BridgeClient,
    edit: EditClient,
    workspace_root: PathBuf,
    workers: WorkerRegistry,
    permission: PermissionBroker,
    artifacts: ArtifactStore,
    descriptors: Vec<CapabilityDescriptor>,
    active_tools: BTreeMap<String, ToolDefinition>,
    skill_tool_map: HashMap<String, String>,
    skill_script_tool_map: HashMap<String, String>,
    mcp_tool_map: HashMap<String, (String, String)>,
    foundation_tool_map: HashMap<String, String>,
    worker_tool_map: HashMap<String, (String, String)>,
    active_skills: HashSet<String>,
    active_mcp: HashSet<String>,
    active_mcp_identity: HashMap<String, McpServerIdentity>,
    foundation_enabled: bool,
    foundation_server: Option<String>,
    foundation_project: Option<String>,
    foundation_memory_store: crate::memory::MemoryStore,
    active_workers: HashSet<String>,
    disabled_capabilities: HashSet<String>,
    read_cache: HashMap<String, Vec<ReadCacheEntry>>,
    searches: HashSet<String>,
    listings: HashSet<String>,
    shell_jobs: shell_jobs::ShellJobs,
    shell_inspections: HashSet<String>,
    permitted_shell_commands: HashSet<String>,
    auxiliary_usage: Option<Usage>,
    latest_mutation: Option<MutationValidation>,
    web_search: WebSearchClient,
    web_searches: HashSet<String>,
    web_sources: HashSet<String>,
    web_reads: HashSet<String>,
    read_only_mcp_tools: HashSet<String>,
    workspace_generation: u64,
    workspace_write_generation: u64,
    protected_write_paths: Vec<PathBuf>,
    session_store: Option<SessionStore>,
    active_session_id: Option<String>,
}

impl ToolRegistry {
    pub fn new(
        bridge: BridgeClient,
        workspace_root: PathBuf,
        workers: WorkerRegistry,
        permission: PermissionBroker,
    ) -> Result<Self> {
        let edit = EditClient::start(&workspace_root)?;
        let mut active_tools = BTreeMap::new();
        for tool in base_tool_definitions() {
            active_tools.insert(tool.name.clone(), tool);
        }
        Ok(Self {
            bridge,
            edit,
            workspace_root,
            workers,
            permission,
            artifacts: ArtifactStore::new()?,
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
            foundation_server: Some("foundation".into()),
            foundation_project: None,
            foundation_memory_store: crate::memory::MemoryStore::default(),
            active_workers: HashSet::new(),
            disabled_capabilities: HashSet::new(),
            read_cache: HashMap::new(),
            searches: HashSet::new(),
            listings: HashSet::new(),
            shell_jobs: shell_jobs::ShellJobs::default(),
            shell_inspections: HashSet::new(),
            permitted_shell_commands: HashSet::new(),
            auxiliary_usage: None,
            latest_mutation: None,
            web_search: WebSearchClient::default(),
            web_searches: HashSet::new(),
            web_sources: HashSet::new(),
            web_reads: HashSet::new(),
            read_only_mcp_tools: HashSet::new(),
            workspace_generation: 0,
            workspace_write_generation: 0,
            protected_write_paths: Vec::new(),
            session_store: None,
            active_session_id: None,
        })
    }

    pub fn set_protected_write_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.protected_write_paths = paths.into_iter().collect();
    }

    pub fn set_session_runtime(&mut self, store: SessionStore, active_session_id: Option<String>) {
        self.artifacts.root = active_session_id
            .as_ref()
            .map(|id| store.directory.join(id).join("artifacts"))
            .unwrap_or_else(|| self.artifacts._directory.path().to_path_buf());
        self.session_store = Some(store);
        self.active_session_id = active_session_id;
    }

    pub fn configure_foundation_memory(
        &mut self,
        enabled: bool,
        server: impl Into<String>,
        project: impl Into<String>,
    ) {
        let server = server.into();
        let project = project.into();
        let changed = self.foundation_server.as_deref() != Some(server.as_str())
            || self.foundation_project.as_deref() != Some(project.as_str())
            || self.foundation_enabled != enabled;
        if changed {
            self.deactivate_foundation();
        }
        self.foundation_enabled = enabled;
        self.foundation_server = Some(server);
        self.foundation_project = Some(project);
        if enabled {
            self.activate_foundation();
        }
    }

    pub fn foundation_memory_active(&self) -> bool {
        self.foundation_enabled
            && self.foundation_server.is_some()
            && self.foundation_project.is_some()
            && self
                .foundation_tool_map
                .contains_key(FOUNDATION_RECALL_TOOL)
    }

    pub fn foundation_memory_guidance(&self) -> Option<&'static str> {
        self.foundation_memory_active().then_some(
            "Yeet project memory is available alongside session-local task_notes and context_history. Keep task progress and exact evidence in those local stores; promote only durable project knowledge to Yeet project memory. Relevant memory is recalled automatically at task start. Use project_memory_remember only for durable project knowledge that will help later sessions. Give mutable current-state facts a stable key so newer values supersede older ones while preserving history. Do not store secrets, credentials, raw transcripts, transient progress chatter, build output, or facts that are cheap to rediscover from the repository. Fresh source evidence overrides stale memory.",
        )
    }

    pub fn recall_foundation_memory(
        &self,
        query: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<String>> {
        if !self.foundation_memory_active() || query.trim().is_empty() {
            return Ok(None);
        }
        let project = self
            .foundation_project
            .as_deref()
            .ok_or_else(|| anyhow!("Project identity unavailable"))?;
        let query = query.chars().take(20_000).collect::<String>();
        let value = self.foundation_memory_store.call(
            "memory_recall",
            project,
            &json!({"query":query,"limit":8}),
            &self.bridge,
            cancel,
        )?;
        Ok(foundation_context_from_tool_result(&value))
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
            tools.push(web_search_tool_definition());
            tools.push(web_read_tool_definition());
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
        ) || self.skill_tool_map.contains_key(name)
            || self.read_only_mcp_tools.contains(name)
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
        let servers = self.bridge.list_mcp_servers()?;
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
            // Memory is native now. Retain old MCP configuration on disk, but
            // do not connect it or expose duplicate memory tools.
            if self.foundation_server.as_deref() == Some(server.name.as_str()) {
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
        if let Ok(skills) = self.bridge.list_skills() {
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
        if let Ok(servers) = self.bridge.list_mcp_servers() {
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
        self.descriptors = descriptors;
    }

    pub fn execute(&mut self, call: &ToolCall, model: &str, cancel: &AtomicBool) -> Result<String> {
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
                let values: Vec<_> = self
                    .descriptors
                    .iter()
                    .filter(|descriptor| {
                        query.is_empty()
                            || descriptor.id.to_ascii_lowercase().contains(&query)
                            || descriptor.description.to_ascii_lowercase().contains(&query)
                    })
                    .take(8)
                    .map(|descriptor| {
                        json!({
                            "id": descriptor.id,
                            "kind": descriptor.kind,
                            "description": truncate_state_value(&descriptor.description, 320)
                        })
                    })
                    .collect();
                Ok(serde_json::to_string(&values)?)
            }
            "activate_capability" => {
                let id = string_arg(&object, "capability")?;
                self.activate(id)
            }
            "read_file" => self.read_file(&object),
            // Hidden compatibility alias for restored sessions created before
            // read_file absorbed batch reads. New requests never expose this schema.
            "read_files" => self.read_files(&object),
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
            "shell_job" => self.shell_job_tool(&object),
            "apply_file_edits" => self.apply_file_edits(&call.arguments),
            other => {
                if let Some(tool) = self.foundation_tool_map.get(other).cloned() {
                    if !self.foundation_enabled {
                        bail!("Project memory is disabled");
                    }
                    let project = self
                        .foundation_project
                        .as_deref()
                        .ok_or_else(|| anyhow!("Project identity is unavailable"))?;
                    let result = self.foundation_memory_store.call(
                        &tool,
                        project,
                        &Value::Object(object.clone()),
                        &self.bridge,
                        cancel,
                    )?;
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
                    return self.bridge.read_skill_file(
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
                        .bridge
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
                if let Some((worker, tool)) = self.worker_tool_map.get(other) {
                    let result =
                        self.workers
                            .execute(worker, tool, &object, &self.workspace_root)?;
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

    pub fn activate_explicit_skill(&mut self, name: &str) -> Result<String> {
        let id = format!("skill:{name}");
        if self.disabled_capabilities.contains(&id) {
            bail!("Capability {id} is disabled for this session");
        }
        self.activate_skill(name, true)
    }

    pub fn finish_task(&mut self, task_id: &str) {
        self.read_cache.clear();
        self.searches.clear();
        self.web_searches.clear();
        self.web_sources.clear();
        self.web_reads.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.permitted_shell_commands.clear();
        self.auxiliary_usage = None;
        self.latest_mutation = None;
        self.workers.finish_task(task_id);
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

    pub fn is_read_only_extension_tool(&self, name: &str) -> bool {
        self.skill_tool_map.contains_key(name)
            || self.read_only_mcp_tools.contains(name)
            || name == FOUNDATION_RECALL_TOOL
    }

    pub fn working_state_summary(&self) -> Option<String> {
        if self.read_cache.is_empty()
            && self.listings.is_empty()
            && self.searches.is_empty()
            && self.web_searches.is_empty()
            && self.web_reads.is_empty()
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
        Some(lines.join("\n"))
    }

    pub fn shutdown(&self) {
        self.web_search.shutdown();
        self.workers.shutdown();
    }

    fn activate(&mut self, id: &str) -> Result<String> {
        if self.disabled_capabilities.contains(id) {
            bail!("Capability {id} is disabled for this session");
        }
        if self.descriptors.is_empty() {
            self.refresh_capabilities();
        }
        if let Some(name) = id.strip_prefix("skill:") {
            return self.activate_skill(name, false);
        }
        if let Some(server) = id.strip_prefix("mcp:") {
            if self.foundation_server.as_deref() == Some(server) {
                if !self.foundation_enabled {
                    bail!("Foundation memory is disabled in this project's settings");
                }
                let already_active = self.foundation_memory_active();
                if !already_active {
                    self.activate_foundation();
                }
                let mut names = self.foundation_tool_map.keys().cloned().collect::<Vec<_>>();
                names.sort();
                return Ok(json!({
                    "activated": id,
                    "alreadyActive": already_active,
                    "projectScoped": true,
                    "tools": names
                })
                .to_string());
            }
            if self.active_mcp.contains(server) {
                return Ok(json!({"activated":id,"alreadyActive":true}).to_string());
            }
            let tools = self.bridge.list_mcp_tools(Some(server))?;
            let mut names = Vec::new();
            for (index, tool) in tools.into_iter().enumerate() {
                let safe = allocate_tool_name(
                    "mcp",
                    &[server, &index.to_string(), &tool.name],
                    self.active_tools.keys(),
                );
                let definition = ToolDefinition {
                    name: safe.clone(),
                    description: tool.description,
                    input_schema: tool.input_schema,
                };
                if tool
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("readOnlyHint"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    self.read_only_mcp_tools.insert(safe.clone());
                } else {
                    self.read_only_mcp_tools.remove(&safe);
                }
                self.mcp_tool_map
                    .insert(safe.clone(), (server.to_owned(), tool.name));
                self.active_tools.insert(safe.clone(), definition);
                names.push(safe);
            }
            self.active_mcp.insert(server.to_owned());
            return Ok(json!({"activated":id,"tools":names}).to_string());
        }
        if let Some(worker) = id.strip_prefix("worker:") {
            if self.active_workers.contains(worker) {
                return Ok(json!({"activated":id,"alreadyActive":true}).to_string());
            }
            let definitions = self.workers.activate(worker)?;
            let mut names = Vec::new();
            for definition in definitions {
                let safe = allocate_tool_name(
                    "worker",
                    &[worker, &definition.name],
                    self.active_tools.keys(),
                );
                self.worker_tool_map
                    .insert(safe.clone(), (worker.to_owned(), definition.name.clone()));
                self.active_tools.insert(
                    safe.clone(),
                    ToolDefinition {
                        name: safe.clone(),
                        ..definition
                    },
                );
                names.push(safe);
            }
            self.active_workers.insert(worker.to_owned());
            return Ok(json!({"activated":id,"tools":names}).to_string());
        }
        bail!("Unknown capability: {id}")
    }

    fn activate_foundation(&mut self) {
        for tool in crate::memory::tool_definitions() {
            let operation = tool.name.strip_prefix("project_").unwrap().to_owned();
            self.foundation_tool_map
                .insert(tool.name.clone(), operation);
            self.active_tools.insert(tool.name.clone(), tool);
        }
    }

    fn activate_skill(&mut self, name: &str, explicit: bool) -> Result<String> {
        let id = format!("skill:{name}");
        if self.active_skills.contains(name) {
            return Ok(json!({"activated":id,"alreadyActive":true}).to_string());
        }
        let skill = self.bridge.load_skill(name)?;
        if !explicit && skill.allow_implicit_invocation == Some(false) {
            bail!("Skill {name} requires explicit user invocation with ${name}");
        }
        let tool_name = allocate_tool_name("skill", &[name, "read_file"], self.active_tools.keys());
        let definition = ToolDefinition::new(
            &tool_name,
            format!("Read a supporting file from Skill {name}."),
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        );
        self.active_tools.insert(tool_name.clone(), definition);
        self.skill_tool_map
            .insert(tool_name.clone(), name.to_owned());
        let script_tool_name = if skill.files.iter().any(|path| path.starts_with("scripts/")) {
            let script_tool_name =
                allocate_tool_name("skill", &[name, "run_script"], self.active_tools.keys());
            let script_definition = ToolDefinition::new(
                &script_tool_name,
                format!(
                    "Run a helper from Skill {name}'s scripts/ under normal sandbox/approval rules."
                ),
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string","description":"Relative script path such as scripts/check.py"},
                        "args":{"type":"array","items":{"type":"string"},"maxItems":64},
                        "timeoutSeconds":{"type":"integer","minimum":1,"maximum":900}
                    },
                    "required":["path"],
                    "additionalProperties":false
                }),
            );
            self.active_tools
                .insert(script_tool_name.clone(), script_definition);
            self.skill_script_tool_map
                .insert(script_tool_name.clone(), name.to_owned());
            Some(script_tool_name)
        } else {
            None
        };
        self.active_skills.insert(name.to_owned());
        Ok(json!({
            "activated":id,
            "instructions":skill.instructions,
            "readTool":tool_name,
            "scriptTool":script_tool_name
        })
        .to_string())
    }

    fn deactivate_mcp(&mut self, server: &str) {
        let tool_names = self
            .mcp_tool_map
            .iter()
            .filter(|(_, (mapped_server, _))| mapped_server == server)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in tool_names {
            self.mcp_tool_map.remove(&name);
            self.active_tools.remove(&name);
            self.read_only_mcp_tools.remove(&name);
        }
        self.active_mcp.remove(server);
        self.active_mcp_identity.remove(server);
    }

    fn deactivate_foundation(&mut self) {
        let tool_names = self.foundation_tool_map.keys().cloned().collect::<Vec<_>>();
        for name in tool_names {
            self.foundation_tool_map.remove(&name);
            self.active_tools.remove(&name);
            self.read_only_mcp_tools.remove(&name);
        }
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
            return self.foundation_enabled;
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

    fn accumulate_auxiliary_usage(&mut self, usage: &Usage) {
        if let Some(existing) = &mut self.auxiliary_usage {
            existing.accumulate(usage);
        } else {
            self.auxiliary_usage = Some(usage.clone());
        }
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
        Ok(self
            .permission
            .request(kind.into(), target.into(), operation.into(), reason.into()))
    }

    fn ensure_file_scope(&self, path: &str, write: bool) -> Result<()> {
        if !path_outside_workspace(&self.workspace_root, path)? {
            return Ok(());
        }
        let operation = if write {
            "outside-project file write"
        } else {
            "outside-project file read"
        };
        let reason = if write {
            "The model requested a file change outside the current project."
        } else {
            "The model requested file access outside the current project."
        };
        if self.request_approval("file", path, operation, reason)? {
            return Ok(());
        }
        bail!("User denied {operation}: {path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_merges_adjacent_ranges() {
        assert_eq!(
            merged_ranges(vec![(10, 20), (1, 5), (6, 9), (30, 31)]),
            vec![(1, 20), (30, 31)]
        );
    }

    #[test]
    fn overlapping_reads_request_only_uncovered_ranges() {
        let entries = vec![
            ReadCacheEntry {
                path: "a".into(),
                snapshot: "s".into(),
                start: 300,
                end: 520,
                total: 1333,
                anchored: String::new(),
            },
            ReadCacheEntry {
                path: "a".into(),
                snapshot: "s".into(),
                start: 761,
                end: 1240,
                total: 1333,
                anchored: String::new(),
            },
        ];

        let covered = covered_ranges_within(&entries, 334, 580);
        assert_eq!(covered, vec![(334, 520)]);
        assert_eq!(uncovered_ranges(334, 580, &covered), vec![(521, 580)]);
    }

    #[test]
    fn cache_coverage_is_scoped_to_current_snapshot() {
        let mut cache = HashMap::new();
        let read = |snapshot: &str, start, end, total| ReadResult {
            path: "a".into(),
            snapshot: snapshot.into(),
            start_line: start,
            end_line: end,
            total_lines: total,
            content: String::new(),
            numbered: String::new(),
            anchored: String::new(),
        };
        cache_read_result(&mut cache, read("old", 1, 10, 20));
        cache_read_result(&mut cache, read("old", 11, 20, 20));
        assert!(coverage_complete(20, &cache["a"]));
        cache_read_result(&mut cache, read("new", 1, 10, 30));
        assert_eq!(cache["a"].len(), 1);
        assert_eq!(next_uncovered(30, &cache["a"]), Some(11));
    }

    #[test]
    fn union_coverage_suppresses_reads_not_covered_by_one_cache_entry() {
        let entries = vec![
            ReadCacheEntry {
                path: "a".into(),
                snapshot: "s".into(),
                start: 1,
                end: 160,
                total: 300,
                anchored: String::new(),
            },
            ReadCacheEntry {
                path: "a".into(),
                snapshot: "s".into(),
                start: 161,
                end: 300,
                total: 300,
                anchored: String::new(),
            },
        ];

        let covered = covered_ranges_within(&entries, 80, 240);
        assert_eq!(covered, vec![(80, 240)]);
        assert!(uncovered_ranges(80, 240, &covered).is_empty());
        assert!(coverage_complete(300, &entries));
    }

    #[test]
    fn tool_names_are_ascii_and_collision_safe() {
        let existing = ["mcp_server_0_read".to_owned()];
        assert_eq!(
            allocate_tool_name("mcp", &["Server", "0", "Read"], existing.iter()),
            "mcp_server_0_read_2"
        );
    }

    #[test]
    fn foundation_wrapper_hides_project_scope_from_model_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "project": {"type": "string"},
                "key": {"type": "string"}
            },
            "required": ["text", "project"]
        })
        .as_object()
        .unwrap()
        .clone();

        let wrapped = foundation_wrapper_schema(&schema);
        assert!(wrapped["properties"].get("project").is_none());
        assert_eq!(wrapped["required"], json!(["text"]));
        assert!(wrapped["properties"].get("key").is_some());
    }

    #[test]
    fn foundation_recall_extracts_only_nonempty_context() {
        let result = json!({
            "content": [{
                "type": "text",
                "text": "{\"context\":\"provider.active = openai\",\"atomCount\":1}"
            }]
        });
        assert_eq!(
            foundation_context_from_tool_result(&result).as_deref(),
            Some("provider.active = openai")
        );
        assert!(
            foundation_context_from_tool_result(&json!({
                "isError": true,
                "content": [{"type":"text","text":"failed"}]
            }))
            .is_none()
        );
    }

    #[test]
    fn project_scope_detection_handles_absolute_and_parent_paths() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        assert!(!path_outside_workspace(&root, "src/new.rs").unwrap());
        assert!(!path_outside_workspace(&root, ".").unwrap());
        assert!(path_outside_workspace(&root, "../outside.txt").unwrap());
        assert!(path_outside_workspace(&root, "/tmp/yeet-outside.txt").unwrap());
    }

    #[test]
    fn workspace_revision_changes_for_uncommitted_worktree_edits() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        assert!(
            std::process::Command::new("git")
                .arg("init")
                .arg(root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("controller.rs"), "const GAIN: f64 = 1.0;\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["add", "."])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args([
                    "-c",
                    "user.name=Yeet Test",
                    "-c",
                    "user.email=yeet@example.invalid",
                    "commit",
                    "-m",
                    "base"
                ])
                .status()
                .unwrap()
                .success()
        );
        let clean = workspace_revision_for_path(root).unwrap();
        fs::write(root.join("controller.rs"), "const GAIN: f64 = 2.0;\n").unwrap();
        let tracked_dirty = workspace_revision_for_path(root).unwrap();
        assert_ne!(clean, tracked_dirty);
        fs::write(root.join("new-controller.rs"), "const NEW: bool = true;\n").unwrap();
        let untracked_dirty = workspace_revision_for_path(root).unwrap();
        assert_ne!(tracked_dirty, untracked_dirty);
    }

    #[test]
    fn builtin_capabilities_cover_independent_tool_surfaces() {
        assert_eq!(
            builtin_capability_for_tool("read_file").map(|value| value.id),
            Some("builtin:file-read")
        );
        assert_eq!(
            builtin_capability_for_tool("apply_file_edits").map(|value| value.id),
            Some("builtin:file-write")
        );
        assert_eq!(
            builtin_capability_for_tool("run_shell").map(|value| value.id),
            Some("builtin:shell")
        );
        assert_eq!(
            builtin_capability_for_tool("read_document").map(|value| value.id),
            Some("builtin:document-read")
        );
        assert_eq!(
            builtin_capability_for_tool("analyze_data").map(|value| value.id),
            Some("builtin:data-analysis")
        );
        assert_eq!(
            builtin_capability_for_tool("artifact_info").map(|value| value.id),
            Some("builtin:artifacts")
        );
        assert_eq!(
            builtin_capability_for_tool("list_sessions").map(|value| value.id),
            Some("builtin:sessions")
        );
        assert_eq!(
            builtin_capability_for_tool("export_session").map(|value| value.id),
            Some("builtin:sessions")
        );
        assert!(builtin_capability_for_tool("find_capabilities").is_none());
        assert!(builtin_capability_for_tool("activate_capability").is_none());
        assert!(builtin_capability_for_tool("web_search").is_none());
    }

    #[test]
    fn shell_inspection_detection_handles_replay_commands() {
        assert!(shell_is_inspection("cat src/lib.rs"));
        assert!(shell_is_inspection("ls -R src | head -n 20"));
        assert!(shell_is_inspection("sed -i '' 's/a/b/' src/lib.rs"));
        assert!(restricted_operation("sed -i '' 's/a/b/' src/lib.rs").is_some());
        assert!(!shell_is_inspection("cargo test"));
        assert!(shell_mentions_path(
            "cat RuntimeSource/src/core.ts",
            "RuntimeSource/src/core.ts"
        ));
    }

    #[test]
    fn shell_evaluation_input_bounds_large_logs_and_keeps_diagnostics() {
        let raw = format!(
            "HEAD\n{}\nerror: important failure\n{}\nTAIL",
            "x".repeat(80_000),
            "y".repeat(80_000)
        );
        let bounded = bounded_shell_evaluation_input(&raw);

        assert!(bounded.chars().count() < 32 * 1024);
        assert!(bounded.starts_with("HEAD"));
        assert!(bounded.contains("error: important failure"));
        assert!(bounded.ends_with("TAIL"));
        assert!(bounded.contains("full log is stored as an artifact"));
    }

    #[test]
    fn clean_successful_shell_output_skips_model_evaluation() {
        let clean = crate::shell::ShellResult {
            command: "cargo test".into(),
            working_directory: ".".into(),
            exit_code: 0,
            succeeded: true,
            duration_milliseconds: 10,
            stdout: Some("48 tests passed".into()),
            stderr: None,
            stdout_bytes: 15,
            stderr_bytes: 0,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let noisy = crate::shell::ShellResult {
            stdout: Some("warning: deprecated".into()),
            ..clean.clone()
        };
        let failed = crate::shell::ShellResult {
            succeeded: false,
            exit_code: 1,
            ..clean.clone()
        };

        assert!(!shell_output_needs_model_evaluation(&clean));
        assert!(shell_output_needs_model_evaluation(&noisy));
        assert!(shell_output_needs_model_evaluation(&failed));
    }

    #[test]
    fn web_search_queries_supports_batched_unique_queries() {
        let object = json!({
            "query": "OpenAI Astra pricing",
            "queries": ["OpenAI Astra pricing", "site:openai.com Astra", "Astra pricing rumors"]
        })
        .as_object()
        .unwrap()
        .clone();

        let queries = web_search_queries(&object).unwrap();

        assert_eq!(
            queries,
            vec![
                "OpenAI Astra pricing",
                "site:openai.com Astra",
                "Astra pricing rumors",
            ]
        );
    }
}

#[cfg(test)]
mod durable_artifact_tests {
    use super::*;
    #[test]
    fn externalized_evidence_survives_registry_restart_and_is_session_scoped() {
        let session = tempfile::tempdir().unwrap();
        let mut first = ArtifactStore::new().unwrap();
        first.root = session.path().join("artifacts");
        let id = first
            .store("exact earlier compiler error\nsecond line")
            .unwrap();
        drop(first);
        let mut restored = ArtifactStore::new().unwrap();
        restored.root = session.path().join("artifacts");
        assert_eq!(
            restored.read(&id, Some(1), Some(2)).unwrap(),
            "exact earlier compiler error\nsecond line"
        );
        assert!(restored.read("../other-session/item", None, None).is_err());
        let other = tempfile::tempdir().unwrap();
        restored.root = other.path().join("artifacts");
        assert!(restored.read(&id, None, None).is_err());
    }
}
