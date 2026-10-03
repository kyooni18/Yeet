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
mod phase;
mod policy;
mod progress;
mod repo_context;
mod runaway;
mod session;
mod session_controls;
mod tool_discovery;
mod tool_protocol;
mod turn_setup;
mod turn_state;
mod verify;
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
use loop_budget::LoopBudget;
pub use policy::SYSTEM_INSTRUCTION;
use policy::{
    ResearchBudget, TaskProfile, is_mutation_tool, looks_like_bounded_analysis,
    looks_like_bounded_explanation, looks_like_capability_request,
    looks_like_coding_implementation_request, looks_like_coding_request,
    looks_like_local_file_lookup, looks_like_planning_or_documentation,
    looks_like_prior_context_request, request_history_for_profile_at, select_tools_for_profile,
    should_preserve_web_tool_surface, task_guidance, task_profile_with_history,
};
use progress::{
    classify_tool_error, content_fingerprint, is_inspection_tool, round_semantic_fingerprint,
    tool_execution_succeeded, tool_failure_fingerprint, tool_made_progress, tool_signature,
};
use runaway::{RunawayDecision, RunawayDetector, RunawayRound};
use session_controls::{
    bridge_transport_error, goal_retry_delay, retryable_goal_error,
    tool_call_indicates_implementation_intent, wait_for_goal,
};
use tool_protocol::{
    PartialToolCall, collect_tool_calls, looks_like_malformed_tool_call,
    normalize_tool_output_for_model, recover_text_tool_calls,
};

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::{
    core::{CallRequest, Message, MessageRole, StreamEvent, StreamPoll, ToolCall, ToolDefinition},
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
            let mut tool_catalog = match profile {
                TaskProfile::Research => self.registry.research_tools(),
                TaskProfile::Agent => {
                    select_tools_for_profile(self.registry.tools(web_search_enabled), profile)
                }
            };
            tool_catalog.extend(context::tools());
            let selected_tools = tool_discovery.attached(&tool_catalog);
            let working_budget = self.context_memory.working_budget();
            let jev_loop = jev::apply_loop_policy(
                &bridge,
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

            let capability_snapshot = self.registry.runtime_capability_snapshot(&selected_tools);
            self.observation_cache.bind_window(self.context_memory.id());
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
            loop_budget.observe_request(request_context_chars);
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
            let execution_phase = execution_evidence
                .recommended_phase(implementation_requested, planning_or_documentation);
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
                    execution_phase: execution_phase.as_str(),
                    successful_mutations: execution_evidence.successful_mutations(),
                    unresolved_failed_mutation: execution_evidence.unresolved_failed_mutation(),
                    verification_attempted: execution_evidence.verification_attempted(),
                    verification_succeeded: execution_evidence.verification_succeeded(),
                    turn_start_pruned_request_only_messages,
                    turn_start_pruned_request_only_chars,
                });
            jev::write_loop_metadata(
                &mut request_metadata,
                jev_loop_advice.as_ref(),
                jev_attempted_this_round,
                jev_request_chars,
            );
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
            let mut attempt_cache_diagnostics = self.cache_continuity.diagnostics(&request);
            let update_plans = self.observation_cache.take_plans();
            if !update_plans.is_empty() {
                attempt_cache_diagnostics["contextCacheUpdates"] = json!(update_plans);
            }
            if attempt_cache_diagnostics["wireHistoryPrefixRewriteDetected"] == json!(true) {
                bail!(
                    "Previously submitted context changed within this window; start an explicit context rollover instead of rewriting history."
                );
            }
            let sent_request_chars = serde_json::to_vec(&request)
                .map(|serialized| serialized.len())
                .unwrap_or(request_context_chars);
            loop_budget.record_sent_request(sent_request_chars);
            emit(AgentEvent::ModelAttemptStarted {
                diagnostics: attempt_cache_diagnostics.clone(),
            });

            self.observation_cache.mark_submitted();
            let mut stream = bridge.stream(&request)?;
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
                    StreamEvent::ReasoningStart => emit(AgentEvent::ReasoningStart),
                    StreamEvent::Activity { title, detail } => {
                        emit(AgentEvent::ProviderActivity { title, detail })
                    }
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
                let inspection_call = is_inspection_tool(&call.name)
                    || self.registry.is_read_only_extension_tool(&call.name);
                let local_lookup_complete = local_file_lookup
                    && local_lookup_read_calls >= 3
                    && (!local_lookup_read_externalized
                        || local_lookup_read_calls >= 4
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
                research_budget.observe_tool(call, &content, inspection_progress);
                let current_write_generation = self.registry.workspace_write_generation();
                let workspace_write_observed =
                    current_write_generation != workspace_write_generation;
                let workspace_mutated = succeeded && workspace_write_observed;
                workspace_write_generation = current_write_generation;
                let (normalized, output_images) =
                    normalize_tool_output_for_model(&content, vision_enabled);
                let (mut model_content, externally_bounded) =
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
                if succeeded
                    && call.name == "read_file"
                    && !externally_bounded
                    && self.observation_cache.prepare_read_update(
                        &mut self.history,
                        &mut model_content,
                        model,
                        cache::ObservationPolicy {
                            prefix_overhead_tokens: (serde_json::to_vec(&(
                                &request.tools,
                                &request.deferred_tools,
                            ))?
                            .len() as u64)
                                .div_ceil(3),
                            may_retire: !goal_mode.load(Ordering::Acquire),
                        },
                        || {
                            let (provider, local_model) = model.split_once('/')?;
                            bridge
                                .list_model_info(provider)
                                .ok()?
                                .into_iter()
                                .find(|entry| entry.id == local_model || entry.id == model)?
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
                let mutation_tool = is_mutation_tool(&call.name);
                let validation_call = progress::is_validation_tool_result(call, &content);
                execution_evidence.observe_tool(
                    call,
                    &content,
                    succeeded,
                    workspace_write_observed,
                    validation_call,
                    mutation_tool,
                );
                if profile == TaskProfile::Agent {
                    let checkpoint_phase = execution_evidence
                        .recommended_phase(implementation_requested, planning_or_documentation);
                    let checkpoint_workspace_state = self.registry.working_state_summary();
                    self.context_memory.set_agent_checkpoint(
                        goal_input,
                        checkpoint_phase.as_str(),
                        execution_evidence.successful_mutations(),
                        execution_evidence.unresolved_failed_mutation(),
                        execution_evidence.verification_attempted(),
                        execution_evidence.verification_succeeded(),
                        execution_evidence.last_validation_evidence(),
                        execution_evidence.recent_evidence(),
                        checkpoint_workspace_state.as_deref(),
                    );
                }
                if goal_mode.load(Ordering::Acquire) {
                    let window_id = self.context_memory.id().to_owned();
                    // Retain the exact index of the tool message even when a
                    // visual payload appends a synthetic user message afterward.
                    let item = goal_observation_item;
                    if let Some(goal) = self.context_memory.goal_mut() {
                        goal.progress = goal_progress.clone();
                        goal.status = goal::GoalStatus::Running;
                        goal.observe(goal::GoalObservation {
                            window_id,
                            item,
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            succeeded,
                            workspace_mutated,
                            validation_call,
                            excerpt: content.chars().take(1_000).collect(),
                        });
                    }
                }
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
            self.context_memory.flush()?;
            if profile == TaskProfile::Agent && !local_file_lookup {
                self.warm_tool_names = tool_discovery.loaded_names().to_vec();
                self.warm_tool_search_enabled = tool_discovery.search_enabled();
            }
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
            let runaway_decision = runaway_detector.observe(
                RunawayRound {
                    progressed: round_progress,
                    mutated: round_mutated,
                    duplicate_inspection,
                    inspection_only,
                    semantic_fingerprint,
                    failure_fingerprints: round_failure_fingerprints,
                    output_fingerprints: round_output_fingerprints,
                    request_context_chars,
                    fresh_calls: round_fresh_calls,
                    repeated_calls: round_repeated_calls,
                },
                rollover_budget.saturating_mul(3) as usize,
            );
            let verification_note = verification.after_round(
                &mut self.registry,
                &mut execution_evidence,
                verify::RoundFacts {
                    mutated: round_mutated,
                    failed_mutation: round_failed_mutation,
                    rollover: matches!(runaway_decision, RunawayDecision::Rollover(_)),
                },
                cancel,
                emit,
            );
            if let Some(note) = verification_note {
                self.history.push(Message::system(note).request_only());
            }
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
