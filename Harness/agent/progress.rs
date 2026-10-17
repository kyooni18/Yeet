//! Tool-result success, validation, and progress classification.

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

/// Completed detached checks carry their original command in the structured result.
/// Launch acknowledgements and periodic running snapshots are not verification.
pub(super) fn is_validation_tool_result(call: &ToolCall, content: &str) -> bool {
    if tool_result_is_replay(content) {
        return false;
    }
    if call.name == "shell_job" {
        if !matches!(
            call.arguments.get("action").and_then(Value::as_str),
            Some("check" | "wait")
        ) {
            return false;
        }
        let Ok(value) = serde_json::from_str::<Value>(content) else {
            return false;
        };
        return value.get("status").and_then(Value::as_str) == Some("completed")
            && value
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    is_validation_tool_call(&ToolCall {
                        id: call.id.clone(),
                        name: "run_shell".into(),
                        arguments: serde_json::json!({"command": command}),
                    })
                });
    }
    is_validation_tool_call(call)
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

    // Only count a trailing validation command: a later successful command can
    // mask a failed check. Respect quotes so echoed command text is not evidence.
    // Do not infer validation from pipelines, asynchronous commands, or a
    // conditional fallback that may not have executed. False negatives are safer.
    if command.contains('|') || command.replace("&&", "").contains('&') {
        return false;
    }
    let segments = crate::shell::split_shell_segments(&command);
    let command = segments.last().map(String::as_str).unwrap_or_default();
    if shell_fragment_is_validation(command) {
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
        "artifact_info" | "read_artifact" | "search_artifact" | "context_status"
        | "context_history" => false,
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
        ] {
            assert!(!tool_made_progress(
                &call(tool, json!({"operation":"read"})),
                r#"{"text":"recovered evidence"}"#,
                true,
            ));
        }
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
    fn validation_recognizes_compound_commands_and_completed_jobs() {
        for command in [
            "cd app && cargo check",
            "pwd; cargo test",
            "cd app\nnpm run build",
        ] {
            assert!(is_validation_tool_call(&call(
                "run_shell",
                json!({"command":command})
            )));
        }
        for command in [
            "echo 'cargo check'",
            "cargo test; echo done",
            "true || cargo test",
            "cargo test | cat",
            "cargo test &",
        ] {
            assert!(!is_validation_tool_call(&call(
                "run_shell",
                json!({"command":command})
            )));
        }
        let job = call("shell_job", json!({"action":"wait","jobId":"check-1"}));
        for status in ["running", "stopping", "forgotten"] {
            assert!(!is_validation_tool_result(
                &job,
                &json!({"status":status,"command":"cargo check"}).to_string()
            ));
        }
        let completed =
            json!({"status":"completed","command":"cargo check","succeeded":true}).to_string();
        assert!(is_validation_tool_result(&job, &completed));
        assert!(!is_validation_tool_result(
            &call("shell_job", json!({"action":"stop"})),
            &completed
        ));
        assert!(!is_validation_tool_result(
            &call(
                "run_shell",
                json!({"command":"cargo check","background":true})
            ),
            "{\"status\":\"running\"}"
        ));
    }

    #[test]
    fn duplicate_replay_is_not_fresh_validation_evidence() {
        assert!(tool_result_is_replay(
            r#"{"succeeded":true,"duplicate":true,"blockedReplay":true}"#
        ));
        assert!(!tool_result_is_replay(r#"{"succeeded":true,"exitCode":0}"#));
    }
}
