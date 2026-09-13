//! Cross-window execution truth for one agent turn.
//!
//! Context windows may roll over while the coordinator continues the same task.
//! These helpers preserve mutation/verification facts across that boundary and
//! keep coordinator-authored completion warnings consistent with executed work.

use serde_json::{Value, json};

use crate::core::{Message, ToolCall, Usage};

const MAX_PROVENANCE_ENTRIES: usize = 24;
const MAX_PROVENANCE_CHANGE_DETAILS: usize = 16;

/// Session-local execution evidence used to keep final change claims honest.
///
/// Whole-worktree inspection can contain dirty state from before this turn or
/// concurrent actors. Only calls that the coordinator observed changing the
/// workspace are admitted here, and structured edits are kept separate from
/// write-capable actions whose exact source-file effects are unknown.
#[derive(Debug, Default)]
pub(super) struct SessionExecutionProvenance {
    exact_mutations: Vec<String>,
    write_capable_actions: Vec<String>,
    validations: Vec<String>,
    omitted_exact_mutations: usize,
    omitted_write_actions: usize,
    omitted_validations: usize,
}

impl SessionExecutionProvenance {
    pub(super) fn observe_tool(
        &mut self,
        call: &ToolCall,
        content: &str,
        succeeded: bool,
        workspace_mutated: bool,
        validation_call: bool,
    ) {
        if workspace_mutated {
            // Validation evidence only applies to the workspace generation it
            // observed. Any later write-capable action invalidates it.
            self.validations.clear();
            self.omitted_validations = 0;
            if call.name == "apply_file_edits" {
                push_bounded_provenance(
                    &mut self.exact_mutations,
                    &mut self.omitted_exact_mutations,
                    summarize_structured_mutation(call),
                );
            } else {
                push_bounded_provenance(
                    &mut self.write_capable_actions,
                    &mut self.omitted_write_actions,
                    summarize_tool_outcome(call, content, succeeded),
                );
            }
        }
        if validation_call {
            push_bounded_provenance(
                &mut self.validations,
                &mut self.omitted_validations,
                summarize_tool_outcome(call, content, succeeded),
            );
        }
    }

    pub(super) fn request_overlay(&self) -> Option<Message> {
        if self.exact_mutations.is_empty()
            && self.write_capable_actions.is_empty()
            && self.validations.is_empty()
        {
            return None;
        }

        let mut text = String::from(
            "Internal session-local execution provenance checkpoint for this user turn. This checkpoint supersedes earlier provenance checkpoints in the same turn for current validation status; exact mutation entries are cumulative unless explicitly omitted. Use the newest checkpoint as the authority for attributing work performed by this turn. Repository status/diff output and source contents can include pre-existing or concurrent changes; observation alone does not establish authorship. In the final answer, claim only the exact source mutations listed below as changes performed by this turn. For write-capable actions with unknown scope, report only that the action ran unless separate exact mutation evidence exists.",
        );
        append_provenance_section(
            &mut text,
            "Exact source mutations performed by this turn",
            &self.exact_mutations,
            self.omitted_exact_mutations,
        );
        append_provenance_section(
            &mut text,
            "Other write-capable actions (not exact source-change provenance)",
            &self.write_capable_actions,
            self.omitted_write_actions,
        );
        append_provenance_section(
            &mut text,
            "Current-generation validation attempts",
            &self.validations,
            self.omitted_validations,
        );
        Some(Message::system(text).request_only())
    }
}

fn push_bounded_provenance(entries: &mut Vec<String>, omitted: &mut usize, value: String) {
    if entries.len() == MAX_PROVENANCE_ENTRIES {
        entries.remove(0);
        *omitted += 1;
    }
    entries.push(value);
}

fn append_provenance_section(text: &mut String, title: &str, entries: &[String], omitted: usize) {
    if entries.is_empty() && omitted == 0 {
        return;
    }
    text.push('\n');
    text.push_str(title);
    text.push_str(":\n");
    if omitted > 0 {
        text.push_str(&format!(
            "- {omitted} earlier entries omitted from this compact ledger; do not invent their details.\n"
        ));
    }
    for entry in entries {
        text.push_str("- ");
        text.push_str(entry);
        text.push('\n');
    }
}

fn summarize_structured_mutation(call: &ToolCall) -> String {
    let Some(changes) = call.arguments.get("changes").and_then(Value::as_array) else {
        return "apply_file_edits: structured mutation succeeded; exact change arguments unavailable"
            .into();
    };
    let mut details = changes
        .iter()
        .take(MAX_PROVENANCE_CHANGE_DETAILS)
        .map(summarize_structured_change)
        .collect::<Vec<_>>();
    if changes.len() > MAX_PROVENANCE_CHANGE_DETAILS {
        details.push(format!(
            "{} additional change object(s) omitted",
            changes.len() - MAX_PROVENANCE_CHANGE_DETAILS
        ));
    }
    if details.is_empty() {
        "apply_file_edits: structured mutation succeeded; no change details present".into()
    } else {
        format!("apply_file_edits: {}", details.join("; "))
    }
}

fn summarize_structured_change(change: &Value) -> String {
    let path = change
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("<unknown path>");
    let mut operations = Vec::new();
    if let Some(file_op) = change.get("fileOp").and_then(Value::as_object) {
        let kind = file_op
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("file-op");
        let mut detail = kind.to_owned();
        if let Some(destination) = file_op.get("destination").and_then(Value::as_str) {
            detail.push_str(" -> ");
            detail.push_str(destination);
        }
        if let Some(value) = file_op.get("text").and_then(Value::as_str) {
            detail.push_str(" text=");
            detail.push_str(&format!("{:?}", bounded_text(value, 160)));
        }
        operations.push(detail);
    }
    if let Some(edits) = change.get("edits").and_then(Value::as_array) {
        for edit in edits.iter().take(MAX_PROVENANCE_CHANGE_DETAILS) {
            operations.push(summarize_structured_edit(edit));
        }
        if edits.len() > MAX_PROVENANCE_CHANGE_DETAILS {
            operations.push(format!(
                "{} additional edit(s) omitted",
                edits.len() - MAX_PROVENANCE_CHANGE_DETAILS
            ));
        }
    }
    if operations.is_empty() {
        format!("{path}: structured change")
    } else {
        format!("{path}: {}", operations.join(", "))
    }
}

fn summarize_structured_edit(edit: &Value) -> String {
    let kind = edit.get("kind").and_then(Value::as_str).unwrap_or("edit");
    let range = edit.get("range").and_then(Value::as_object);
    let mut detail = kind.to_owned();
    if let Some(range) = range {
        let start = range.get("start").and_then(Value::as_u64);
        let end = range.get("end").and_then(Value::as_u64);
        if let (Some(start), Some(end)) = (start, end) {
            detail.push_str(&format!(" lines {start}-{end}"));
        }
    }
    if let Some(value) = edit.get("text").and_then(Value::as_str) {
        detail.push_str(" text=");
        detail.push_str(&format!("{:?}", bounded_text(value, 160)));
    }
    detail
}

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

pub(super) fn analysis_inspection_threshold(bounded_explanation: bool) -> usize {
    if bounded_explanation { 3 } else { 4 }
}

pub(super) fn is_initial_mutation_attempt(
    implementation_requested: bool,
    successful_mutations: usize,
    unresolved_failed_mutation: bool,
    calls: &[ToolCall],
) -> bool {
    implementation_requested
        && successful_mutations == 0
        && !unresolved_failed_mutation
        && calls
            .iter()
            .any(|call| super::policy::is_mutation_tool(&call.name))
}

pub(super) fn suppressed_tool_events(
    calls: &[ToolCall],
    reason: &str,
) -> Vec<super::api::AgentEvent> {
    calls
        .iter()
        .cloned()
        .map(|call| super::api::AgentEvent::ToolExecutionSuppressed {
            call,
            reason: reason.to_owned(),
        })
        .collect()
}

pub(super) fn implementation_completion_blocker(
    implementation_requested: bool,
    successful_mutations: usize,
    unresolved_failed_mutation: bool,
    planning_or_documentation: bool,
    verification_attempted: bool,
    verification_succeeded: bool,
) -> Option<String> {
    if unresolved_failed_mutation {
        Some(
            "the most recent workspace mutation failure was never resolved by a later successful edit"
                .to_owned(),
        )
    } else if implementation_requested && successful_mutations == 0 {
        Some("implementation was requested, but no workspace mutation completed".to_owned())
    } else if implementation_requested
        && successful_mutations > 0
        && !planning_or_documentation
        && !verification_succeeded
    {
        Some(if verification_attempted {
            "workspace changes were made, but every recognized build/test/check verification failed"
                .to_owned()
        } else {
            "workspace changes were made, but no build/test/check verification completed successfully"
                .to_owned()
        })
    } else {
        None
    }
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

/// Combine immutable request fingerprints with provider-reported cache/billing telemetry.
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
    let transport_attempts = usage.and_then(|usage| usage.transport_attempts);
    let cost_equivalent_input = usage.and_then(|usage| usage.cost_equivalent_input_tokens);
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
        object.insert(
            "costEquivalentInputTokens".into(),
            json!(cost_equivalent_input),
        );
        object.insert("transportAttempts".into(), json!(transport_attempts));
        object.insert("cacheHitRate".into(), json!(cache_hit_rate));
        object.insert("cacheWriteRate".into(), json!(cache_write_rate));
        object.insert(
            "providerCacheDiagnosticType".into(),
            json!(usage.and_then(|usage| usage.provider_cache_diagnostic_type.as_deref())),
        );
        object.insert(
            "providerCacheMissReason".into(),
            json!(usage.and_then(|usage| usage.provider_cache_miss_reason.as_deref())),
        );
        object.insert(
            "providerCacheMissedTokens".into(),
            json!(usage.and_then(|usage| usage.provider_cache_missed_tokens)),
        );
        object.insert(
            "providerComparisonReusableTokens".into(),
            json!(usage.and_then(|usage| usage.provider_comparison_reusable_tokens)),
        );
        let client_reason = request_diagnostics
            .get("cacheEpochReason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let provider_miss = usage.and_then(|usage| usage.provider_cache_diagnostic_type.as_deref())
            == Some("cache_miss");
        let attribution = if status != "miss" {
            None
        } else if provider_miss {
            Some("provider-reported")
        } else if client_reason != "stable" {
            Some("client-surface-change")
        } else {
            Some("stable-provider-miss-unattributed")
        };
        object.insert("cacheMissAttribution".into(), json!(attribution));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: format!("{name}-id"),
            name: name.into(),
            arguments,
        }
    }
    #[test]
    fn session_provenance_does_not_attribute_observed_dirty_worktree() {
        let mut provenance = SessionExecutionProvenance::default();
        let observed = tool_call("run_shell", json!({"command":"inspect src/preexisting.rs"}));
        provenance.observe_tool(&observed, r#"{"exitCode":0}"#, true, false, false);

        let edit = tool_call(
            "apply_file_edits",
            json!({"changes":[{"path":"src/owned.rs","edits":[{"kind":"replace","range":{"start":7,"end":7},"text":"new_value();"}]}]}),
        );
        provenance.observe_tool(&edit, r#"{"diagnostics":[]}"#, true, true, false);
        let check = tool_call("run_shell", json!({"command":"cargo check"}));
        provenance.observe_tool(&check, r#"{"exitCode":0}"#, true, false, true);

        let overlay = provenance.request_overlay().unwrap().content.unwrap();
        assert!(overlay.contains("src/owned.rs"));
        assert!(overlay.contains("replace lines 7-7"));
        assert!(overlay.contains("new_value();"));
        assert!(overlay.contains("cargo check"));
        assert!(!overlay.contains("src/preexisting.rs"));
        assert!(overlay.contains("observation alone does not establish authorship"));
    }
    #[test]
    fn session_provenance_separates_unknown_scope_write_actions() {
        let mut provenance = SessionExecutionProvenance::default();
        let shell = tool_call("run_shell", json!({"command":"./codegen.sh"}));
        provenance.observe_tool(&shell, r#"{"exitCode":0}"#, true, true, false);

        let overlay = provenance.request_overlay().unwrap().content.unwrap();
        assert!(overlay.contains("Other write-capable actions"));
        assert!(overlay.contains("./codegen.sh"));
        assert!(!overlay.contains("Exact source mutations performed by this turn"));
        assert!(overlay.contains("not exact source-change provenance"));
    }

    #[test]
    fn later_mutation_invalidates_earlier_validation_provenance() {
        let mut provenance = SessionExecutionProvenance::default();
        let first = tool_call(
            "apply_file_edits",
            json!({"changes":[{"path":"src/a.rs","fileOp":{"kind":"create","text":"a"}}]}),
        );
        provenance.observe_tool(&first, r#"{"diagnostics":[]}"#, true, true, false);
        let check = tool_call("run_shell", json!({"command":"cargo check"}));
        provenance.observe_tool(&check, r#"{"exitCode":0}"#, true, false, true);
        let second = tool_call(
            "apply_file_edits",
            json!({"changes":[{"path":"src/b.rs","fileOp":{"kind":"create","text":"b"}}]}),
        );
        provenance.observe_tool(&second, r#"{"diagnostics":[]}"#, true, true, false);

        let overlay = provenance.request_overlay().unwrap().content.unwrap();
        assert!(overlay.contains("src/a.rs"));
        assert!(overlay.contains("src/b.rs"));
        assert!(!overlay.contains("cargo check"));
        assert!(!overlay.contains("Current-generation validation attempts"));
    }
}
