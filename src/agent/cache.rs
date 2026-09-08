//! Prompt-cache boundary placement and cache diagnostics.
//!
//! Stable overlays are kept before a deterministic breakpoint so repeated
//! tool rounds can reuse the same provider-side prompt prefix.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::core::{CallRequest, Message, MessageRole};

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

/// Summarizes cache-stable request portions for runtime diagnostics.
pub(super) fn request_cache_diagnostics(request: &CallRequest) -> Value {
    let breakpoint_index = request
        .messages
        .iter()
        .rposition(|message| message.cache_breakpoint == Some(true));
    let stable_history = request
        .messages
        .iter()
        .filter(|message| message.request_only != Some(true))
        .cloned()
        .collect::<Vec<_>>();
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
    json!({
        "contextKey": request.context_key,
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
    })
}

/// Computes a lowercase SHA-256 digest for diagnostic fingerprints.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
