mod api;
mod cache;
mod context;
#[cfg(test)]
mod continuity_tests;
mod coordinator_support;
mod goal;
mod history;
mod job;
mod limits;
mod loop_budget;
mod model_stream;
mod nested_instructions;
mod phase;
mod policy;
mod progress;
mod repo_context;
mod request_assembly;
mod session;
mod session_controls;
mod tool_discovery;
mod tool_protocol;
mod tool_round;
mod turn_setup;
mod turn_state;
use api::AgentTurnRequest;
pub use api::{AgentEvent, AgentRunOutcome, AgentRunRequest};
pub(crate) use coordinator_support::is_internal_coordinator_system_message;
use coordinator_support::{
    attached_harness_flags, check_cancel, explicit_skill_name, render_matching_debate_memory,
    settle_interrupted_context_batch, supports_native_deferred_tools,
};
use history::{TURN_CONTEXT_BOUNDARY, append_skill_instruction};
use limits::*;
use loop_budget::LoopBudget;
pub use policy::SYSTEM_INSTRUCTION;
use progress::{tool_execution_succeeded, tool_made_progress};
use session_controls::{
    bridge_transport_error, goal_retry_delay, retryable_goal_error, wait_for_goal,
};
use tool_protocol::{collect_tool_calls, normalize_tool_output_for_model, recover_text_tool_calls};

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[cfg(test)]
use crate::core::ToolDefinition;
use crate::{
    core::{CallRequest, Message, MessageRole, ToolCall},
    tools::{BridgeHandle, ToolRegistry},
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

/// Cache diagnostics for a model call made outside the agent loop, such as a
/// debate stage, in the same shape as `agent-model-attempt-finished`.
pub(crate) fn model_call_cache_diagnostics(
    request: &CallRequest,
    usage: Option<&crate::core::Usage>,
) -> Value {
    turn_state::usage_diagnostics(&cache::diagnostics(request), usage)
}

pub struct AgentCoordinator {
    runtime_agent_id: Option<crate::agents::AgentId>,
    bridge: BridgeHandle,
    registry: ToolRegistry,
    history: Vec<Message>,
    retained_debate_knowledge: Vec<crate::debate::RetainedDebateKnowledge>,
    context_key: String,
    context_memory: context::ContextMemory,
    cache_continuity: cache::ContinuityTracker,
    previous_turn_working_state: Option<String>,
    attached_skills: HashSet<String>,
    skill_instruction_history: HashSet<String>,
    // Provider-visible tool schemas stay append-only across ordinary Agent turns.
    // Shrinking the envelope at each user turn invalidates otherwise reusable prompt
    // prefixes, so remember the warmed surface until an explicit session/history reset.
    warm_tool_names: Vec<String>,
    warm_tool_search_enabled: bool,
}

impl AgentCoordinator {
    /// One user turn: send the conversation, run whatever tools the model calls,
    /// and repeat until it answers without tool calls. The coordinator does not
    /// classify requests, gate completion, or inject corrective nudges.
    fn run_turn<F>(
        &mut self,
        request: AgentTurnRequest<'_>,
        goal_mode: &AtomicBool,
        goal_input: &str,
        emit: &mut F,
    ) -> Result<AgentRunOutcome>
    where
        F: FnMut(AgentEvent),
    {
        let turn_setup::TurnSetup {
            model,
            reasoning_level,
            cancel,
            attached_capabilities,
            mut goal_epoch,
            mut goal_progress,
            current_request,
            request_input,
            vision_enabled,
            web_search_enabled,
            native_deferred_tools_supported,
            mut tool_discovery,
            mut loop_budget,
            workspace_revision,
            turn_stable_overlays,
            mut turn_context_orientation,
            mut last_context_updates,
        } = self.prepare_turn(request, goal_mode)?;
        let bridge = self.bridge.client()?;
        let mut execution_evidence = turn_state::TurnExecutionEvidence::default();
        let mut retry_instruction: Option<String> = None;
        let mut model_attempts = 0usize;
        let mut tool_rounds = 0usize;
        let mut malformed_repairs = 0usize;
        let mut length_continuations = 0usize;
        let mut workspace_write_generation = self.registry.workspace_write_generation();
        loop {
            check_cancel(cancel)?;
            self.context_memory.sync(&self.history)?;
            if self.context_memory.rollover_requested {
                let handoff =
                    execution_evidence.rollover_handoff(retry_instruction.take().as_deref());
                self.rollover(&current_request, handoff, &mut loop_budget)?;
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
            }
            if let Some(instruction) = retry_instruction.take() {
                self.history
                    .push(Message::system(instruction).request_only());
            }
            model_attempts += 1;
            let request_assembly::ToolSelection {
                selected_tools,
                deferred_tools,
                deferred_names,
                catalog: tool_catalog,
                working_budget,
                ..
            } = request_assembly::select_tools(request_assembly::ToolSelectionInput {
                registry: &self.registry,
                context_memory: &self.context_memory,
                discovery: &tool_discovery,
                web_search_enabled,
                deferred_discovery: native_deferred_tools_supported,
            });
            let request_assembly::PreparedHistory {
                request_messages,
                request_context_chars,
                rollover_budget,
                has_rolloverable_trace,
            } = self.prepare_request_history(request_assembly::HistoryInput {
                selected_tools: &selected_tools,
                turn_stable_overlays: &turn_stable_overlays,
                turn_context_orientation: &turn_context_orientation,
                last_context_updates: &mut last_context_updates,
                request_input: &request_input,
                loop_budget: &mut loop_budget,
            })?;
            let proactive = loop_budget.proactive_rollover_reason(
                request_context_chars,
                self.context_memory.estimated_tokens,
                rollover_budget,
            );
            if has_rolloverable_trace
                && (proactive.is_some() || self.context_memory.estimated_tokens > rollover_budget)
            {
                let handoff = execution_evidence.rollover_handoff(proactive.as_deref());
                self.rollover(&current_request, handoff, &mut loop_budget)?;
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
                continue;
            }
            if self.context_memory.estimated_tokens > rollover_budget {
                bail!(
                    "The task input and tool schemas exceed the fresh working-context budget; reduce the input or attached capabilities."
                );
            }
            let request_assembly::PreparedRequest {
                request,
                diagnostics: attempt_cache_diagnostics,
            } = self.finalize_request(request_assembly::FinalizationInput {
                model,
                request_messages,
                selected_tools,
                deferred_tools,
                model_attempts,
                tool_rounds,
                loop_budget: &loop_budget,
                working_budget,
                native_deferred_tools_supported,
                tool_discovery: &tool_discovery,
                workspace_revision: workspace_revision.as_deref(),
                reasoning_level,
                attached_capabilities: attached_capabilities.as_deref(),
            })?;
            let sent_request_chars = serde_json::to_vec(&request)
                .map(|serialized| serialized.len())
                .unwrap_or(request_context_chars);
            loop_budget.record_sent_request(sent_request_chars);
            emit(AgentEvent::ModelAttemptStarted {
                diagnostics: attempt_cache_diagnostics.clone(),
            });

            let model_stream::StreamedAttempt {
                mut text,
                mut emitted_text,
                decoded,
                partial,
                finish_reason,
                finish_usage,
                finish_provider_state,
            } = model_stream::consume(&bridge, &request, cancel, emit)?;
            check_cancel(cancel)?;
            emit(AgentEvent::ModelAttemptFinished(
                turn_state::usage_diagnostics(&attempt_cache_diagnostics, finish_usage.as_ref()),
                finish_usage.clone(),
            ));
            loop_budget.observe_usage(finish_usage.as_ref());

            let (mut calls, malformed_calls) = collect_tool_calls(decoded, partial);
            if !malformed_calls.is_empty() {
                for call in &malformed_calls {
                    emit(AgentEvent::ToolExecutionFinished {
                        call: call.clone(),
                        succeeded: false,
                        result: "Malformed or incomplete tool arguments; retrying.".into(),
                    });
                }
                if malformed_repairs < MALFORMED_TOOL_REPAIR_LIMIT {
                    malformed_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(std::mem::take(
                            &mut emitted_text,
                        )));
                    }
                    retry_instruction = Some("The previous tool call had malformed or incomplete JSON arguments. Reissue it with one complete JSON object.".into());
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }
                bail!("model repeatedly emitted malformed structured tool-call arguments");
            }
            if calls.is_empty() {
                let recovered = recover_text_tool_calls(&text);
                if !recovered.is_empty() {
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(std::mem::take(
                            &mut emitted_text,
                        )));
                    }
                    text.clear();
                    for (index, call) in recovered.iter().cloned().enumerate() {
                        emit(AgentEvent::ToolCall { index, call });
                    }
                    calls = recovered;
                }
            }
            if finish_reason == "error" {
                if !text.is_empty() {
                    self.history.push(
                        Message::assistant(text, None)
                            .with_provider_state(finish_provider_state.clone()),
                    );
                }
                emit(AgentEvent::Finished {
                    reason: finish_reason,
                    usage: finish_usage,
                });
                bail!("model stream ended with an error finish reason");
            }
            if calls.is_empty() {
                if finish_reason == "length" && length_continuations < LENGTH_CONTINUATION_LIMIT {
                    length_continuations += 1;
                    if !text.is_empty() {
                        self.history.push(
                            Message::assistant(text, None)
                                .with_provider_state(finish_provider_state.clone()),
                        );
                    }
                    retry_instruction = Some(
                        "Your previous response hit the output-token limit. Continue where it stopped without repeating it."
                            .into(),
                    );
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }
                let stopped_early = match finish_reason.as_str() {
                    "length" => Some("Model output reached the token limit."),
                    "context_length" => {
                        Some("Provider stopped because the model context window was exceeded.")
                    }
                    "content_filter" => Some(
                        "Provider content filtering stopped the model response before normal completion.",
                    ),
                    "unknown" => Some(
                        "Provider ended the model response without a recognized completion reason.",
                    ),
                    _ => None,
                };
                if let Some(reason) = stopped_early {
                    if !text.is_empty() {
                        self.history.push(
                            Message::assistant(text, None)
                                .with_provider_state(finish_provider_state.clone()),
                        );
                    }
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    let reason = reason.to_owned();
                    return Ok(if goal_mode.load(Ordering::Acquire) {
                        AgentRunOutcome::GoalPaused { reason }
                    } else {
                        AgentRunOutcome::CompletedUnverified { reason }
                    });
                }

                self.history.push(
                    Message::assistant(text, None)
                        .with_provider_state(finish_provider_state.clone()),
                );
                emit(AgentEvent::Finished {
                    reason: if goal_mode.load(Ordering::Acquire) {
                        "goal_checkpoint".into()
                    } else {
                        finish_reason
                    },
                    usage: finish_usage,
                });
                if !goal_mode.load(Ordering::Acquire) {
                    return Ok(AgentRunOutcome::Completed);
                }

                // A Goal job keeps working until the independent judge accepts it.
                let (verdict, judge_usage, judge_request_chars) = self.judge_goal(
                    goal_input,
                    model,
                    reasoning_level,
                    &execution_evidence,
                    cancel,
                    emit,
                )?;
                loop_budget.record_sent_request(judge_request_chars);
                loop_budget.observe_usage(Some(&judge_usage));
                if let Some(goal) = self.context_memory.goal_mut() {
                    goal.checkpoint(&verdict);
                }
                self.context_memory.sync(&self.history)?;
                self.context_memory.flush()?;
                emit(AgentEvent::GoalJudge {
                    passed: verdict.passed,
                    reason: verdict.reason.clone(),
                });
                if verdict.passed {
                    goal_mode.store(false, Ordering::Release);
                    return Ok(AgentRunOutcome::Completed);
                }
                goal_epoch = goal_epoch.saturating_add(1);
                emit(AgentEvent::GoalCheckpoint {
                    epoch: goal_epoch,
                    reason: verdict.reason.clone(),
                });
                let action = goal_progress.checkpoint(false);
                if let Some(goal) = self.context_memory.goal_mut() {
                    goal.progress = goal_progress.clone();
                    goal.status = match action {
                        goal::GoalCheckpointAction::Continue => goal::GoalStatus::Running,
                        goal::GoalCheckpointAction::Recover => goal::GoalStatus::Recovering,
                    };
                }
                self.context_memory.flush()?;
                if action == goal::GoalCheckpointAction::Recover {
                    self.context_memory.rollover_requested = true;
                }
                if !wait_for_goal(cancel, goal_mode, GOAL_CONTINUATION_DELAY) {
                    check_cancel(cancel)?;
                    return Ok(AgentRunOutcome::GoalPaused {
                        reason: verdict.reason,
                    });
                }
                retry_instruction = Some(format!(
                    "Goal not yet met (checkpoint {goal_epoch}): {}. Keep working.",
                    verdict.reason
                ));
                continue;
            }

            self.history.push(
                Message::assistant("", Some(calls.clone()))
                    .with_provider_state(finish_provider_state.clone()),
            );
            emit(AgentEvent::Finished {
                reason: finish_reason,
                usage: finish_usage,
            });
            tool_rounds += 1;
            let mut round_output_budget = ToolRegistry::round_output_budget(calls.len());
            let mut parallel_mcp_results = self.prepare_tool_batch(&calls, cancel, model, emit);
            let parallel_mcp_active = parallel_mcp_results.is_some();
            for call in &calls {
                check_cancel(cancel)?;
                if !parallel_mcp_active {
                    emit(AgentEvent::ToolExecutionStarted(call.clone()));
                }
                let (content, transport_succeeded) =
                    self.dispatch_tool_call(tool_round::DispatchInput {
                        call,
                        model,
                        cancel,
                        web_search_enabled,
                        deferred_names: &deferred_names,
                        tool_catalog: &tool_catalog,
                        parallel_mcp_results: &mut parallel_mcp_results,
                        tool_discovery: &mut tool_discovery,
                    })?;
                let succeeded = tool_execution_succeeded(call, &content, transport_succeeded);
                if succeeded {
                    tool_round::load_activated_tools(call, &content, &mut tool_discovery);
                }
                if tool_made_progress(call, &content, succeeded) {
                    goal_progress.record_progress();
                }
                let current_write_generation = self.registry.workspace_write_generation();
                let workspace_write_observed =
                    current_write_generation != workspace_write_generation;
                workspace_write_generation = current_write_generation;
                let (normalized, output_images) =
                    normalize_tool_output_for_model(&content, vision_enabled);
                let (model_content, externally_bounded) =
                    self.registry
                        .bound_round_output(call, normalized, &mut round_output_budget)?;
                if externally_bounded {
                    tool_discovery.load(["read_artifact"]);
                }
                let tool_round::InsertedResult {
                    recorded_content,
                    goal_observation_item,
                } = self.insert_tool_result(tool_round::InsertionInput {
                    call,
                    model_content,
                    output_images,
                    goal_mode,
                })?;
                self.observe_tool_evidence(tool_round::EvidenceInput {
                    call,
                    content: &content,
                    succeeded,
                    workspace_write_observed,
                    goal_mode,
                    goal_progress: &goal_progress,
                    goal_observation_item,
                    execution_evidence: &mut execution_evidence,
                });
                if let Some(usage) = self.registry.consume_auxiliary_usage() {
                    emit(AgentEvent::AuxiliaryUsage {
                        usage,
                        already_counted_calls: 0,
                    });
                }
                emit(AgentEvent::ToolExecutionFinished {
                    call: call.clone(),
                    succeeded,
                    result: recorded_content,
                });
            }
            self.finish_tool_round(&calls, &tool_discovery)?;
        }
    }

    fn rollover(
        &mut self,
        current_request: &Message,
        handoff: Message,
        loop_budget: &mut LoopBudget,
    ) -> Result<()> {
        self.context_memory.rollover_with_handoff(
            &mut self.history,
            Some(current_request),
            Some(&handoff),
        )?;
        self.registry.reset_model_evidence_window();
        loop_budget.record_rollover();
        Ok(())
    }
}
