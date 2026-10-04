mod api;
mod cache;
mod context;
#[cfg(test)]
mod continuity_tests;
mod coordinator_support;
mod goal;
mod history;
mod jev;
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
mod runaway;
mod session;
mod session_controls;
mod tool_discovery;
mod tool_protocol;
mod tool_round;
mod turn_setup;
mod turn_state;
mod verify;
use api::AgentTurnRequest;
pub use api::{AgentEvent, AgentRunOutcome, AgentRunRequest};
pub(crate) use coordinator_support::is_internal_coordinator_system_message;
use coordinator_support::{
    append_dynamic_turn_checkpoints, attached_harness_flags, check_cancel, explicit_skill_name,
    render_matching_debate_memory, settle_interrupted_context_batch,
    should_inherit_implementation_turn, supports_native_deferred_tools,
};
use history::{TURN_CONTEXT_BOUNDARY, append_skill_instruction};
use limits::*;
use loop_budget::LoopBudget;
pub use policy::SYSTEM_INSTRUCTION;
use policy::{
    ResearchBudget, TaskProfile, looks_like_bounded_analysis, looks_like_bounded_explanation,
    looks_like_capability_request, looks_like_coding_implementation_request,
    looks_like_coding_request, looks_like_local_file_lookup, looks_like_planning_or_documentation,
    looks_like_prior_context_request, should_preserve_web_tool_surface, task_guidance,
    task_profile_with_history,
};
use progress::{
    content_fingerprint, round_semantic_fingerprint, tool_failure_fingerprint, tool_made_progress,
    tool_signature,
};
use runaway::{RunawayDecision, RunawayDetector};
use session_controls::{
    bridge_transport_error, goal_retry_delay, retryable_goal_error, wait_for_goal,
};
use tool_protocol::{
    collect_tool_calls, looks_like_malformed_tool_call, normalize_tool_output_for_model,
    recover_text_tool_calls,
};

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::{
    core::{CallRequest, Message, MessageRole, ToolCall, ToolDefinition},
    tools::{BridgeHandle, ToolRegistry},
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

pub struct AgentCoordinator {
    runtime_agent_id: Option<crate::agents::AgentId>,
    bridge: BridgeHandle,
    registry: ToolRegistry,
    history: Vec<Message>,
    retained_debate_knowledge: Vec<crate::debate::RetainedDebateKnowledge>,
    context_key: String,
    context_memory: context::ContextMemory,
    cache_continuity: cache::ContinuityTracker,
    observation_cache: cache::ObservationCache,
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
            profile,
            local_file_lookup,
            capability_discovery_requested,
            mut implementation_requested,
            planning_or_documentation,
            bounded_explanation,
            bounded_analysis,
            native_deferred_tools_supported,
            mut research_budget,
            mut model_attempts,
            mut tool_rounds,
            mut malformed_repairs,
            mut empty_repairs,
            mut length_continuations,
            mut implementation_repairs,
            mut execution_evidence,
            mut consecutive_no_progress,
            mut progressful_inspection_rounds,
            mut implementation_inspection_checkpoint_used,
            mut retry_instruction,
            mut final_consistency_pending,
            mut final_consistency_used,
            mut completion_gate_repairs,
            mut local_lookup_read_calls,
            mut local_lookup_read_externalized,
            mut local_lookup_recovery_calls,
            mut local_lookup_shell_calls,
            mut call_counts,
            mut workspace_generation,
            mut workspace_write_generation,
            mut tool_discovery,
            mut loop_budget,
            mut research_stop_grace_used,
            mut analysis_stop_grace_used,
            mut runaway_detector,
            mut last_provenance_checkpoint,
            workspace_revision,
            turn_stable_overlays,
            mut turn_context_orientation,
            mut last_context_updates,
            turn_start_pruned_request_only_messages,
            turn_start_pruned_request_only_chars,
            mut verification,
        } = self.prepare_turn(request, goal_mode)?;
        let bridge = self.bridge.client()?;
        self.observation_cache.start_turn();
        let mut jev_attempted = false;
        let mut last_jev_instruction: Option<String> = None;
        loop {
            check_cancel(cancel)?;
            self.context_memory.sync(&self.history)?;
            if self.context_memory.rollover_requested {
                let rollover_guidance = retry_instruction.take();
                let handoff = execution_evidence.rollover_handoff(rollover_guidance.as_deref());
                self.context_memory.rollover_with_handoff(
                    &mut self.history,
                    Some(&current_request),
                    Some(&handoff),
                )?;
                self.registry.reset_model_evidence_window();
                call_counts.clear();
                loop_budget.record_rollover();
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
                last_jev_instruction = None;
            }
            append_dynamic_turn_checkpoints(
                &mut self.history,
                execution_evidence.request_overlay(),
                &mut last_provenance_checkpoint,
                &mut retry_instruction,
                &mut final_consistency_pending,
            );
            self.deliver_agent_notifications(emit);
            model_attempts += 1;
            let request_assembly::ToolSelection {
                policy: jev_loop,
                catalog: tool_catalog,
                working_budget,
            } = request_assembly::select_tools(
                request_assembly::ToolSelectionInput {
                    bridge: &bridge,
                    registry: &self.registry,
                    context_memory: &self.context_memory,
                    discovery: &tool_discovery,
                    profile,
                    web_search_enabled,
                    cancel,
                    deferred_discovery: native_deferred_tools_supported
                        && capability_discovery_requested,
                },
                jev::LoopInput {
                    goal: goal_input,
                    profile: match profile {
                        TaskProfile::Agent => "agent",
                        TaskProfile::Research => "research",
                    },
                    model_attempts,
                    tool_rounds,
                    consecutive_no_progress,
                    progressful_inspection_rounds,
                    implementation_requested,
                    successful_mutations: execution_evidence.successful_mutations(),
                    verification_attempted: execution_evidence.verification_attempted(),
                    verification_succeeded: execution_evidence.verification_succeeded(),
                    unresolved_failed_mutation: execution_evidence.unresolved_failed_mutation(),
                    planning_or_documentation,
                    bounded_analysis,
                    bounded_explanation,
                    local_file_lookup,
                    research: (profile == TaskProfile::Research)
                        .then(|| research_budget.loop_state()),
                    retry_instruction: retry_instruction.as_deref(),
                    recent_evidence: execution_evidence.recent_evidence(),
                    consult_jev: !local_file_lookup,
                },
            );
            let jev::LoopPolicy {
                selected_tools,
                deferred_tools,
                deferred_names,
                callable_names,
                instruction: jev_loop_instruction,
                advice: jev_loop_advice,
                jev_attempted: jev_attempted_this_round,
                jev_request_chars,
            } = jev_loop;
            if jev_attempted_this_round {
                loop_budget.record_sent_request(jev_request_chars);
                emit(AgentEvent::AuxiliaryUsage {
                    usage: crate::core::Usage {
                        model_calls: Some(1),
                        ..crate::core::Usage::default()
                    },
                    already_counted_calls: 0,
                });
            }
            jev::append_loop_instruction(
                &mut self.history,
                jev_loop_instruction.as_deref(),
                &mut last_jev_instruction,
            );

            let request_assembly::PreparedHistory {
                request_messages,
                request_context_chars,
                rollover_budget,
                has_rolloverable_trace,
            } = self.prepare_request_history(request_assembly::HistoryInput {
                selected_tools: &selected_tools,
                turn_stable_overlays: &turn_stable_overlays,
                turn_context_orientation: &turn_context_orientation,
                tool_discovery: &tool_discovery,
                last_context_updates: &mut last_context_updates,
                request_input: &request_input,
                profile,
                loop_budget: &mut loop_budget,
            })?;
            if has_rolloverable_trace
                && let Some(message) = loop_budget.proactive_rollover_reason(
                    request_context_chars,
                    self.context_memory.estimated_tokens,
                    rollover_budget,
                )
            {
                let handoff = execution_evidence.rollover_handoff(Some(&message));
                self.context_memory.rollover_with_handoff(
                    &mut self.history,
                    Some(&current_request),
                    Some(&handoff),
                )?;
                self.registry.reset_model_evidence_window();
                call_counts.clear();
                loop_budget.record_rollover();
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
                last_jev_instruction = None;
                continue;
            }
            if self.context_memory.estimated_tokens > rollover_budget && has_rolloverable_trace {
                let handoff = execution_evidence.rollover_handoff(None);
                self.context_memory.rollover_with_handoff(
                    &mut self.history,
                    Some(&current_request),
                    Some(&handoff),
                )?;
                self.registry.reset_model_evidence_window();
                call_counts.clear();
                loop_budget.record_rollover();
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
                last_jev_instruction = None;
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
                profile,
                selected_tools,
                deferred_tools,
                jev_loop_advice: jev_loop_advice.as_ref(),
                execution_evidence: &execution_evidence,
                implementation_requested,
                planning_or_documentation,
                model_attempts,
                tool_rounds,
                loop_budget: &loop_budget,
                working_budget,
                native_deferred_tools_supported,
                tool_discovery: &tool_discovery,
                turn_start_pruned_request_only_messages,
                turn_start_pruned_request_only_chars,
                jev_attempted_this_round,
                jev_request_chars,
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

            self.observation_cache.mark_submitted();
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
            self.observation_cache.observe_usage(finish_usage.as_ref());

            let (mut calls, malformed_calls) = collect_tool_calls(decoded, partial);
            if !malformed_calls.is_empty() {
                for call in &malformed_calls {
                    emit(AgentEvent::ToolExecutionFinished {
                        call: call.clone(),
                        succeeded: false,
                        result: "Malformed or incomplete structured tool arguments; Yeet is retrying the model turn instead of executing this call.".into(),
                    });
                }
                if malformed_repairs < MALFORMED_TOOL_REPAIR_LIMIT {
                    malformed_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(std::mem::take(
                            &mut emitted_text,
                        )));
                    }
                    retry_instruction = Some("Internal execution correction: the previous structured tool call had malformed or incomplete JSON arguments. Reissue the needed tool call with one complete JSON object matching the attached schema. Do not repeat or continue the truncated argument fragment.".into());
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
                if finish_reason == "length" {
                    if length_continuations < LENGTH_CONTINUATION_LIMIT {
                        length_continuations += 1;
                        if !text.is_empty() {
                            self.history.push(
                                Message::assistant(text, None)
                                    .with_provider_state(finish_provider_state.clone()),
                            );
                        }
                        retry_instruction = Some(
                            "Internal continuation: the previous model response reached its output-token limit. Continue from the existing partial response without repeating it. If a structured tool call was cut off, reissue the complete call rather than continuing a JSON fragment."
                                .into(),
                        );
                        emit(AgentEvent::Finished {
                            reason: finish_reason,
                            usage: finish_usage,
                        });
                        continue;
                    }
                    if !text.is_empty() {
                        self.history.push(
                            Message::assistant(text, None)
                                .with_provider_state(finish_provider_state.clone()),
                        );
                    }
                    let reason = "Model output reached the token limit again after one bounded continuation.".to_owned();
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    return Ok(if goal_mode.load(Ordering::Acquire) {
                        AgentRunOutcome::GoalPaused { reason }
                    } else {
                        AgentRunOutcome::CompletedUnverified { reason }
                    });
                }
                if finish_reason == "context_length" || finish_reason == "content_filter" {
                    if !text.is_empty() {
                        self.history.push(
                            Message::assistant(text, None)
                                .with_provider_state(finish_provider_state.clone()),
                        );
                    }
                    let reason = if finish_reason == "context_length" {
                        "Provider stopped because the model context window was exceeded.".to_owned()
                    } else {
                        "Provider content filtering stopped the model response before normal completion.".to_owned()
                    };
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    return Ok(if goal_mode.load(Ordering::Acquire) {
                        AgentRunOutcome::GoalPaused { reason }
                    } else {
                        AgentRunOutcome::CompletedUnverified { reason }
                    });
                }
                if finish_reason == "unknown" {
                    if !text.is_empty() {
                        self.history.push(
                            Message::assistant(text, None)
                                .with_provider_state(finish_provider_state.clone()),
                        );
                    }
                    let reason =
                        "Provider ended the model response without a recognized completion reason."
                            .to_owned();
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    return Ok(if goal_mode.load(Ordering::Acquire) {
                        AgentRunOutcome::GoalPaused { reason }
                    } else {
                        AgentRunOutcome::CompletedUnverified { reason }
                    });
                }
                let trimmed = text.trim();

                // A Goal checkpoint stays in this execution loop: tool access, working
                // state, budgets, and loop guards must not reset on model end-of-turn.
                if goal_mode.load(Ordering::Acquire) {
                    check_cancel(cancel)?;
                    self.history.push(
                        Message::assistant(text, None)
                            .with_provider_state(finish_provider_state.clone()),
                    );
                    emit(AgentEvent::Finished {
                        reason: "goal_checkpoint".into(),
                        usage: finish_usage,
                    });
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
                    self.context_memory.sync(&self.history)?;
                    self.context_memory.flush()?;
                    // Counters are independent of the retained history window.
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
                        runaway_detector = RunawayDetector::default();
                        retry_instruction = Some(format!(
                            "Goal recovery after checkpoint {goal_epoch}: {}. Continue the same job in a fresh working window. Preserve completed work and the current workspace-evidence index, choose a materially different next action, and do not replay equivalent inspection.",
                            verdict.reason
                        ));
                    }
                    if !wait_for_goal(cancel, goal_mode, GOAL_CONTINUATION_DELAY) {
                        check_cancel(cancel)?;
                        return Ok(AgentRunOutcome::GoalPaused {
                            reason: verdict.reason,
                        });
                    }
                    if action == goal::GoalCheckpointAction::Continue {
                        retry_instruction = Some(format!(
                            "Goal job checkpoint {goal_epoch}: {}. Select the next concrete action from this feedback and existing evidence. Do not repeat a completion summary. If blocked, state the specific missing input or permission instead of repeating work.",
                            verdict.reason
                        ));
                    }
                    continue;
                }
                if looks_like_malformed_tool_call(trimmed)
                    && malformed_repairs < MALFORMED_TOOL_REPAIR_LIMIT
                {
                    malformed_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(emitted_text));
                    }
                    retry_instruction = Some("Internal execution correction: your previous response encoded a tool call as plain text. If a tool is needed, invoke the available structured tool directly; otherwise return the actual final answer in normal prose.".into());
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }

                if trimmed.is_empty()
                    && implementation_requested
                    && empty_repairs < EMPTY_RESPONSE_REPAIR_LIMIT
                {
                    empty_repairs += 1;
                    retry_instruction = Some("Internal execution correction: the previous attempt ended without a usable assistant response or structured tool call. Continue the task now. Use structured tools if work remains; otherwise provide a concise final answer.".into());
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }
                if let Some(correction) =
                    research_budget.completion_retry(profile, completion_gate_repairs)
                {
                    completion_gate_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(emitted_text));
                    }
                    retry_instruction = Some(correction);
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }

                if implementation_requested
                    && execution_evidence.successful_mutations() == 0
                    && implementation_repairs < IMPLEMENTATION_REPAIR_LIMIT
                {
                    implementation_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(emitted_text));
                    }
                    retry_instruction = Some(
                        "Coordinator completion state: coding implementation was requested, but no workspace mutation has succeeded yet."
                            .into(),
                    );
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }

                let completion_blocker = execution_evidence
                    .completion_blocker(implementation_requested, planning_or_documentation);
                if let Some(blocker) = completion_blocker {
                    if completion_gate_repairs < COMPLETION_GATE_REPAIR_LIMIT {
                        completion_gate_repairs += 1;
                        if !emitted_text.is_empty() {
                            emit(AgentEvent::DiscardAssistantText(emitted_text));
                        }
                        retry_instruction = Some(format!(
                            "Coordinator completion state: {blocker}. successfulWorkspaceMutations={}; verificationAttempted={}; verificationSucceeded={}.",
                            execution_evidence.successful_mutations(),
                            execution_evidence.verification_attempted(),
                            execution_evidence.verification_succeeded(),
                        ));
                        emit(AgentEvent::Finished {
                            reason: finish_reason,
                            usage: finish_usage,
                        });
                        continue;
                    }

                    let warning = execution_evidence.completion_warning(&blocker);
                    let final_text = if text.trim().is_empty() {
                        warning.clone()
                    } else {
                        format!("{text}\n\n{warning}")
                    };
                    // Preserve useful completion details; append the coordinator's
                    // verification caveat rather than replacing the entire answer.
                    emit(AgentEvent::TextDelta(if emitted_text.is_empty() {
                        final_text.clone()
                    } else {
                        format!("\n\n{warning}")
                    }));
                    self.history.push(Message::assistant(final_text, None));
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    return Ok(AgentRunOutcome::CompletedUnverified { reason: blocker });
                }
                self.history.push(
                    Message::assistant(text, None)
                        .with_provider_state(finish_provider_state.clone()),
                );
                emit(AgentEvent::Finished {
                    reason: finish_reason,
                    usage: finish_usage,
                });
                return Ok(AgentRunOutcome::Completed);
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
            let mut duplicate_inspection = false;
            let mut round_progress = false;
            let mut round_failed_mutation = false;
            let mut round_mutated = false;
            let mut round_fresh_calls = 0usize;
            let mut round_repeated_calls = 0usize;
            let mut round_failure_fingerprints = Vec::new();
            let mut round_output_fingerprints = Vec::new();
            let semantic_fingerprint = round_semantic_fingerprint(&calls);
            let mut round_output_budget = ToolRegistry::round_output_budget(calls.len());
            let mut batch_signatures = HashSet::new();
            let parallel_batch_eligible = !local_file_lookup
                && calls.len() > 1
                && calls.iter().all(|call| {
                    let signature = tool_signature(call);
                    callable_names.contains(&call.name)
                        && !call_counts.contains_key(&signature)
                        && batch_signatures.insert(signature)
                })
                && self.registry.can_parallel_read_only_mcp_batch(&calls);
            let mut parallel_mcp_results = if parallel_batch_eligible {
                for call in &calls {
                    emit(AgentEvent::ToolExecutionStarted(call.clone()));
                }
                self.registry
                    .execute_parallel_read_only_mcp_batch(&calls, cancel)
                    .map(Vec::into_iter)
            } else {
                None
            };
            let parallel_mcp_active = parallel_mcp_results.is_some();
            self.registry.prepare_tool_batch(&calls, model);
            for call in &calls {
                check_cancel(cancel)?;
                let current_generation = self.registry.workspace_generation();
                if current_generation != workspace_generation {
                    // Workspace writes invalidate structured source-cache replay identity.
                    call_counts.clear();
                    workspace_generation = current_generation;
                }
                let signature = tool_signature(call);
                let count = call_counts.entry(signature).or_default();
                let repeated_call = *count > 0;
                *count += 1;
                if repeated_call {
                    round_repeated_calls += 1;
                } else {
                    round_fresh_calls += 1;
                }
                if !parallel_mcp_active {
                    emit(AgentEvent::ToolExecutionStarted(call.clone()));
                }
                let tool_round::DispatchOutcome {
                    content,
                    transport_succeeded,
                    inspection_call,
                    duplicate_inspection: blocked_duplicate,
                } = self.dispatch_tool_call(tool_round::DispatchInput {
                    call,
                    model,
                    cancel,
                    profile,
                    web_search_enabled,
                    local_file_lookup,
                    local_lookup_read_calls,
                    local_lookup_read_externalized,
                    local_lookup_recovery_calls,
                    local_lookup_shell_calls,
                    repeated_call,
                    callable_names: &callable_names,
                    deferred_names: &deferred_names,
                    tool_catalog: &tool_catalog,
                    parallel_mcp_results: &mut parallel_mcp_results,
                    tool_discovery: &mut tool_discovery,
                })?;
                duplicate_inspection |= blocked_duplicate;
                let tool_round::ObservedDispatch {
                    succeeded,
                    inspection_progress,
                    workspace_write_observed,
                    workspace_mutated,
                } = self.observe_dispatch_result(tool_round::DispatchObservationInput {
                    call,
                    content: &content,
                    transport_succeeded,
                    inspection_call,
                    implementation_requested: &mut implementation_requested,
                    tool_discovery: &mut tool_discovery,
                    goal_progress: &mut goal_progress,
                    research_budget: &mut research_budget,
                    workspace_write_generation: &mut workspace_write_generation,
                });
                let (normalized, output_images) =
                    normalize_tool_output_for_model(&content, vision_enabled);
                let (model_content, externally_bounded) =
                    self.registry
                        .bound_round_output(call, normalized, &mut round_output_budget)?;
                if externally_bounded && !tool_discovery::is_side_tool(&call.name) {
                    // Attach the artifact reader directly. Enabling search_tools
                    // here cost two tool-envelope changes (gateway, then reader),
                    // each a full prompt-cache miss, before recovery could start.
                    tool_discovery.load(["read_artifact"]);
                }
                if local_file_lookup && call.name == "read_file" && succeeded {
                    local_lookup_read_calls += 1;
                    local_lookup_read_externalized = externally_bounded;
                }
                if local_file_lookup && call.name == "run_shell" && succeeded {
                    local_lookup_shell_calls += 1;
                }
                if local_file_lookup && call.name == "read_artifact" && succeeded {
                    local_lookup_recovery_calls += 1;
                }
                let tool_round::InsertedResult {
                    recorded_content,
                    goal_observation_item,
                } = self.insert_tool_result(tool_round::InsertionInput {
                    call,
                    model_content,
                    output_images,
                    model,
                    request: &request,
                    bridge: &bridge,
                    goal_mode,
                    succeeded,
                    externally_bounded,
                })?;
                let tool_round::EvidenceOutcome { mutation_tool } =
                    self.observe_tool_evidence(tool_round::EvidenceInput {
                        call,
                        content: &content,
                        succeeded,
                        workspace_write_observed,
                        workspace_mutated,
                        profile,
                        implementation_requested,
                        planning_or_documentation,
                        goal_mode,
                        goal_input,
                        goal_progress: &goal_progress,
                        goal_observation_item,
                        execution_evidence: &mut execution_evidence,
                    });
                if workspace_write_observed {
                    if workspace_mutated && mutation_tool {
                        // Verification is normally needed only after a confirmed source write.
                        tool_discovery.load(["run_shell"]);
                    }
                    round_mutated = true;
                    call_counts.clear();
                    workspace_generation = self.registry.workspace_generation();
                    if workspace_mutated
                        && mutation_tool
                        && planning_or_documentation
                        && !final_consistency_used
                        && self.registry.latest_write_validation_passed() == Some(true)
                    {
                        final_consistency_pending = true;
                        final_consistency_used = true;
                    }
                    if !succeeded {
                        round_failed_mutation = true;
                    }
                } else if !succeeded && mutation_tool {
                    // Failed structured edits may need one focused source refresh before retrying.
                    round_failed_mutation = true;
                }
                if !succeeded {
                    round_failure_fingerprints.push(tool_failure_fingerprint(call, &content));
                    if !jev_attempted {
                        jev_attempted = true;
                        let evaluation = jev::advise(
                            &bridge,
                            cancel,
                            goal_input,
                            &call.name,
                            &content,
                            execution_evidence.recent_evidence(),
                        );
                        if evaluation.attempted {
                            loop_budget.record_sent_request(evaluation.request_chars);
                            emit(AgentEvent::AuxiliaryUsage {
                                usage: crate::core::Usage {
                                    model_calls: Some(1),
                                    ..crate::core::Usage::default()
                                },
                                already_counted_calls: 0,
                            });
                        }
                        if let Some(advice) = evaluation.advice {
                            retry_instruction = Some(format!(
                                "Jev advisory: {} (confidence {:.0}%, probability {:.0}%). This is auxiliary model advice, not an execution directive.",
                                advice.decision,
                                advice.confidence * 100.0,
                                advice.probability * 100.0,
                            ));
                        }
                    }
                } else if inspection_progress {
                    round_output_fingerprints.push(content_fingerprint(&content));
                }
                round_progress |=
                    workspace_mutated || tool_made_progress(call, &content, succeeded);
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
            let runaway_decision = self.finish_tool_round(
                tool_round::RoundBookkeepingInput {
                    calls: &calls,
                    profile,
                    local_file_lookup,
                    tool_discovery: &tool_discovery,
                    consecutive_no_progress: &mut consecutive_no_progress,
                    progressful_inspection_rounds: &mut progressful_inspection_rounds,
                    runaway_detector: &mut runaway_detector,
                    verification: &mut verification,
                    execution_evidence: &mut execution_evidence,
                    cancel,
                    request_context_chars,
                    rollover_budget,
                },
                tool_round::RoundEvidence {
                    progressed: round_progress,
                    mutated: round_mutated,
                    failed_mutation: round_failed_mutation,
                    duplicate_inspection,
                    semantic_fingerprint,
                    failure_fingerprints: round_failure_fingerprints,
                    output_fingerprints: round_output_fingerprints,
                    fresh_calls: round_fresh_calls,
                    repeated_calls: round_repeated_calls,
                },
                emit,
            )?;
            let analysis_threshold = turn_state::analysis_inspection_threshold(bounded_explanation);
            if let RunawayDecision::Rollover(message) = &runaway_decision {
                self.context_memory.rollover_requested = true;
                retry_instruction = Some(message.clone());
            } else if let RunawayDecision::Warn(message) = &runaway_decision {
                retry_instruction = Some(message.clone());
            } else if profile == TaskProfile::Research
                && research_budget.sufficient(progressful_inspection_rounds)
                && !research_stop_grace_used
            {
                research_stop_grace_used = true;
                retry_instruction =
                    Some(research_budget.checkpoint_message(progressful_inspection_rounds));
            } else if bounded_analysis
                && execution_evidence.successful_mutations() == 0
                && progressful_inspection_rounds >= analysis_threshold
                && !analysis_stop_grace_used
            {
                analysis_stop_grace_used = true;
                retry_instruction = Some("Coordinator observation: several focused inspection rounds have produced evidence; no additional evidence gap has been identified by the coordinator.".into());
            } else if round_failed_mutation {
                retry_instruction = Some("Coordinator observation: the previous file edit failed. This result does not by itself identify a sandbox denial; stale source or an edit-shape mismatch remain possible.".into());
            } else if implementation_requested
                && execution_evidence.successful_mutations() == 0
                && !implementation_inspection_checkpoint_used
                && progressful_inspection_rounds >= IMPLEMENTATION_INSPECTION_CHECKPOINT
            {
                implementation_inspection_checkpoint_used = true;
                retry_instruction = Some("Coordinator observation: several focused inspection rounds have produced source evidence and no workspace mutation has been observed yet.".into());
            } else if consecutive_no_progress >= NO_PROGRESS_DECISION_THRESHOLD {
                retry_instruction = Some(
                    if let Some(message) = research_budget.no_progress_correction(profile, true) {
                        message
                    } else if implementation_requested
                        && execution_evidence.successful_mutations() == 0
                    {
                        "Coordinator observation: several consecutive tool rounds produced no material progress, and no workspace mutation has been observed yet.".into()
                    } else {
                        "Coordinator observation: several consecutive tool rounds produced no material progress.".into()
                    },
                );
            } else if consecutive_no_progress >= NO_PROGRESS_CORRECTION_THRESHOLD {
                retry_instruction = Some(
                    if let Some(message) = research_budget.no_progress_correction(profile, false) {
                        message
                    } else if implementation_requested
                        && execution_evidence.successful_mutations() == 0
                    {
                        if duplicate_inspection {
                            "Coordinator observation: the last round repeated previously covered inspection and added no new evidence; no workspace mutation has been observed yet.".into()
                        } else {
                            "Coordinator observation: the last tool round added no material evidence, and no workspace mutation has been observed yet.".into()
                        }
                    } else if duplicate_inspection {
                        "Coordinator observation: the last round repeated previously covered inspection and added no new evidence.".into()
                    } else {
                        "Coordinator observation: the last tool round added no material evidence."
                            .into()
                    },
                );
            } else if let RunawayDecision::Warn(message) = runaway_decision {
                retry_instruction = Some(format!("Coordinator runtime observation: {message}"));
            }
        }
    }
}
