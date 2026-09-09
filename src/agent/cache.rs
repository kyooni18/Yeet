//! Prompt-cache boundary placement and cache diagnostics.
//!
//! Stable overlays are kept before a deterministic breakpoint so repeated
//! tool rounds can reuse the same provider-side prompt prefix.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::core::{CallRequest, Message, MessageRole, Usage};

/// Tracks whether the durable, non-request-only conversation remains an
/// append-only prefix across model attempts. Provider caches can reuse beyond
/// Yeet's explicit breakpoint, so rewriting an older tool result can destroy a
/// much larger implicit cached prefix even while `cacheSurfaceHash` is stable.
#[derive(Default)]
pub(super) struct ContinuityTracker {
    scope: Option<String>,
    previous_history: Option<Vec<Message>>,
}

impl ContinuityTracker {
    pub(super) fn diagnostics(&mut self, request: &CallRequest) -> Value {
        let mut value = diagnostics(request);
        let scope = request
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("contextWindowId"))
            .cloned()
            .or_else(|| request.context_key.clone())
            .unwrap_or_default();
        let current = canonical_history(request);
        let same_scope = self.scope.as_deref() == Some(scope.as_str());
        let previous = same_scope.then(|| self.previous_history.as_ref()).flatten();

        let (continues, common_messages, common_chars, first_divergent) =
            if let Some(previous) = previous {
                let common = previous
                    .iter()
                    .zip(&current)
                    .take_while(|(left, right)| left == right)
                    .count();
                let continues = common == previous.len();
                let chars = serde_json::to_vec(&current[..common])
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                (
                    Some(continues),
                    Some(common),
                    Some(chars),
                    (!continues).then_some(common),
                )
            } else {
                (None, None, None, None)
            };

        if let Some(object) = value.as_object_mut() {
            object.insert("stableHistoryMessages".into(), json!(current.len()));
            object.insert(
                "previousStableHistoryMessages".into(),
                json!(previous.map(Vec::len)),
            );
            object.insert("historyPrefixContinues".into(), json!(continues));
            object.insert(
                "historyPrefixRewriteDetected".into(),
                json!(continues == Some(false)),
            );
            object.insert(
                "unchangedHistoryPrefixMessages".into(),
                json!(common_messages),
            );
            object.insert("unchangedHistoryPrefixChars".into(), json!(common_chars));
            object.insert(
                "firstDivergentHistoryMessageIndex".into(),
                json!(first_divergent),
            );
        }
        self.scope = Some(scope);
        self.previous_history = Some(current);
        value
    }
}

fn canonical_history(request: &CallRequest) -> Vec<Message> {
    request
        .messages
        .iter()
        .filter(|message| message.request_only != Some(true))
        .cloned()
        .collect()
}

/// Marks the current user message as the stable prompt-cache boundary.
#[cfg(test)]
pub(super) fn mark_turn_cache_breakpoint(messages: &mut [Message], input: &str) {
    if let Some(message) = messages.iter_mut().rev().find(|message| {
        message.role == MessageRole::User && message.content.as_deref() == Some(input)
    }) {
        message.cache_breakpoint = Some(true);
    }
}

/// Inserts turn-stable overlays and moves the cache boundary after every
/// consecutive turn-stable system message. Explicit Skill instructions are
/// stored in canonical history immediately after the user message; keeping
/// the boundary before them made their often-large prompt miss the cache on
/// every tool round.
pub(super) fn insert_turn_stable_overlays(
    messages: &mut Vec<Message>,
    input: &str,
    overlays: &[Message],
) {
    let Some(user_index) = messages.iter().rposition(|message| {
        message.role == MessageRole::User && message.content.as_deref() == Some(input)
    }) else {
        return;
    };
    if !overlays.is_empty() {
        let insertion = user_index + 1;
        messages.splice(insertion..insertion, overlays.iter().cloned());
    }

    // Everything from the current user through the immediately following
    // system messages is immutable for this turn. The first assistant/tool
    // item starts the growing trace and must remain outside the breakpoint.
    let boundary = messages
        .iter()
        .enumerate()
        .skip(user_index + 1)
        .take_while(|(_, message)| message.role == MessageRole::System)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(user_index);
    if let Some(message) = messages.get_mut(boundary) {
        message.cache_breakpoint = Some(true);
    }
}

/// Adds rolling cache breakpoints to the newest tool results in the current
/// turn. The stable turn boundary remains marked as the oldest breakpoint,
/// while recent tool evidence advances the provider cache over the append-only
/// trace instead of repeatedly caching only the small turn prefix.
///
/// Four explicit breakpoints is the tightest supported provider limit in our
/// adapters, so reserve one for the stable base and use the remaining three for
/// recent tool results. These markers are applied only to the request copy;
/// canonical session history remains unchanged.
pub(super) fn advance_turn_cache_breakpoints(messages: &mut [Message], input: &str) {
    const ROLLING_TOOL_BREAKPOINTS: usize = 3;
    let Some(user_index) = messages.iter().rposition(|message| {
        message.role == MessageRole::User && message.content.as_deref() == Some(input)
    }) else {
        return;
    };

    let tool_indices = messages
        .iter()
        .enumerate()
        .skip(user_index + 1)
        .filter(|(_, message)| {
            message.role == MessageRole::Tool && message.request_only != Some(true)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let keep_from = tool_indices.len().saturating_sub(ROLLING_TOOL_BREAKPOINTS);
    for index in tool_indices.into_iter().skip(keep_from) {
        if let Some(message) = messages.get_mut(index) {
            message.cache_breakpoint = Some(true);
        }
    }
}

/// Summarizes cache-stable request portions for runtime diagnostics.
pub(super) fn diagnostics(request: &CallRequest) -> Value {
    let breakpoint_index = request
        .messages
        .iter()
        .rposition(|message| message.cache_breakpoint == Some(true));
    let stable_history = canonical_history(request);
    let prefix = breakpoint_index
        .map(|index| request.messages[..=index].to_vec())
        .unwrap_or_default();
    let prefix_json = serde_json::to_vec(&prefix).unwrap_or_default();
    let history_json = serde_json::to_vec(&stable_history).unwrap_or_default();
    let tools_json =
        serde_json::to_vec(request.tools.as_deref().unwrap_or(&[])).unwrap_or_default();
    let tool_order = request
        .tools
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let tool_order_json = serde_json::to_vec(&tool_order).unwrap_or_default();
    let mut cache_surface = tools_json.clone();
    cache_surface.extend_from_slice(&prefix_json);
    let estimated_cache_surface_tokens = (cache_surface.len() as u64).div_ceil(3);
    json!({
        "contextKey": request.context_key,
        "model": request.model,
        "purpose": request.metadata.as_ref().and_then(|metadata| metadata.get("purpose")),
        "lane": request.metadata.as_ref().and_then(|metadata| metadata.get("lane")),
        "attachedCapabilities": request.attached_capabilities,
        "modelAttempt": request.metadata.as_ref().and_then(|metadata| metadata.get("modelAttempt")),
        "toolRound": request.metadata.as_ref().and_then(|metadata| metadata.get("toolRound")),
        "expectedCacheReuses": request.metadata.as_ref().and_then(|metadata| metadata.get("expectedCacheReuses")),
        "cacheEnabled": request.prompt_cache == Some(true),
        "cacheBreakpointIndex": breakpoint_index,
        "stablePrefixChars": prefix_json.len(),
        "stablePrefixHash": sha256_hex(&prefix_json),
        "stableHistoryChars": history_json.len(),
        "historyHash": sha256_hex(&history_json),
        "toolSchemaChars": tools_json.len(),
        "toolSchemaHash": sha256_hex(&tools_json),
        "toolOrder": tool_order,
        "toolOrderHash": sha256_hex(&tool_order_json),
        "cacheSurfaceHash": sha256_hex(&cache_surface),
        "estimatedCacheSurfaceTokens": estimated_cache_surface_tokens,
    })
}

/// Adds provider-reported cache usage to the immutable request fingerprints so
/// one durable event explains both what prefix Yeet expected to reuse and what
/// the provider actually billed/read from cache.
pub(super) fn usage_diagnostics(request_diagnostics: &Value, usage: Option<&Usage>) -> Value {
    let input = usage.and_then(|usage| usage.input_tokens).unwrap_or(0);
    let cached = usage
        .and_then(|usage| usage.cached_input_tokens)
        .map(|value| value.min(input));
    let cache_write = usage
        .and_then(|usage| usage.cache_write_input_tokens)
        .map(|value| value.min(input.saturating_sub(cached.unwrap_or(0))));
    let ordinary = cached.map(|cached| {
        input
            .saturating_sub(cached)
            .saturating_sub(cache_write.unwrap_or(0))
    });
    let cache_hit_rate = (input > 0)
        .then(|| cached.map(|cached| cached as f64 / input as f64))
        .flatten();
    let cache_write_rate = (input > 0)
        .then(|| cache_write.map(|cache_write| cache_write as f64 / input as f64))
        .flatten();
    let status = if input == 0 {
        "unknown"
    } else if request_diagnostics["cacheEnabled"] != json!(true) {
        "disabled"
    } else {
        match cached {
            Some(value) if value > 0 => "hit",
            Some(_) => "miss",
            None => "unreported",
        }
    };

    let mut value = request_diagnostics.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("cacheStatus".into(), json!(status));
        object.insert("inputTokens".into(), json!(input));
        object.insert("cachedInputTokens".into(), json!(cached));
        object.insert("cacheWriteInputTokens".into(), json!(cache_write));
        object.insert("ordinaryInputTokens".into(), json!(ordinary));
        object.insert("cacheHitRate".into(), json!(cache_hit_rate));
        object.insert("cacheWriteRate".into(), json!(cache_write_rate));
    }
    value
}

/// Computes a lowercase SHA-256 digest for diagnostic fingerprints.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ToolDefinition;

    fn request(tools: Vec<ToolDefinition>, trace: &[&str]) -> CallRequest {
        let mut messages = vec![Message::system("stable"), Message::user("fix it")];
        messages[1].cache_breakpoint = Some(true);
        messages.extend(trace.iter().map(|value| Message::assistant(*value, None)));
        let mut request = CallRequest::simple("openai/gpt-5.6-sol", messages);
        request.context_key = Some("stable-session".into());
        request.prompt_cache = Some(true);
        request.tools = Some(tools);
        request
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            format!("{name} description"),
            json!({"type":"object","properties":{}}),
        )
    }

    #[test]
    fn cache_surface_stays_stable_as_only_the_post_breakpoint_trace_grows() {
        let tools = vec![
            tool("read_file"),
            tool("apply_file_edits"),
            tool("run_shell"),
        ];
        let first = request(tools.clone(), &["read source"]);
        let later = request(tools, &["read source", "edited", "verified"]);
        let first_diag = diagnostics(&first);
        let later_diag = diagnostics(&later);

        assert_eq!(
            first_diag["cacheSurfaceHash"],
            later_diag["cacheSurfaceHash"]
        );
        assert_ne!(first_diag["historyHash"], later_diag["historyHash"]);
    }

    #[test]
    fn rolling_breakpoints_follow_recent_tool_results_without_mutating_old_trace() {
        let mut messages = vec![Message::system("stable"), Message::user("research it")];
        insert_turn_stable_overlays(
            &mut messages,
            "research it",
            &[Message::system("stable turn overlay").request_only()],
        );
        for index in 0..4 {
            messages.push(Message::assistant(format!("tool call {index}"), None));
            messages.push(Message::tool(
                format!("result {index}"),
                format!("call-{index}"),
                Some("web_read".into()),
            ));
        }

        advance_turn_cache_breakpoints(&mut messages, "research it");

        let breakpoints = messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                (message.cache_breakpoint == Some(true)).then_some(index)
            })
            .collect::<Vec<_>>();
        assert_eq!(breakpoints.len(), 4);
        assert_eq!(
            messages[breakpoints[0]].content.as_deref(),
            Some("stable turn overlay")
        );
        assert_eq!(
            messages[breakpoints[1]].content.as_deref(),
            Some("result 1")
        );
        assert_eq!(
            messages[breakpoints[2]].content.as_deref(),
            Some("result 2")
        );
        assert_eq!(
            messages[breakpoints[3]].content.as_deref(),
            Some("result 3")
        );
        assert_ne!(messages[4].cache_breakpoint, Some(true));
    }

    #[test]
    fn adding_a_tool_schema_changes_the_provider_cache_surface() {
        let first = request(vec![tool("read_file")], &[]);
        let promoted = request(vec![tool("read_file"), tool("apply_file_edits")], &[]);
        let first_diag = diagnostics(&first);
        let promoted_diag = diagnostics(&promoted);

        assert_ne!(
            first_diag["toolSchemaHash"],
            promoted_diag["toolSchemaHash"]
        );
        assert_ne!(
            first_diag["cacheSurfaceHash"],
            promoted_diag["cacheSurfaceHash"]
        );
    }

    #[test]
    fn continuity_tracker_accepts_append_only_history_growth() {
        let tools = vec![tool("read_file")];
        let first = request(tools.clone(), &["read source"]);
        let later = request(tools, &["read source", "edited", "verified"]);
        let mut tracker = ContinuityTracker::default();

        let first_diag = tracker.diagnostics(&first);
        let later_diag = tracker.diagnostics(&later);

        assert_eq!(first_diag["historyPrefixContinues"], Value::Null);
        assert_eq!(later_diag["historyPrefixContinues"], true);
        assert_eq!(later_diag["historyPrefixRewriteDetected"], false);
        assert_eq!(later_diag["firstDivergentHistoryMessageIndex"], Value::Null);
    }

    #[test]
    fn continuity_tracker_detects_rewriting_older_tool_history() {
        let tools = vec![tool("read_file")];
        let first = request(
            tools.clone(),
            &["large original tool result", "later result"],
        );
        let rewritten = request(
            tools,
            &["compacted old result", "later result", "new result"],
        );
        let mut tracker = ContinuityTracker::default();

        tracker.diagnostics(&first);
        let result = tracker.diagnostics(&rewritten);

        assert_eq!(result["historyPrefixContinues"], false);
        assert_eq!(result["historyPrefixRewriteDetected"], true);
        assert_eq!(result["firstDivergentHistoryMessageIndex"], 2);
    }

    #[test]
    fn cache_usage_diagnostics_separate_hits_writes_and_ordinary_input() {
        let request = request(vec![tool("read_file")], &[]);
        let diagnostics = diagnostics(&request);
        let usage = Usage {
            input_tokens: Some(10_000),
            cached_input_tokens: Some(6_000),
            cache_write_input_tokens: Some(1_500),
            ..Default::default()
        };
        let result = usage_diagnostics(&diagnostics, Some(&usage));

        assert_eq!(result["cacheStatus"], "hit");
        assert_eq!(result["inputTokens"], 10_000);
        assert_eq!(result["cachedInputTokens"], 6_000);
        assert_eq!(result["cacheWriteInputTokens"], 1_500);
        assert_eq!(result["ordinaryInputTokens"], 2_500);
        assert_eq!(result["cacheHitRate"], 0.6);
        assert_eq!(result["cacheWriteRate"], 0.15);
    }

    #[test]
    fn enabled_cache_with_measured_input_and_no_hit_is_reported_as_a_miss() {
        let request = request(vec![tool("read_file")], &[]);
        let diagnostics = diagnostics(&request);
        let usage = Usage {
            input_tokens: Some(2_048),
            cached_input_tokens: Some(0),
            ..Default::default()
        };
        let result = usage_diagnostics(&diagnostics, Some(&usage));

        assert_eq!(result["cacheStatus"], "miss");
        assert_eq!(result["ordinaryInputTokens"], 2_048);
        assert_eq!(result["cacheHitRate"], 0.0);
    }

    #[test]
    fn missing_provider_cache_details_are_unreported_not_a_false_miss() {
        let request = request(vec![tool("read_file")], &[]);
        let diagnostics = diagnostics(&request);
        let usage = Usage {
            input_tokens: Some(2_048),
            ..Default::default()
        };
        let result = usage_diagnostics(&diagnostics, Some(&usage));

        assert_eq!(result["cacheStatus"], "unreported");
        assert_eq!(result["cachedInputTokens"], Value::Null);
        assert_eq!(result["ordinaryInputTokens"], Value::Null);
        assert_eq!(result["cacheHitRate"], Value::Null);
    }
}
