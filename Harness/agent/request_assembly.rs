//! Per-attempt tool envelope selection and loop policy before request assembly.

use std::sync::atomic::AtomicBool;

use crate::core::{BridgeClient, CallRequest, ToolDefinition};
use crate::tools::ToolRegistry;

use super::{
    AgentCoordinator, LoopBudget, Message, MessageRole, Result, cache, context,
    coordinator_support::{capability_guidance, configure_tool_access},
    history::append_context_updates,
    jev,
    limits::MODEL_ATTEMPT_TIMEOUT_MS,
    policy::TaskProfile,
    policy::request_history_for_profile_at,
    policy::select_tools_for_profile,
    session,
    tool_discovery::ToolDiscovery,
    turn_state,
};
use serde_json::{Value, json};

/// Read-only inputs for selecting the provider-visible tool surface of one attempt.
pub(super) struct ToolSelectionInput<'a> {
    pub bridge: &'a BridgeClient,
    pub registry: &'a ToolRegistry,
    pub context_memory: &'a context::ContextMemory,
    pub discovery: &'a ToolDiscovery,
    pub profile: TaskProfile,
    pub web_search_enabled: bool,
    pub cancel: &'a AtomicBool,
    pub deferred_discovery: bool,
}

/// The active catalog also serves lazy tool discovery during the ensuing round.
pub(super) struct ToolSelection {
    pub policy: jev::LoopPolicy,
    pub catalog: Vec<ToolDefinition>,
    pub working_budget: u64,
}

pub(super) fn select_tools(
    input: ToolSelectionInput<'_>,
    loop_input: jev::LoopInput<'_>,
) -> ToolSelection {
    let mut tool_catalog = match input.profile {
        TaskProfile::Research => input.registry.research_tools(),
        TaskProfile::Agent => select_tools_for_profile(
            input.registry.tools(input.web_search_enabled),
            input.profile,
        ),
    };
    tool_catalog.extend(context::tools());
    let selected_tools = input.discovery.attached(&tool_catalog);
    let working_budget = input.context_memory.working_budget();
    let policy = jev::apply_loop_policy(
        input.bridge,
        input.cancel,
        selected_tools,
        &tool_catalog,
        input.deferred_discovery,
        |name| input.registry.is_provider_defer_candidate(name),
        |name| input.registry.is_read_only_extension_tool(name),
        loop_input,
    );
    ToolSelection {
        policy,
        catalog: tool_catalog,
        working_budget,
    }
}

/// Inputs whose overlays and accounting are updated for a single model attempt.
pub(super) struct HistoryInput<'a> {
    pub selected_tools: &'a [ToolDefinition],
    pub turn_stable_overlays: &'a [Message],
    pub turn_context_orientation: &'a str,
    pub tool_discovery: &'a ToolDiscovery,
    pub last_context_updates: &'a mut Vec<Message>,
    pub request_input: &'a str,
    pub profile: TaskProfile,
    pub loop_budget: &'a mut LoopBudget,
}

pub(super) struct PreparedHistory {
    pub request_messages: Vec<Message>,
    pub request_context_chars: usize,
    pub rollover_budget: u64,
    pub has_rolloverable_trace: bool,
}

impl AgentCoordinator {
    pub(super) fn prepare_request_history(
        &mut self,
        input: HistoryInput<'_>,
    ) -> Result<PreparedHistory> {
        let capability_snapshot = self
            .registry
            .runtime_capability_snapshot(input.selected_tools);
        self.observation_cache.bind_window(self.context_memory.id());
        let mut stable_request_overlays = input.turn_stable_overlays.to_vec();
        stable_request_overlays
            .push(Message::system(input.turn_context_orientation.to_owned()).request_only());
        stable_request_overlays.push(
            Message::system(capability_guidance(
                capability_snapshot,
                input.tool_discovery.search_enabled(),
            ))
            .request_only(),
        );
        append_context_updates(
            &mut self.history,
            input.last_context_updates,
            &stable_request_overlays,
        );
        let mut request_messages =
            request_history_for_profile_at(&self.history, input.profile, self.history.len());
        cache::advance_turn_cache_breakpoints(&mut request_messages, input.request_input);
        let request_context_chars = serde_json::to_string(&request_messages)
            .map(|serialized| serialized.len())
            .unwrap_or_else(|_| {
                request_messages
                    .iter()
                    .filter_map(|message| message.content.as_deref())
                    .map(str::len)
                    .sum::<usize>()
            });
        self.context_memory.estimated_tokens = context::estimate_messages(&request_messages)?
            + (serde_json::to_vec(input.selected_tools)?.len() as u64).div_ceil(3);
        let rollover_budget = self.context_memory.rollover_budget();
        let has_rolloverable_trace = self
            .history
            .iter()
            .any(|message| matches!(message.role, MessageRole::Assistant | MessageRole::Tool));
        input.loop_budget.observe_request(request_context_chars);
        Ok(PreparedHistory {
            request_messages,
            request_context_chars,
            rollover_budget,
            has_rolloverable_trace,
        })
    }
}

/// Values that determine one provider request after the loop has ruled out rollover.
pub(super) struct FinalizationInput<'a> {
    pub model: &'a str,
    pub request_messages: Vec<Message>,
    pub profile: TaskProfile,
    pub selected_tools: Vec<ToolDefinition>,
    pub deferred_tools: Vec<ToolDefinition>,
    pub jev_loop_advice: Option<&'a jev::LoopAdvice>,
    pub execution_evidence: &'a turn_state::TurnExecutionEvidence,
    pub implementation_requested: bool,
    pub planning_or_documentation: bool,
    pub model_attempts: usize,
    pub tool_rounds: usize,
    pub loop_budget: &'a LoopBudget,
    pub working_budget: u64,
    pub native_deferred_tools_supported: bool,
    pub tool_discovery: &'a ToolDiscovery,
    pub turn_start_pruned_request_only_messages: usize,
    pub turn_start_pruned_request_only_chars: usize,
    pub jev_attempted_this_round: bool,
    pub jev_request_chars: usize,
    pub workspace_revision: Option<&'a str>,
    pub reasoning_level: &'a str,
    pub attached_capabilities: Option<&'a [String]>,
}

pub(super) struct PreparedRequest {
    pub request: CallRequest,
    pub diagnostics: Value,
}

impl AgentCoordinator {
    pub(super) fn finalize_request(
        &mut self,
        input: FinalizationInput<'_>,
    ) -> Result<PreparedRequest> {
        let mut request = CallRequest::simple(input.model, input.request_messages);
        request.context_key = Some(match input.profile {
            TaskProfile::Agent => self.context_key.clone(),
            TaskProfile::Research => format!("{}:research", self.context_key),
        });
        request.prompt_cache = Some(true);
        request.timeout_ms = Some(MODEL_ATTEMPT_TIMEOUT_MS);
        request.deferred_tools = (!input.deferred_tools.is_empty()).then_some(input.deferred_tools);
        configure_tool_access(&mut request, input.selected_tools, false);
        if jev::forces_tool_free(input.jev_loop_advice) {
            request.tool_choice = Some(json!("none"));
        }
        let execution_phase = input.execution_evidence.recommended_phase(
            input.implementation_requested,
            input.planning_or_documentation,
        );
        let mut request_metadata = turn_state::request_metadata(turn_state::RequestMetadataInput {
            profile: input.profile,
            context_key: request.context_key.as_deref(),
            context_memory: &self.context_memory,
            model_attempts: input.model_attempts,
            tool_rounds: input.tool_rounds,
            loop_budget: input.loop_budget,
            working_budget: input.working_budget,
            deferred_tool_count: request.deferred_tools.as_ref().map(Vec::len).unwrap_or(0),
            native_deferred_tools_supported: input.native_deferred_tools_supported,
            search_loaded_tool_count: input.tool_discovery.loaded_count(),
            execution_phase: execution_phase.as_str(),
            successful_mutations: input.execution_evidence.successful_mutations(),
            unresolved_failed_mutation: input.execution_evidence.unresolved_failed_mutation(),
            verification_attempted: input.execution_evidence.verification_attempted(),
            verification_succeeded: input.execution_evidence.verification_succeeded(),
            turn_start_pruned_request_only_messages: input.turn_start_pruned_request_only_messages,
            turn_start_pruned_request_only_chars: input.turn_start_pruned_request_only_chars,
        });
        jev::write_loop_metadata(
            &mut request_metadata,
            input.jev_loop_advice,
            input.jev_attempted_this_round,
            input.jev_request_chars,
        );
        if let Some(revision) = input.workspace_revision {
            request_metadata.insert("workspaceRevision".into(), revision.to_owned());
        }
        if input.reasoning_level != "auto" {
            request_metadata.insert("reasoningLevel".into(), input.reasoning_level.to_owned());
        }
        request.metadata = Some(request_metadata);
        request.attached_capabilities = input
            .attached_capabilities
            .map(session::runtime_attached_capabilities);
        let mut attempt_cache_diagnostics = self.cache_continuity.diagnostics(&request);
        let update_plans = self.observation_cache.take_plans();
        if !update_plans.is_empty() {
            attempt_cache_diagnostics["contextCacheUpdates"] = json!(update_plans);
        }
        if attempt_cache_diagnostics["wireHistoryPrefixRewriteDetected"] == json!(true) {
            anyhow::bail!(
                "Previously submitted context changed within this window; start an explicit context rollover instead of rewriting history."
            );
        }
        Ok(PreparedRequest {
            request,
            diagnostics: attempt_cache_diagnostics,
        })
    }
}
