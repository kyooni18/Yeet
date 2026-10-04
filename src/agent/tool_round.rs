//! Dispatch one tool call with the round's replay and capability guards.

use std::{collections::HashSet, sync::atomic::AtomicBool};

use serde_json::json;

use crate::core::{ToolCall, ToolDefinition};

use super::{
    AgentCoordinator, Result, context,
    coordinator_support::{
        attached_web_search_capability_result, check_cancel,
        supports_anthropic_deferred_tool_references,
    },
    policy::TaskProfile,
    progress::{classify_tool_error, is_inspection_tool},
    tool_discovery::{self, ToolDiscovery},
};

pub(super) type ParallelResults = Option<std::vec::IntoIter<std::result::Result<String, String>>>;

/// Inputs to dispatch, including the counters that bound a local file lookup.
pub(super) struct DispatchInput<'a> {
    pub call: &'a ToolCall,
    pub model: &'a str,
    pub cancel: &'a AtomicBool,
    pub profile: TaskProfile,
    pub web_search_enabled: bool,
    pub local_file_lookup: bool,
    pub local_lookup_read_calls: usize,
    pub local_lookup_read_externalized: bool,
    pub local_lookup_recovery_calls: usize,
    pub local_lookup_shell_calls: usize,
    pub repeated_call: bool,
    pub callable_names: &'a HashSet<String>,
    pub deferred_names: &'a HashSet<String>,
    pub tool_catalog: &'a [ToolDefinition],
    pub parallel_mcp_results: &'a mut ParallelResults,
    pub tool_discovery: &'a mut ToolDiscovery,
}

pub(super) struct DispatchOutcome {
    pub content: String,
    pub transport_succeeded: bool,
    pub inspection_call: bool,
    pub duplicate_inspection: bool,
}

impl AgentCoordinator {
    pub(super) fn dispatch_tool_call(
        &mut self,
        input: DispatchInput<'_>,
    ) -> Result<DispatchOutcome> {
        let call = input.call;
        let inspection_call =
            is_inspection_tool(&call.name) || self.registry.is_read_only_extension_tool(&call.name);
        let local_lookup_complete = input.local_file_lookup
            && input.local_lookup_read_calls >= 3
            && (!input.local_lookup_read_externalized
                || input.local_lookup_read_calls >= 4
                || input.local_lookup_recovery_calls >= 1);
        let local_lookup_selection_complete = input.local_file_lookup
            && input.local_lookup_read_calls == 0
            && input.local_lookup_shell_calls >= 1;
        let mut duplicate_inspection = false;
        let (content, transport_succeeded) = if !input.callable_names.contains(&call.name) {
            (json!({"error":"Tool schema is not attached. Call search_tools to load it, then call it on the next request."}).to_string(), false)
        } else if call.name == tool_discovery::SEARCH_TOOL {
            let content = if supports_anthropic_deferred_tool_references(input.model) {
                input.tool_discovery.search_with_deferred(
                    &call.arguments,
                    input.tool_catalog,
                    input.deferred_names,
                )
            } else {
                input
                    .tool_discovery
                    .search(&call.arguments, input.tool_catalog)
            };
            (content, true)
        } else if local_lookup_complete && (inspection_call || call.name == "run_shell") {
            (json!({
                "duplicate": true,
                "contentAlreadyReturned": true,
                "blockedReplay": true,
                "localLookupComplete": true,
                "tool": call.name,
                "hint": "The bounded local lookup already read the selected file. Answer from that evidence now; request only one narrower read_file range if a specific section is genuinely missing."
            }).to_string(), true)
        } else if local_lookup_selection_complete && call.name == "run_shell" {
            (json!({
                "duplicate": true,
                "blockedReplay": true,
                "localLookupSelectionComplete": true,
                "tool": call.name,
                "hint": "The focused native metadata command already ran. Use its selected path with read_file now; do not run another discovery command."
            }).to_string(), true)
        } else if input.repeated_call && inspection_call {
            duplicate_inspection = true;
            (json!({
                "duplicate": true,
                "contentAlreadyReturned": true,
                "blockedReplay": true,
                "tool": call.name,
                "hint": "This exact inspection call already ran in the current workspace generation. Reuse its result or choose a materially different action."
            }).to_string(), true)
        } else if let Some(execution) = input
            .parallel_mcp_results
            .as_mut()
            .and_then(|results| results.next())
        {
            check_cancel(input.cancel)?;
            match execution {
                Ok(content) => (content, true),
                Err(message) => (
                    json!({
                        "error": message,
                        "reason": classify_tool_error(&message),
                    })
                    .to_string(),
                    false,
                ),
            }
        } else if let Some(content) =
            attached_web_search_capability_result(call, input.web_search_enabled)
        {
            (content, true)
        } else {
            let execution = if context::is_context_tool(&call.name) {
                self.context_memory.sync(&self.history)?;
                self.context_memory.execute(call)
            } else if input.profile == TaskProfile::Research {
                self.registry
                    .execute_research(call, input.model, input.cancel)
            } else {
                self.registry.execute(call, input.model, input.cancel)
            };
            check_cancel(input.cancel)?;
            match execution {
                Ok(content) => (content, true),
                Err(error) => {
                    let message = error.to_string();
                    (
                        json!({
                            "error": message,
                            "reason": classify_tool_error(&message),
                        })
                        .to_string(),
                        false,
                    )
                }
            }
        };
        Ok(DispatchOutcome {
            content,
            transport_succeeded,
            inspection_call,
            duplicate_inspection,
        })
    }
}
