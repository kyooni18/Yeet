//! Dispatch tool calls and insert cache-aware results into the model history.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
};

use serde_json::{Value, json};

use crate::core::{BridgeClient, CallRequest, ImageAttachment, ToolCall, ToolDefinition};

use super::{
    AgentCoordinator, AgentEvent, LoopBudget, Message, Result, cache, context,
    coordinator_support::{
        attached_web_search_capability_result, check_cancel,
        supports_anthropic_deferred_tool_references,
    },
    goal, jev,
    policy::{TaskProfile, is_mutation_tool},
    progress::{
        self, classify_tool_error, content_fingerprint, is_inspection_tool,
        tool_execution_succeeded, tool_failure_fingerprint, tool_made_progress,
    },
    runaway::{RunawayDecision, RunawayDetector, RunawayRound},
    session_controls::tool_call_indicates_implementation_intent,
    tool_discovery::{self, ToolDiscovery},
    turn_state::TurnExecutionEvidence,
    verify::{self, VerificationMonitor},
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

/// State invalidated or latched when a tool changes the workspace generation.
pub(super) struct WorkspaceWriteInput<'a> {
    pub workspace_write_observed: bool,
    pub workspace_mutated: bool,
    pub mutation_tool: bool,
    pub succeeded: bool,
    pub planning_or_documentation: bool,
    pub tool_discovery: &'a mut ToolDiscovery,
    pub round_mutated: &'a mut bool,
    pub call_counts: &'a mut HashMap<String, usize>,
    pub workspace_generation: &'a mut u64,
    pub final_consistency_pending: &'a mut bool,
    pub final_consistency_used: &'a mut bool,
    pub round_failed_mutation: &'a mut bool,
}

impl AgentCoordinator {
    pub(super) fn account_workspace_write(&self, input: WorkspaceWriteInput<'_>) {
        if input.workspace_write_observed {
            if input.workspace_mutated && input.mutation_tool {
                // Verification is normally needed only after a confirmed source write.
                input.tool_discovery.load(["run_shell"]);
            }
            *input.round_mutated = true;
            input.call_counts.clear();
            *input.workspace_generation = self.registry.workspace_generation();
            if input.workspace_mutated
                && input.mutation_tool
                && input.planning_or_documentation
                && !*input.final_consistency_used
                && self.registry.latest_write_validation_passed() == Some(true)
            {
                *input.final_consistency_pending = true;
                *input.final_consistency_used = true;
            }
            if !input.succeeded {
                *input.round_failed_mutation = true;
            }
        } else if !input.succeeded && input.mutation_tool {
            // Failed structured edits may need one focused source refresh before retrying.
            *input.round_failed_mutation = true;
        }
    }
}

/// Mutable turn state affected by a dispatched result before model output insertion.
pub(super) struct DispatchObservationInput<'a> {
    pub call: &'a ToolCall,
    pub content: &'a str,
    pub transport_succeeded: bool,
    pub inspection_call: bool,
    pub implementation_requested: &'a mut bool,
    pub tool_discovery: &'a mut ToolDiscovery,
    pub goal_progress: &'a mut goal::GoalProgress,
    pub research_budget: &'a mut super::ResearchBudget,
    pub workspace_write_generation: &'a mut u64,
}

pub(super) struct ObservedDispatch {
    pub succeeded: bool,
    pub inspection_progress: bool,
    pub workspace_write_observed: bool,
    pub workspace_mutated: bool,
}

impl AgentCoordinator {
    pub(super) fn observe_dispatch_result(
        &self,
        input: DispatchObservationInput<'_>,
    ) -> ObservedDispatch {
        let call = input.call;
        let content = input.content;
        let succeeded = tool_execution_succeeded(call, content, input.transport_succeeded);
        if tool_call_indicates_implementation_intent(call, content, succeeded) {
            *input.implementation_requested = true;
        }
        if succeeded && call.name == "activate_capability" {
            let activated = serde_json::from_str::<Value>(content)
                .ok()
                .and_then(|value| value.get("tools").and_then(Value::as_array).cloned())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            if !activated.is_empty() {
                input
                    .tool_discovery
                    .load(activated.iter().map(String::as_str));
            }
        }
        if call.name == "run_shell"
            && succeeded
            && call.arguments.get("background").and_then(Value::as_bool) == Some(true)
        {
            // Detached execution is the only reason a local lookup
            // needs the shell-job control surface.
            input.tool_discovery.load(["shell_job"]);
        }
        let inspection_progress =
            input.inspection_call && tool_made_progress(call, content, succeeded);
        if *input.implementation_requested
            && inspection_progress
            && matches!(call.name.as_str(), "read_file" | "search_workspace")
        {
            input.tool_discovery.load(["apply_file_edits"]);
        }
        if tool_made_progress(call, content, succeeded) {
            input.goal_progress.record_progress();
        }
        input
            .research_budget
            .observe_tool(call, content, inspection_progress);
        let current_write_generation = self.registry.workspace_write_generation();
        let workspace_write_observed =
            current_write_generation != *input.workspace_write_generation;
        let workspace_mutated = succeeded && workspace_write_observed;
        *input.workspace_write_generation = current_write_generation;
        ObservedDispatch {
            succeeded,
            inspection_progress,
            workspace_write_observed,
            workspace_mutated,
        }
    }
}

/// Inputs for the cache-aware insertion of one model-visible tool result.
pub(super) struct InsertionInput<'a> {
    pub call: &'a ToolCall,
    pub model_content: String,
    pub output_images: Vec<ImageAttachment>,
    pub model: &'a str,
    pub request: &'a CallRequest,
    pub bridge: &'a BridgeClient,
    pub goal_mode: &'a AtomicBool,
    pub succeeded: bool,
    pub externally_bounded: bool,
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
        let mut model_content = input.model_content;
        if input.succeeded
            && input.call.name == "read_file"
            && !input.externally_bounded
            && self.observation_cache.prepare_read_update(
                &mut self.history,
                &mut model_content,
                input.model,
                cache::ObservationPolicy {
                    prefix_overhead_tokens: (serde_json::to_vec(&(
                        &input.request.tools,
                        &input.request.deferred_tools,
                    ))?
                    .len() as u64)
                        .div_ceil(3),
                    may_retire: !input.goal_mode.load(Ordering::Acquire),
                },
                || {
                    let (provider, local_model) = input.model.split_once('/')?;
                    input
                        .bridge
                        .list_model_info(provider)
                        .ok()?
                        .into_iter()
                        .find(|entry| entry.id == local_model || entry.id == input.model)?
                        .pricing
                },
            )
        {
            // A costed, deliberate prefix retirement starts a new local
            // continuity baseline. Provider cache keys stay stable so
            // the unchanged prefix before the retirement can still hit.
            self.cache_continuity = Default::default();
        }
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

/// Evidence and checkpoint inputs for a tool result already in model history.
pub(super) struct EvidenceInput<'a> {
    pub call: &'a ToolCall,
    pub content: &'a str,
    pub succeeded: bool,
    pub workspace_write_observed: bool,
    pub workspace_mutated: bool,
    pub profile: TaskProfile,
    pub implementation_requested: bool,
    pub planning_or_documentation: bool,
    pub goal_mode: &'a AtomicBool,
    pub goal_input: &'a str,
    pub goal_progress: &'a goal::GoalProgress,
    pub goal_observation_item: usize,
    pub execution_evidence: &'a mut TurnExecutionEvidence,
}

pub(super) struct EvidenceOutcome {
    pub mutation_tool: bool,
}

impl AgentCoordinator {
    pub(super) fn observe_tool_evidence(&mut self, input: EvidenceInput<'_>) -> EvidenceOutcome {
        let mutation_tool = is_mutation_tool(&input.call.name);
        let validation_call = progress::is_validation_tool_result(input.call, input.content);
        input.execution_evidence.observe_tool(
            input.call,
            input.content,
            input.succeeded,
            input.workspace_write_observed,
            validation_call,
            mutation_tool,
        );
        if input.profile == TaskProfile::Agent {
            let checkpoint_phase = input.execution_evidence.recommended_phase(
                input.implementation_requested,
                input.planning_or_documentation,
            );
            let checkpoint_workspace_state = self.registry.working_state_summary();
            self.context_memory.set_agent_checkpoint(
                input.goal_input,
                checkpoint_phase.as_str(),
                input.execution_evidence.successful_mutations(),
                input.execution_evidence.unresolved_failed_mutation(),
                input.execution_evidence.verification_attempted(),
                input.execution_evidence.verification_succeeded(),
                input.execution_evidence.last_validation_evidence(),
                input.execution_evidence.recent_evidence(),
                checkpoint_workspace_state.as_deref(),
            );
        }
        if input.goal_mode.load(Ordering::Acquire) {
            let window_id = self.context_memory.id().to_owned();
            // Retain the exact index of the tool message even when a
            // visual payload appends a synthetic user message afterward.
            let item = input.goal_observation_item;
            if let Some(goal) = self.context_memory.goal_mut() {
                goal.progress = input.goal_progress.clone();
                goal.status = goal::GoalStatus::Running;
                goal.observe(goal::GoalObservation {
                    window_id,
                    item,
                    tool_call_id: input.call.id.clone(),
                    tool_name: input.call.name.clone(),
                    succeeded: input.succeeded,
                    workspace_mutated: input.workspace_mutated,
                    validation_call,
                    excerpt: input.content.chars().take(1_000).collect(),
                });
            }
        }
        EvidenceOutcome { mutation_tool }
    }
}

/// Facts accumulated while dispatching and recording every call in a round.
pub(super) struct RoundEvidence {
    pub progressed: bool,
    pub mutated: bool,
    pub failed_mutation: bool,
    pub duplicate_inspection: bool,
    pub semantic_fingerprint: String,
    pub failure_fingerprints: Vec<String>,
    pub output_fingerprints: Vec<u64>,
    pub fresh_calls: usize,
    pub repeated_calls: usize,
}

/// Turn state advanced after a complete tool round, before retry policy runs.
pub(super) struct RoundBookkeepingInput<'a> {
    pub calls: &'a [ToolCall],
    pub profile: TaskProfile,
    pub local_file_lookup: bool,
    pub tool_discovery: &'a ToolDiscovery,
    pub consecutive_no_progress: &'a mut usize,
    pub progressful_inspection_rounds: &'a mut usize,
    pub runaway_detector: &'a mut RunawayDetector,
    pub verification: &'a mut VerificationMonitor,
    pub execution_evidence: &'a mut TurnExecutionEvidence,
    pub cancel: &'a AtomicBool,
    pub request_context_chars: usize,
    pub rollover_budget: u64,
}

impl AgentCoordinator {
    pub(super) fn finish_tool_round<F>(
        &mut self,
        input: RoundBookkeepingInput<'_>,
        evidence: RoundEvidence,
        emit: &mut F,
    ) -> Result<RunawayDecision>
    where
        F: FnMut(AgentEvent),
    {
        self.context_memory.flush()?;
        if input.profile == TaskProfile::Agent && !input.local_file_lookup {
            self.warm_tool_names = input.tool_discovery.loaded_names().to_vec();
            self.warm_tool_search_enabled = input.tool_discovery.search_enabled();
        }
        if evidence.progressed {
            *input.consecutive_no_progress = 0;
        } else {
            *input.consecutive_no_progress += 1;
        }
        if evidence.progressed
            && input.calls.iter().all(|call| {
                is_inspection_tool(&call.name)
                    || self.registry.is_read_only_extension_tool(&call.name)
            })
        {
            *input.progressful_inspection_rounds += 1;
        }
        let inspection_only = input.calls.iter().all(|call| {
            is_inspection_tool(&call.name) || self.registry.is_read_only_extension_tool(&call.name)
        });
        let runaway_decision = input.runaway_detector.observe(
            RunawayRound {
                progressed: evidence.progressed,
                mutated: evidence.mutated,
                duplicate_inspection: evidence.duplicate_inspection,
                inspection_only,
                semantic_fingerprint: evidence.semantic_fingerprint,
                failure_fingerprints: evidence.failure_fingerprints,
                output_fingerprints: evidence.output_fingerprints,
                request_context_chars: input.request_context_chars,
                fresh_calls: evidence.fresh_calls,
                repeated_calls: evidence.repeated_calls,
            },
            input.rollover_budget.saturating_mul(3) as usize,
        );
        let round = verify::RoundFacts {
            calls: input.calls,
            mutated: evidence.mutated,
            failed_mutation: evidence.failed_mutation,
            rollover: matches!(runaway_decision, RunawayDecision::Rollover(_)),
        };
        self.append_round_feedback(
            input.verification,
            input.execution_evidence,
            round,
            input.cancel,
            emit,
        );
        Ok(runaway_decision)
    }
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

/// Per-round failure fingerprints and the once-per-turn Jev advisory budget.
pub(super) struct FailureObservationInput<'a> {
    pub call: &'a ToolCall,
    pub content: &'a str,
    pub succeeded: bool,
    pub inspection_progress: bool,
    pub bridge: &'a BridgeClient,
    pub cancel: &'a AtomicBool,
    pub goal_input: &'a str,
    pub execution_evidence: &'a TurnExecutionEvidence,
    pub jev_attempted: &'a mut bool,
    pub loop_budget: &'a mut LoopBudget,
    pub retry_instruction: &'a mut Option<String>,
    pub round_failure_fingerprints: &'a mut Vec<String>,
    pub round_output_fingerprints: &'a mut Vec<u64>,
}

pub(super) fn record_failure_or_inspection<F>(input: FailureObservationInput<'_>, emit: &mut F)
where
    F: FnMut(AgentEvent),
{
    if !input.succeeded {
        input
            .round_failure_fingerprints
            .push(tool_failure_fingerprint(input.call, input.content));
        if !*input.jev_attempted {
            *input.jev_attempted = true;
            let evaluation = jev::advise(
                input.bridge,
                input.cancel,
                input.goal_input,
                &input.call.name,
                input.content,
                input.execution_evidence.recent_evidence(),
            );
            if evaluation.attempted {
                input
                    .loop_budget
                    .record_sent_request(evaluation.request_chars);
                emit(AgentEvent::AuxiliaryUsage {
                    usage: crate::core::Usage {
                        model_calls: Some(1),
                        ..crate::core::Usage::default()
                    },
                    already_counted_calls: 0,
                });
            }
            if let Some(advice) = evaluation.advice {
                *input.retry_instruction = Some(format!(
                    "Jev advisory: {} (confidence {:.0}%, probability {:.0}%). This is auxiliary model advice, not an execution directive.",
                    advice.decision,
                    advice.confidence * 100.0,
                    advice.probability * 100.0,
                ));
            }
        }
    } else if input.inspection_progress {
        input
            .round_output_fingerprints
            .push(content_fingerprint(input.content));
    }
}
