//! Agent-event projection.
//!
//! Translates streaming agent events into live UI/session state and into the
//! bounded records appended to the session event log. Converting a live
//! session into stored form lives in `persistence`.

use super::*;

/// Bounds large tool results before writing forensic event records.
fn bounded_event_result(result: &str) -> Value {
    const MAX_RESULT_CHARS: usize = 64 * 1024;
    if result.chars().count() <= MAX_RESULT_CHARS {
        return json!(result);
    }
    let head = result
        .chars()
        .take(MAX_RESULT_CHARS / 2)
        .collect::<String>();
    let tail = result
        .chars()
        .rev()
        .take(MAX_RESULT_CHARS / 2)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    json!({
        "truncated": true,
        "originalChars": result.chars().count(),
        "head": head,
        "tail": tail,
    })
}

/// Converts durable agent events into compact JSONL log records.
pub(super) fn agent_event_log_value(event: &AgentEvent) -> Option<Value> {
    match event {
        AgentEvent::ModelAttemptStarted { diagnostics } => Some(json!({
            "type":"agent-model-attempt-started",
            "cacheDiagnostics": diagnostics,
        })),
        AgentEvent::ModelAttemptFinished(diagnostics, usage) => Some(json!({
            "type":"agent-model-attempt-finished",
            "cacheDiagnostics": diagnostics,
            "usage": usage,
        })),
        AgentEvent::Start => Some(json!({"type":"agent-response-started"})),
        AgentEvent::ProviderActivity { title, detail } => Some(json!({
            "type": "agent-provider-activity", "title": title, "detail": detail,
        })),
        AgentEvent::ReasoningStart
        | AgentEvent::ReasoningDelta(_)
        | AgentEvent::ReasoningSummaryDelta(_)
        | AgentEvent::TextDelta(_)
        | AgentEvent::ToolCallDelta { .. } => None,
        AgentEvent::DiscardAssistantText(text) => {
            Some(json!({"type":"agent-text-discarded", "chars":text.chars().count()}))
        }
        AgentEvent::ToolCall { index, call } => {
            Some(json!({"type":"agent-tool-call", "index":index, "call":call}))
        }
        AgentEvent::ToolExecutionStarted(call) => {
            Some(json!({"type":"agent-tool-started", "call":call}))
        }
        AgentEvent::ToolExecutionFinished {
            call,
            succeeded,
            result,
        } => Some(json!({
            "type":"agent-tool-finished",
            "call":call,
            "succeeded":succeeded,
            "result":bounded_event_result(result),
        })),
        AgentEvent::ToolExecutionSuppressed { call, reason } => Some(json!({
            "type":"agent-tool-suppressed",
            "call":call,
            "reason":reason,
        })),
        AgentEvent::AuxiliaryUsage {
            usage,
            already_counted_calls,
        } => Some(json!({
            "type":"agent-auxiliary-usage",
            "usage":usage,
            "alreadyCountedCalls":already_counted_calls,
        })),
        AgentEvent::GoalCheckpoint { epoch, reason } => Some(json!({
            "type":"agent-goal-checkpoint",
            "epoch":epoch,
            "reason":reason,
        })),
        AgentEvent::GoalJudge { passed, reason } => Some(json!({
            "type":"agent-goal-judge",
            "passed":passed,
            "reason":reason,
        })),
        AgentEvent::GoalRetry {
            attempt,
            delay_ms,
            error,
        } => Some(json!({
            "type":"agent-goal-retry",
            "attempt":attempt,
            "delayMs":delay_ms,
            "error":error,
        })),
        AgentEvent::Finished { reason, usage } => {
            Some(json!({"type":"agent-response-finished", "reason":reason, "usage":usage}))
        }
    }
}

/// Applies one agent event to the live bridge state and transcript.
pub(super) fn apply_agent_event(state: &mut SharedSession, event: AgentEvent) {
    match event {
        AgentEvent::ModelAttemptStarted { .. } => {
            state.state.credit_usage = state.state.credit_usage.saturating_add(1)
        }
        AgentEvent::ModelAttemptFinished(..) => {}
        AgentEvent::Start => state.set_activity("thinking", "Thinking", None),
        AgentEvent::ProviderActivity { title, detail } => {
            state.set_activity("provider-tool", &title, detail);
        }
        AgentEvent::ReasoningStart => {
            state.seal_reasoning_segment();
            state.set_activity(
                "reasoning",
                "Thinking",
                Some("Waiting for a model summary".into()),
            );
        }
        AgentEvent::ReasoningDelta(delta) => {
            state.append_reasoning(&delta, false);
        }
        AgentEvent::ReasoningSummaryDelta(delta) => {
            state.append_reasoning(&delta, true);
        }
        AgentEvent::TextDelta(delta) => {
            if !delta.is_empty() {
                state.set_activity("responding", "Responding", None);
            }
            state.append_assistant_text(&delta);
        }
        AgentEvent::DiscardAssistantText(text) => state.discard_assistant_text(&text),
        AgentEvent::ToolCallDelta {
            index,
            id,
            name,
            arguments_delta,
        } => {
            let rendered = {
                let call = state
                    .meta
                    .pending_tool_calls
                    .entry(index)
                    .or_insert_with(|| ConversationToolCall {
                        id: Uuid::new_v4().to_string(),
                        index: Some(index as i64),
                        call_id: id.clone(),
                        name: name.clone().unwrap_or_else(|| "tool".into()),
                        arguments: String::new(),
                        status: ToolCallStatus::Preparing,

                        label: None,
                        detail: None,
                        started_at: None,
                        ended_at: None,
                        duration_ms: None,
                        attempt: None,
                        parent_call_id: None,
                        parallel_group_id: None,
                        job_id: None,
                        result: None,
                        error: None,
                    });
                if let Some(id) = id.filter(|value| !value.is_empty()) {
                    call.call_id = Some(id);
                }
                if let Some(name) = name.filter(|value| !value.is_empty()) {
                    call.name = name;
                }
                if let Some(delta) = arguments_delta {
                    call.arguments.push_str(&delta);
                }
                call.clone()
            };
            let call_name = rendered.name.clone();
            state.sync_tool_call(rendered);
            state.set_activity("tool", "Preparing", Some(tool_activity_title(&call_name)));
        }
        AgentEvent::ToolCall { index, call } => {
            let rendered = {
                let pending = state
                    .meta
                    .pending_tool_calls
                    .entry(index)
                    .or_insert_with(|| ConversationToolCall {
                        id: Uuid::new_v4().to_string(),
                        index: Some(index as i64),
                        call_id: Some(call.id.clone()),
                        name: call.name.clone(),
                        arguments: String::new(),
                        status: ToolCallStatus::Preparing,

                        label: None,
                        detail: None,
                        started_at: None,
                        ended_at: None,
                        duration_ms: None,
                        attempt: None,
                        parent_call_id: None,
                        parallel_group_id: None,
                        job_id: None,
                        result: None,
                        error: None,
                    });
                pending.index = Some(index as i64);
                pending.call_id = Some(call.id.clone());
                pending.name = call.name.clone();
                pending.arguments = pretty_json(&call.arguments);
                pending.status = ToolCallStatus::Preparing;
                pending.clone()
            };
            state.sync_tool_call(rendered);
            state.set_activity("tool", "Preparing", Some(tool_activity_title(&call.name)));
        }
        AgentEvent::ToolExecutionStarted(call) => {
            state.set_tool_call_execution_status(&call, ToolCallStatus::Preparing, None, None);
            state.set_activity("tool", &tool_activity_title(&call.name), tool_detail(&call));
        }
        AgentEvent::ToolExecutionFinished {
            call,
            succeeded,
            result,
        } => {
            let (result, error) = if succeeded {
                (Some(result), None)
            } else {
                (None, Some(result))
            };
            state.set_tool_call_execution_status(
                &call,
                if succeeded {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                },
                result,
                error,
            );
            // Derive presentation from the existing call, without another model turn.
            // Only successful execution gets a past-tense completion label.
            let (completed_title, failed_title) = match call.name.rsplit('.').next().unwrap_or("") {
                "web_search" => ("Searched web", "Web search failed"),
                "web_read" => ("Read web page", "Web page read failed"),
                "search_workspace" => ("Searched workspace", "Workspace search failed"),
                "read_file" => ("Read file", "File read failed"),
                "apply_file_edits" => ("Applied file edits", "File edits failed"),
                "run_shell" => ("Ran command", "Command failed"),
                "list_sessions" => ("Listed sessions", "Session listing failed"),
                "export_session" => ("Exported session", "Session export failed"),
                "activate_capability" => ("Activated capability", "Capability activation failed"),
                _ => ("Tool completed", "Tool failed"),
            };
            state.set_activity(
                if succeeded {
                    "tool-complete"
                } else {
                    "tool-failed"
                },
                if succeeded {
                    completed_title
                } else {
                    failed_title
                },
                tool_detail(&call),
            );
        }
        AgentEvent::ToolExecutionSuppressed { call, reason } => {
            state.set_tool_call_execution_status(
                &call,
                ToolCallStatus::Suppressed,
                Some(reason.clone()),
                None,
            );
            state.set_activity("tool-suppressed", "Suppressed", Some(reason));
        }
        AgentEvent::AuxiliaryUsage {
            usage,
            already_counted_calls,
        } => record_usage(&mut state.state, &usage, already_counted_calls),
        AgentEvent::GoalCheckpoint { epoch, reason } => {
            state.set_activity(
                "goal",
                "Goal · judging",
                Some(format!("Epoch {epoch} · {reason}")),
            );
        }
        AgentEvent::GoalJudge { passed, reason } => {
            state.set_activity(
                "goal",
                if passed {
                    "Goal · accepted"
                } else {
                    "Goal · rejected"
                },
                Some(reason),
            );
        }
        AgentEvent::GoalRetry {
            attempt,
            delay_ms,
            error,
        } => {
            state.set_activity(
                "retrying",
                "API · retrying",
                Some(format!(
                    "Retry {attempt} of 5 · retry in {:.1}s · {error}",
                    delay_ms as f64 / 1000.0
                )),
            );
        }
        AgentEvent::Finished { reason, usage } => {
            if let Some(usage) = usage {
                if let Some(input_tokens) = usage.input_tokens {
                    state.state.current_context_tokens = Some(input_tokens);
                }
                record_usage(&mut state.state, &usage, 1);
            }
            if reason == "tool_call" {
                state.set_activity("thinking", "Thinking", None);
            } else if reason != "error" {
                state.set_activity("finishing", "Finishing", None);
            }
        }
    }
}

/// Accumulates token usage and provider-call credits into harness state.
pub(super) fn record_usage(state: &mut HarnessState, usage: &Usage, already_counted_calls: u64) {
    let represented = usage.model_calls.unwrap_or(1).max(1);
    state.credit_usage = state
        .credit_usage
        .saturating_add(represented.saturating_sub(already_counted_calls));
    state.token_usage.accumulate(usage);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_phases_clear_stale_reasoning_without_inventing_a_transcript() {
        let mut session = SharedSession::new("claude-api/test".into(), "auto".into());
        apply_agent_event(&mut session, AgentEvent::ReasoningStart);
        assert!(session.state.active_reasoning_entry_id.is_none());
        apply_agent_event(
            &mut session,
            AgentEvent::ReasoningSummaryDelta("Checking sources".into()),
        );
        assert!(session.state.active_reasoning_entry_id.is_some());
        apply_agent_event(
            &mut session,
            AgentEvent::ProviderActivity {
                title: "Searching web".into(),
                detail: Some("Provider-managed tool".into()),
            },
        );
        assert!(session.state.active_reasoning_entry_id.is_none());
        assert!(session.state.active_reasoning_summary.is_empty());
        assert!(session.conversation_mut().iter().any(|entry| matches!(
            &entry.kind, ConversationKind::Activity { activity } if activity.title == "Searching web"
        )));
        apply_agent_event(&mut session, AgentEvent::ReasoningStart);
        assert!(session.state.active_reasoning_entry_id.is_none());
        apply_agent_event(&mut session, AgentEvent::TextDelta("Answer".into()));
        assert!(session.state.active_assistant_entry_id.is_some());
        assert!(session.state.active_reasoning_entry_id.is_none());
    }

    #[test]
    fn auxiliary_usage_does_not_double_count_a_started_model_call() {
        let mut state = HarnessState {
            credit_usage: 1,
            ..HarnessState::default()
        };
        let usage = Usage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            model_calls: Some(1),
            ..Usage::default()
        };

        record_usage(&mut state, &usage, 1);
        assert_eq!(state.credit_usage, 1);
        assert_eq!(state.token_usage.input_tokens, Some(100));
        assert_eq!(state.token_usage.output_tokens, Some(20));

        record_usage(&mut state, &usage, 0);
        assert_eq!(state.credit_usage, 2);
        assert_eq!(state.token_usage.input_tokens, Some(200));
        assert_eq!(state.token_usage.output_tokens, Some(40));
    }
}
