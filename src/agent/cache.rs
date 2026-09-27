//! Prompt-cache boundary placement and cache diagnostics.
//!
//! Keep an append-only canonical user boundary across turns, then add a second
//! turn-local boundary after stable request-only overlays for repeated tool rounds.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::core::Usage;
use crate::core::{CallRequest, Message, MessageRole, ToolDefinition};

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
        let previous_epoch = self.cache_epoch;
        let window = request
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("contextWindowId"))
            .cloned()
            .or_else(|| request.context_key.clone())
            .unwrap_or_default();
        // A lane switch is not a context rollover and cannot authorize rewrites.
        let scope = window;
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
            // Extending a breakpoint never excuses deleting or rewriting an
            // already-submitted suffix, including request-only guidance.
            let cache_relevant_history_rewrite = history_rewritten || wire_history_rewritten;
            let reason = if tool_changed {
                "tool-envelope-change"
            } else if base_rewritten {
                "base-prefix-change"
            } else if wire_history_rewritten && !history_rewritten {
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
        // The caller rejects rewrites before dispatch. Do not adopt a rejected
        // candidate: retrying it must still fail against the last accepted prefix.
        if wire_continues == Some(false) {
            self.cache_epoch = previous_epoch;
            return value;
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

/// Inserts turn-stable overlays while keeping the oldest explicit cache boundary
/// on the canonical user message. The overlay tail gets a second boundary so
/// repeated tool rounds can still cache large turn-local guidance, but the next
/// user turn can reuse the earlier canonical prefix after request-only overlays
/// disappear.
#[cfg(test)]
#[allow(dead_code)] // Called from child test modules under all-target Clippy builds.
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
    // The provider limits *new* cache writes per request, not the number of
    // historical breakpoints that may be used for lookup. Counting every
    // marker in the conversation eventually starves each later turn of
    // rolling tool breakpoints and makes the reusable prefix end too early.
    // Only markers introduced after this turn's canonical user boundary can
    // consume the current turn's write budget.
    let reserved = messages
        .iter()
        .enumerate()
        .filter(|(index, message)| {
            *index > user_index
                && message.cache_breakpoint == Some(true)
                && message.role != MessageRole::Tool
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

    for key in [
        "turnCumulativeRequestChars",
        "turnCostEquivalentInputTokens",
        "turnInputTokensSinceRollover",
        "turnRolloverCount",
        "turnBudgetStage",
        "windowModelCalls",
        "searchLoadedToolCount",
        "deferredToolCount",
        "nativeDeferredToolsSupported",
    ] {
        result.insert(
            key.into(),
            json!(
                request
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get(key))
            ),
        );
    }
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
#[allow(dead_code)] // Called from child test modules under all-target Clippy builds.
pub(super) fn usage_diagnostics(request_diagnostics: &Value, usage: Option<&Usage>) -> Value {
    super::turn_state::usage_diagnostics(request_diagnostics, usage)
}

/// Computes a lowercase SHA-256 digest for diagnostic fingerprints.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod continuity_guard_tests {
    use super::*;

    fn request(window: &str, lane: &str, messages: Vec<Message>) -> CallRequest {
        let mut request = CallRequest::simple("test-model", messages);
        request.context_key = Some(window.into());
        request.metadata = Some(
            [
                ("contextWindowId".into(), window.into()),
                ("lane".into(), lane.into()),
            ]
            .into(),
        );
        request
    }

    #[test]
    fn window_lifecycle_metadata_is_preserved_in_attempt_diagnostics() {
        let mut request = request("window-2", "agent", vec![Message::user("task")]);
        let metadata = request.metadata.as_mut().expect("metadata");
        for (key, value) in [
            ("turnCumulativeRequestChars", "768000"),
            ("turnCostEquivalentInputTokens", "260000"),
            ("turnInputTokensSinceRollover", "42000"),
            ("turnRolloverCount", "3"),
            ("turnBudgetStage", "window-growing"),
            ("windowModelCalls", "6"),
            ("searchLoadedToolCount", "4"),
            ("deferredToolCount", "1"),
            ("nativeDeferredToolsSupported", "true"),
        ] {
            metadata.insert(key.into(), value.into());
        }

        let diagnostics = diagnostics(&request);
        assert_eq!(diagnostics["turnBudgetStage"], json!("window-growing"));
        assert_eq!(diagnostics["turnRolloverCount"], json!("3"));
        assert_eq!(diagnostics["turnCumulativeRequestChars"], json!("768000"));
        assert_eq!(diagnostics["windowModelCalls"], json!("6"));
        assert_eq!(diagnostics["searchLoadedToolCount"], json!("4"));
    }

    #[test]
    fn rejected_rewrite_never_becomes_the_accepted_prefix() {
        let mut tracker = ContinuityTracker::default();
        let original = request(
            "window-1",
            "agent",
            vec![
                Message::user("task"),
                Message::system("submitted guidance").request_only(),
            ],
        );
        tracker.diagnostics(&original);
        let rewritten = request("window-1", "agent", vec![Message::user("task")]);
        for _ in 0..2 {
            let diagnostics = tracker.diagnostics(&rewritten);
            assert_eq!(diagnostics["wireHistoryPrefixRewriteDetected"], json!(true));
            assert_eq!(
                diagnostics["cacheRelevantHistoryRewriteDetected"],
                json!(true)
            );
        }
        let mut continued = original;
        continued
            .messages
            .push(Message::assistant("new response", None));
        let diagnostics = tracker.diagnostics(&continued);
        assert_eq!(diagnostics["wireHistoryPrefixContinues"], json!(true));
        assert_eq!(diagnostics["cacheEpochReason"], json!("stable"));
        assert_eq!(diagnostics["cacheEpoch"], json!(1));
    }

    #[test]
    fn lane_changes_cannot_hide_a_same_window_rewrite() {
        let mut tracker = ContinuityTracker::default();
        tracker.diagnostics(&request(
            "window-1",
            "agent",
            vec![Message::user("original")],
        ));
        let diagnostics = tracker.diagnostics(&request(
            "window-1",
            "research",
            vec![Message::user("rewritten")],
        ));
        assert_eq!(diagnostics["wireHistoryPrefixRewriteDetected"], json!(true));
    }

    #[test]
    fn explicit_window_rollover_allows_a_new_prefix() {
        let mut tracker = ContinuityTracker::default();
        tracker.diagnostics(&request(
            "window-1",
            "agent",
            vec![Message::user("original")],
        ));
        let next = request("window-2", "agent", vec![Message::user("handoff")]);
        let diagnostics = tracker.diagnostics(&next);
        assert_eq!(
            diagnostics["wireHistoryPrefixRewriteDetected"],
            json!(false)
        );
        assert_eq!(diagnostics["cacheEpochReason"], json!("window-start"));
        assert_eq!(
            tracker.diagnostics(&next)["wireHistoryPrefixContinues"],
            json!(true)
        );
    }

    #[test]
    fn advancing_breakpoint_does_not_excuse_an_old_tool_result_rewrite() {
        let mut tracker = ContinuityTracker::default();
        let mut original = request(
            "window-1",
            "agent",
            vec![
                Message::user("task"),
                Message::tool("original evidence", "call-1", Some("read_file".into())),
            ],
        );
        original.messages[0].cache_breakpoint = Some(true);
        tracker.diagnostics(&original);
        original.messages[0].cache_breakpoint = None;
        original.messages[1].cache_breakpoint = Some(true);
        assert_eq!(
            tracker.diagnostics(&original)["wireHistoryPrefixContinues"],
            json!(true)
        );
        original.messages[1].content = Some("short summary".into());
        assert_eq!(
            tracker.diagnostics(&original)["cacheRelevantHistoryRewriteDetected"],
            json!(true)
        );
    }
}
