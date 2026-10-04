//! Durable Goal job lifecycle, API recovery, and strict completion judging.

use super::goal::{
    GOAL_JUDGE_SYSTEM_INSTRUCTION, GoalVerdict, parse_goal_verdict, render_goal_evidence,
};
use super::*;
use crate::core::Usage;
use uuid::Uuid;

impl AgentCoordinator {
    pub fn run<F>(&mut self, request: AgentRunRequest<'_>, mut emit: F) -> Result<AgentRunOutcome>
    where
        F: FnMut(AgentEvent),
    {
        let registry = crate::agents::global();
        let workspace = self.registry.workspace_root().to_path_buf();
        let id = match self.runtime_agent_id {
            Some(id) => id,
            None => {
                let id = registry.register("Yeet", request.model, workspace.clone(), None)?;
                self.runtime_agent_id = Some(id);
                id
            }
        };
        let _run = registry.begin_run(id, request.model, workspace)?;
        self.registry.bind_runtime_agent(id);
        let AgentRunRequest {
            input,
            images,
            model,
            reasoning_level,
            attached_capabilities,
            disabled_capabilities,
            goal_mode,
            cancel,
            continuation: initial_continuation,
            max_output_tokens,
        } = request;
        self.output_token_cap = max_output_tokens;
        self.output_checkpoint_threshold = self
            .output_token_cap
            .as_ref()
            .map(|cap| (cap.load(Ordering::Acquire) / 4).clamp(256, 2_048));
        self.output_checkpoint_prompted = false;
        self.context_memory.load(&mut self.history)?;
        settle_interrupted_context_batch(&mut self.history);
        let goal_input = if initial_continuation {
            self.history
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::User)
                .and_then(|message| message.content.as_deref())
                .unwrap_or(input)
                .to_owned()
        } else {
            input.to_owned()
        };
        let running_goal = goal_mode.load(Ordering::Acquire);
        let goal_input = if running_goal {
            if initial_continuation
                && self
                    .context_memory
                    .goal()
                    .is_some_and(|goal| goal.status == goal::GoalStatus::Succeeded)
            {
                goal_mode.store(false, Ordering::Release);
                return Ok(AgentRunOutcome::Completed);
            }
            let objective = self
                .context_memory
                .start_goal(&goal_input, initial_continuation);
            self.context_memory.sync(&self.history)?;
            self.context_memory.flush()?;
            objective
        } else {
            goal_input
        };
        self.context_memory.capacity = self
            .bridge
            .context_length(model)
            .ok()
            .flatten()
            .filter(|capacity| *capacity > 0);
        let task_id = Uuid::new_v4().to_string();
        self.registry
            .set_disabled_capabilities(disabled_capabilities);
        self.sync_attached_skills(attached_capabilities.as_deref())?;
        self.sync_attached_skyline(attached_capabilities.as_deref())?;
        self.registry.attach_enabled_mcp_servers()?;
        self.registry.begin_task(&task_id);
        let mut continuation = initial_continuation;
        let mut retry_attempt = 0u32;
        let goal_retry_reason: Option<String> = None;
        let result = loop {
            let attempt = self.run_turn(
                AgentTurnRequest {
                    input,
                    images: if continuation {
                        Vec::new()
                    } else {
                        images.clone()
                    },
                    model,
                    reasoning_level,
                    attached_capabilities: attached_capabilities.clone(),
                    cancel: &cancel,
                    continuation,
                    goal_retry_reason: goal_retry_reason.as_deref(),
                },
                &goal_mode,
                &goal_input,
                &mut |event| {
                    if let AgentEvent::GoalJudge { passed, reason } = &event {
                        let _ = registry.record_decision(
                            id,
                            crate::agents::AgentDecision {
                                summary: format!("Goal judge passed={passed}: {reason}"),
                            },
                        );
                    }
                    // Successful tool execution separates independent outages. Judge
                    // responses alone must not reset a persistently failing judge lane.
                    if matches!(
                        &event,
                        AgentEvent::ToolExecutionFinished {
                            succeeded: true,
                            ..
                        }
                    ) {
                        retry_attempt = 0;
                    }
                    emit(event);
                },
            );

            match attempt {
                Ok(outcome) => break Ok(outcome),
                Err(error) => {
                    if cancel.load(Ordering::Acquire) {
                        break Err(error);
                    }
                    let message = error.to_string();
                    if !retryable_goal_error(&message) {
                        if goal_mode.load(Ordering::Acquire) {
                            break Ok(AgentRunOutcome::GoalPaused { reason: message });
                        }
                        break Err(error);
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    let Some(delay) = goal_retry_delay(retry_attempt) else {
                        if goal_mode.load(Ordering::Acquire) {
                            break Ok(AgentRunOutcome::GoalPaused {
                                reason: format!(
                                    "API recovery exhausted; resume after resolving: {message}"
                                ),
                            });
                        }
                        break Err(error);
                    };
                    if bridge_transport_error(&message) {
                        let _ = self
                            .bridge
                            .restart()
                            .and_then(|_| self.registry.attach_enabled_mcp_servers());
                    }
                    if running_goal && let Some(goal) = self.context_memory.goal_mut() {
                        goal.status = goal::GoalStatus::Recovering;
                        goal.reason = Some(message.clone());
                    }
                    emit(AgentEvent::GoalRetry {
                        attempt: retry_attempt,
                        delay_ms: delay.as_millis().min(u128::from(u64::MAX)) as u64,
                        error: message,
                    });
                    let _ = self.context_memory.sync(&self.history);
                    let _ = self.context_memory.flush();
                    continuation = true;
                    // API recovery also applies outside Goal mode. Cancellation still
                    // interrupts the cooldown immediately (within the polling interval).
                    let was_goal = goal_mode.load(Ordering::Acquire);
                    let always_enabled = AtomicBool::new(true);
                    if !wait_for_goal(
                        &cancel,
                        if was_goal {
                            &goal_mode
                        } else {
                            &always_enabled
                        },
                        delay,
                    ) {
                        if cancel.load(Ordering::Acquire) {
                            break Err(anyhow!("cancelled"));
                        }
                        break Err(error);
                    }
                }
            }
        };
        if running_goal && let Some(goal) = self.context_memory.goal_mut() {
            match &result {
                Ok(AgentRunOutcome::Completed) if goal.status == goal::GoalStatus::Succeeded => {}
                Ok(AgentRunOutcome::GoalPaused { reason })
                | Ok(AgentRunOutcome::CompletedUnverified { reason }) => {
                    goal.status = goal::GoalStatus::Paused;
                    goal.reason = Some(reason.clone());
                }
                Err(error) => {
                    goal.status = if cancel.load(Ordering::Acquire) {
                        goal::GoalStatus::Cancelled
                    } else {
                        goal::GoalStatus::Paused
                    };
                    goal.reason = Some(error.to_string());
                }
                _ => {
                    goal.status = goal::GoalStatus::Paused;
                }
            }
        }
        let _ = registry.record_decision(
            id,
            crate::agents::AgentDecision {
                summary: match &result {
                    Ok(outcome) => format!("Run outcome: {outcome:?}"),
                    Err(error) => format!("Run failed: {error}"),
                },
            },
        );
        self.previous_turn_working_state = self.registry.working_state_summary();
        self.registry.finish_task(&task_id);
        settle_interrupted_context_batch(&mut self.history);
        self.context_memory.sync(&self.history)?;
        self.context_memory.flush()?;
        result
    }

    pub(super) fn judge_goal<F>(
        &self,
        input: &str,
        model: &str,
        reasoning_level: &str,
        execution_evidence: &turn_state::TurnExecutionEvidence,
        cancel: &AtomicBool,
        emit: &mut F,
    ) -> Result<(GoalVerdict, Usage, usize)>
    where
        F: FnMut(AgentEvent),
    {
        check_cancel(cancel)?;
        const FORMAT_REPAIR_LIMIT: usize = 2;
        let current_tool_observations = render_goal_evidence(&self.history);
        let retained = self
            .context_memory
            .goal()
            .map(|goal| serde_json::to_string(&goal.observations).unwrap_or_default())
            .unwrap_or_default();
        let execution = serde_json::to_string_pretty(&execution_evidence.goal_evidence())
            .unwrap_or_else(|_| "{}".into());
        let evidence = format!(
            "AUTHORITATIVE EXECUTION STATE (coordinator-observed facts):\n{execution}\n\nRETAINED TOOL OBSERVATIONS (bounded excerpts with exact window/item locators):\n{retained}\n\nCURRENT TOOL OBSERVATIONS (tool messages only; no assistant claims):\n{current_tool_observations}"
        );
        let base_messages = vec![
            Message::system(GOAL_JUDGE_SYSTEM_INSTRUCTION),
            Message::user(format!("GOAL:\n{input}\n\nWORKER EVIDENCE:\n{evidence}")),
        ];
        let mut previous_invalid_response: Option<String> = None;
        let mut judge_usage = Usage::default();
        let mut judge_request_chars = 0usize;

        for format_repair_attempt in 0..=FORMAT_REPAIR_LIMIT {
            check_cancel(cancel)?;
            let mut messages = base_messages.clone();
            if format_repair_attempt > 0 {
                if let Some(previous) = previous_invalid_response.as_deref()
                    && !previous.trim().is_empty()
                {
                    messages.push(Message::assistant(previous.to_owned(), None));
                }
                messages.push(Message::user(
                    "Judge protocol repair: the previous response was not valid JSON. Return exactly one JSON object matching the required verdict/evidence/remaining schema. No markdown, prose, or code fences.",
                ));
            }

            let mut request = CallRequest::simple(model, messages);
            request.temperature = Some(0.0);
            request.max_tokens = Some(
                self.output_token_cap
                    .as_ref()
                    .map_or(2_048, |cap| cap.load(Ordering::Acquire).max(256).min(2_048)),
            );
            request.timeout_ms = Some(MODEL_ATTEMPT_TIMEOUT_MS);
            let mut request_metadata = HashMap::from([
                ("lane".into(), "goal-judge".into()),
                ("strict".into(), "true".into()),
                ("agentId".into(), "goal-judge".into()),
                ("yeetVersion".into(), env!("CARGO_PKG_VERSION").to_owned()),
                (
                    "formatRepairAttempt".into(),
                    format_repair_attempt.to_string(),
                ),
            ]);
            if reasoning_level != "auto" {
                request_metadata.insert("reasoningLevel".into(), reasoning_level.to_owned());
            }
            request.metadata = Some(request_metadata);
            emit(AgentEvent::ModelAttemptStarted {
                diagnostics: json!({
                    "lane":"goal-judge",
                    "strict":true,
                    "formatRepairAttempt":format_repair_attempt
                }),
            });
            judge_request_chars = judge_request_chars.saturating_add(
                serde_json::to_vec(&request)
                    .map(|serialized| serialized.len())
                    .unwrap_or_default(),
            );
            let result = self.bridge.complete_cancellable(&request, cancel)?;
            emit(AgentEvent::ModelAttemptFinished(
                json!({
                    "lane":"goal-judge",
                    "strict":true,
                    "formatRepairAttempt":format_repair_attempt
                }),
                result.usage.clone(),
            ));
            if let Some(usage) = result.usage.as_ref() {
                judge_usage.accumulate(usage);
                emit(AgentEvent::AuxiliaryUsage {
                    usage: usage.clone(),
                    already_counted_calls: 1,
                });
            }
            let verdict = parse_goal_verdict(&result.text);
            let protocol_error = verdict
                .reason
                .starts_with("strict goal judge returned invalid JSON:");
            if !protocol_error || format_repair_attempt == FORMAT_REPAIR_LIMIT {
                return Ok((verdict, judge_usage, judge_request_chars));
            }
            previous_invalid_response = Some(result.text.chars().take(4_096).collect::<String>());
        }

        unreachable!("Goal judge format-repair loop always returns a verdict")
    }
}
