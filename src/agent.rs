mod api;
mod cache;
mod context;
mod coordinator_support;
mod goal;
mod history;
mod jev;
mod job;
mod limits;
mod loop_budget;
mod policy;
mod progress;
mod runaway;
mod session;
mod session_controls;
mod tool_discovery;
mod tool_protocol;
mod turn_setup;
mod turn_state;
use api::AgentTurnRequest;
pub use api::{AgentEvent, AgentRunOutcome, AgentRunRequest};
use cache::advance_turn_cache_breakpoints;
pub(crate) use coordinator_support::is_internal_coordinator_system_message;
use coordinator_support::{
    append_dynamic_turn_checkpoints, attached_harness_flags, attached_web_search_capability_result,
    capability_guidance, check_cancel, configure_tool_access, explicit_skill_name,
    render_matching_debate_memory, settle_interrupted_context_batch,
    should_inherit_implementation_turn, supports_anthropic_deferred_tool_references,
    supports_native_deferred_tools,
};
use history::{TURN_CONTEXT_BOUNDARY, append_context_updates, append_skill_instruction};
use limits::*;
use loop_budget::{LoopBudget, LoopBudgetDecision};
pub use policy::SYSTEM_INSTRUCTION;
use policy::{
    ResearchBudget, TaskProfile, is_mutation_tool, looks_like_bounded_analysis,
    looks_like_bounded_explanation, looks_like_capability_request, looks_like_coding_request,
    looks_like_implementation_request, looks_like_local_file_lookup,
    looks_like_planning_or_documentation, looks_like_prior_context_request,
    request_history_for_profile_at, select_tools_for_profile, should_preserve_web_tool_surface,
    task_guidance, task_profile_with_history,
};
use progress::{
    classify_tool_error, content_fingerprint, is_inspection_tool, is_validation_tool_call,
    round_semantic_fingerprint, tool_execution_succeeded, tool_failure_fingerprint,
    tool_made_progress, tool_signature,
};
use runaway::{RUNAWAY_FINALIZATION_RETRY_LIMIT, RunawayDecision, RunawayDetector, RunawayRound};
use session_controls::{
    bridge_transport_error, goal_retry_delay, retryable_goal_error,
    tool_call_indicates_implementation_intent, wait_for_goal,
};
use tool_protocol::{
    PartialToolCall, collect_tool_calls, looks_like_malformed_tool_call,
    normalize_tool_output_for_model, recover_text_tool_calls,
};
use turn_state::{
    SessionExecutionProvenance, completion_warning, rollover_handoff_message,
    summarize_tool_outcome,
};

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::{
    core::{
        BridgeClient, CallRequest, Message, MessageRole, StreamEvent, StreamPoll, ToolCall,
        ToolDefinition,
    },
    tools::ToolRegistry,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

pub struct AgentCoordinator {
    bridge: BridgeClient,
    registry: ToolRegistry,
    history: Vec<Message>,
    retained_debate_knowledge: Vec<crate::debate::RetainedDebateKnowledge>,
    context_key: String,
    context_memory: context::ContextMemory,
    cache_continuity: cache::ContinuityTracker,
    previous_turn_working_state: Option<String>,
    attached_skills: HashSet<String>,
    skill_instruction_history: HashSet<String>,
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
            mut successful_mutations,
            mut consecutive_no_progress,
            mut progressful_inspection_rounds,
            mut implementation_inspection_checkpoint_used,
            mut retry_instruction,
            mut final_consistency_pending,
            mut final_consistency_used,
            mut completion_gate_repairs,
            mut unresolved_failed_mutation,
            mut verification_attempted,
            mut verification_succeeded,
            mut last_validation_evidence,
            mut recent_execution_evidence,
            mut session_provenance,
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
            mut runaway_finalization,
            mut runaway_finalization_repairs,
            mut last_provenance_checkpoint,
            workspace_revision,
            turn_stable_overlays,
            mut turn_context_orientation,
            mut last_context_updates,
        } = self.prepare_turn(request, goal_mode)?;
        let mut jev_attempted = false;
        let mut last_jev_instruction: Option<String> = None;
        loop {
            check_cancel(cancel)?;
            let working_state = (profile == TaskProfile::Agent && retry_instruction.is_some())
                .then(|| self.registry.working_state_summary())
                .flatten();
            append_dynamic_turn_checkpoints(
                &mut self.history,
                session_provenance.request_overlay(),
                &mut last_provenance_checkpoint,
                &mut retry_instruction,
                working_state,
                &mut final_consistency_pending,
            );
            model_attempts += 1;
            let mut tool_catalog = match profile {
                TaskProfile::Research => self.registry.research_tools(),
                TaskProfile::Agent => {
                    select_tools_for_profile(self.registry.tools(web_search_enabled), profile)
                }
            };
            tool_catalog.extend(context::tools());
            let mut selected_tools = tool_discovery.attached(&tool_catalog);
            self.context_memory.sync(&self.history)?;
            if self.context_memory.rollover_requested {
                let handoff = rollover_handoff_message(
                    successful_mutations,
                    unresolved_failed_mutation,
                    verification_attempted,
                    verification_succeeded,
                    last_validation_evidence.as_deref(),
                    self.registry.working_state_summary().as_deref(),
                    &recent_execution_evidence,
                );
                self.context_memory.rollover_with_handoff(
                    &mut self.history,
                    Some(&current_request),
                    Some(&handoff),
                )?;
                self.context_key = self.context_memory.id().to_owned();
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
                last_jev_instruction = None;
                tool_discovery.load(["context_history", "task_notes"]);
                selected_tools = tool_discovery.attached(&tool_catalog);
            }
            let working_budget = self.context_memory.working_budget();
            let jev_loop = jev::apply_loop_policy(
                &self.bridge,
                cancel,
                selected_tools,
                &tool_catalog,
                native_deferred_tools_supported && capability_discovery_requested,
                |name| self.registry.is_provider_defer_candidate(name),
                |name| self.registry.is_read_only_extension_tool(name),
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
                    successful_mutations,
                    verification_attempted,
                    verification_succeeded,
                    unresolved_failed_mutation,
                    planning_or_documentation,
                    bounded_analysis,
                    bounded_explanation,
                    local_file_lookup,
                    research: (profile == TaskProfile::Research)
                        .then(|| research_budget.loop_state()),
                    retry_instruction: retry_instruction.as_deref(),
                    recent_evidence: &recent_execution_evidence,
                },
            );
            let selected_tools = jev_loop.selected_tools;
            let deferred_tools = jev_loop.deferred_tools;
            let deferred_names = jev_loop.deferred_names;
            let callable_names = jev_loop.callable_names;
            let jev_loop_instruction = jev_loop.instruction;
            let jev_loop_advice = jev_loop.advice;
            jev::append_loop_instruction(
                &mut self.history,
                jev_loop_instruction.as_deref(),
                &mut last_jev_instruction,
            );

            let capability_snapshot = self.registry.runtime_capability_snapshot(&selected_tools);
            let mut stable_request_overlays = turn_stable_overlays.clone();
            stable_request_overlays
                .push(Message::system(turn_context_orientation.clone()).request_only());
            stable_request_overlays.push(
                Message::system(capability_guidance(
                    capability_snapshot,
                    tool_discovery.search_enabled(),
                ))
                .request_only(),
            );
            append_context_updates(
                &mut self.history,
                &mut last_context_updates,
                &stable_request_overlays,
            );
            let mut request_messages =
                request_history_for_profile_at(&self.history, profile, self.history.len());
            advance_turn_cache_breakpoints(&mut request_messages, &request_input);
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
                + (serde_json::to_vec(&selected_tools)?.len() as u64).div_ceil(3);
            let rollover_budget = self.context_memory.rollover_budget();
            let has_rolloverable_trace = self
                .history
                .iter()
                .any(|message| matches!(message.role, MessageRole::Assistant | MessageRole::Tool));
            if self.context_memory.estimated_tokens > rollover_budget && has_rolloverable_trace {
                let handoff = rollover_handoff_message(
                    successful_mutations,
                    unresolved_failed_mutation,
                    verification_attempted,
                    verification_succeeded,
                    last_validation_evidence.as_deref(),
                    self.registry.working_state_summary().as_deref(),
                    &recent_execution_evidence,
                );
                self.context_memory.rollover_with_handoff(
                    &mut self.history,
                    Some(&current_request),
                    Some(&handoff),
                )?;
                self.context_key = self.context_memory.id().to_owned();
                turn_context_orientation = self.context_memory.orientation();
                last_context_updates.clear();
                last_jev_instruction = None;
                tool_discovery.load(["context_history", "task_notes"]);
                continue;
            }
            if self.context_memory.estimated_tokens > rollover_budget {
                bail!(
                    "The task input and tool schemas exceed the fresh working-context budget; reduce the input or attached capabilities."
                );
            }
            loop_budget.observe_request(request_context_chars);
            let mut request = CallRequest::simple(model, request_messages);
            request.context_key = Some(match profile {
                TaskProfile::Agent => self.context_key.clone(),
                TaskProfile::Research => format!("{}:research", self.context_key),
            });
            request.prompt_cache = Some(true);
            request.timeout_ms = Some(MODEL_ATTEMPT_TIMEOUT_MS);
            request.deferred_tools = (!deferred_tools.is_empty()).then_some(deferred_tools);
            configure_tool_access(&mut request, selected_tools, false);
            if jev::forces_tool_free(jev_loop_advice.as_ref()) {
                request.tool_choice = Some(json!("none"));
            }
            let mut request_metadata =
                turn_state::request_metadata(turn_state::RequestMetadataInput {
                    profile,
                    context_key: request.context_key.as_deref(),
                    context_memory: &self.context_memory,
                    model_attempts,
                    tool_rounds,
                    loop_budget: &loop_budget,
                    working_budget,
                    deferred_tool_count: request.deferred_tools.as_ref().map(Vec::len).unwrap_or(0),
                    native_deferred_tools_supported,
                    search_loaded_tool_count: tool_discovery.loaded_count(),
                });
            jev::write_loop_metadata(&mut request_metadata, jev_loop_advice.as_ref());
            if let Some(revision) = workspace_revision.as_deref() {
                request_metadata.insert("workspaceRevision".into(), revision.to_owned());
            }
            if reasoning_level != "auto" {
                request_metadata.insert("reasoningLevel".into(), reasoning_level.to_owned());
            }
            request.metadata = Some(request_metadata);
            request.attached_capabilities = attached_capabilities
                .as_deref()
                .map(session::runtime_attached_capabilities);
            let attempt_cache_diagnostics = self.cache_continuity.diagnostics(&request);
            if attempt_cache_diagnostics["wireHistoryPrefixRewriteDetected"] == json!(true) {
                bail!(
                    "Previously submitted context changed within this window; start an explicit context rollover instead of rewriting history."
                );
            }
            emit(AgentEvent::ModelAttemptStarted {
                diagnostics: attempt_cache_diagnostics.clone(),
            });

            let mut stream = self.bridge.stream(&request)?;
            let mut text = String::new();
            let mut emitted_text = String::new();
            let mut tool_call_seen = false;
            let mut decoded: HashMap<usize, ToolCall> = HashMap::new();
            let mut partial: HashMap<usize, PartialToolCall> = HashMap::new();
            let mut finish_reason = "stop".to_owned();
            let mut finish_usage = None;
            let mut finish_provider_state = None;

            loop {
                if cancel.load(Ordering::Acquire) {
                    stream.cancel();
                    bail!("cancelled");
                }
                let event = match stream.poll(Duration::from_millis(50))? {
                    StreamPoll::Event(event) => event,
                    StreamPoll::Timeout => continue,
                    StreamPoll::Done => break,
                };
                match event {
                    StreamEvent::Start => emit(AgentEvent::Start),
                    StreamEvent::ReasoningDelta(delta) => emit(AgentEvent::ReasoningDelta(delta)),
                    StreamEvent::ReasoningSummaryDelta(delta) => {
                        emit(AgentEvent::ReasoningSummaryDelta(delta))
                    }
                    StreamEvent::TextDelta(delta) => {
                        if !tool_call_seen {
                            text.push_str(&delta);
                            emitted_text.push_str(&delta);
                            emit(AgentEvent::TextDelta(delta));
                        }
                    }
                    StreamEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                    } => {
                        if !tool_call_seen {
                            tool_call_seen = true;
                            if !emitted_text.is_empty() {
                                emit(AgentEvent::DiscardAssistantText(std::mem::take(
                                    &mut emitted_text,
                                )));
                                text.clear();
                            }
                        }
                        partial
                            .entry(index)
                            .or_insert_with(|| {
                                PartialToolCall::new(index, id.clone(), name.clone())
                            })
                            .apply(id.clone(), name.clone(), arguments_delta.clone());
                        emit(AgentEvent::ToolCallDelta {
                            index,
                            id,
                            name,
                            arguments_delta,
                        });
                    }
                    StreamEvent::ToolCall { index, tool_call } => {
                        if !tool_call_seen {
                            tool_call_seen = true;
                            if !emitted_text.is_empty() {
                                emit(AgentEvent::DiscardAssistantText(std::mem::take(
                                    &mut emitted_text,
                                )));
                                text.clear();
                            }
                        }
                        decoded.insert(index, tool_call.clone());
                        emit(AgentEvent::ToolCall {
                            index,
                            call: tool_call,
                        });
                    }
                    StreamEvent::Finish {
                        finish_reason: reason,
                        usage,
                        provider_state,
                    } => {
                        finish_reason = reason;
                        finish_usage = usage;
                        finish_provider_state = provider_state;
                    }
                }
            }
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
            if runaway_finalization && !calls.is_empty() {
                for call in &calls {
                    emit(AgentEvent::ToolExecutionSuppressed {
                        call: call.clone(),
                        reason: "Runaway guard required tool-free finalization".into(),
                    });
                }
                if runaway_finalization_repairs < RUNAWAY_FINALIZATION_RETRY_LIMIT {
                    runaway_finalization_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(std::mem::take(
                            &mut emitted_text,
                        )));
                    }
                    retry_instruction = Some(
                        "Internal runaway-guard correction: multiple loop signals were confirmed. Do not request more tools. Return the best final answer from the evidence already present and identify any unresolved blocker.".into(),
                    );
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }
                if goal_mode.load(Ordering::Acquire) {
                    return Ok(AgentRunOutcome::GoalPaused {
                        reason: "Runaway recovery ignored the checkpoint; resume with a different approach.".into(),
                    });
                }
                bail!(
                    "Runaway guard finalization was ignored after a tool-free answer was required"
                );
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
                    let verdict =
                        self.judge_goal(goal_input, model, reasoning_level, cancel, emit)?;
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
                    let action = goal_progress.checkpoint(runaway_finalization);
                    if let Some(goal) = self.context_memory.goal_mut() {
                        goal.progress = goal_progress.clone();
                        goal.status = match action {
                            goal::GoalCheckpointAction::Continue => goal::GoalStatus::Running,
                            goal::GoalCheckpointAction::Recover => goal::GoalStatus::Recovering,
                            goal::GoalCheckpointAction::Pause => goal::GoalStatus::Paused,
                        };
                    }
                    self.context_memory.flush()?;
                    match action {
                        goal::GoalCheckpointAction::Pause => {
                            return Ok(AgentRunOutcome::GoalPaused {
                                reason: format!(
                                    "No progress after bounded recovery. Resume with new input or a different approach: {}",
                                    verdict.reason
                                ),
                            });
                        }
                        goal::GoalCheckpointAction::Recover => {
                            runaway_finalization = false;
                            runaway_finalization_repairs = 0;
                            runaway_detector = RunawayDetector::default();
                            self.history.push(Message::system(format!(
                                "Goal recovery: {}. Preserve completed work and evidence. Choose a materially different action, not the repeated operation. If external input or permission is required, identify it explicitly.", verdict.reason
                            )));
                        }
                        goal::GoalCheckpointAction::Continue => {}
                    }
                    if !wait_for_goal(cancel, goal_mode, GOAL_CONTINUATION_DELAY) {
                        check_cancel(cancel)?;
                        return Ok(AgentRunOutcome::GoalPaused {
                            reason: verdict.reason,
                        });
                    }
                    retry_instruction = Some(format!(
                        "Goal job checkpoint {goal_epoch}: {}. Select the next concrete action from this feedback and existing evidence. Do not repeat a completion summary. If blocked, state the specific missing input or permission instead of repeating work.",
                        verdict.reason
                    ));
                    continue;
                }
                let trimmed = text.trim();
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
                    && successful_mutations == 0
                    && implementation_repairs < IMPLEMENTATION_REPAIR_LIMIT
                {
                    implementation_repairs += 1;
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(emitted_text));
                    }
                    retry_instruction = Some("Internal execution correction: this is an implementation/fix request, but no workspace mutation has succeeded yet. Continue with the available tools if a change is needed. If no edit is justified or possible, return a concrete evidence-based final answer explaining that instead of a plan.".into());
                    emit(AgentEvent::Finished {
                        reason: finish_reason,
                        usage: finish_usage,
                    });
                    continue;
                }

                let completion_blocker = turn_state::implementation_completion_blocker(
                    implementation_requested,
                    successful_mutations,
                    unresolved_failed_mutation,
                    planning_or_documentation,
                    verification_attempted,
                    verification_succeeded,
                );
                if let Some(blocker) = completion_blocker {
                    if completion_gate_repairs < COMPLETION_GATE_REPAIR_LIMIT {
                        completion_gate_repairs += 1;
                        if !emitted_text.is_empty() {
                            emit(AgentEvent::DiscardAssistantText(emitted_text));
                        }
                        retry_instruction = Some(format!(
                            "Internal completion gate: {blocker}. Do not claim implementation is complete yet. If no mutation has succeeded, either make the smallest justified workspace change now or state the concrete blocker. If a mutation succeeded, resolve any failed edit and run one focused validation command appropriate to the project (build, test, compile/check, or lint; use git diff --check only when the workspace is actually a Git worktree). Reuse existing evidence and do not restart broad inspection."
                        ));
                        emit(AgentEvent::Finished {
                            reason: finish_reason,
                            usage: finish_usage,
                        });
                        continue;
                    }

                    let warning = completion_warning(
                        &blocker,
                        successful_mutations,
                        last_validation_evidence.as_deref(),
                    );
                    if !emitted_text.is_empty() {
                        emit(AgentEvent::DiscardAssistantText(emitted_text));
                    }
                    emit(AgentEvent::TextDelta(warning.clone()));
                    self.history.push(Message::assistant(warning, None));
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
                let inspection_call = is_inspection_tool(&call.name)
                    || self.registry.is_read_only_extension_tool(&call.name);
                let local_lookup_complete = local_file_lookup
                    && local_lookup_read_calls >= 1
                    && (!local_lookup_read_externalized
                        || local_lookup_read_calls >= 2
                        || local_lookup_recovery_calls >= 1);
                let local_lookup_selection_complete = local_file_lookup
                    && local_lookup_read_calls == 0
                    && local_lookup_shell_calls >= 1;
                let (content, transport_succeeded) = if !callable_names.contains(&call.name) {
                    (json!({"error":"Tool schema is not attached. Call search_tools to load it, then call it on the next request."}).to_string(), false)
                } else if call.name == tool_discovery::SEARCH_TOOL {
                    let content = if supports_anthropic_deferred_tool_references(model) {
                        tool_discovery.search_with_deferred(
                            &call.arguments,
                            &tool_catalog,
                            &deferred_names,
                        )
                    } else {
                        tool_discovery.search(&call.arguments, &tool_catalog)
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
                } else if repeated_call && inspection_call {
                    duplicate_inspection = true;
                    (json!({
                        "duplicate": true,
                        "contentAlreadyReturned": true,
                        "blockedReplay": true,
                        "tool": call.name,
                        "hint": "This exact inspection call already ran in the current workspace generation. Reuse its result or choose a materially different action."
                    }).to_string(), true)
                } else if let Some(execution) = parallel_mcp_results
                    .as_mut()
                    .and_then(|results| results.next())
                {
                    check_cancel(cancel)?;
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
                    attached_web_search_capability_result(call, web_search_enabled)
                {
                    (content, true)
                } else {
                    let execution = if context::is_context_tool(&call.name) {
                        self.context_memory.sync(&self.history)?;
                        self.context_memory.execute(call)
                    } else if profile == TaskProfile::Research {
                        self.registry.execute_research(call, model, cancel)
                    } else {
                        self.registry.execute(call, model, cancel)
                    };
                    check_cancel(cancel)?;
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
                let succeeded = tool_execution_succeeded(call, &content, transport_succeeded);
                if tool_call_indicates_implementation_intent(call, &content, succeeded) {
                    implementation_requested = true;
                }
                if succeeded && call.name == "activate_capability" {
                    let activated = serde_json::from_str::<Value>(&content)
                        .ok()
                        .and_then(|value| value.get("tools").and_then(Value::as_array).cloned())
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect::<Vec<_>>();
                    if !activated.is_empty() {
                        tool_discovery.load(activated.iter().map(String::as_str));
                    }
                }
                if call.name == "run_shell"
                    && succeeded
                    && call.arguments.get("background").and_then(Value::as_bool) == Some(true)
                {
                    // Detached execution is the only reason a local lookup
                    // needs the shell-job control surface.
                    tool_discovery.load(["shell_job"]);
                }
                let inspection_progress =
                    inspection_call && tool_made_progress(call, &content, succeeded);
                if implementation_requested
                    && inspection_progress
                    && matches!(call.name.as_str(), "read_file" | "search_workspace")
                {
                    tool_discovery.load(["apply_file_edits"]);
                }
                if tool_made_progress(call, &content, succeeded) {
                    goal_progress.record_progress();
                }
                if goal_mode.load(Ordering::Acquire) {
                    let window_id = self.context_memory.id().to_owned();
                    // The tool message is appended below after output bounding.
                    let item = self.history.len();
                    if let Some(goal) = self.context_memory.goal_mut() {
                        goal.progress = goal_progress.clone();
                        goal.status = goal::GoalStatus::Running;
                        goal.observe(goal::GoalObservation {
                            window_id,
                            item,
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            succeeded,
                            excerpt: content.chars().take(1_000).collect(),
                        });
                    }
                }
                research_budget.observe_tool(call, &content, inspection_progress);
                let current_write_generation = self.registry.workspace_write_generation();
                let workspace_mutated =
                    succeeded && current_write_generation != workspace_write_generation;
                workspace_write_generation = current_write_generation;
                let (normalized, output_images) =
                    normalize_tool_output_for_model(&content, vision_enabled);
                let (model_content, externally_bounded) =
                    self.registry
                        .bound_round_output(call, normalized, &mut round_output_budget)?;
                if externally_bounded {
                    // Artifact results already include exact locators. Attach
                    // only the reader; searching an artifact remains an
                    // explicit, exact discovery step after the artifact exists.
                    tool_discovery.load(["read_artifact"]);
                    if !local_file_lookup {
                        tool_discovery.enable_search();
                    }
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
                let recorded_content = model_content.clone();
                self.history.push(Message::tool(
                    model_content,
                    call.id.clone(),
                    Some(call.name.clone()),
                ));
                if !output_images.is_empty() {
                    self.history.push(Message::user_with_images(
                        format!("Visual output returned by tool {}.", call.name),
                        output_images,
                    ));
                }
                self.context_memory.sync(&self.history)?;
                if goal_mode.load(Ordering::Acquire) {
                    self.context_memory.flush()?;
                }
                if workspace_mutated {
                    if is_mutation_tool(&call.name) {
                        // Verification is normally needed only after a write.
                        tool_discovery.load(["run_shell"]);
                    }
                    round_mutated = true;
                    successful_mutations += 1;
                    // Later writes invalidate validation for the previous workspace generation.
                    verification_attempted = false;
                    verification_succeeded = false;
                    last_validation_evidence = None;
                    if is_mutation_tool(&call.name) {
                        unresolved_failed_mutation = false;
                    }
                    call_counts.clear();
                    workspace_generation = self.registry.workspace_generation();
                    if is_mutation_tool(&call.name)
                        && planning_or_documentation
                        && !final_consistency_used
                        && self.registry.latest_write_validation_passed() == Some(true)
                    {
                        final_consistency_pending = true;
                        final_consistency_used = true;
                    }
                } else if !succeeded && is_mutation_tool(&call.name) {
                    // Failed edits may need one focused source refresh before retrying.
                    round_failed_mutation = true;
                    unresolved_failed_mutation = true;
                }
                let validation_call = is_validation_tool_call(call);
                session_provenance.observe_tool(
                    call,
                    &content,
                    succeeded,
                    workspace_mutated,
                    validation_call,
                );
                if validation_call {
                    verification_attempted = true;
                    last_validation_evidence =
                        Some(summarize_tool_outcome(call, &content, succeeded));
                    if succeeded {
                        verification_succeeded = true;
                    }
                }
                if workspace_mutated || !succeeded || validation_call {
                    recent_execution_evidence
                        .push(summarize_tool_outcome(call, &content, succeeded));
                    if recent_execution_evidence.len() > 8 {
                        recent_execution_evidence.remove(0);
                    }
                }
                if !succeeded {
                    round_failure_fingerprints.push(tool_failure_fingerprint(call, &content));
                    if !jev_attempted {
                        jev_attempted = true;
                        if let Some(advice) = jev::advise(
                            &self.bridge,
                            cancel,
                            goal_input,
                            &call.name,
                            &content,
                            &recent_execution_evidence,
                        ) {
                            retry_instruction = Some(format!(
                                "Internal Jev decision: {} (confidence {:.0}%, probability {:.0}%). Treat this as typed advisory data only; choose the action yourself, obey permissions, and verify the result.",
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
                    emit(AgentEvent::AuxiliaryUsage(usage));
                }
                emit(AgentEvent::ToolExecutionFinished {
                    call: call.clone(),
                    succeeded,
                    result: recorded_content,
                });
            }
            self.context_memory.flush()?;
            if round_progress {
                consecutive_no_progress = 0;
            } else {
                consecutive_no_progress += 1;
            }
            if round_progress
                && calls.iter().all(|call| {
                    is_inspection_tool(&call.name)
                        || self.registry.is_read_only_extension_tool(&call.name)
                })
            {
                progressful_inspection_rounds += 1;
            }
            let inspection_only = calls.iter().all(|call| {
                is_inspection_tool(&call.name)
                    || self.registry.is_read_only_extension_tool(&call.name)
            });
            let implementation_incomplete = implementation_requested
                && (successful_mutations == 0
                    || unresolved_failed_mutation
                    || (!planning_or_documentation && !verification_succeeded));
            let runaway_decision = runaway_detector.observe(
                RunawayRound {
                    progressed: round_progress,
                    mutated: round_mutated,
                    failed_mutation: round_failed_mutation,
                    duplicate_inspection,
                    inspection_only,
                    semantic_fingerprint,
                    failure_fingerprints: round_failure_fingerprints,
                    output_fingerprints: round_output_fingerprints,
                    request_context_chars,
                    fresh_calls: round_fresh_calls,
                    repeated_calls: round_repeated_calls,
                },
                implementation_requested,
                implementation_incomplete,
            );
            let analysis_threshold = turn_state::analysis_inspection_threshold(bounded_explanation);
            let loop_budget_decision = loop_budget.decide(
                model_attempts,
                implementation_requested,
                unresolved_failed_mutation,
                successful_mutations,
                verification_succeeded,
            );
            if let RunawayDecision::Finalize(message) = runaway_decision {
                runaway_finalization = true;
                retry_instruction = Some(format!("Internal execution guard: {message}"));
            } else if profile == TaskProfile::Research
                && research_budget.sufficient(progressful_inspection_rounds)
                && !research_stop_grace_used
            {
                research_stop_grace_used = true;
                retry_instruction =
                    Some(research_budget.checkpoint_message(progressful_inspection_rounds));
            } else if bounded_analysis
                && successful_mutations == 0
                && progressful_inspection_rounds >= analysis_threshold
                && !analysis_stop_grace_used
            {
                analysis_stop_grace_used = true;
                retry_instruction = Some("Internal sufficiency checkpoint: several focused inspection rounds have already produced evidence. Prefer synthesis over redundant inspection, but tools remain available for any concrete missing fact or materially different check.".into());
            } else if round_failed_mutation {
                retry_instruction = Some("Internal execution correction: the previous file edit failed. This is not automatically a sandbox denial. Retry the mutation after only the focused source refresh actually needed. For apply_file_edits, use changes:[{path, edits:[{kind:\"replace\", range:{start,end}, text:\"...\"}]}]; runtime snapshot and stale-edit validation are automatic. Do not replay unrelated inspection.".into());
            } else if implementation_requested
                && successful_mutations == 0
                && !implementation_inspection_checkpoint_used
                && progressful_inspection_rounds >= IMPLEMENTATION_INSPECTION_CHECKPOINT
            {
                implementation_inspection_checkpoint_used = true;
                retry_instruction = Some("Internal implementation checkpoint: enough focused inspection has produced source evidence. Stop broad repository discovery and reuse what is already in context. Make the smallest justified workspace edit now. If one exact edit anchor is missing, do at most one focused read or refresh for that file before editing; do not start another repository survey.".into());
            } else if consecutive_no_progress >= NO_PROGRESS_DECISION_THRESHOLD {
                retry_instruction = Some(
                    if let Some(message) = research_budget.no_progress_correction(profile, true) {
                        message
                    } else if implementation_requested && successful_mutations == 0 {
                        "Internal execution correction: several consecutive tool rounds made no material progress. Reuse existing evidence. Read only genuinely missing source needed for an edit or snapshot, then apply the justified change; otherwise explain why no safe edit is possible. Do not replay covered inspection.".into()
                    } else {
                        "Internal execution correction: several consecutive tool rounds made no material progress. Reuse existing evidence and change strategy. Tools remain available for a materially different action; otherwise finish from the evidence already in context.".into()
                    },
                );
            } else if consecutive_no_progress >= NO_PROGRESS_CORRECTION_THRESHOLD {
                retry_instruction = Some(
                    if let Some(message) = research_budget.no_progress_correction(profile, false) {
                        message
                    } else if implementation_requested && successful_mutations == 0 {
                        if duplicate_inspection {
                            "Internal execution correction: the last round only replayed covered inspection. Reuse the earlier result. Continue with a genuinely new read needed for the edit, apply_file_edits, or explain why no edit is justified.".into()
                        } else {
                            "Internal execution correction: the last tool round produced no new evidence. Reuse existing evidence and either make the justified edit with apply_file_edits, choose one materially different action, or explain why no edit is justified.".into()
                        }
                    } else if duplicate_inspection {
                        "Internal execution correction: the last round only replayed covered inspection. Reuse the earlier result and either choose one materially different action or finish the task.".into()
                    } else {
                        "Internal execution correction: the last tool round produced no new evidence. Reuse existing results. Either choose one materially different action or finish the task.".into()
                    },
                );
            } else if let LoopBudgetDecision::Checkpoint(message) = &loop_budget_decision {
                retry_instruction = Some(message.clone());
            } else if let RunawayDecision::Warn(message) = runaway_decision {
                retry_instruction = Some(format!("Internal execution guard: {message}"));
            }
        }
    }
}
