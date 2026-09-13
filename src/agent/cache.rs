//! Prompt-cache boundary placement and cache diagnostics.
//!
//! Keep an append-only canonical user boundary across turns, then add a second
//! turn-local boundary after stable request-only overlays for repeated tool rounds.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::core::Usage;
use crate::core::{CallRequest, Message, MessageRole, ToolDefinition};

#[cfg(test)]
mod wire_tests;

/// Tracks whether the durable, non-request-only conversation remains an
/// append-only prefix across model attempts. Provider caches can reuse beyond
/// Yeet's explicit breakpoint, so rewriting an older tool result can destroy a
/// much larger implicit cached prefix even while `cacheSurfaceHash` is stable.
#[derive(Default)]
pub(super) struct ContinuityTracker {
    scope: Option<String>,
    previous_history: Option<Vec<Message>>,
    previous_wire_history: Option<Vec<Message>>,
    previous_base_prefix: Option<Vec<Message>>,
    previous_tool_envelope_hash: Option<String>,
    cache_epoch: u64,
}

impl ContinuityTracker {
    pub(super) fn rebind_session(&mut self, previous: Option<&str>, next: Option<&str>) {
        if previous != next {
            *self = Self::default();
        }
    }

    pub(super) fn diagnostics(&mut self, request: &CallRequest) -> Value {
        let mut value = diagnostics(request);
        let window = request
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("contextWindowId"))
            .cloned()
            .or_else(|| request.context_key.clone())
            .unwrap_or_default();
        let lane = request
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("lane"))
            .cloned()
            .unwrap_or_default();
        let scope = format!("{lane}:{window}");
        let current = canonical_history(request);
        let current_wire = canonical_wire_history(request);
        let current_base = canonical_base_prefix(request);
        let same_scope = self.scope.as_deref() == Some(scope.as_str());
        let previous = same_scope
            .then_some(self.previous_history.as_ref())
            .flatten();
        let previous_wire = same_scope
            .then_some(self.previous_wire_history.as_ref())
            .flatten();
        let previous_base = same_scope
            .then_some(self.previous_base_prefix.as_ref())
            .flatten();

        let (continues, common_messages, common_chars, first_divergent) =
            prefix_continuity(previous.map(Vec::as_slice), &current);
        let (wire_continues, wire_common_messages, wire_common_chars, wire_first_divergent) =
            prefix_continuity(previous_wire.map(Vec::as_slice), &current_wire);
        let base_prefix_continues =
            previous_base.map(|previous| current_base.as_slice().starts_with(previous.as_slice()));
        let base_prefix_extended = previous_base.is_some_and(|previous| {
            current_base.len() > previous.len()
                && current_base.as_slice().starts_with(previous.as_slice())
        });

        let tool_envelope_hash = value
            .get("toolEnvelopeHash")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let (cache_epoch, cache_epoch_reason, cache_relevant_history_rewrite) = if !same_scope {
            self.cache_epoch = 1;
            (self.cache_epoch, "window-start", false)
        } else {
            let tool_changed =
                self.previous_tool_envelope_hash.as_ref() != tool_envelope_hash.as_ref();
            let base_rewritten = base_prefix_continues == Some(false);
            let history_rewritten = continues == Some(false);
            let wire_history_rewritten = wire_continues == Some(false);
            // A new turn naturally extends the reusable base while completed-turn
            // tail evidence and request-only overlays may disappear. That is not a
            // full reset. Within an unchanged base, either semantic or wire-order
            // divergence destroys implicit exact-prefix reuse.
            let cache_relevant_history_rewrite =
                (history_rewritten || wire_history_rewritten) && !base_prefix_extended;
            let reason = if tool_changed {
                "tool-envelope-change"
            } else if base_rewritten {
                "base-prefix-change"
            } else if wire_history_rewritten && !history_rewritten && !base_prefix_extended {
                "wire-history-rewrite"
            } else if cache_relevant_history_rewrite {
                "history-rewrite"
            } else {
                "stable"
            };
            if tool_changed || base_rewritten || cache_relevant_history_rewrite {
                self.cache_epoch = self.cache_epoch.max(1).saturating_add(1);
            }
            (
                self.cache_epoch.max(1),
                reason,
                cache_relevant_history_rewrite,
            )
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
                "cacheRelevantHistoryRewriteDetected".into(),
                json!(cache_relevant_history_rewrite),
            );
            object.insert("basePrefixContinues".into(), json!(base_prefix_continues));
            object.insert("wireHistoryPrefixContinues".into(), json!(wire_continues));
            object.insert(
                "wireHistoryPrefixRewriteDetected".into(),
                json!(wire_continues == Some(false)),
            );
            object.insert(
                "unchangedWireHistoryPrefixMessages".into(),
                json!(wire_common_messages),
            );
            object.insert(
                "unchangedWireHistoryPrefixChars".into(),
                json!(wire_common_chars),
            );
            object.insert(
                "firstDivergentWireHistoryMessageIndex".into(),
                json!(wire_first_divergent),
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
            object.insert("cacheEpoch".into(), json!(cache_epoch));
            object.insert("cacheEpochReason".into(), json!(cache_epoch_reason));
        }
        self.scope = Some(scope);
        self.previous_history = Some(current);
        self.previous_wire_history = Some(current_wire);
        self.previous_base_prefix = Some(current_base);
        self.previous_tool_envelope_hash = tool_envelope_hash;
        value
    }
}

fn prefix_continuity(
    previous: Option<&[Message]>,
    current: &[Message],
) -> (Option<bool>, Option<usize>, Option<usize>, Option<usize>) {
    let Some(previous) = previous else {
        return (None, None, None, None);
    };
    let common = previous
        .iter()
        .zip(current)
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
}

fn canonical_wire_history(request: &CallRequest) -> Vec<Message> {
    request
        .messages
        .iter()
        .cloned()
        .map(|mut message| {
            // The provider sees request-only message content, but these bookkeeping
            // flags are adapter controls rather than semantic wire text.
            message.cache_breakpoint = None;
            message.request_only = None;
            message
        })
        .collect()
}

fn canonical_history(request: &CallRequest) -> Vec<Message> {
    request
        .messages
        .iter()
        .filter(|message| message.request_only != Some(true))
        .cloned()
        .map(|mut message| {
            // Cache breakpoints are a request-local placement plan, not
            // semantic conversation content. Excluding them prevents rolling
            // marker rotation from being misreported as a history rewrite.
            message.cache_breakpoint = None;
            message.request_only = None;
            message
        })
        .collect()
}

fn canonical_base_prefix(request: &CallRequest) -> Vec<Message> {
    let Some(end) = request
        .messages
        .iter()
        .position(|message| message.cache_breakpoint == Some(true))
    else {
        return Vec::new();
    };
    request.messages[..=end]
        .iter()
        .cloned()
        .map(|mut message| {
            message.cache_breakpoint = None;
            message.request_only = None;
            message
        })
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

/// Inserts turn-stable overlays while keeping the oldest explicit cache boundary
/// on the canonical user message. The overlay tail gets a second boundary so
/// repeated tool rounds can still cache large turn-local guidance, but the next
/// user turn can reuse the earlier canonical prefix after request-only overlays
/// disappear.
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
    if let Some(message) = messages.get_mut(user_index) {
        message.cache_breakpoint = Some(true);
    }
    if !overlays.is_empty() {
        let insertion = user_index + 1;
        messages.splice(insertion..insertion, overlays.iter().cloned());
        let overlay_boundary = messages
            .iter()
            .enumerate()
            .skip(user_index + 1)
            .take_while(|(_, message)| message.role == MessageRole::System)
            .map(|(index, _)| index)
            .last();
        if let Some(index) = overlay_boundary
            && let Some(message) = messages.get_mut(index)
        {
            message.cache_breakpoint = Some(true);
        }
    }
}

/// Adds rolling cache breakpoints to the newest tool results in the current
/// turn. Four explicit breakpoints is the tightest provider limit in our
/// adapters. Preserve every non-tool boundary already placed for the current
/// turn (canonical user plus optional stable overlay tail) and spend only the
/// remaining slots on the newest append-only tool evidence.
pub(super) fn advance_turn_cache_breakpoints(messages: &mut [Message], input: &str) {
    const MAX_CACHE_BREAKPOINTS: usize = 4;
    let Some(user_index) = messages.iter().rposition(|message| {
        message.role == MessageRole::User && message.content.as_deref() == Some(input)
    }) else {
        return;
    };
    let reserved = messages
        .iter()
        .filter(|message| {
            message.cache_breakpoint == Some(true) && message.role != MessageRole::Tool
        })
        .count()
        .min(MAX_CACHE_BREAKPOINTS);
    let rolling_tool_breakpoints = MAX_CACHE_BREAKPOINTS.saturating_sub(reserved);
    if rolling_tool_breakpoints == 0 {
        return;
    }
    let tool_indices = messages
        .iter()
        .enumerate()
        .skip(user_index + 1)
        .filter(|(_, message)| {
            message.role == MessageRole::Tool && message.request_only != Some(true)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let keep_from = tool_indices.len().saturating_sub(rolling_tool_breakpoints);
    for index in tool_indices.into_iter().skip(keep_from) {
        if let Some(message) = messages.get_mut(index) {
            message.cache_breakpoint = Some(true);
        }
    }
}

fn canonical_tool_schema_json(tools: &[ToolDefinition]) -> Vec<u8> {
    let mut canonical = tools.to_vec();
    canonical.sort_by(|left, right| left.name.cmp(&right.name));
    serde_json::to_vec(&canonical).unwrap_or_default()
}

/// Summarizes the immutable cache base separately from the rolling lookup
/// frontier. A normal append-only tool round should advance the latter without
/// making diagnostics report that Yeet rewrote its stable cache contract.
pub(super) fn diagnostics(request: &CallRequest) -> Value {
    let base_breakpoint_index = request
        .messages
        .iter()
        .position(|message| message.cache_breakpoint == Some(true));
    let breakpoint_index = request
        .messages
        .iter()
        .rposition(|message| message.cache_breakpoint == Some(true));
    let stable_history = canonical_history(request);
    let breakpoint_indices = request
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.cache_breakpoint == Some(true)).then_some(index))
        .collect::<Vec<_>>();
    let base_prefix = base_breakpoint_index
        .map(|index| request.messages[..=index].to_vec())
        .unwrap_or_default();
    let rolling_prefix = breakpoint_index
        .map(|index| request.messages[..=index].to_vec())
        .unwrap_or_default();
    let base_prefix_json = serde_json::to_vec(&base_prefix).unwrap_or_default();
    let rolling_prefix_json = serde_json::to_vec(&rolling_prefix).unwrap_or_default();
    let history_json = serde_json::to_vec(&stable_history).unwrap_or_default();
    let tools_json =
        serde_json::to_vec(request.tools.as_deref().unwrap_or(&[])).unwrap_or_default();
    let deferred_tools_json =
        serde_json::to_vec(request.deferred_tools.as_deref().unwrap_or(&[])).unwrap_or_default();
    let logical_tools_json = canonical_tool_schema_json(request.tools.as_deref().unwrap_or(&[]));
    let logical_deferred_tools_json =
        canonical_tool_schema_json(request.deferred_tools.as_deref().unwrap_or(&[]));
    let tool_order = request
        .tools
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let tool_order_json = serde_json::to_vec(&tool_order).unwrap_or_default();
    let deferred_tool_order = request
        .deferred_tools
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let deferred_tool_order_json = serde_json::to_vec(&deferred_tool_order).unwrap_or_default();
    let breakpoint_plan_json = serde_json::to_vec(&breakpoint_indices).unwrap_or_default();
    let system_instruction_chars = request
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .filter_map(|message| message.content.as_deref())
        .map(str::len)
        .sum::<usize>();
    let model_visible_tool_result_chars = request
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .filter_map(|message| message.content.as_deref())
        .map(str::len)
        .sum::<usize>();
    let cache_family = request
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("cacheFamily"))
        .cloned()
        .or_else(|| request.context_key.clone());
    let prompt_cache_key_hash = cache_family.as_ref().map(|family| {
        // Match the TypeScript JSON.stringify field order exactly so this is the
        // digest suffix actually sent as OpenAI prompt_cache_key/session-id.
        let model = serde_json::to_string(&request.model).unwrap_or_else(|_| "\"\"".into());
        let family = serde_json::to_string(family).unwrap_or_else(|_| "\"\"".into());
        let identity = format!("{{\"version\":2,\"model\":{model},\"family\":{family}}}");
        sha256_hex(identity.as_bytes())
            .chars()
            .take(48)
            .collect::<String>()
    });
    let mut tool_envelope = tools_json.clone();
    tool_envelope.extend_from_slice(&deferred_tools_json);
    let mut cache_surface = tool_envelope.clone();
    cache_surface.extend_from_slice(&base_prefix_json);
    let mut rolling_cache_surface = tool_envelope.clone();
    rolling_cache_surface.extend_from_slice(&rolling_prefix_json);
    let estimated_cache_surface_tokens = (cache_surface.len() as u64).div_ceil(3);
    let estimated_rolling_cache_surface_tokens = (rolling_cache_surface.len() as u64).div_ceil(3);
    let mut result = serde_json::Map::new();
    result.insert("contextKey".into(), json!(request.context_key));
    result.insert("cacheFamily".into(), json!(cache_family));
    result.insert("promptCacheKeyHash".into(), json!(prompt_cache_key_hash));
    result.insert("model".into(), json!(request.model));
    result.insert(
        "purpose".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("purpose"))
        ),
    );
    result.insert(
        "lane".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("lane"))
        ),
    );
    result.insert(
        "sessionId".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("sessionId"))
        ),
    );
    result.insert(
        "contextWindowId".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("contextWindowId"))
        ),
    );
    result.insert(
        "contextWindowNumber".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("contextWindowNumber"))
        ),
    );
    result.insert(
        "attachedCapabilities".into(),
        json!(request.attached_capabilities),
    );
    result.insert(
        "modelAttempt".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("modelAttempt"))
        ),
    );
    result.insert(
        "toolRound".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("toolRound"))
        ),
    );
    result.insert(
        "expectedCacheReuses".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("expectedCacheReuses"))
        ),
    );
    result.insert(
        "turnCumulativeInputTokens".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("turnCumulativeInputTokens"))
        ),
    );
    result.insert(
        "turnEstimatedCostUsd".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("turnEstimatedCostUsd"))
        ),
    );
    result.insert(
        "peakRequestChars".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("peakRequestChars"))
        ),
    );
    result.insert(
        "contextEstimatedTokens".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("contextEstimatedTokens"))
        ),
    );
    result.insert(
        "contextWorkingBudget".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("contextWorkingBudget"))
        ),
    );
    result.insert(
        "yeetVersion".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("yeetVersion"))
        ),
    );
    result.insert(
        "workspaceRevision".into(),
        json!(
            request
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("workspaceRevision"))
        ),
    );
    result.insert(
        "cacheEnabled".into(),
        json!(request.prompt_cache == Some(true)),
    );
    result.insert(
        "baseCacheBreakpointIndex".into(),
        json!(base_breakpoint_index),
    );
    result.insert("cacheBreakpointIndex".into(), json!(breakpoint_index));
    result.insert("basePrefixChars".into(), json!(base_prefix_json.len()));
    result.insert(
        "basePrefixHash".into(),
        json!(sha256_hex(&base_prefix_json)),
    );
    result.insert("stablePrefixChars".into(), json!(base_prefix_json.len()));
    result.insert(
        "stablePrefixHash".into(),
        json!(sha256_hex(&base_prefix_json)),
    );
    result.insert(
        "rollingFrontierChars".into(),
        json!(rolling_prefix_json.len()),
    );
    result.insert(
        "rollingFrontierHash".into(),
        json!(sha256_hex(&rolling_prefix_json)),
    );
    result.insert("stableHistoryChars".into(), json!(history_json.len()));
    result.insert("historyHash".into(), json!(sha256_hex(&history_json)));
    result.insert(
        "semanticHistoryHash".into(),
        json!(sha256_hex(&history_json)),
    );
    result.insert("breakpointIndices".into(), json!(breakpoint_indices));
    result.insert(
        "breakpointPlanHash".into(),
        json!(sha256_hex(&breakpoint_plan_json)),
    );
    result.insert(
        "systemInstructionChars".into(),
        json!(system_instruction_chars),
    );
    result.insert(
        "modelVisibleToolResultChars".into(),
        json!(model_visible_tool_result_chars),
    );
    result.insert("toolSchemaChars".into(), json!(tools_json.len()));
    result.insert(
        "toolSchemaHash".into(),
        json!(sha256_hex(&logical_tools_json)),
    );
    result.insert(
        "wireAttachedToolSchemaHash".into(),
        json!(sha256_hex(&tools_json)),
    );
    result.insert("toolOrder".into(), json!(tool_order));
    result.insert("toolOrderHash".into(), json!(sha256_hex(&tool_order_json)));
    result.insert(
        "deferredToolSchemaChars".into(),
        json!(deferred_tools_json.len()),
    );
    result.insert(
        "deferredToolSchemaHash".into(),
        json!(sha256_hex(&logical_deferred_tools_json)),
    );
    result.insert(
        "wireDeferredToolSchemaHash".into(),
        json!(sha256_hex(&deferred_tools_json)),
    );
    result.insert("deferredToolOrder".into(), json!(deferred_tool_order));
    result.insert(
        "deferredToolOrderHash".into(),
        json!(sha256_hex(&deferred_tool_order_json)),
    );
    result.insert("wireToolSchemaChars".into(), json!(tool_envelope.len()));
    result.insert("toolEnvelopeHash".into(), json!(sha256_hex(&tool_envelope)));
    result.insert("cacheSurfaceHash".into(), json!(sha256_hex(&cache_surface)));
    result.insert(
        "rollingCacheSurfaceHash".into(),
        json!(sha256_hex(&rolling_cache_surface)),
    );
    result.insert(
        "estimatedCacheSurfaceTokens".into(),
        json!(estimated_cache_surface_tokens),
    );
    result.insert(
        "estimatedRollingCacheSurfaceTokens".into(),
        json!(estimated_rolling_cache_surface_tokens),
    );
    Value::Object(result)
}

#[cfg(test)]
pub(super) fn usage_diagnostics(request_diagnostics: &Value, usage: Option<&Usage>) -> Value {
    super::turn_state::usage_diagnostics(request_diagnostics, usage)
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
            Some("research it")
        );
        assert_eq!(
            messages[breakpoints[1]].content.as_deref(),
            Some("stable turn overlay")
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
    fn stable_base_hash_survives_rolling_frontier_growth() {
        let tools = vec![tool("web_read")];
        let mut messages = vec![Message::system("stable"), Message::user("research it")];
        insert_turn_stable_overlays(
            &mut messages,
            "research it",
            &[Message::system("stable turn overlay").request_only()],
        );
        messages.push(Message::assistant("tool call 0", None));
        messages.push(Message::tool("result 0", "call-0", Some("web_read".into())));
        advance_turn_cache_breakpoints(&mut messages, "research it");

        let mut first = CallRequest::simple("openai/gpt-5.6-sol", messages.clone());
        first.context_key = Some("stable-session".into());
        first.prompt_cache = Some(true);
        first.tools = Some(tools.clone());
        let first_diag = diagnostics(&first);

        messages.push(Message::assistant("tool call 1", None));
        messages.push(Message::tool("result 1", "call-1", Some("web_read".into())));
        advance_turn_cache_breakpoints(&mut messages, "research it");
        let mut later = CallRequest::simple("openai/gpt-5.6-sol", messages);
        later.context_key = Some("stable-session".into());
        later.prompt_cache = Some(true);
        later.tools = Some(tools);
        let later_diag = diagnostics(&later);

        assert_eq!(first_diag["basePrefixHash"], later_diag["basePrefixHash"]);
        assert_eq!(
            first_diag["stablePrefixHash"],
            later_diag["stablePrefixHash"]
        );
        assert_eq!(
            first_diag["cacheSurfaceHash"],
            later_diag["cacheSurfaceHash"]
        );
        assert_ne!(
            first_diag["rollingFrontierHash"],
            later_diag["rollingFrontierHash"]
        );
        assert_ne!(
            first_diag["rollingCacheSurfaceHash"],
            later_diag["rollingCacheSurfaceHash"]
        );
    }

    #[test]
    fn continuity_ignores_request_local_breakpoint_rotation() {
        let mut tracker = ContinuityTracker::default();
        let mut first = request(vec![tool("read_file")], &[]);
        first.metadata = Some(std::collections::HashMap::from([
            ("lane".into(), "agent".into()),
            ("contextWindowId".into(), "window-a".into()),
        ]));
        first.messages.push(Message::tool(
            "same evidence",
            "call-1",
            Some("read_file".into()),
        ));
        first.messages.last_mut().unwrap().cache_breakpoint = Some(true);
        let initial = tracker.diagnostics(&first);
        assert!(initial["historyPrefixContinues"].is_null());

        let mut second = first.clone();
        second.messages.last_mut().unwrap().cache_breakpoint = None;
        second.messages.push(Message::tool(
            "new evidence",
            "call-2",
            Some("read_file".into()),
        ));
        second.messages.last_mut().unwrap().cache_breakpoint = Some(true);
        let later = tracker.diagnostics(&second);
        assert_eq!(later["historyPrefixContinues"], json!(true));
        assert_eq!(later["historyPrefixRewriteDetected"], json!(false));
        assert_ne!(initial["breakpointPlanHash"], later["breakpointPlanHash"]);
        assert_eq!(initial["cacheEpoch"], 1);
        assert_eq!(later["cacheEpoch"], 1);
        assert_eq!(later["cacheEpochReason"], "stable");
    }

    #[test]
    fn adding_a_tool_schema_changes_the_provider_cache_surface_but_not_affinity_family() {
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
        assert_eq!(
            first_diag["promptCacheKeyHash"],
            promoted_diag["promptCacheKeyHash"]
        );
    }

    #[test]
    fn logical_tool_hashes_ignore_reordering_while_wire_envelope_detects_it() {
        let mut first = request(vec![tool("alpha"), tool("beta")], &[]);
        first.deferred_tools = Some(vec![tool("mcp_alpha"), tool("mcp_beta")]);
        let mut reordered = request(vec![tool("beta"), tool("alpha")], &[]);
        reordered.deferred_tools = Some(vec![tool("mcp_beta"), tool("mcp_alpha")]);

        let first_diag = diagnostics(&first);
        let reordered_diag = diagnostics(&reordered);

        assert_eq!(
            first_diag["toolSchemaHash"],
            reordered_diag["toolSchemaHash"]
        );
        assert_eq!(
            first_diag["deferredToolSchemaHash"],
            reordered_diag["deferredToolSchemaHash"]
        );
        assert_ne!(
            first_diag["wireAttachedToolSchemaHash"],
            reordered_diag["wireAttachedToolSchemaHash"]
        );
        assert_ne!(
            first_diag["wireDeferredToolSchemaHash"],
            reordered_diag["wireDeferredToolSchemaHash"]
        );
        assert_ne!(first_diag["toolOrderHash"], reordered_diag["toolOrderHash"]);
        assert_ne!(
            first_diag["deferredToolOrderHash"],
            reordered_diag["deferredToolOrderHash"]
        );
        assert_ne!(
            first_diag["toolEnvelopeHash"],
            reordered_diag["toolEnvelopeHash"]
        );
        assert_ne!(
            first_diag["cacheSurfaceHash"],
            reordered_diag["cacheSurfaceHash"]
        );
    }

    #[test]
    fn deferred_tool_definitions_are_part_of_the_wire_tool_envelope() {
        let first = request(vec![tool("read_file")], &[]);
        let mut later = first.clone();
        later.deferred_tools = Some(vec![tool("mcp_krpc_observe")]);

        let first_diag = diagnostics(&first);
        let later_diag = diagnostics(&later);
        assert_ne!(
            first_diag["cacheSurfaceHash"],
            later_diag["cacheSurfaceHash"]
        );
        assert_ne!(
            first_diag["toolEnvelopeHash"],
            later_diag["toolEnvelopeHash"]
        );
        assert_ne!(
            first_diag["deferredToolSchemaHash"],
            later_diag["deferredToolSchemaHash"]
        );
        assert_eq!(
            first_diag["promptCacheKeyHash"],
            later_diag["promptCacheKeyHash"]
        );
        assert!(
            later_diag["wireToolSchemaChars"].as_u64().unwrap()
                > later_diag["toolSchemaChars"].as_u64().unwrap()
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
        assert_eq!(first_diag["cacheEpoch"], 1);
        assert_eq!(later_diag["cacheEpoch"], 1);
        assert_eq!(later_diag["cacheEpochReason"], "stable");
    }

    #[test]
    fn current_turn_request_only_checkpoints_remain_wire_append_only() {
        let tools = vec![tool("read_file")];
        let mut first = request(tools.clone(), &[]);
        first.metadata = Some(std::collections::HashMap::from([
            ("lane".into(), "agent".into()),
            ("contextWindowId".into(), "window-a".into()),
        ]));
        first.messages.push(Message::assistant("tool call 0", None));
        first.messages.push(Message::tool(
            "result 0",
            "call-0",
            Some("read_file".into()),
        ));
        first
            .messages
            .push(Message::system("retry checkpoint 1").request_only());

        let mut later = first.clone();
        later.messages.push(Message::assistant("tool call 1", None));
        later.messages.push(Message::tool(
            "result 1",
            "call-1",
            Some("read_file".into()),
        ));
        later
            .messages
            .push(Message::system("provenance checkpoint 2").request_only());

        let mut tracker = ContinuityTracker::default();
        let first_diag = tracker.diagnostics(&first);
        let later_diag = tracker.diagnostics(&later);
        assert_eq!(first_diag["cacheEpochReason"], "window-start");
        assert_eq!(later_diag["historyPrefixContinues"], true);
        assert_eq!(later_diag["wireHistoryPrefixContinues"], true);
        assert_eq!(later_diag["wireHistoryPrefixRewriteDetected"], false);
        assert_eq!(later_diag["cacheRelevantHistoryRewriteDetected"], false);
        assert_eq!(later_diag["cacheEpochReason"], "stable");
        assert_eq!(later_diag["cacheEpoch"], 1);
    }

    #[test]
    fn same_session_runtime_rebind_preserves_cache_continuity() {
        let request = request(vec![tool("read_file")], &["read source"]);
        let mut tracker = ContinuityTracker::default();
        let first = tracker.diagnostics(&request);

        tracker.rebind_session(Some("session-a"), Some("session-a"));
        let same_session = tracker.diagnostics(&request);
        assert_eq!(same_session["cacheEpoch"], 1);
        assert_eq!(same_session["cacheEpochReason"], "stable");

        tracker.rebind_session(Some("session-a"), Some("session-b"));
        let changed_session = tracker.diagnostics(&request);
        assert_eq!(changed_session["cacheEpoch"], 1);
        assert_eq!(changed_session["cacheEpochReason"], "window-start");
        assert_eq!(first["cacheEpochReason"], "window-start");
    }

    #[test]
    fn continuity_tracks_append_only_new_turn_and_resets_on_new_window() {
        let mut tracker = ContinuityTracker::default();
        let mut first = request(vec![tool("read_file")], &[]);
        first.metadata = Some(std::collections::HashMap::from([
            ("lane".into(), "agent".into()),
            ("contextWindowId".into(), "window-a".into()),
        ]));
        first
            .messages
            .insert(2, Message::system("turn-only overlay").request_only());
        let first_diag = tracker.diagnostics(&first);

        let mut next_turn = first.clone();
        next_turn
            .messages
            .retain(|message| message.request_only != Some(true));
        next_turn.messages[1].cache_breakpoint = None;
        next_turn.messages.push(Message::assistant("done", None));
        next_turn
            .messages
            .push(Message::user("next task").cache_breakpoint());
        let next_diag = tracker.diagnostics(&next_turn);

        assert_eq!(first_diag["cacheEpoch"], 1);
        assert_eq!(first_diag["cacheEpochReason"], "window-start");
        assert_eq!(next_diag["historyPrefixContinues"], true);
        assert_eq!(next_diag["historyPrefixRewriteDetected"], false);
        assert_eq!(next_diag["basePrefixContinues"], true);
        assert_eq!(next_diag["cacheRelevantHistoryRewriteDetected"], false);
        assert_ne!(first_diag["basePrefixHash"], next_diag["basePrefixHash"]);
        assert_eq!(
            first_diag["promptCacheKeyHash"],
            next_diag["promptCacheKeyHash"]
        );
        assert_eq!(next_diag["cacheEpoch"], 1);
        assert_eq!(next_diag["cacheEpochReason"], "stable");

        next_turn
            .metadata
            .as_mut()
            .unwrap()
            .insert("contextWindowId".into(), "window-b".into());
        let new_window_diag = tracker.diagnostics(&next_turn);
        assert_eq!(new_window_diag["historyPrefixContinues"], Value::Null);
        assert_eq!(new_window_diag["historyPrefixRewriteDetected"], false);
        assert_eq!(new_window_diag["cacheEpoch"], 1);
        assert_eq!(new_window_diag["cacheEpochReason"], "window-start");
    }

    #[test]
    fn skill_attachment_state_changes_append_without_rewriting_cached_history() {
        let mut tracker = ContinuityTracker::default();
        let mut request = request(vec![tool("read_file")], &[]);
        request.metadata = Some(std::collections::HashMap::from([
            ("lane".into(), "agent".into()),
            ("contextWindowId".into(), "skill-cache-window".into()),
        ]));
        let original_history = request.messages.clone();
        let first = tracker.diagnostics(&request);

        request.messages.push(Message::assistant("done", None));
        request.messages.push(Message::system(
            "Attached Skill: skyline\nPersistent instructions",
        ));
        request.messages.push(Message::user("continue"));
        let attached = tracker.diagnostics(&request);
        assert_eq!(request.messages[..original_history.len()], original_history);
        assert_eq!(attached["historyPrefixContinues"], true);
        assert_eq!(attached["historyPrefixRewriteDetected"], false);
        assert_eq!(attached["cacheEpoch"], first["cacheEpoch"]);
        assert_eq!(attached["cacheEpochReason"], "stable");

        let before_detach = request.messages.clone();
        request
            .messages
            .push(Message::assistant("done again", None));
        request.messages.push(Message::system(
            "Skill attachment state: skyline detached. Earlier history remains unchanged.",
        ));
        request.messages.push(Message::user("continue without it"));
        let detached = tracker.diagnostics(&request);
        assert_eq!(request.messages[..before_detach.len()], before_detach);
        assert_eq!(detached["historyPrefixContinues"], true);
        assert_eq!(detached["historyPrefixRewriteDetected"], false);
        assert_eq!(detached["cacheEpoch"], first["cacheEpoch"]);
        assert_eq!(detached["cacheEpochReason"], "stable");
    }

    #[test]
    fn continuity_tracker_detects_rewriting_older_tool_history() {
        let tools = vec![tool("read_file")];
        let first = request(
            tools.clone(),
            &["large original tool result", "later result"],
        );
        let mut rewritten = request(
            tools,
            &["compacted old result", "later result", "new result"],
        );
        let mut tracker = ContinuityTracker::default();

        let initial = tracker.diagnostics(&first);
        let result = tracker.diagnostics(&rewritten);

        assert_eq!(initial["cacheEpoch"], 1);
        assert_eq!(initial["cacheEpochReason"], "window-start");
        assert_eq!(result["historyPrefixContinues"], false);
        assert_eq!(result["historyPrefixRewriteDetected"], true);
        assert_eq!(result["cacheRelevantHistoryRewriteDetected"], true);
        assert_eq!(result["firstDivergentHistoryMessageIndex"], 2);
        assert_eq!(result["cacheEpoch"], 2);
        assert_eq!(result["cacheEpochReason"], "history-rewrite");

        rewritten
            .messages
            .push(Message::assistant("post-rewrite append", None));
        let recovered = tracker.diagnostics(&rewritten);
        assert_eq!(recovered["historyPrefixContinues"], true);
        assert_eq!(recovered["historyPrefixRewriteDetected"], false);
        assert_eq!(recovered["cacheEpoch"], 2);
        assert_eq!(recovered["cacheEpochReason"], "stable");
    }

    #[test]
    fn cache_usage_diagnostics_separate_hits_writes_and_ordinary_input() {
        let request = request(vec![tool("read_file")], &[]);
        let diagnostics = diagnostics(&request);
        let usage = Usage {
            input_tokens: Some(10_000),
            cached_input_tokens: Some(6_000),
            cache_write_input_tokens: Some(1_500),
            cost_equivalent_input_tokens: Some(4_975),
            transport_attempts: Some(2),
            ..Default::default()
        };
        let result = usage_diagnostics(&diagnostics, Some(&usage));

        assert_eq!(result["cacheStatus"], "hit");
        assert_eq!(result["inputTokens"], 10_000);
        assert_eq!(result["cachedInputTokens"], 6_000);
        assert_eq!(result["cacheWriteInputTokens"], 1_500);
        assert_eq!(result["ordinaryInputTokens"], 2_500);
        assert_eq!(result["costEquivalentInputTokens"], 4_975);
        assert_eq!(result["transportAttempts"], 2);
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

    #[test]
    fn provider_cache_miss_diagnostics_remain_separate_from_client_epoch_reason() {
        let request = request(vec![tool("read_file")], &[]);
        let mut diagnostics = diagnostics(&request);
        diagnostics["cacheEpochReason"] = json!("stable");
        let usage = Usage {
            input_tokens: Some(4_096),
            cached_input_tokens: Some(0),
            provider_cache_diagnostic_type: Some("cache_miss".into()),
            provider_cache_miss_reason: Some("input_changed".into()),
            provider_cache_missed_tokens: Some(1_024),
            provider_comparison_reusable_tokens: Some(3_072),
            ..Default::default()
        };
        let result = usage_diagnostics(&diagnostics, Some(&usage));

        assert_eq!(result["cacheStatus"], "miss");
        assert_eq!(result["cacheEpochReason"], "stable");
        assert_eq!(result["providerCacheDiagnosticType"], "cache_miss");
        assert_eq!(result["providerCacheMissReason"], "input_changed");
        assert_eq!(result["providerCacheMissedTokens"], 1_024);
        assert_eq!(result["providerComparisonReusableTokens"], 3_072);
        assert_eq!(result["cacheMissAttribution"], "provider-reported");
    }
}
