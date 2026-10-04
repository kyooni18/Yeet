//! Per-attempt tool envelope selection and loop policy before request assembly.

use std::sync::atomic::AtomicBool;

use crate::core::{BridgeClient, ToolDefinition};
use crate::tools::ToolRegistry;

use super::{
    context, jev, policy::TaskProfile, policy::select_tools_for_profile,
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
