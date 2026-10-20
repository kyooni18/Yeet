//! Per-attempt tool envelope selection and request assembly.

use crate::core::{CallRequest, ToolDefinition};
use crate::tools::ToolRegistry;

use super::{
    AgentCoordinator, LoopBudget, Message, MessageRole, Result, cache, context,
    coordinator_support::{capability_guidance, configure_tool_access},
    history::append_context_updates,
    limits::MODEL_ATTEMPT_TIMEOUT_MS,
    session,
    tool_discovery::ToolDiscovery,
    turn_state,
};
use serde_json::{Value, json};

/// Read-only inputs for selecting the provider-visible tool surface of one attempt.
pub(super) struct ToolSelectionInput<'a> {
    pub registry: &'a ToolRegistry,
    pub context_memory: &'a context::ContextMemory,
    pub discovery: &'a ToolDiscovery,
    pub web_search_enabled: bool,
    pub deferred_discovery: bool,
}

/// The active catalog also serves lazy tool discovery during the ensuing round.
pub(super) struct ToolSelection {
    pub selected_tools: Vec<ToolDefinition>,
    pub deferred_tools: Vec<ToolDefinition>,
    pub deferred_names: std::collections::HashSet<String>,
    pub catalog: Vec<ToolDefinition>,
    pub working_budget: u64,
}

pub(super) fn select_tools(input: ToolSelectionInput<'_>) -> ToolSelection {
    // The task profile shapes guidance only; it never narrows the catalog.
    let mut tool_catalog = input.registry.tools(input.web_search_enabled);
    tool_catalog.extend(context::tools());
    let selected_tools = input.discovery.attached(&tool_catalog);
    let working_budget = input.context_memory.working_budget();
    let attached_names = selected_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<std::collections::HashSet<_>>();
    let deferred_tools = if input.deferred_discovery {
        tool_catalog
            .iter()
            .filter(|tool| {
                !attached_names.contains(&tool.name)
                    && input.registry.is_provider_defer_candidate(&tool.name)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let deferred_names = deferred_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<std::collections::HashSet<_>>();
    ToolSelection {
        selected_tools,
        deferred_tools,
        deferred_names,
        catalog: tool_catalog,
        working_budget,
    }
}

/// Inputs whose overlays and accounting are updated for a single model attempt.
pub(super) struct HistoryInput<'a> {
    pub selected_tools: &'a [ToolDefinition],
    pub turn_stable_overlays: &'a [Message],
    pub turn_context_orientation: &'a str,
    pub last_context_updates: &'a mut Vec<Message>,
    pub request_input: &'a str,
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
        let mut stable_request_overlays = input.turn_stable_overlays.to_vec();
        stable_request_overlays
            .push(Message::system(input.turn_context_orientation.to_owned()).request_only());
        stable_request_overlays
            .push(Message::system(capability_guidance(capability_snapshot)).request_only());
        append_context_updates(
            &mut self.history,
            input.last_context_updates,
            &stable_request_overlays,
        );
        self.submitted_context_updates = input.last_context_updates.clone();
        let mut request_messages = self.history.clone();
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
    pub selected_tools: Vec<ToolDefinition>,
    pub deferred_tools: Vec<ToolDefinition>,
    pub model_attempts: usize,
    pub tool_rounds: usize,
    pub loop_budget: &'a LoopBudget,
    pub working_budget: u64,
    pub native_deferred_tools_supported: bool,
    pub tool_discovery: &'a ToolDiscovery,
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
        request.context_key = Some(self.context_key.clone());
        request.prompt_cache = Some(true);
        request.timeout_ms = Some(MODEL_ATTEMPT_TIMEOUT_MS);
        request.deferred_tools = (!input.deferred_tools.is_empty()).then_some(input.deferred_tools);
        configure_tool_access(&mut request, input.selected_tools);
        let mut request_metadata = turn_state::request_metadata(turn_state::RequestMetadataInput {
            context_key: request.context_key.as_deref(),
            context_memory: &self.context_memory,
            model_attempts: input.model_attempts,
            tool_rounds: input.tool_rounds,
            loop_budget: input.loop_budget,
            working_budget: input.working_budget,
            deferred_tool_count: request.deferred_tools.as_ref().map(Vec::len).unwrap_or(0),
            native_deferred_tools_supported: input.native_deferred_tools_supported,
            search_loaded_tool_count: input.tool_discovery.loaded_count(),
        });
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
        let attempt_cache_diagnostics = self.cache_continuity.diagnostics(&request);
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
