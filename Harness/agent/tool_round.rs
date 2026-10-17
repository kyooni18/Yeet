//! Dispatch tool calls and append their results to the model history.

use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};

use serde_json::{Value, json};

use crate::core::{ImageAttachment, ToolCall, ToolDefinition};

use super::{
    AgentCoordinator, AgentEvent, Message, Result, context,
    coordinator_support::{
        attached_web_search_capability_result, check_cancel,
        supports_anthropic_deferred_tool_references,
    },
    goal, nested_instructions,
    progress::{self, classify_tool_error},
    tool_discovery::{self, ToolDiscovery},
    turn_state::TurnExecutionEvidence,
};

pub(super) type ParallelResults = Option<std::vec::IntoIter<std::result::Result<String, String>>>;

impl AgentCoordinator {
    /// Runs an all-read-only MCP batch concurrently when possible.
    pub(super) fn prepare_tool_batch<F>(
        &mut self,
        calls: &[ToolCall],
        cancel: &AtomicBool,
        model: &str,
        emit: &mut F,
    ) -> ParallelResults
    where
        F: FnMut(AgentEvent),
    {
        let results = if calls.len() > 1 && self.registry.can_parallel_read_only_mcp_batch(calls)
        {
            for call in calls {
                emit(AgentEvent::ToolExecutionStarted(call.clone()));
            }
            self.registry
                .execute_parallel_read_only_mcp_batch(calls, cancel)
                .map(Vec::into_iter)
        } else {
            None
        };
        self.registry.prepare_tool_batch(calls, model);
        results
    }
}

pub(super) struct DispatchInput<'a> {
    pub call: &'a ToolCall,
    pub model: &'a str,
    pub cancel: &'a AtomicBool,
    pub web_search_enabled: bool,
    pub deferred_names: &'a HashSet<String>,
    pub tool_catalog: &'a [ToolDefinition],
    pub parallel_mcp_results: &'a mut ParallelResults,
    pub tool_discovery: &'a mut ToolDiscovery,
}

impl AgentCoordinator {
    /// Executes one call and returns its content and transport success. Any
    /// catalog tool may be called; calling one attaches its schema from then on.
    pub(super) fn dispatch_tool_call(&mut self, input: DispatchInput<'_>) -> Result<(String, bool)> {
        let call = input.call;
        if call.name == tool_discovery::SEARCH_TOOL {
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
            return Ok((content, true));
        }
        if !context::is_context_tool(&call.name)
            && input.tool_catalog.iter().any(|tool| tool.name == call.name)
        {
            input.tool_discovery.load([call.name.as_str()]);
        }
        if let Some(execution) = input
            .parallel_mcp_results
            .as_mut()
            .and_then(|results| results.next())
        {
            check_cancel(input.cancel)?;
            return Ok(match execution {
                Ok(content) => (content, true),
                Err(message) => (error_payload(&message), false),
            });
        }
        if let Some(content) = attached_web_search_capability_result(call, input.web_search_enabled)
        {
            return Ok((content, true));
        }
        let execution = if context::is_context_tool(&call.name) {
            self.context_memory.sync(&self.history)?;
            self.context_memory.execute(call)
        } else {
            self.registry.execute(call, input.model, input.cancel)
        };
        check_cancel(input.cancel)?;
        Ok(match execution {
            Ok(content) => (content, true),
            Err(error) => (error_payload(&error.to_string()), false),
        })
    }

    /// Persists the window, keeps the warmed tool surface, and attaches any
    /// nested AGENTS.md instructions for directories the round touched.
    pub(super) fn finish_tool_round(
        &mut self,
        calls: &[ToolCall],
        tool_discovery: &ToolDiscovery,
    ) -> Result<()> {
        self.context_memory.flush()?;
        self.warm_tool_names = tool_discovery.loaded_names().to_vec();
        self.warm_tool_search_enabled = tool_discovery.search_enabled();
        let (session_cwd, _) = self.registry.session_environment();
        let nested = nested_instructions::overlays_for_round(
            &self.history,
            &self.registry.workspace_root().to_string_lossy(),
            &session_cwd,
            calls,
        );
        self.history.extend(nested);
        Ok(())
    }
}

fn error_payload(message: &str) -> String {
    json!({"error": message, "reason": classify_tool_error(message)}).to_string()
}

/// A successful capability activation attaches the tools it exposes.
pub(super) fn load_activated_tools(call: &ToolCall, content: &str, discovery: &mut ToolDiscovery) {
    if call.name != "activate_capability" {
        return;
    }
    let activated = serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| value.get("tools").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    discovery.load(activated.iter().filter_map(Value::as_str));
}

/// One model-visible tool result to append to history.
pub(super) struct InsertionInput<'a> {
    pub call: &'a ToolCall,
    pub model_content: String,
    pub output_images: Vec<ImageAttachment>,
    pub goal_mode: &'a AtomicBool,
}

pub(super) struct InsertedResult {
    pub recorded_content: String,
    pub goal_observation_item: usize,
}

impl AgentCoordinator {
    pub(super) fn insert_tool_result(
        &mut self,
        input: InsertionInput<'_>,
    ) -> Result<InsertedResult> {
        let model_content = input.model_content;
        let recorded_content = model_content.clone();
        let goal_observation_item = self.history.len();
        self.history.push(Message::tool(
            model_content,
            input.call.id.clone(),
            Some(input.call.name.clone()),
        ));
        if !input.output_images.is_empty() {
            self.history.push(Message::user_with_images(
                format!("Visual output returned by tool {}.", input.call.name),
                input.output_images,
            ));
        }
        self.context_memory.sync(&self.history)?;
        if input.goal_mode.load(Ordering::Acquire) {
            self.context_memory.flush()?;
        }
        Ok(InsertedResult {
            recorded_content,
            goal_observation_item,
        })
    }
}

/// Evidence inputs for a tool result already in model history.
pub(super) struct EvidenceInput<'a> {
    pub call: &'a ToolCall,
    pub content: &'a str,
    pub succeeded: bool,
    pub workspace_write_observed: bool,
    pub goal_mode: &'a AtomicBool,
    pub goal_progress: &'a goal::GoalProgress,
    pub goal_observation_item: usize,
    pub execution_evidence: &'a mut TurnExecutionEvidence,
}

impl AgentCoordinator {
    /// Records what happened for rollover handoffs and the goal judge.
    pub(super) fn observe_tool_evidence(&mut self, input: EvidenceInput<'_>) {
        let validation_call = progress::is_validation_tool_result(input.call, input.content);
        input.execution_evidence.observe_tool(
            input.call,
            input.content,
            input.succeeded,
            input.workspace_write_observed,
            validation_call,
            input.call.name == "apply_file_edits",
        );
        if !input.goal_mode.load(Ordering::Acquire) {
            return;
        }
        let window_id = self.context_memory.id().to_owned();
        if let Some(goal) = self.context_memory.goal_mut() {
            goal.progress = input.goal_progress.clone();
            goal.status = goal::GoalStatus::Running;
            goal.observe(goal::GoalObservation {
                window_id,
                item: input.goal_observation_item,
                tool_call_id: input.call.id.clone(),
                tool_name: input.call.name.clone(),
                succeeded: input.succeeded,
                workspace_mutated: input.succeeded && input.workspace_write_observed,
                validation_call,
                excerpt: input.content.chars().take(1_000).collect(),
            });
        }
    }
}
