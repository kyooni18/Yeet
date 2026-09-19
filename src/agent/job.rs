//! Durable Goal job lifecycle, API recovery, and strict completion judging.

use super::goal::{
    GOAL_JUDGE_SYSTEM_INSTRUCTION, GoalVerdict, parse_goal_verdict, render_goal_evidence,
};
use super::*;
use uuid::Uuid;

impl AgentCoordinator {
    pub fn run<F>(&mut self, request: AgentRunRequest<'_>, mut emit: F) -> Result<AgentRunOutcome>
    where
        F: FnMut(AgentEvent),
    {
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
        } = request;
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
        self.context_key = self.context_memory.id().to_owned();
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
                    if running_goal {
                        if let Some(goal) = self.context_memory.goal_mut() {
                            goal.status = goal::GoalStatus::Recovering;
                            goal.reason = Some(message.clone());
                        }
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
        if running_goal {
            if let Some(goal) = self.context_memory.goal_mut() {
                match &result {
                    Ok(AgentRunOutcome::Completed)
                        if goal.status == goal::GoalStatus::Succeeded => {}
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
        }
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
        cancel: &AtomicBool,
        emit: &mut F,
    ) -> Result<GoalVerdict>
    where
        F: FnMut(AgentEvent),
    {
        check_cancel(cancel)?;
        let recent_evidence = render_goal_evidence(&self.history);
        let retained = self
            .context_memory
            .goal()
            .map(|goal| serde_json::to_string(&goal.observations).unwrap_or_default())
            .unwrap_or_default();
        let evidence = format!(
            "Retained tool observations (excerpts; exact window/item locators included):\n{retained}\n\nCurrent observations:\n{recent_evidence}"
        );
        let mut request = CallRequest::simple(
            model,
            vec![
                Message::system(GOAL_JUDGE_SYSTEM_INSTRUCTION),
                Message::user(format!("GOAL:\n{input}\n\nWORKER EVIDENCE:\n{evidence}")),
            ],
        );
        request.temperature = Some(0.0);
        request.max_tokens = Some(2_048);
        request.timeout_ms = Some(MODEL_ATTEMPT_TIMEOUT_MS);
        let mut request_metadata = HashMap::from([
            ("lane".into(), "goal-judge".into()),
            ("strict".into(), "true".into()),
            ("agentId".into(), "goal-judge".into()),
            ("yeetVersion".into(), env!("CARGO_PKG_VERSION").to_owned()),
        ]);
        if reasoning_level != "auto" {
            request_metadata.insert("reasoningLevel".into(), reasoning_level.to_owned());
        }
        request.metadata = Some(request_metadata);
        emit(AgentEvent::ModelAttemptStarted {
            diagnostics: json!({"lane":"goal-judge", "strict":true}),
        });
        let result = self.bridge.complete_cancellable(&request, cancel)?;
        emit(AgentEvent::ModelAttemptFinished(
            json!({"lane":"goal-judge", "strict":true}),
            result.usage.clone(),
        ));
        if let Some(usage) = result.usage {
            emit(AgentEvent::AuxiliaryUsage(usage));
        }
        Ok(parse_goal_verdict(&result.text))
    }
}
