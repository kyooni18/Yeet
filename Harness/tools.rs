use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agents::{AgentGroupHandle, group::commands::LEGACY_PROPOSE_TOOL},
    core::{BridgeClient, ToolCall, ToolDefinition, Usage},
    edit::{ApplyResult, EditClient},
    general,
    harness::sandbox::{SandboxMode, SandboxStore},
    permission::PermissionBroker,
    project_settings::ServiceBackend,
    session_store::SessionStore,
    shell::{ShellExecutionRequest, ShellProgress, restricted_operation, run_shell_cancellable},
    web_search::{WebSearchBackend, WebSearchClient, WebSearchRequest},
    workers::WorkerRegistry,
};

mod artifact_output;
mod artifact_stream;
mod auto_check;
mod bridge_handle;
mod capability_runtime;
mod catalog;
mod computer_use;
mod context;
mod definitions;
mod edit_lock;
mod editing;
mod environment;
mod evidence;
mod external;
mod io;
mod paths;
pub mod repo_map;
mod service_backends;
mod session_capabilities;
mod session_tools;
mod shell_jobs;
pub(crate) use shell_jobs::ShellJobAccess;
mod shell_runtime;
mod support;
mod symbol_tools;
pub mod symbols;
mod syntax_edit;
mod text_edit;
mod token_efficiency;

pub(crate) use bridge_handle::BridgeHandle;
use catalog::{ExternalRoute, ToolCatalog};
use context::ToolExecutionContext;
use definitions::BUILTIN_CAPABILITIES;
pub(crate) use definitions::direct_mcp_tool_definitions;
use definitions::{base_tool_definitions, web_read_tool_definition, web_search_tool_definition};
pub use definitions::{is_coding_builtin_tool, is_general_builtin_tool};
use evidence::{EditReadCoverage, MutationValidation, ToolEvidence};
use paths::canonicalize_existing_ancestor;
pub(crate) use paths::workspace_revision_for_path;
use service_backends::ToolServiceConfiguration;
pub(crate) use support::canonical_web_source_key;
use support::{
    ArtifactStore, McpServerIdentity, ReadCacheEntry, allocate_stable_tool_name,
    append_bounded_state_set, builtin_capability_for_tool, cache_read_result,
    collect_web_source_urls, coverage_complete, mcp_result_text, merged_ranges, next_uncovered,
    shell_quote, string_arg, trim_cache_to_preview, truncate_state_value, usize_arg,
    web_search_queries,
};

const DEFAULT_READ_LINES: usize = 160;
const EXPLICIT_READ_LINES: usize = 480;
const DEFAULT_INLINE_BYTES: usize = 12 * 1024;
const EXPLICIT_INLINE_BYTES: usize = 16 * 1024;
const MAX_CAPABILITY_SEARCH_RESULTS: usize = 8;
const MAX_PARALLEL_MCP_CALLS: usize = 4;
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

/// Returns apply_file_edits change objects whether the call used `changes` or `patch`.
pub fn file_edit_changes(arguments: &Value) -> Vec<Value> {
    if let Some(changes) = arguments.get("changes").and_then(Value::as_array) {
        return changes.clone();
    }
    arguments
        .get("patch")
        .and_then(Value::as_str)
        .and_then(|patch| text_edit::parse_patch(patch).ok())
        .unwrap_or_default()
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

pub struct ToolRegistry {
    bridge: BridgeHandle,
    context: ToolExecutionContext,
    edit: Option<EditClient>,
    edit_generation: u64,
    workers: WorkerRegistry,
    artifacts: ArtifactStore,
    artifacts_enabled: bool,
    catalog: ToolCatalog,
    services: ToolServiceConfiguration,
    agent_group: Option<AgentGroupHandle>,
    evidence: ToolEvidence,
    shell_jobs: shell_jobs::ShellJobs,
    /// Directory the last foreground shell command ended in; later commands
    /// without an explicit workingDirectory start there, like one live shell.
    shell_cwd: Option<PathBuf>,
    permitted_shell_commands: HashSet<String>,
    web_search: WebSearchClient,
    /// Only the main agent may interrupt the user with questions.
    user_questions_enabled: bool,
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
        Ok(Self {
            bridge,
            context: ToolExecutionContext::new(workspace_root, permission),
            edit: None,
            edit_generation: 0,
            workers,
            artifacts: ArtifactStore::new()?,
            artifacts_enabled: true,
            catalog: ToolCatalog::with_builtin_tools(),
            services: ToolServiceConfiguration::default(),
            agent_group: None,
            evidence: ToolEvidence::default(),
            shell_jobs: shell_jobs::ShellJobs::default(),
            shell_cwd: None,
            permitted_shell_commands: HashSet::new(),
            web_search: WebSearchClient::default(),
            user_questions_enabled: false,
        })
    }

    fn edit_mut(&mut self) -> Result<&mut EditClient> {
        if self.edit.is_none() {
            let edit = EditClient::start(&self.context.workspace_root)?;
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
        // A registry is reused when the local frontend changes sessions. Never
        // expose the previous session's job IDs or cwd in the new one. The
        // first None -> Some transition assigns the current unsaved session.
        if (self.context.active_session_id.is_some()
            && self.context.active_session_id != active_session_id)
            || active_session_id.is_none()
        {
            self.shell_jobs.reset();
            self.shell_cwd = None;
        }
        self.artifacts.root = active_session_id
            .as_ref()
            .map(|id| store.directory.join(id).join("artifacts"))
            .unwrap_or_else(|| self.artifacts._directory.path().to_path_buf());
        self.context.session_store = Some(store);
        self.context.active_session_id = active_session_id;
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
        if let Some(previous) = &self.agent_group {
            for name in previous.tool_names() {
                self.catalog.active_tools.remove(*name);
            }
        }
        self.agent_group = group;
        if let Some(group) = &self.agent_group {
            for definition in group.tool_definitions() {
                self.catalog
                    .active_tools
                    .insert(definition.name.clone(), definition);
            }
        }
    }

    pub(crate) fn agent_group_tool_names(&self) -> &'static [&'static str] {
        self.agent_group
            .as_ref()
            .map_or(&[], AgentGroupHandle::tool_names)
    }

    /// Lets one response's foreground agent calls run concurrently.
    pub(crate) fn prepare_tool_batch(&self, calls: &[ToolCall], model: &str) {
        if let Some(group) = &self.agent_group {
            group.prestart(calls, model, self.context.active_session_id.clone());
        }
    }

    /// Exposes `ask_user` to this registry. Called only for the main agent.
    pub(crate) fn enable_user_questions(&mut self) {
        self.user_questions_enabled = true;
        let definition = definitions::ask_user_tool_definition();
        self.catalog
            .active_tools
            .insert(definition.name.clone(), definition);
    }

    pub(crate) fn set_permission_label(&mut self, label: String) {
        self.context.permission_label = Some(label);
    }

    pub(crate) fn agent_group_guidance(&self) -> Option<String> {
        self.agent_group
            .as_ref()
            .and_then(AgentGroupHandle::group_guidance)
    }

    pub(crate) fn agent_orchestration_enabled(&self) -> bool {
        self.agent_group.is_some()
    }

    pub fn runtime_capability_snapshot(&self, visible_tools: &[ToolDefinition]) -> Value {
        let policy = SandboxStore::new(&self.context.workspace_root)
            .and_then(|store| store.load())
            .ok();
        json!({
            "visibleTools": visible_tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            "sandboxMode": policy.as_ref().map(|policy| match policy.mode {
                SandboxMode::Sandboxed => "sandboxed",
                SandboxMode::Unlimited => "unlimited",
            }),
            "autoApprove": policy.as_ref().map(|policy| policy.auto_approve),
            "activeSessionProtected": !self.context.protected_write_paths.is_empty(),
            "activeSessionId": self.context.active_session_id,
            "workspaceRoot": self.context.workspace_root,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.context.workspace_root
    }

    pub fn workspace_identity(&self) -> (String, Option<String>) {
        let root = self
            .context
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| self.context.workspace_root.clone());
        (
            root.display().to_string(),
            workspace_revision_for_path(&root),
        )
    }

    pub fn tools(&self, web_search_enabled: bool) -> Vec<ToolDefinition> {
        // Return the eligible catalog. The agent attaches schemas on demand
        // and retains loaded schemas for the rest of its turn.
        let mut tools = self.catalog.enabled_definitions();
        if web_search_enabled {
            tools.extend(self.configured_web_tool_definitions());
        }
        tools
    }

    pub fn set_disabled_capabilities(&mut self, disabled: impl IntoIterator<Item = String>) {
        self.catalog.disabled_capabilities = disabled
            .into_iter()
            .filter(|id| !crate::project_settings::is_permanent_core_capability(id))
            .collect();
        self.catalog.descriptors.clear();
    }

    pub(crate) fn restrict_execution(&mut self, deny_writes: bool, deny_shell: bool) {
        self.catalog.deny_writes = deny_writes;
        self.catalog.deny_shell = deny_shell;
    }

    /// Research uses the normal registry, with the same execution guard as its schema filter.
    pub fn research_tools(&self) -> Vec<ToolDefinition> {
        self.tools(true)
            .into_iter()
            .filter(|tool| self.catalog.research_tool_allowed(&tool.name))
            .collect()
    }

    /// Read-only MCP calls in one response run concurrently, in waves of
    /// MAX_PARALLEL_MCP_CALLS.
    pub fn can_parallel_read_only_mcp_batch(&self, calls: &[ToolCall]) -> bool {
        calls.len() >= 2
            && calls.iter().all(|call| {
                self.tool_enabled(&call.name) && self.catalog.read_only_mcp_target(call).is_some()
            })
    }

    /// Executes a batch only when every call is an MCP tool explicitly annotated read-only.
    /// The shared bridge supports concurrent request ids; mutation-capable tools never enter this path.
    pub fn execute_parallel_read_only_mcp_batch(
        &self,
        calls: &[ToolCall],
        cancel: &AtomicBool,
    ) -> Option<Vec<std::result::Result<String, String>>> {
        if !self.can_parallel_read_only_mcp_batch(calls) {
            return None;
        }
        let targets = calls
            .iter()
            .map(|call| {
                let (server, tool) = self.catalog.read_only_mcp_target(call)?;
                let arguments = call.arguments.as_object()?.clone();
                Some((server, tool, arguments))
            })
            .collect::<Option<Vec<_>>>()?;
        let bridge = self.bridge_client().ok()?;
        // At most MAX_PARALLEL_MCP_CALLS requests are in flight; larger batches
        // run in successive waves instead of falling back to fully serial.
        Some(
            targets
                .chunks(MAX_PARALLEL_MCP_CALLS)
                .flat_map(|wave| self.run_mcp_wave(&bridge, wave, cancel))
                .collect(),
        )
    }

    fn run_mcp_wave(
        &self,
        bridge: &BridgeClient,
        wave: &[(String, String, Map<String, Value>)],
        cancel: &AtomicBool,
    ) -> Vec<std::result::Result<String, String>> {
        if cancel.load(Ordering::Acquire) {
            return wave.iter().map(|_| Err("cancelled".into())).collect();
        }
        thread::scope(|scope| {
            let handles = wave
                .iter()
                .map(|(server, tool, arguments)| {
                    let bridge = bridge.clone();
                    scope.spawn(move || {
                        bridge
                            .call_mcp_tool_cancellable(server, tool, arguments, cancel)
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
        })
    }

    pub fn execute_research(
        &mut self,
        call: &ToolCall,
        model: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        if !self.catalog.research_tool_allowed(&call.name) {
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
            .catalog
            .active_mcp
            .iter()
            .filter(|name| !configured.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        for server in stale {
            self.deactivate_mcp(&server);
        }
        for server in servers {
            if self.catalog.capability_disabled("mcp", &server.name) {
                continue;
            }
            let identity = McpServerIdentity::from(&server);
            if self.catalog.active_mcp.contains(&server.name) {
                if self.catalog.active_mcp_identity.get(&server.name) == Some(&identity) {
                    continue;
                }
                self.deactivate_mcp(&server.name);
            }
            // One unavailable optional MCP must not take the entire agent down.
            // It remains discoverable and can be retried explicitly later.
            if self.activate(&format!("mcp:{}", server.name)).is_ok() {
                self.catalog
                    .active_mcp_identity
                    .insert(server.name, identity);
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
                            && !self.catalog.capability_disabled("skill", &skill.name)
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
                    .filter(|server| !self.catalog.capability_disabled("mcp", &server.name))
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
        self.catalog.descriptors = descriptors;
    }

    pub fn execute(&mut self, call: &ToolCall, model: &str, cancel: &AtomicBool) -> Result<String> {
        self.execute_live(call, model, cancel, &mut |_| {})
    }

    /// Like `execute`, but `progress` receives real partial results (currently
    /// foreground shell output) while the tool is still running.
    pub fn execute_live(
        &mut self,
        call: &ToolCall,
        model: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(String),
    ) -> Result<String> {
        self.sync_edit_state();
        if self.shell_jobs.take_workspace_refresh() {
            // Refresh evidence without treating every poll/read as agent progress.
            let generation = self.evidence.workspace_generation;
            self.invalidate_workspace_cache();
            self.evidence.workspace_generation = generation;
        }
        if matches!(call.name.as_str(), "computer_use" | "computer_use_reset") {
            crate::computer_use::availability()?;
        }
        if !self.tool_enabled(&call.name) {
            bail!("Tool {} is disabled for this session", call.name);
        }
        if self.catalog.execution_prohibited(&call.name) {
            bail!("Tool {} is prohibited by agent execution policy", call.name);
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
                let values = capability_search_values(&self.catalog.descriptors, &query);
                Ok(serde_json::to_string(&values)?)
            }
            "activate_capability" => {
                let id = string_arg(&object, "capability")?;
                self.activate(id)
            }
            _ if self.agent_group.as_ref().is_some_and(|group| {
                group.tool_names().contains(&call.name.as_str()) || call.name == LEGACY_PROPOSE_TOOL
            }) =>
            {
                self.agent_group.as_ref().unwrap().execute(
                    call,
                    &object,
                    model,
                    self.context.active_session_id.clone(),
                    cancel,
                )
            }
            "read_file" => self.read_file(&object),
            // Hidden compatibility alias for restored sessions created before
            // read_file absorbed batch reads. New requests never expose this schema.
            "read_files" => self.read_files(&object),
            // Compatibility path for historical calls. list_files is no longer
            // part of the model-visible built-in catalog; use run_shell for
            // native directory and metadata inspection instead.
            "list_files" => self.list_files(&object),
            "search_workspace" => self.search_workspace(&object),
            "outline" => self.outline_tool(&object),
            "find_symbol" => self.find_symbol_tool(&object),
            "web_search" => self.web_search(&object, cancel),
            "web_read" => self.web_read(&object, cancel),
            "read_document" => self.read_document_tool(&object),
            "analyze_data" => self.analyze_data_tool(&object),
            "artifact_info" | "read_artifact" | "search_artifact" => {
                self.execute_artifact_tool(&call.name, &object)
            }
            "request_shell_permission" => self.request_shell_permission_tool(&object),
            "ask_user" if self.user_questions_enabled => self.ask_user_tool(&object, cancel),
            "run_shell" => self.run_shell_tool_live(&object, model, cancel, progress),
            "shell_job" => self.shell_job_tool(&object, cancel),
            "computer_use" => self.computer_use_tool(&object, cancel),
            "computer_use_reset" => self.computer_use_reset_tool(&object, cancel),
            "apply_file_edits" => self.apply_file_edits(&call.arguments, cancel),
            other => match self.catalog.external_route(other) {
                Some(route) => self.execute_external(route, &object, model, cancel),
                None => bail!("Unknown direct tool: {other}"),
            },
        }
    }

    pub fn activate_capability(&mut self, id: &str) -> Result<String> {
        self.activate(id)
    }

    pub fn begin_task(&mut self, task_id: &str) {
        self.context.active_task_id = Some(task_id.to_owned());
    }

    pub fn finish_task(&mut self, task_id: &str) {
        if self.context.active_task_id.as_deref() == Some(task_id) {
            self.context.active_task_id = None;
        }
        self.evidence.finish_task();
        self.permitted_shell_commands.clear();
        self.workers.finish_task(task_id);
    }

    /// Starts a fresh model-visible evidence window without discarding
    /// task-wide edit snapshots, permissions, artifacts, mutations, or workers.
    /// Context rollover removes prior tool results from the request, so duplicate
    /// suppression must forget which inspection bytes were previously shown.
    pub(super) fn reset_model_evidence_window(&mut self) {
        self.evidence.reset_model_visible_window();
    }

    /// Direct MCP calls do not reveal client-context lifetime. Clear only
    /// duplicate-suppression state between calls while preserving state required
    /// by a subsequent dependent call (fresh edit snapshots and web source grants).
    pub(crate) fn reset_direct_mcp_visibility(&mut self) {
        self.evidence.reset_model_visible_window();
    }

    pub fn consume_auxiliary_usage(&mut self) -> Option<Usage> {
        self.evidence.auxiliary_usage.take()
    }

    pub fn latest_write_validation_passed(&self) -> Option<bool> {
        self.evidence
            .latest_mutation
            .as_ref()
            .map(MutationValidation::write_validation_passed)
    }

    pub fn workspace_generation(&self) -> u64 {
        self.evidence.workspace_generation
    }

    pub fn workspace_write_generation(&self) -> u64 {
        self.evidence.workspace_write_generation
    }

    pub fn has_shell_jobs(&self) -> bool {
        self.shell_jobs.has_jobs()
    }

    // Used by the preserved pending remote shell-job layer.
    #[allow(dead_code)]
    pub(crate) fn has_running_shell_jobs(&self) -> bool {
        self.shell_jobs.has_running_jobs()
    }

    pub(crate) fn shell_job_access(&self) -> ShellJobAccess {
        self.shell_jobs.access()
    }

    pub fn working_state_summary(&self) -> Option<String> {
        if self.evidence.read_cache.is_empty()
            && self.evidence.listings.is_empty()
            && self.evidence.searches.is_empty()
            && self.evidence.shell_inspections.is_empty()
            && self.evidence.web_searches.is_empty()
            && self.evidence.web_reads.is_empty()
            && self.evidence.latest_mutation.is_none()
        {
            return None;
        }
        const MAX_SOURCE_PATHS: usize = 24;
        const MAX_STATE_KEYS: usize = 12;
        let mut lines = Vec::new();
        if !self.evidence.read_cache.is_empty() {
            lines.push("Source coverage:".to_owned());
            let mut paths: Vec<_> = self.evidence.read_cache.keys().cloned().collect();
            paths.sort();
            let omitted = paths.len().saturating_sub(MAX_SOURCE_PATHS);
            for path in paths.into_iter().take(MAX_SOURCE_PATHS) {
                let entries = &self.evidence.read_cache[&path];
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
            &self.evidence.listings,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(
            &mut lines,
            "Searches",
            &self.evidence.searches,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(
            &mut lines,
            "Shell inspections",
            &self.evidence.shell_inspections,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(
            &mut lines,
            "Web searches",
            &self.evidence.web_searches,
            MAX_STATE_KEYS,
        );
        append_bounded_state_set(
            &mut lines,
            "Web source reads",
            &self.evidence.web_reads,
            MAX_STATE_KEYS,
        );
        if let Some(mutation) = &self.evidence.latest_mutation {
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
        self.catalog.tool_enabled(tool_name)
    }
}
