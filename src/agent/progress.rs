//! Tool-result progress classification and loop fingerprints.
//!
//! These helpers convert heterogeneous tool results into stable success,
//! validation, novelty, and failure signals consumed by the agent loop.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde_json::Value;

use crate::core::ToolCall;

/// Interprets transport and structured result fields as execution success.
pub(super) fn tool_execution_succeeded(
    call: &ToolCall,
    content: &str,
    transport_succeeded: bool,
) -> bool {
    if !transport_succeeded {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return true;
    };
    if value.get("error").is_some()
        || value.get("failed").and_then(Value::as_bool) == Some(true)
        || value.get("blocked").and_then(Value::as_bool) == Some(true)
    {
        return false;
    }
    if call.name == "run_shell" || call.name == "shell_job" {
        return value
            .get("succeeded")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    }
    true
}

/// Detects shell calls whose purpose is post-change validation.
pub(super) fn is_validation_tool_call(call: &ToolCall) -> bool {
    if call.name != "run_shell"
        || call.arguments.get("background").and_then(Value::as_bool) == Some(true)
    {
        return false;
    }
    let command = call
        .arguments
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let purpose = call
        .arguments
        .get("purpose")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let combined = format!("{purpose}\n{command}");
    [
        "git diff --check",
        "cargo check",
        "cargo test",
        "cargo clippy",
        "swift test",
        "go test",
        "pytest",
        "npm test",
        "npm run test",
        "npm run build",
        "npm run lint",
        "pnpm test",
        "pnpm run test",
        "pnpm build",
        "pnpm run build",
        "pnpm lint",
        "yarn test",
        "yarn build",
        "bun test",
        "tsc --noemit",
        "ruff check",
        "python -m compileall",
        "cmake --build",
        "make test",
    ]
    .iter()
    .any(|needle| combined.contains(needle))
        || [
            "verify",
            "verification",
            "validate",
            "validation",
            "compile",
            "build",
            "test",
            "lint",
            "check",
        ]
        .iter()
        .any(|needle| purpose.contains(needle))
}

/// Returns whether a tool primarily inspects rather than mutates state.
pub(super) fn is_inspection_tool(name: &str) -> bool {
    matches!(
        name,
        "find_capabilities"
            | "activate_capability"
            | "list_files"
            | "read_file"
            | "search_workspace"
            | "web_search"
            | "web_read"
            | "read_document"
            | "analyze_data"
            | "artifact_info"
            | "read_artifact"
            | "search_artifact"
    )
}

/// Determines whether a successful call produced novel evidence or mutation.
pub(super) fn tool_made_progress(call: &ToolCall, content: &str, succeeded: bool) -> bool {
    if !succeeded {
        return false;
    }
    let value: Value = serde_json::from_str(content).unwrap_or(Value::Null);
    match call.name.as_str() {
        "apply_file_edits" => true,
        "find_capabilities" => value.as_array().is_some_and(|values| !values.is_empty()),
        "activate_capability" => {
            value.get("alreadyActive").and_then(Value::as_bool) != Some(true)
                && value.get("error").is_none()
        }
        "read_file" | "read_files" => value.as_array().map_or_else(
            || payload_made_progress(&value),
            |values| values.iter().any(payload_made_progress),
        ),
        "web_search" => value.get("searches").and_then(Value::as_array).map_or_else(
            || payload_made_progress(&value),
            |values| values.iter().any(payload_made_progress),
        ),
        "run_shell" => {
            payload_made_progress(&value)
                && value
                    .get("succeeded")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
        }
        _ => payload_made_progress(&value),
    }
}

/// Maps raw tool failures into stable categories for retry decisions.
pub(super) fn classify_tool_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("unsupported_value")
        || lower.contains("unsupported value")
        || lower.contains("reasoning.effort")
        || lower.contains("invalid model parameter")
    {
        "provider_configuration"
    } else if lower.contains("active yeet runtime state")
        || lower.contains("active session") && lower.contains("refus")
    {
        "runtime_state_protected"
    } else if lower.contains("disabled for this session")
        || lower.contains("unknown direct tool")
        || lower.contains("not a read-only research tool")
    {
        "not_available"
    } else if lower.contains("approval") || lower.contains("permission") {
        "approval_required"
    } else if lower.contains("sandbox")
        || lower.contains("outside-project")
        || lower.contains("outside project")
    {
        "sandbox_denied"
    } else if lower.contains("cancel") || lower.contains("interrupt") {
        "cancelled"
    } else if lower.contains("snapshot") || lower.contains("anchor") || lower.contains("stale") {
        "stale_source"
    } else {
        "tool_failed"
    }
}

/// Returns whether a generic structured payload represents useful new work.
fn payload_made_progress(value: &Value) -> bool {
    value.get("duplicate").and_then(Value::as_bool) != Some(true)
        && value.get("contentOmitted").and_then(Value::as_bool) != Some(true)
        && value.get("failed").and_then(Value::as_bool) != Some(true)
        && value.get("blocked").and_then(Value::as_bool) != Some(true)
        && value.get("error").is_none()
}

/// Produces an exact call signature for duplicate-call detection.
pub(super) fn tool_signature(call: &ToolCall) -> String {
    format!("{}\0{}", call.name, call.arguments)
}

/// Produces an order-independent semantic fingerprint for a tool round.
pub(super) fn round_semantic_fingerprint(calls: &[ToolCall]) -> String {
    let mut families = calls.iter().map(semantic_tool_family).collect::<Vec<_>>();
    families.sort();
    families.join("|")
}

/// Reduces a call to the resource/action family relevant for loop detection.
fn semantic_tool_family(call: &ToolCall) -> String {
    let path = call
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase());
    match call.name.as_str() {
        "search_workspace" => format!("search_workspace:{}", path.unwrap_or_else(|| ".".into())),
        "read_file" | "read_files" => {
            let mut paths = call
                .arguments
                .get("requests")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|request| request.get("path").and_then(Value::as_str))
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                paths.push(path.unwrap_or_else(|| "?".into()));
            }
            paths.sort();
            paths.dedup();
            format!("read_file:{}", paths.join(","))
        }
        "list_files" => format!("list_files:{}", path.unwrap_or_else(|| ".".into())),
        "web_search" => "web_search".into(),
        "web_read" => "web_read".into(),
        "run_shell" => "run_shell".into(),
        "apply_file_edits" => {
            let mut paths = call
                .arguments
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .map(|value| value.to_ascii_lowercase())
                .collect::<Vec<_>>();
            paths.sort();
            paths.dedup();
            format!("apply_file_edits:{}", paths.join(","))
        }
        other => other.to_owned(),
    }
}

/// Produces a stable failure fingerprint from a call and its error payload.
pub(super) fn tool_failure_fingerprint(call: &ToolCall, content: &str) -> String {
    let reason = serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("reason")
                .and_then(Value::as_str)
                .or_else(|| value.get("error").and_then(Value::as_str))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| content.chars().take(160).collect());
    format!(
        "{}:{}",
        semantic_tool_family(call),
        normalize_loop_text(&reason)
    )
}

/// Hashes normalized output for repeated-result detection.
pub(super) fn content_fingerprint(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    normalize_loop_text(content).hash(&mut hasher);
    hasher.finish()
}

/// Bounds and normalizes free-form text before comparison.
fn normalize_loop_text(value: &str) -> String {
    value
        .split_whitespace()
        .take(256)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
