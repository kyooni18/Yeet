//! Cross-window execution truth for one agent turn.
//!
//! Context windows may roll over while the coordinator continues the same task.
//! These helpers preserve mutation/verification facts across that boundary and
//! keep coordinator-authored completion warnings consistent with executed work.

use serde_json::Value;

use crate::core::{Message, ToolCall};

pub(super) fn rollover_handoff_message(
    successful_mutations: usize,
    unresolved_failed_mutation: bool,
    verification_attempted: bool,
    verification_succeeded: bool,
    last_validation_evidence: Option<&str>,
    working_state: Option<&str>,
    recent_execution_evidence: &[String],
) -> Message {
    let verification = if verification_succeeded {
        "passed"
    } else if verification_attempted {
        "attempted-but-not-passed"
    } else {
        "not-yet-attempted"
    };
    let mut text = format!(
        "Internal context rollover handoff. Continue the same task from this state; do not restart broad repository discovery. successfulWorkspaceMutations={successful_mutations}; unresolvedFailedMutation={unresolved_failed_mutation}; verification={verification}."
    );
    if let Some(validation) = last_validation_evidence {
        text.push_str("\nLast validation: ");
        text.push_str(validation);
    }
    if let Some(state) = working_state.filter(|state| !state.trim().is_empty()) {
        text.push_str("\nWorking evidence state:\n");
        text.push_str(state);
    }
    if !recent_execution_evidence.is_empty() {
        text.push_str("\nRecent execution evidence:\n");
        for evidence in recent_execution_evidence {
            text.push_str("- ");
            text.push_str(evidence);
            text.push('\n');
        }
    }
    Message::system(text)
}

pub(super) fn summarize_tool_outcome(call: &ToolCall, content: &str, succeeded: bool) -> String {
    let subject = match call.name.as_str() {
        "run_shell" => call
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .map(|command| format!("run_shell `{}`", bounded_text(command, 240)))
            .unwrap_or_else(|| "run_shell".into()),
        "apply_file_edits" => {
            let paths = call
                .arguments
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .take(8)
                .collect::<Vec<_>>();
            if paths.is_empty() {
                "apply_file_edits".into()
            } else {
                format!("apply_file_edits [{}]", paths.join(", "))
            }
        }
        other => other.to_owned(),
    };
    let mut details = vec![format!("succeeded={succeeded}")];
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        if let Some(exit) = value.get("exitCode").and_then(Value::as_i64) {
            details.push(format!("exit={exit}"));
        }
        if value.get("duplicate").and_then(Value::as_bool) == Some(true) {
            details.push("duplicate=true".into());
        }
        if value.get("blockedReplay").and_then(Value::as_bool) == Some(true) {
            details.push("blockedReplay=true".into());
        }
        for key in ["reason", "error", "summary", "stderr"] {
            if let Some(value) = value.get(key).and_then(Value::as_str) {
                let value = bounded_text(value, if key == "stderr" { 320 } else { 220 });
                if !value.trim().is_empty() {
                    details.push(format!("{key}={value}"));
                }
            }
        }
    }
    bounded_text(&format!("{subject}: {}", details.join("; ")), 900)
}

pub(super) fn completion_warning(
    blocker: &str,
    successful_mutations: usize,
    last_validation_evidence: Option<&str>,
) -> String {
    let mut warning = format!("Verification incomplete: {blocker}.");
    if successful_mutations > 0 {
        warning.push_str(&format!(
            " {successful_mutations} workspace mutation(s) succeeded and remain part of the task state."
        ));
    }
    if let Some(validation) = last_validation_evidence {
        warning.push_str(" Last validation: ");
        warning.push_str(validation);
    }
    warning
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut iter = value.chars();
    let mut text = iter.by_ref().take(max_chars).collect::<String>();
    if iter.next().is_some() {
        text.push('…');
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
