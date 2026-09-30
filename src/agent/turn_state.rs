//! Cross-window execution truth for one agent turn.
//!
//! Context windows may roll over while the coordinator continues the same task.
//! These helpers preserve mutation/verification facts across that boundary and
//! keep coordinator-authored completion warnings consistent with executed work.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::core::{Message, ToolCall, Usage};

use super::{
    context::ContextMemory,
    loop_budget::LoopBudget,
    phase::{self, AgentPhase, PhaseState},
    policy::TaskProfile,
};

const MAX_PROVENANCE_ENTRIES: usize = 24;
const MAX_PROVENANCE_CHANGE_DETAILS: usize = 16;
const MAX_RECENT_EXECUTION_EVIDENCE: usize = 8;
const MAX_RECENT_INSPECTION_EVIDENCE: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
struct InspectionEvidenceLocator {
    summary: String,
    paths: Vec<String>,
}

/// Authoritative execution facts for one agent turn.
///
/// The coordinator may decide strategy, but mutation authorship, validation
/// freshness, and evidence retention belong here so Goal judging, completion
/// gating, rollover, and final-answer provenance all consume the same state.
#[derive(Debug, Default)]
pub(super) struct TurnExecutionEvidence {
    observed_tool_calls: usize,
    successful_mutations: usize,
    unresolved_failed_mutation: bool,
    verification_attempted: bool,
    verification_succeeded: bool,
    last_validation_evidence: Option<String>,
    recent_execution_evidence: Vec<String>,
    recent_inspection_evidence: Vec<InspectionEvidenceLocator>,
    provenance: SessionExecutionProvenance,
}

impl TurnExecutionEvidence {
    pub(super) fn successful_mutations(&self) -> usize {
        self.successful_mutations
    }

    pub(super) fn unresolved_failed_mutation(&self) -> bool {
        self.unresolved_failed_mutation
    }

    pub(super) fn verification_attempted(&self) -> bool {
        self.verification_attempted
    }

    pub(super) fn verification_succeeded(&self) -> bool {
        self.verification_succeeded
    }

    pub(super) fn last_validation_evidence(&self) -> Option<&str> {
        self.last_validation_evidence.as_deref()
    }

    pub(super) fn recent_evidence(&self) -> &[String] {
        &self.recent_execution_evidence
    }

    pub(super) fn recommended_phase(
        &self,
        implementation_requested: bool,
        planning_or_documentation: bool,
    ) -> AgentPhase {
        phase::recommend(
            PhaseState {
                observed_tool_calls: self.observed_tool_calls,
                successful_mutations: self.successful_mutations,
                unresolved_failed_mutation: self.unresolved_failed_mutation,
                verification_succeeded: self.verification_succeeded,
                has_inspection_evidence: !self.recent_inspection_evidence.is_empty(),
            },
            implementation_requested,
            planning_or_documentation,
        )
    }

    pub(super) fn request_overlay(&mut self) -> Option<Message> {
        self.provenance.request_overlay()
    }

    pub(super) fn observe_tool(
        &mut self,
        call: &ToolCall,
        content: &str,
        succeeded: bool,
        workspace_write_observed: bool,
        validation_call: bool,
        mutation_tool: bool,
    ) {
        self.observed_tool_calls = self.observed_tool_calls.saturating_add(1);
        if workspace_write_observed {
            // Validation applies only to the workspace generation it observed.
            self.verification_attempted = false;
            self.verification_succeeded = false;
            self.last_validation_evidence = None;
            if succeeded {
                self.successful_mutations = self.successful_mutations.saturating_add(1);
                self.unresolved_failed_mutation = false;
            } else {
                // A failed or timed-out write-capable command may have partially changed state.
                self.unresolved_failed_mutation = true;
            }
        } else if !succeeded && mutation_tool {
            self.unresolved_failed_mutation = true;
        }

        if workspace_write_observed {
            invalidate_inspection_evidence(&mut self.recent_inspection_evidence, call);
        } else if succeeded
            && !validation_call
            && let Some(locator) = summarize_inspection_locator(call, content)
        {
            if let Some(index) = self
                .recent_inspection_evidence
                .iter()
                .position(|existing| existing.summary == locator.summary)
            {
                self.recent_inspection_evidence.remove(index);
            }
            self.recent_inspection_evidence.push(locator);
            if self.recent_inspection_evidence.len() > MAX_RECENT_INSPECTION_EVIDENCE {
                self.recent_inspection_evidence.remove(0);
            }
        }

        self.provenance.observe_tool(
            call,
            content,
            succeeded,
            workspace_write_observed,
            validation_call,
        );

        if validation_call {
            self.verification_attempted = true;
            self.last_validation_evidence = Some(summarize_tool_outcome(call, content, succeeded));
            if succeeded {
                self.verification_succeeded = true;
            }
        }

        if workspace_write_observed || !succeeded || validation_call {
            self.recent_execution_evidence
                .push(summarize_tool_outcome(call, content, succeeded));
            if self.recent_execution_evidence.len() > MAX_RECENT_EXECUTION_EVIDENCE {
                self.recent_execution_evidence.remove(0);
            }
        }
    }

    pub(super) fn completion_blocker(
        &self,
        implementation_requested: bool,
        planning_or_documentation: bool,
    ) -> Option<String> {
        implementation_completion_blocker(
            implementation_requested,
            self.successful_mutations,
            self.unresolved_failed_mutation,
            planning_or_documentation,
            self.verification_attempted,
            self.verification_succeeded,
        )
    }

    pub(super) fn completion_warning(&self, blocker: &str) -> String {
        completion_warning(
            blocker,
            self.successful_mutations,
            self.last_validation_evidence(),
        )
    }

    pub(super) fn rollover_handoff(&self, working_state: Option<&str>) -> Message {
        rollover_handoff_message(
            self.successful_mutations,
            self.unresolved_failed_mutation,
            self.verification_attempted,
            self.verification_succeeded,
            self.last_validation_evidence(),
            working_state,
            &self.recent_execution_evidence,
            &self.recent_inspection_evidence,
        )
    }

    /// Structured evidence handed to the semantic Goal judge.
    ///
    /// This intentionally excludes assistant/user prose. Tool observations and
    /// exact provenance remain separately available, but completion facts come
    /// from coordinator-observed execution state rather than model claims.
    pub(super) fn goal_evidence(&self) -> Value {
        json!({
            "agentPhase": self.recommended_phase(true, false).as_str(),
            "observedToolCalls": self.observed_tool_calls,
            "successfulWorkspaceMutations": self.successful_mutations,
            "unresolvedFailedMutation": self.unresolved_failed_mutation,
            "verification": {
                "attempted": self.verification_attempted,
                "succeeded": self.verification_succeeded,
                "lastEvidence": self.last_validation_evidence,
            },
            "recentExecutionEvidence": self.recent_execution_evidence,
            "recentInspectionEvidence": self.recent_inspection_evidence.iter().map(|item| item.summary.as_str()).collect::<Vec<_>>(),
            "provenance": self.provenance.goal_evidence(),
        })
    }
}

pub(super) struct RequestMetadataInput<'a> {
    pub profile: TaskProfile,
    pub context_key: Option<&'a str>,
    pub context_memory: &'a ContextMemory,
    pub model_attempts: usize,
    pub tool_rounds: usize,
    pub loop_budget: &'a LoopBudget,
    pub working_budget: u64,
    pub deferred_tool_count: usize,
    pub native_deferred_tools_supported: bool,
    pub search_loaded_tool_count: usize,
    pub execution_phase: &'a str,
    pub successful_mutations: usize,
    pub unresolved_failed_mutation: bool,
    pub verification_attempted: bool,
    pub verification_succeeded: bool,
    pub turn_start_pruned_request_only_messages: usize,
    pub turn_start_pruned_request_only_chars: usize,
}

pub(super) fn request_metadata(input: RequestMetadataInput<'_>) -> HashMap<String, String> {
    HashMap::from([
        (
            "lane".into(),
            match input.profile {
                TaskProfile::Agent => "agent",
                TaskProfile::Research => "research",
            }
            .into(),
        ),
        ("contextManagement".into(), "recoverable-windows".into()),
        (
            "cacheFamily".into(),
            input.context_key.unwrap_or_default().to_owned(),
        ),
        ("agentId".into(), "root".into()),
        (
            "sessionId".into(),
            input
                .context_memory
                .session_id()
                .unwrap_or(input.context_key.unwrap_or_default())
                .to_owned(),
        ),
        (
            "contextWindowId".into(),
            input.context_memory.id().to_owned(),
        ),
        (
            "contextWindowNumber".into(),
            input.context_memory.number().to_string(),
        ),
        ("modelAttempt".into(), input.model_attempts.to_string()),
        ("toolRound".into(), input.tool_rounds.to_string()),
        ("agentCoreVersion".into(), "2".into()),
        ("agentPhase".into(), input.execution_phase.to_owned()),
        (
            "successfulWorkspaceMutations".into(),
            input.successful_mutations.to_string(),
        ),
        (
            "unresolvedFailedMutation".into(),
            input.unresolved_failed_mutation.to_string(),
        ),
        (
            "verificationAttempted".into(),
            input.verification_attempted.to_string(),
        ),
        (
            "verificationSucceeded".into(),
            input.verification_succeeded.to_string(),
        ),
        ("expectedCacheReuses".into(), "1".into()),
        (
            "turnStartPrunedRequestOnlyMessages".into(),
            input.turn_start_pruned_request_only_messages.to_string(),
        ),
        (
            "turnStartPrunedRequestOnlyChars".into(),
            input.turn_start_pruned_request_only_chars.to_string(),
        ),
        (
            "turnCumulativeInputTokens".into(),
            input.loop_budget.cumulative_input_tokens().to_string(),
        ),
        (
            "turnCumulativeRequestChars".into(),
            input
                .loop_budget
                .cumulative_sent_request_chars()
                .to_string(),
        ),
        (
            "turnCostEquivalentInputTokens".into(),
            input
                .loop_budget
                .cumulative_cost_equivalent_input_tokens()
                .to_string(),
        ),
        (
            "turnInputTokensSinceRollover".into(),
            input.loop_budget.input_tokens_since_rollover().to_string(),
        ),
        (
            "turnRolloverCount".into(),
            input.loop_budget.rollover_count().to_string(),
        ),
        (
            "turnBudgetStage".into(),
            input.loop_budget.stage_label().to_owned(),
        ),
        (
            "windowModelCalls".into(),
            input.loop_budget.window_model_calls().to_string(),
        ),
        (
            "turnEstimatedCostUsd".into(),
            format!("{:.6}", input.loop_budget.estimated_cost_usd()),
        ),
        (
            "peakRequestChars".into(),
            input.loop_budget.peak_request_chars().to_string(),
        ),
        (
            "contextEstimatedTokens".into(),
            input.context_memory.estimated_tokens.to_string(),
        ),
        (
            "contextWorkingBudget".into(),
            input.working_budget.to_string(),
        ),
        (
            "deferredToolCount".into(),
            input.deferred_tool_count.to_string(),
        ),
        (
            "nativeDeferredToolsSupported".into(),
            input.native_deferred_tools_supported.to_string(),
        ),
        (
            "searchLoadedToolCount".into(),
            input.search_loaded_tool_count.to_string(),
        ),
        ("yeetVersion".into(), env!("CARGO_PKG_VERSION").to_owned()),
    ])
}

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
    pending_exact_mutations: Vec<String>,
    pending_write_actions: Vec<String>,
    pending_validations: Vec<String>,
    pending_validation_reset: bool,
}

impl SessionExecutionProvenance {
    pub(super) fn observe_tool(
        &mut self,
        call: &ToolCall,
        content: &str,
        succeeded: bool,
        workspace_write_observed: bool,
        validation_call: bool,
    ) {
        if workspace_write_observed {
            // Validation evidence only applies to the workspace generation it
            // observed. A failed write-capable action may still have partial effects.
            self.validations.clear();
            self.omitted_validations = 0;
            self.pending_validation_reset = true;
            if succeeded && call.name == "apply_file_edits" {
                let summary = summarize_structured_mutation(call);
                push_bounded_provenance(
                    &mut self.exact_mutations,
                    &mut self.omitted_exact_mutations,
                    summary.clone(),
                );
                push_pending_provenance(&mut self.pending_exact_mutations, summary);
            } else {
                let summary = summarize_tool_outcome(call, content, succeeded);
                push_bounded_provenance(
                    &mut self.write_capable_actions,
                    &mut self.omitted_write_actions,
                    summary.clone(),
                );
                push_pending_provenance(&mut self.pending_write_actions, summary);
            }
        }
        if validation_call {
            let summary = summarize_tool_outcome(call, content, succeeded);
            push_bounded_provenance(
                &mut self.validations,
                &mut self.omitted_validations,
                summary.clone(),
            );
            push_pending_provenance(&mut self.pending_validations, summary);
        }
    }

    fn goal_evidence(&self) -> Value {
        json!({
            "exactMutations": self.exact_mutations,
            "writeCapableActions": self.write_capable_actions,
            "currentGenerationValidations": self.validations,
            "omitted": {
                "exactMutations": self.omitted_exact_mutations,
                "writeCapableActions": self.omitted_write_actions,
                "validations": self.omitted_validations,
            }
        })
    }

    pub(super) fn request_overlay(&mut self) -> Option<Message> {
        let exact_mutations = std::mem::take(&mut self.pending_exact_mutations);
        let write_capable_actions = std::mem::take(&mut self.pending_write_actions);
        let validations = std::mem::take(&mut self.pending_validations);
        let validation_reset = std::mem::replace(&mut self.pending_validation_reset, false);
        if exact_mutations.is_empty()
            && write_capable_actions.is_empty()
            && validations.is_empty()
            && !validation_reset
        {
            return None;
        }

        let mut text = String::from(
            "Coordinator execution provenance delta (observed facts; not instructions).",
        );
        if validation_reset {
            text.push_str("\nEarlier validation evidence was invalidated by a later write.");
        }
        append_provenance_section(
            &mut text,
            "New exact source mutations performed by this turn",
            &exact_mutations,
            0,
        );
        append_provenance_section(
            &mut text,
            "New write-capable actions (not exact source-change provenance)",
            &write_capable_actions,
            0,
        );
        append_provenance_section(
            &mut text,
            "New current-generation validation attempts",
            &validations,
            0,
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

fn push_pending_provenance(entries: &mut Vec<String>, value: String) {
    if entries.len() == MAX_PROVENANCE_ENTRIES {
        entries.remove(0);
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

#[allow(clippy::too_many_arguments)] // Turn accounting updates one cohesive state transition.
fn rollover_handoff_message(
    successful_mutations: usize,
    unresolved_failed_mutation: bool,
    verification_attempted: bool,
    verification_succeeded: bool,
    last_validation_evidence: Option<&str>,
    working_state: Option<&str>,
    recent_execution_evidence: &[String],
    recent_inspection_evidence: &[InspectionEvidenceLocator],
) -> Message {
    let verification = if verification_succeeded {
        "passed"
    } else if verification_attempted {
        "attempted-but-not-passed"
    } else {
        "not-yet-attempted"
    };
    let mut text = format!(
        "Coordinator context rollover state. The same task and execution state carry into this fresh window. Prior source and tool payloads are not model-visible here, and historical locators do not imply current source freshness. successfulWorkspaceMutations={successful_mutations}; unresolvedFailedMutation={unresolved_failed_mutation}; verification={verification}."
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

    if !recent_inspection_evidence.is_empty() {
        text.push_str(
            "\nCurrent workspace evidence index (locators only; source text is intentionally not replayed across windows):\n",
        );
        for evidence in recent_inspection_evidence {
            text.push_str("- ");
            text.push_str(&evidence.summary);
            text.push('\n');
        }
        text.push_str(
            "Locator entries are pointers to workspace evidence captured before rollover; source freshness may need to be re-established when it is material.\n",
        );
    }
    Message::system(text)
}

fn summarize_inspection_locator(
    call: &ToolCall,
    content: &str,
) -> Option<InspectionEvidenceLocator> {
    match call.name.as_str() {
        "read_file" | "read_files" => {
            let mut ranges = Vec::new();
            let mut paths = Vec::new();
            if let Some(path) = call.arguments.get("path").and_then(Value::as_str) {
                let start = call
                    .arguments
                    .get("startLine")
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                let end = call
                    .arguments
                    .get("endLine")
                    .and_then(Value::as_u64)
                    .map(|line| line.to_string())
                    .unwrap_or_else(|| "default-window".into());
                paths.push(path.to_owned());
                ranges.push(format!("{path}:{start}-{end}"));
            }
            if let Some(requests) = call.arguments.get("requests").and_then(Value::as_array) {
                for request in requests.iter().take(8) {
                    let Some(path) = request.get("path").and_then(Value::as_str) else {
                        continue;
                    };
                    let start = request
                        .get("startLine")
                        .and_then(Value::as_u64)
                        .unwrap_or(1);
                    let end = request
                        .get("endLine")
                        .and_then(Value::as_u64)
                        .map(|line| line.to_string())
                        .unwrap_or_else(|| "default-window".into());
                    paths.push(path.to_owned());
                    ranges.push(format!("{path}:{start}-{end}"));
                }
            }
            (!ranges.is_empty()).then(|| InspectionEvidenceLocator {
                summary: format!("read_file {}", ranges.join(", ")),
                paths,
            })
        }
        "search_workspace" => {
            let query = call
                .arguments
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let search_root = call
                .arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".");
            let mut hits = Vec::new();
            let mut paths = Vec::new();
            if let Ok(value) = serde_json::from_str::<Value>(content)
                && let Some(matches) = value.get("matches").and_then(Value::as_array)
            {
                for hit in matches.iter().take(6) {
                    if let Some(hit_path) = hit.get("path").and_then(Value::as_str) {
                        paths.push(hit_path.to_owned());
                        let line = hit.get("line").and_then(Value::as_u64);
                        hits.push(match line {
                            Some(line) => format!("{hit_path}:{line}"),
                            None => hit_path.to_owned(),
                        });
                    }
                }
            }
            let suffix = if hits.is_empty() {
                String::new()
            } else {
                format!(" -> {}", hits.join(", "))
            };
            Some(InspectionEvidenceLocator {
                summary: format!(
                    "search_workspace path={search_root} query={:?}{suffix}",
                    bounded_text(query, 160)
                ),
                // With no concrete hit, the locator is query state rather than
                // source state and should be invalidated on the next write.
                paths,
            })
        }
        "list_files" => {
            let path = call
                .arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".");
            Some(InspectionEvidenceLocator {
                summary: format!("list_files path={path}"),
                paths: vec![path.to_owned()],
            })
        }
        "read_document" => {
            let path = call
                .arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Some(InspectionEvidenceLocator {
                summary: format!("read_document path={path}"),
                paths: vec![path.to_owned()],
            })
        }
        "run_shell" => call
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .map(|command| InspectionEvidenceLocator {
                summary: format!("run_shell {}", bounded_text(command, 240)),
                paths: Vec::new(),
            }),
        _ => None,
    }
}

fn invalidate_inspection_evidence(
    evidence: &mut Vec<InspectionEvidenceLocator>,
    mutation: &ToolCall,
) {
    let changed_paths = mutation_paths(mutation);
    if changed_paths.is_empty() {
        evidence.clear();
        return;
    }

    evidence.retain(|locator| {
        !locator.paths.is_empty()
            && locator.paths.iter().all(|observed| {
                changed_paths
                    .iter()
                    .all(|changed| !paths_overlap(observed, changed))
            })
    });
}

fn mutation_paths(call: &ToolCall) -> Vec<String> {
    if call.name != "apply_file_edits" {
        return Vec::new();
    }

    let mut paths = Vec::new();
    if let Some(changes) = call.arguments.get("changes").and_then(Value::as_array) {
        for change in changes.iter().take(16) {
            if let Some(path) = change.get("path").and_then(Value::as_str) {
                paths.push(path.to_owned());
            }
            if let Some(destination) = change
                .get("fileOp")
                .and_then(Value::as_object)
                .and_then(|operation| operation.get("destination"))
                .and_then(Value::as_str)
            {
                paths.push(destination.to_owned());
            }
        }
    }
    paths
}

fn paths_overlap(left: &str, right: &str) -> bool {
    fn normalized(path: &str) -> &str {
        path.trim_end_matches('/')
    }

    let left = normalized(left);
    let right = normalized(right);
    left == right
        || (!left.is_empty()
            && right
                .strip_prefix(left)
                .is_some_and(|suffix| suffix.starts_with('/')))
        || (!right.is_empty()
            && left
                .strip_prefix(right)
                .is_some_and(|suffix| suffix.starts_with('/')))
}
pub(super) fn summarize_tool_outcome(call: &ToolCall, content: &str, succeeded: bool) -> String {
    let subject = match call.name.as_str() {
        "run_shell" => call
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .map(|command| format!("run_shell `{}`", bounded_text(command, 120)))
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
                let value = bounded_text(value, if key == "stderr" { 180 } else { 140 });
                if !value.trim().is_empty() {
                    details.push(format!("{key}={value}"));
                }
            }
        }
    }
    bounded_text(&format!("{subject}: {}", details.join("; ")), 480)
}

pub(super) fn analysis_inspection_threshold(bounded_explanation: bool) -> usize {
    if bounded_explanation { 3 } else { 4 }
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
#[path = "turn_state/write_observation_tests.rs"]
mod write_observation_tests;

#[cfg(test)]
mod execution_evidence_tests {
    use super::*;

    fn shell_call(command: &str) -> ToolCall {
        ToolCall {
            id: command.to_owned(),
            name: "run_shell".into(),
            arguments: json!({"command": command}),
        }
    }

    #[test]
    fn goal_evidence_is_structured_coordinator_state() {
        let mut evidence = TurnExecutionEvidence::default();
        evidence.observe_tool(
            &shell_call("cargo test"),
            r#"{"exitCode":0,"succeeded":true}"#,
            true,
            false,
            true,
            false,
        );
        let value = evidence.goal_evidence();
        assert_eq!(value["verification"]["succeeded"], json!(true));
        assert!(value["provenance"]["currentGenerationValidations"].is_array());
    }

    #[test]
    fn provenance_request_overlays_emit_deltas_not_a_repeated_ledger() {
        let mut evidence = TurnExecutionEvidence::default();
        for path in ["src/first.rs", "src/second.rs"] {
            let edit = ToolCall {
                id: path.into(),
                name: "apply_file_edits".into(),
                arguments: json!({
                    "changes": [{"path": path, "edits": [{"kind": "replace", "text": "updated"}]}]
                }),
            };
            evidence.observe_tool(&edit, r#"{"changed":true}"#, true, true, false, true);
            let overlay = evidence.request_overlay().expect("new provenance delta");
            let content = overlay.content.as_deref().unwrap_or_default();
            assert!(content.contains(path));
            assert!(content.len() < 1_500);
        }
        assert!(evidence.request_overlay().is_none());
    }

    #[test]
    fn rollover_handoff_carries_current_source_locators_without_source_payload() {
        let mut evidence = TurnExecutionEvidence::default();
        let read = ToolCall {
            id: "read-1".into(),
            name: "read_file".into(),
            arguments: json!({
                "path": "src/remote.rs",
                "startLine": 120,
                "endLine": 180
            }),
        };
        evidence.observe_tool(
            &read,
            "120: fn remote() { /* deliberately large source body */ }",
            true,
            false,
            false,
            false,
        );

        let handoff = evidence.rollover_handoff(Some("change strategy"));
        let handoff = handoff.content.as_deref().unwrap_or_default();
        assert!(handoff.contains("read_file src/remote.rs:120-180"));
        assert!(handoff.contains("change strategy"));
        assert!(!handoff.contains("deliberately large source body"));
    }

    #[test]
    fn workspace_mutation_invalidates_pre_mutation_source_locators() {
        let mut evidence = TurnExecutionEvidence::default();
        let search = ToolCall {
            id: "search-1".into(),
            name: "search_workspace".into(),
            arguments: json!({"path":"src","query":"remote"}),
        };
        evidence.observe_tool(
            &search,
            r#"{"matches":[{"path":"src/remote.rs","line":42}]}"#,
            true,
            false,
            false,
            false,
        );

        let unrelated = ToolCall {
            id: "read-unrelated".into(),
            name: "read_file".into(),
            arguments: json!({
                "path":"src/cache.rs",
                "startLine":10,
                "endLine":20
            }),
        };
        evidence.observe_tool(
            &unrelated,
            "10: unrelated cache source",
            true,
            false,
            false,
            false,
        );
        let handoff = evidence.rollover_handoff(None);
        assert!(
            handoff
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("src/remote.rs:42")
        );

        let edit = ToolCall {
            id: "edit".into(),
            name: "apply_file_edits".into(),
            arguments: json!({"changes":[{"path":"src/remote.rs","edits":[]}]}),
        };
        evidence.observe_tool(&edit, r#"{"changed":true}"#, true, true, false, true);
        let handoff = evidence.rollover_handoff(None);
        assert!(
            !handoff
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("src/remote.rs:42")
        );

        assert!(
            handoff
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("src/cache.rs:10-20")
        );
    }
}
