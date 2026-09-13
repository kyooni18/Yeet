//! Agent-event projection and durable session persistence.
//!
//! This module translates streaming agent events into UI/session state and
//! owns the canonical conversion from a live session into persisted storage.

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
        AgentEvent::ReasoningDelta(_)
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
        AgentEvent::AuxiliaryUsage(usage) => {
            Some(json!({"type":"agent-auxiliary-usage", "usage":usage}))
        }
        AgentEvent::InfinityCheckpoint { epoch, reason } => Some(json!({
            "type":"agent-infinity-checkpoint",
            "epoch":epoch,
            "reason":reason,
        })),
        AgentEvent::InfinityRetry {
            attempt,
            delay_ms,
            error,
        } => Some(json!({
            "type":"agent-infinity-retry",
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
        AgentEvent::ReasoningDelta(delta) => {
            state.set_activity("reasoning", "Reasoning", None);
            state.append_reasoning(&delta, false);
        }
        AgentEvent::ReasoningSummaryDelta(delta) => {
            state.set_activity("reasoning", "Reasoning", None);
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
                        status: ToolCallStatus::Streaming,
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
            state.set_activity("tool", "Preparing", Some(call_name));
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
                        status: ToolCallStatus::Streaming,
                    });
                pending.index = Some(index as i64);
                pending.call_id = Some(call.id.clone());
                pending.name = call.name.clone();
                pending.arguments = pretty_json(&call.arguments);
                pending.status = ToolCallStatus::Streaming;
                pending.clone()
            };
            state.sync_tool_call(rendered);
            state.set_activity("tool", "Preparing", Some(call.name));
        }
        AgentEvent::ToolExecutionStarted(call) => {
            state.set_tool_call_execution_status(&call, ToolCallStatus::Streaming);
            state.set_activity("tool", &tool_activity_title(&call.name), tool_detail(&call));
        }
        AgentEvent::ToolExecutionFinished {
            call,
            succeeded,
            result: _,
        } => {
            state.set_tool_call_execution_status(
                &call,
                if succeeded {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                },
            );
            state.set_activity(
                if succeeded {
                    "tool-complete"
                } else {
                    "tool-failed"
                },
                if succeeded { "Completed" } else { "Failed" },
                tool_detail(&call),
            );
        }
        AgentEvent::ToolExecutionSuppressed { call, reason } => {
            state.set_tool_call_execution_status(&call, ToolCallStatus::Suppressed);
            state.set_activity("tool-suppressed", "Suppressed", Some(reason));
        }
        AgentEvent::AuxiliaryUsage(usage) => record_usage(&mut state.state, &usage, 0),
        AgentEvent::InfinityCheckpoint { epoch, reason } => {
            state.set_activity(
                "infinity",
                "Infinity · continuing",
                Some(format!("Epoch {epoch} · {reason}")),
            );
        }
        AgentEvent::InfinityRetry {
            attempt,
            delay_ms,
            error,
        } => {
            state.set_activity(
                "retrying",
                "Infinity · retrying",
                Some(format!(
                    "Attempt {attempt} · retry in {:.1}s · {error}",
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

/// Accumulates token usage and provider-call credits into bridge state.
pub(super) fn record_usage(state: &mut BridgeState, usage: &Usage, already_counted_calls: u64) {
    let represented = usage.model_calls.unwrap_or(1).max(1);
    state.credit_usage = state
        .credit_usage
        .saturating_add(represented.saturating_sub(already_counted_calls));
    state.token_usage.accumulate(usage);
}

/// Persists the current live session while holding its state lock.
pub(super) fn persist_locked(
    state: &mut SharedSession,
    store: &SessionStore,
    workspace: &Path,
    history: Vec<Message>,
) -> Result<()> {
    let conversation = state.state.conversation.clone().unwrap_or_default();
    if !conversation
        .iter()
        .any(|entry| matches!(entry.kind, ConversationKind::User { .. }))
        && state.state.current_session_id.is_none()
    {
        return Ok(());
    }
    let id = state
        .state
        .current_session_id
        .clone()
        .unwrap_or_else(SessionStore::new_id);
    let created_at = state.meta.created_at.unwrap_or_else(Utc::now);
    let title = state
        .meta
        .title
        .clone()
        .unwrap_or_else(|| fallback_title(&conversation));
    state.state.current_session_id = Some(id.clone());
    state.meta.created_at = Some(created_at);
    state.meta.title = Some(title.clone());
    store.save(&StoredSession {
        version: 4,
        debate: state.state.debate.clone(),
        id: id.clone(),
        title,
        created_at,
        updated_at: Utc::now(),
        workspace_root: workspace.display().to_string(),
        model: state.state.active_model.clone(),
        token_usage: state.state.token_usage.clone(),
        credit_usage: state.state.credit_usage,
        conversation,
        model_history: storage_model_history(history),
        runs: state.meta.runs.clone(),
        retained_debate_knowledge: state.meta.retained_debate_knowledge.clone(),
        attached_harness_capabilities: state.meta.attached_capabilities.clone(),
        disabled_capabilities: state.meta.disabled_capabilities.clone(),
    })?;
    store.set_infinity_mode(&id, state.state.infinity_mode)
}

/// Removes the coordinator's internal coding prompt before session persistence.
pub(super) fn storage_model_history(mut history: Vec<Message>) -> Vec<Message> {
    if history
        .first()
        .is_some_and(is_internal_coordinator_system_message)
    {
        history.remove(0);
    }
    history
}
