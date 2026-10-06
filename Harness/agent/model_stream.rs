//! Consumption of one provider stream into a model attempt.
//!
//! Forwards provider events to the agent event stream and accumulates the
//! attempt's text, tool calls and finish metadata. Once a response turns into
//! a tool round, provisional assistant prose already shown to the user is
//! retracted (`DiscardAssistantText`) and later text deltas are ignored, so a
//! tool round never also commits prose. Cancellation is observed between
//! polls and cancels the provider request.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Result, bail};

use super::{AgentEvent, tool_protocol::PartialToolCall};
use crate::core::{
    BridgeClient, CallRequest, ProviderState, StreamEvent, StreamPoll, ToolCall, Usage,
};

/// Everything one streamed model attempt produced.
pub(super) struct StreamedAttempt {
    /// Committable assistant text (empty once a tool call was seen).
    pub(super) text: String,
    /// Text already emitted to the user that may still need retraction.
    pub(super) emitted_text: String,
    pub(super) decoded: HashMap<usize, ToolCall>,
    pub(super) partial: HashMap<usize, PartialToolCall>,
    pub(super) finish_reason: String,
    pub(super) finish_usage: Option<Usage>,
    pub(super) finish_provider_state: Option<ProviderState>,
}

pub(super) fn consume<F>(
    bridge: &BridgeClient,
    request: &CallRequest,
    cancel: &AtomicBool,
    emit: &mut F,
) -> Result<StreamedAttempt>
where
    F: FnMut(AgentEvent),
{
    let mut stream = bridge.stream(request)?;
    let mut attempt = StreamedAttempt {
        text: String::new(),
        emitted_text: String::new(),
        decoded: HashMap::new(),
        partial: HashMap::new(),
        finish_reason: "stop".to_owned(),
        finish_usage: None,
        finish_provider_state: None,
    };
    let mut tool_call_seen = false;

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
                    attempt.text.push_str(&delta);
                    attempt.emitted_text.push_str(&delta);
                    emit(AgentEvent::TextDelta(delta));
                }
            }
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                enter_tool_round(&mut tool_call_seen, &mut attempt, emit);
                attempt
                    .partial
                    .entry(index)
                    .or_insert_with(|| PartialToolCall::new(index, id.clone(), name.clone()))
                    .apply(id.clone(), name.clone(), arguments_delta.clone());
                emit(AgentEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                });
            }
            StreamEvent::ToolCall { index, tool_call } => {
                enter_tool_round(&mut tool_call_seen, &mut attempt, emit);
                attempt.decoded.insert(index, tool_call.clone());
                emit(AgentEvent::ToolCall {
                    index,
                    call: tool_call,
                });
            }
            StreamEvent::Finish {
                finish_reason,
                usage,
                provider_state,
            } => {
                attempt.finish_reason = finish_reason;
                attempt.finish_usage = usage;
                attempt.finish_provider_state = provider_state;
            }
        }
    }
    Ok(attempt)
}

/// Switches the attempt into a tool round on its first tool-call event,
/// retracting provisional prose already shown to the user.
fn enter_tool_round<F>(tool_call_seen: &mut bool, attempt: &mut StreamedAttempt, emit: &mut F)
where
    F: FnMut(AgentEvent),
{
    if *tool_call_seen {
        return;
    }
    *tool_call_seen = true;
    if !attempt.emitted_text.is_empty() {
        emit(AgentEvent::DiscardAssistantText(std::mem::take(
            &mut attempt.emitted_text,
        )));
        attempt.text.clear();
    }
}
