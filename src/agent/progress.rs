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

/// Detects shell calls whose command actually performs post-change validation.
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

    if shell_fragment_is_validation(&command) {
        return true;
    }

    let purpose_claims_validation = [
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
    .any(|needle| purpose.contains(needle));
    purpose_claims_validation
        && [
            " check",
            " test",
            " lint",
            " build",
            " compile",
            " verify",
            " validate",
            "check.sh",
            "test.sh",
            "verify.sh",
        ]
        .iter()
        .any(|needle| command.contains(needle))
}

fn shell_fragment_is_validation(fragment: &str) -> bool {
    let fragment = fragment.trim();
    let validation_commands = [
        "git diff --check",
        "cargo check",
        "cargo test",
        "cargo clippy",
        "cargo build",
        "swift test",
        "swift build",
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
        "python -m py_compile",
        "python3 -m py_compile",
        "python -m compileall",
        "python3 -m compileall",
        "cmake --build",
        "make test",
        "xcodebuild",
        "gradle test",
        "gradle build",
        "mvn test",
        "mvn package",
    ];
    validation_commands.iter().any(|command| {
        fragment == *command
            || fragment
                .strip_prefix(command)
                .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
    }) || ((fragment == "cargo fmt"
        || fragment.starts_with("cargo fmt ")
        || fragment == "rustfmt"
        || fragment.starts_with("rustfmt "))
        && fragment.contains("--check"))
}

/// Replay/duplicate results can be operationally successful, but they are not
/// fresh validation evidence for the current model-visible window.
pub(super) fn tool_result_is_replay(content: &str) -> bool {
    serde_json::from_str::<Value>(content).is_ok_and(|value| {
        value.get("duplicate").and_then(Value::as_bool) == Some(true)
            || value.get("blockedReplay").and_then(Value::as_bool) == Some(true)
    })
}

/// Returns whether a tool primarily inspects rather than mutates state.
pub(super) fn is_inspection_tool(name: &str) -> bool {
    matches!(
        name,
        "search_tools"
            | "find_capabilities"
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
            | "context_status"
            | "context_history"
            | "task_notes"
            | "project_memory_recall"
            | "project_memory_get"
            | "project_memory_connections"
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
        "search_tools" => {
            value
                .get("loaded")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
                || value
                    .get("deferred")
                    .and_then(Value::as_array)
                    .is_some_and(|items| !items.is_empty())
        }
        "artifact_info"
        | "read_artifact"
        | "search_artifact"
        | "context_status"
        | "context_history"
        | "project_memory_recall"
        | "project_memory_get"
        | "project_memory_connections" => false,
        "task_notes" => {
            matches!(
                call.arguments.get("operation").and_then(Value::as_str),
                Some("append" | "replace")
            ) && value.get("saved").is_some()
        }
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
            |searches| {
                searches.iter().any(|search| {
                    search
                        .get("results")
                        .and_then(Value::as_array)
                        .is_some_and(|results| !results.is_empty())
                        || search
                            .get("numberOfResults")
                            .and_then(Value::as_u64)
                            .is_some_and(|count| count > 0)
                })
            },
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
    } else if lower.contains("workspace mutation lease") {
        "workspace_busy"
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
    if call.name == "web_read"
        && let Some(url) = call.arguments.get("url").and_then(Value::as_str)
    {
        // maxChars only changes how much of the already-fetched source is
        // previewed. The source identity is the URL, so a second read with a
        // different preview size is still a duplicate inspection.
        return format!("web_read\0{}", crate::tools::canonical_web_source_key(url));
    }
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
        "web_search" => {
            let mut queries = call
                .arguments
                .get("queries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if queries.is_empty()
                && let Some(query) = call.arguments.get("query").and_then(Value::as_str)
            {
                queries.push(query.trim().to_ascii_lowercase());
            }
            queries.sort();
            queries.dedup();
            format!("web_search:{}", queries.join("|"))
        }
        "web_read" => format!(
            "web_read:{}",
            call.arguments
                .get("url")
                .and_then(Value::as_str)
                .map(crate::tools::canonical_web_source_key)
                .as_deref()
                .unwrap_or("?")
        ),
        "run_shell" => {
            // Different shell commands are different evidence actions. Collapsing every
            // shell invocation into one family made normal inspect/build/test sequences
            // look like a structural loop and could force an early context rollover.
            let command_family = call
                .arguments
                .get("command")
                .and_then(Value::as_str)
                .map(|command| {
                    command
                        .split_whitespace()
                        .take(2)
                        .map(str::to_ascii_lowercase)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "?".into());
            format!("run_shell:{command_family}")
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: format!("{name}-1"),
            name: name.to_owned(),
            arguments,
        }
    }

    #[test]
    fn side_recovery_reads_do_not_count_as_forward_progress() {
        for tool in [
            "artifact_info",
            "read_artifact",
            "search_artifact",
            "context_status",
            "context_history",
            "project_memory_recall",
            "project_memory_get",
            "project_memory_connections",
        ] {
            assert!(!tool_made_progress(
                &call(tool, json!({"operation":"read"})),
                r#"{"text":"recovered evidence"}"#,
                true,
            ));
        }
        assert!(!tool_made_progress(
            &call("task_notes", json!({"operation":"read","name":"state"})),
            r#"{"text":"note"}"#,
            true,
        ));
    }

    #[test]
    fn task_note_write_still_counts_as_progress() {
        assert!(tool_made_progress(
            &call(
                "task_notes",
                json!({"operation":"replace","name":"state","content":"next"}),
            ),
            r#"{"saved":"state"}"#,
            true,
        ));
    }

    #[test]
    fn source_inspection_purpose_does_not_fake_validation() {
        assert!(!is_validation_tool_call(&call(
            "run_shell",
            json!({
                "command":"sed -n '1,220p' src/remote.rs",
                "purpose":"Verify the current remote lifecycle source"
            }),
        )));
        assert!(is_validation_tool_call(&call(
            "run_shell",
            json!({"command":"cargo check","purpose":"Verify compilation"}),
        )));
    }

    #[test]
    fn duplicate_replay_is_not_fresh_validation_evidence() {
        assert!(tool_result_is_replay(
            r#"{"succeeded":true,"duplicate":true,"blockedReplay":true}"#
        ));
        assert!(!tool_result_is_replay(r#"{"succeeded":true,"exitCode":0}"#));
    }
}
