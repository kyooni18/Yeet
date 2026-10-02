//! Small coordinator helpers for turn scoping, cache/deferred capability checks, and recovery.

use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    core::{CallRequest, Message, MessageRole, ToolCall, ToolDefinition},
    web_search::CAPABILITY_ID as WEB_SEARCH_CAPABILITY_ID,
};

pub(crate) fn is_internal_coordinator_system_message(message: &Message) -> bool {
    message.role == MessageRole::System
        && message.content.as_deref().is_some_and(|content| {
            content.starts_with("You are Yeet's agent.")
                || content.starts_with("You are Yeet's coding agent.")
                || content.starts_with("You are Yeet's general agent.")
        })
}

pub(super) fn render_matching_debate_memory(
    knowledge: &[crate::debate::RetainedDebateKnowledge],
    workspace_root: &str,
    workspace_revision: Option<&str>,
) -> Option<String> {
    let matching = knowledge
        .iter()
        .filter(|memory| {
            let subject = &memory.subject_ref;
            if !subject.workspace_bound || subject.workspace_root != workspace_root {
                return false;
            }
            match (subject.revision.as_deref(), workspace_revision) {
                (Some(expected), Some(current)) => expected == current,
                _ => false,
            }
        })
        .map(|memory| {
            json!({
                "sourceDebateRunId": memory.source_debate_run_id,
                "subject": memory.subject_ref,
                "confidence": memory.confidence,
                "supportedClaims": memory.supported_claims,
                "strongInferences": memory.strong_inferences,
                "unresolved": memory.unresolved,
            })
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return None;
    }
    Some(format!(
        "Typed retained debate knowledge for this exact workspace revision: {}. Supported claims are provenance-scoped to their evidence IDs. Strong inferences and unresolved items are not facts. If fresh source evidence conflicts with this memory, prefer the fresh source evidence.",
        serde_json::to_string(&matching).unwrap_or_else(|_| "[]".into()),
    ))
}

pub(super) fn explicit_skill_name(input: &str) -> Option<&str> {
    let token = input.split_whitespace().next()?;
    let name = token.strip_prefix('$')?;
    if name.is_empty() || name.len() > 64 || name.chars().next()?.is_ascii_digit() {
        return None;
    }
    name.split('-')
        .all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
        .then_some(name)
}

pub(super) fn supports_native_deferred_tools(model: &str) -> bool {
    let Some((provider, local_model)) = model.split_once('/') else {
        return false;
    };
    if provider == "anthropic" {
        return supports_anthropic_deferred_tool_references(model);
    }
    if provider != "openai" {
        return false;
    }
    let Some(version) = local_model.strip_prefix("gpt-") else {
        return false;
    };
    let numeric = version
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .collect::<Vec<_>>();
    let Ok(major) = numeric.first().copied().unwrap_or_default().parse::<u64>() else {
        return false;
    };
    let minor = numeric
        .get(1)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if major > 5 || (major == 5 && minor >= 5) {
        return true;
    }
    if major != 5 || minor != 4 {
        return false;
    }
    !local_model.starts_with("gpt-5.4-mini") && !local_model.starts_with("gpt-5.4-nano")
}

pub(super) fn supports_anthropic_deferred_tool_references(model: &str) -> bool {
    let Some((provider, local_model)) = model.split_once('/') else {
        return false;
    };
    if provider != "anthropic" {
        return false;
    }
    if local_model.starts_with("claude-fable-5")
        || local_model.starts_with("claude-mythos-5")
        || local_model.starts_with("claude-opus-5")
    {
        return true;
    }
    ["opus", "sonnet", "haiku"].into_iter().any(|family| {
        let prefix = format!("claude-{family}-4-");
        local_model
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split('-').next())
            .and_then(|minor| minor.parse::<u64>().ok())
            .is_some_and(|minor| minor >= 5)
    })
}

pub(super) fn attached_harness_flags(attached: Option<&[String]>) -> (bool, bool, bool) {
    // Image support is an automatic request behavior, not a session capability.
    let vision_enabled = true;
    let web_search_enabled =
        attached.is_none_or(|values| values.iter().any(|value| value == WEB_SEARCH_CAPABILITY_ID));
    let web_search_explicitly_attached =
        attached.is_some_and(|values| values.iter().any(|value| value == WEB_SEARCH_CAPABILITY_ID));
    (
        vision_enabled,
        web_search_enabled,
        web_search_explicitly_attached,
    )
}

pub(super) fn capability_guidance(mut snapshot: Value, discovery_enabled: bool) -> String {
    if let Some(object) = snapshot.as_object_mut() {
        object.remove("visibleTools");
        object.remove("activeSessionId");
    }
    let discovery_hint = if discovery_enabled {
        "Missing schema? use search_tools."
    } else {
        "Only the currently attached tools are available; use them directly and finish when the evidence is sufficient."
    };
    format!(
        "Runtime: {snapshot}. {discovery_hint} Current tool results and sandbox/approval state override stale assumptions."
    )
}

pub(super) fn append_dynamic_turn_checkpoints(
    history: &mut Vec<Message>,
    provenance: Option<Message>,
    last_provenance_checkpoint: &mut Option<String>,
    retry_instruction: &mut Option<String>,
    final_consistency_pending: &mut bool,
) {
    if let Some(provenance) = provenance
        && let Some(content) = provenance.content.as_deref()
        && last_provenance_checkpoint.as_deref() != Some(content)
    {
        *last_provenance_checkpoint = Some(content.to_owned());
        history.push(provenance);
    }
    if let Some(correction) = retry_instruction.take() {
        let duplicate = history.iter().rev().any(|message| {
            message.request_only == Some(true)
                && message.content.as_deref() == Some(correction.as_str())
        });
        if !duplicate {
            history.push(Message::system(correction).request_only());
        }
    }
    if *final_consistency_pending {
        history.push(Message::system("Coordinator state: the requested workspace write succeeded and write validation passed. The current result is eligible for a final consistency review against the user's request and the evidence already collected.").request_only());
        *final_consistency_pending = false;
    }
}

pub(super) fn should_inherit_implementation_turn(input: &str, history: &[Message]) -> bool {
    let value = input.trim().to_ascii_lowercase();
    if value.is_empty() || value.chars().count() > 160 {
        return false;
    }
    if super::policy::looks_like_web_research_request(input)
        || super::policy::looks_like_planning_or_documentation(input)
        || super::policy::looks_like_bounded_explanation(input)
        || super::policy::looks_like_bounded_analysis(input)
    {
        return false;
    }
    let continuation = [
        "continue",
        "continue it",
        "continue fixing",
        "proceed",
        "go on",
        "keep going",
        "do it",
        "finish it",
        "finish this",
        "make it better",
        "make this better",
        "again",
        "same thing",
        "계속",
        "계속해",
        "계속 진행",
        "진행해",
        "마저 해",
        "마저해",
    ]
    .iter()
    .any(|phrase| value == *phrase || value.starts_with(&format!("{phrase} ")));
    if !continuation {
        return false;
    }
    let current_index = history
        .iter()
        .rposition(|message| {
            message.role == MessageRole::User && message.content.as_deref() == Some(input)
        })
        .unwrap_or(history.len());
    let Some(previous_user_index) = history[..current_index].iter().rposition(|message| {
        message.role == MessageRole::User && message.request_only != Some(true)
    }) else {
        return false;
    };
    let previous_requested_implementation = history[previous_user_index]
        .content
        .as_deref()
        .is_some_and(super::policy::looks_like_coding_implementation_request);
    let previous_structured_edit = history[previous_user_index + 1..current_index]
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .filter_map(|message| message.tool_calls.as_ref())
        .flatten()
        .any(|call| call.name == "apply_file_edits");
    previous_requested_implementation || previous_structured_edit
}

pub(super) fn settle_interrupted_context_batch(history: &mut Vec<Message>) {
    let Some(index) = history
        .iter()
        .rposition(|message| message.role == MessageRole::Assistant)
    else {
        return;
    };
    let Some(calls) = history[index].tool_calls.clone() else {
        return;
    };
    let answered: HashSet<String> = history[index + 1..]
        .iter()
        .filter_map(|message| message.tool_call_id.clone())
        .collect();
    for call in calls
        .into_iter()
        .filter(|call| !answered.contains(&call.id))
    {
        history.push(Message::tool(
            json!({"error":"Tool batch interrupted before a result was recorded; effects may have occurred. Inspect current state before retrying."}).to_string(),
            call.id,
            Some(call.name),
        ));
    }
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        bail!("cancelled")
    }
    Ok(())
}

pub(super) fn configure_tool_access(
    request: &mut CallRequest,
    tools: Vec<ToolDefinition>,
    _decision_requested: bool,
) {
    // Finalization is enforced by the coordinator after the response. Keep the
    // provider-visible tool envelope and tool-choice shape unchanged so a final
    // synthesis call can reuse the same prompt prefix instead of forcing a cold
    // request merely to prevent another tool call.
    request.tools = (!tools.is_empty()).then_some(tools);
    request.tool_choice = None;
}

pub(super) fn attached_web_search_capability_result(
    call: &ToolCall,
    enabled: bool,
) -> Option<String> {
    if !enabled {
        return None;
    }
    match call.name.as_str() {
        "find_capabilities" => {
            let query = call
                .arguments
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if query.contains("web") || query.contains("search") {
                return Some(json!([{
                    "id":"web-search",
                    "kind":"attached-local",
                    "description":"Web search is enabled. Use search_tools with query web_search to attach its schema; capability activation is not required."
                }]).to_string());
            }
        }
        "activate_capability"
            if call.arguments.get("capability").and_then(Value::as_str)
                == Some(WEB_SEARCH_CAPABILITY_ID) =>
        {
            return Some(
                json!({
                    "activated":"web-search",
                    "alreadyActive":true,
                    "tools":["web_search"],
                    "hint":"Use search_tools with query web_search to attach its schema, then call web_search."
                })
                .to_string(),
            );
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_retry_directives_are_not_replayed_in_one_window() {
        let mut history = Vec::new();
        let mut provenance = None;
        let mut retry = Some("same correction".to_owned());
        let mut final_consistency = false;

        append_dynamic_turn_checkpoints(
            &mut history,
            None,
            &mut provenance,
            &mut retry,
            &mut final_consistency,
        );
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, MessageRole::System);

        retry = Some("same correction".to_owned());
        append_dynamic_turn_checkpoints(
            &mut history,
            None,
            &mut provenance,
            &mut retry,
            &mut final_consistency,
        );
        assert_eq!(history.len(), 1);

        retry = Some("different correction".to_owned());
        append_dynamic_turn_checkpoints(
            &mut history,
            None,
            &mut provenance,
            &mut retry,
            &mut final_consistency,
        );
        assert_eq!(history.len(), 2);
    }
}
