//! Per-attempt tool envelope selection and loop policy before request assembly.

use std::sync::atomic::AtomicBool;

use crate::core::{BridgeClient, ToolDefinition};
use crate::tools::ToolRegistry;

use super::{
    AgentCoordinator, LoopBudget, Message, MessageRole, Result, cache, context,
    coordinator_support::capability_guidance, history::append_context_updates, jev,
    policy::TaskProfile, policy::request_history_for_profile_at, policy::select_tools_for_profile,
    tool_discovery::ToolDiscovery,
};

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
