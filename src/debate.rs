//! Fixed-stage debate with isolated advocates and order-balanced jury ballots.
mod general;
mod knowledge;
mod requests;
pub mod research;
#[cfg(test)]
mod tests;
use crate::core::{CallResult, Message};
use anyhow::{Result, ensure};
pub use general::*;
pub use knowledge::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

pub const STAGES: [&str; 4] = ["Opening", "Rebuttal", "Strengthening", "Closing"];
pub const JURY_EARLY_BALLOTS: usize = 2;
pub const JURY_TARGET_BALLOTS: usize = 4;
/// Malformed or failed ballots are retried, but never indefinitely.
pub const JURY_MAX_ATTEMPTS: usize = 8;
const ARGUMENT_PACKET_CHARS: usize = 6_000;
const DOSSIER_PACKET_CHARS: usize = 4_000;
const EVIDENCE_PACKET_EXCERPT_CHARS: usize = 800;
const EVIDENCE_ITEMS_PER_RECORD: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebateModels {
    pub pro: String,
    pub con: String,
    pub jury: String,
}
impl DebateModels {
    pub fn same(model: &str) -> Self {
        Self {
            pro: model.into(),
            con: model.into(),
            jury: model.into(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        for (role, model) in [("Pro", &self.pro), ("Con", &self.con), ("Jury", &self.jury)] {
            ensure!(!model.trim().is_empty(), "Select a model for {role}.");
            ensure!(
                model == model.trim() && !model.chars().any(char::is_whitespace),
                "Invalid {role} model ID."
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebateState {
    /// Immutable identity for the concrete thing being debated. This keeps
    /// phrases such as "current implementation" anchored to the workspace
    /// that existed when the debate started instead of letting later model
    /// calls silently reinterpret them as a generic technology category.
    #[serde(default)]
    pub subject: DebateSubject,
    /// Deterministic, local-only orientation context captured before the
    /// framing call. This is persisted so exported debate records show what
    /// the moderator knew about the project before research began.
    #[serde(default)]
    pub framing_project_brief: Option<String>,
    #[serde(default)]
    pub models: DebateModels,
    #[serde(default)]
    pub contract: Option<DebateContract>,
    #[serde(default)]
    research: Vec<ResearchRecord>,
    #[serde(default)]
    pub ready_to_close: [bool; 2],
    #[serde(default)]
    pub closing_stage: Option<usize>,
    #[serde(default)]
    pub checkpoints: Vec<DebateCheckpoint>,
    #[serde(default)]
    pub closing_reason: Option<String>,
    pub topic: String,
    pub status: String,
    pub speeches: Vec<Speech>,
    pub ballots: Vec<Ballot>,
    #[serde(default)]
    pub jury_attempts: Vec<JuryAttempt>,
    #[serde(default)]
    pub knowledge_summary: Option<DebateKnowledgeSummary>,
    pub verdict: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub tool: String,
    pub input: Value,
    pub excerpt: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchRecord {
    pub stage: usize,
    pub pro: bool,
    #[serde(default)]
    pub status: ResearchStatus,
    pub notes: String,
    /// Useful while a research turn is live, but intentionally not persisted.
    /// Persisting raw provider/tool history duplicates the durable evidence
    /// packet and makes debate snapshots balloon over time.
    #[serde(default, skip_serializing)]
    pub history: Vec<Message>,
    pub successful_reads: usize,
    #[serde(default)]
    pub evidence: Vec<EvidenceItem>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchStatus {
    Collecting,
    Collected,
    /// Legacy records only existed after synthesis completed, so treating a
    /// missing field as synthesized preserves their original meaning.
    #[default]
    Synthesized,
    SynthesisFailed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebateSubject {
    /// Whether proposition referents such as "current implementation" are
    /// bound to the concrete workspace below.
    #[serde(default)]
    pub workspace_bound: bool,
    #[serde(default)]
    pub workspace_root: String,
    #[serde(default)]
    pub revision: Option<String>,
    /// Small deterministic set of project anchors discovered before framing.
    #[serde(default)]
    pub anchor_files: Vec<String>,
}

impl DebateSubject {
    pub fn render(&self) -> String {
        if !self.workspace_bound {
            return "No concrete workspace binding is required by this proposition.".into();
        }
        format!(
            "Concrete workspace subject: root={} ; revision={} ; project anchors=[{}]. This identity is immutable for the debate. Claims about the current implementation must be established from this workspace's source evidence before generic external evidence is used.",
            self.workspace_root,
            self.revision.as_deref().unwrap_or("unknown"),
            if self.anchor_files.is_empty() {
                "none discovered".into()
            } else {
                self.anchor_files.join(", ")
            }
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Speech {
    pub stage: usize,
    pub pro: bool,
    pub text: String,
    #[serde(default)]
    pub ready_to_close: bool,
    #[serde(default)]
    pub continue_research: bool,
}
impl Speech {
    pub fn from_response(stage: usize, pro: bool, text: String) -> Self {
        let trimmed = text.trim_end();
        let mut lines: Vec<&str> = trimmed.lines().collect();
        let control = lines.last().map(|line| line.trim());
        let ready_to_close = control == Some("[[READY_TO_CLOSE]]");
        let continue_research = control == Some("[[CONTINUE_RESEARCH]]");
        if matches!(
            control,
            Some("[[READY_TO_CLOSE]]" | "[[CONTINUE_RESEARCH]]")
        ) {
            lines.pop();
        }
        let body = lines.join("\n").trim_end().to_owned();
        Self {
            stage,
            pro,
            text: body,
            ready_to_close,
            continue_research,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ballot {
    pub pro: Vec<u8>,
    pub con: Vec<u8>,
    /// One score array per framed answer. Binary ballots contain two entries.
    #[serde(default)]
    pub candidate_scores: Vec<Vec<u8>>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JuryAttempt {
    pub reversed: bool,
    pub accepted: bool,
    pub raw: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebateIssueStatus {
    ProEstablished,
    ConEstablished,
    Unresolved,
}

impl DebateIssueStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::ProEstablished => "PRO established",
            Self::ConEstablished => "CON established",
            Self::Unresolved => "Unresolved",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebateIssue {
    pub id: String,
    pub issue: String,
    pub status: DebateIssueStatus,
    pub reason: String,
    /// Whether the current strengthening stage added material that advances
    /// this exact issue. This is deliberately issue-local: progress on one
    /// dispute must not be allowed to rewrite a different settled dispute.
    #[serde(default)]
    pub material_change: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebateCheckpoint {
    pub stage: usize,
    pub issues: Vec<DebateIssue>,
    pub material_progress: bool,
    pub should_continue: bool,
    pub reason: String,
}

impl DebateCheckpoint {
    pub fn parse_response(stage: usize, response: &CallResult) -> Result<Self> {
        ensure!(
            response.finish_reason != "length",
            "debate checkpoint response was truncated"
        );
        let calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| call.name == "submit_debate_checkpoint")
            .collect();
        ensure!(
            calls.len() <= 1,
            "debate checkpoint returned multiple tool calls"
        );
        let value = if let Some(call) = calls.first() {
            call.arguments.clone()
        } else {
            ensure!(
                response.tool_calls.is_empty(),
                "debate checkpoint returned an unexpected tool call"
            );
            parse_json_object(&response.text)?
        };
        #[derive(Deserialize)]
        struct Payload {
            issues: Vec<DebateIssue>,
            material_progress: bool,
            should_continue: bool,
            reason: String,
        }
        let mut payload: Payload = serde_json::from_value(value)?;
        ensure!(
            !payload.issues.is_empty() && payload.issues.len() <= 12,
            "debate checkpoint must contain 1-12 issues"
        );
        ensure!(
            !payload.reason.trim().is_empty(),
            "debate checkpoint reason is empty"
        );
        let mut ids = HashSet::new();
        for issue in &mut payload.issues {
            issue.id = canonical_issue_id(&issue.id);
            issue.issue = issue.issue.trim().to_owned();
            issue.reason = issue.reason.trim().to_owned();
            ensure!(
                !issue.id.is_empty() && !issue.issue.is_empty() && !issue.reason.is_empty(),
                "debate checkpoint contains an empty issue field"
            );
            ensure!(
                ids.insert(issue.id.clone()),
                "debate checkpoint contains duplicate issue IDs"
            );
        }
        let material_progress =
            payload.material_progress && payload.issues.iter().any(|issue| issue.material_change);
        let has_unresolved = payload
            .issues
            .iter()
            .any(|issue| issue.status == DebateIssueStatus::Unresolved);
        Ok(Self {
            stage,
            issues: payload.issues,
            material_progress,
            // Repetition is never allowed to buy another round merely because
            // a model asks for one. Continuing also requires a genuinely open
            // issue; a settled ledger cannot manufacture another round.
            should_continue: payload.should_continue && material_progress && has_unresolved,
            reason: payload.reason.trim().to_owned(),
        })
    }

    pub fn render(&self) -> String {
        let mut output = format!(
            "{} · {}\nReason: {}",
            if self.material_progress {
                "Material progress"
            } else {
                "No material progress"
            },
            if self.should_continue {
                "continue"
            } else {
                "close"
            },
            self.reason.trim()
        );
        for issue in &self.issues {
            output.push_str(&format!(
                "\n- {} [{}] {}: {}",
                issue.id.trim(),
                issue.status.label(),
                issue.issue.trim(),
                issue.reason.trim()
            ));
        }
        output
    }
}

fn canonical_issue_id(value: &str) -> String {
    value.trim().to_lowercase()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebateContract {
    pub scope: String,
    pub pro_burden: String,
    pub con_burden: String,
    #[serde(default)]
    pub decision_criteria: Vec<String>,
    #[serde(default)]
    pub explicit_ambiguities: Vec<String>,
    /// None preserves the legacy binary proposition protocol. A general kind
    /// activates candidate-based evaluation for the same live debate run.
    #[serde(default)]
    pub general_kind: Option<GeneralQuestionKind>,
    #[serde(default)]
    pub answer_format: String,
    #[serde(default)]
    pub candidate_answers: Vec<String>,
}

impl DebateContract {
    pub fn minimal(topic: &str) -> Self {
        Self::minimal_with_grounding(topic, None)
    }

    pub fn minimal_with_grounding(topic: &str, grounding: Option<&str>) -> Self {
        let grounding = grounding.map(str::trim).filter(|value| !value.is_empty());
        Self {
            scope: if let Some(grounding) = grounding {
                format!(
                    "Evaluate the proposition exactly as written without silently redefining its terms: {topic}. Concrete evaluation context: {grounding}"
                )
            } else {
                format!(
                    "Evaluate the proposition exactly as written without silently redefining its terms: {topic}"
                )
            },
            pro_burden: "Establish the proposition with evidence and answer material counterexamples."
                .into(),
            con_burden: "Show that the proposition is false, materially overstated, or not established by the available evidence."
                .into(),
            decision_criteria: vec![
                "Logical validity and causal/technical coherence".into(),
                "Direct evidential support and source quality".into(),
                "Response to material counterexamples and objections".into(),
                "Calibration to uncertainty and missing evidence".into(),
            ],
            explicit_ambiguities: vec![
                "Undefined terms remain explicit uncertainties; advocates may argue interpretations but may not silently substitute a different proposition."
                    .into(),
            ],
            general_kind: None,
            answer_format: "Decide whether the proposition is established, not established, or unresolved.".into(),
            candidate_answers: Vec::new(),
        }
    }

    pub(crate) fn bound_to_topic(mut self, topic: &str) -> Self {
        let proposition = topic.trim();
        let binding = format!("Binding proposition (verbatim): {proposition}");
        if !self.scope.trim().starts_with(&binding) {
            let operational = self.scope.trim();
            self.scope = format!(
                "{binding}\nOperational scope is subordinate to that proposition and may only clarify it: {operational}"
            );
        }
        self
    }

    pub fn parse_response(response: &CallResult) -> Result<Self> {
        ensure!(
            response.finish_reason != "length",
            "debate contract response was truncated"
        );
        let calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| call.name == "submit_debate_contract")
            .collect();
        ensure!(
            calls.len() <= 1,
            "debate moderator returned multiple contract tool calls"
        );
        let value = if let Some(call) = calls.first() {
            call.arguments.clone()
        } else {
            ensure!(
                response.tool_calls.is_empty(),
                "debate moderator returned an unexpected tool call"
            );
            parse_json_object(&response.text)?
        };
        let mut contract: Self = serde_json::from_value(value)?;
        contract.scope = contract.scope.trim().to_owned();
        contract.pro_burden = contract.pro_burden.trim().to_owned();
        contract.con_burden = contract.con_burden.trim().to_owned();
        contract.decision_criteria = contract
            .decision_criteria
            .into_iter()
            .map(|item| item.trim().to_owned())
            .collect();
        contract.explicit_ambiguities = contract
            .explicit_ambiguities
            .into_iter()
            .map(|item| item.trim().to_owned())
            .collect();
        contract.answer_format = contract.answer_format.trim().to_owned();
        contract.candidate_answers = contract
            .candidate_answers
            .into_iter()
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect();
        ensure!(!contract.scope.is_empty(), "debate contract scope is empty");
        ensure!(
            !contract.pro_burden.is_empty() && !contract.con_burden.is_empty(),
            "debate contract burdens are empty"
        );
        ensure!(
            (2..=6).contains(&contract.decision_criteria.len())
                && contract
                    .decision_criteria
                    .iter()
                    .all(|item| !item.is_empty()),
            "debate contract criteria are empty"
        );
        let mut criteria = HashSet::new();
        ensure!(
            contract
                .decision_criteria
                .iter()
                .all(|item| criteria.insert(item.to_lowercase())),
            "debate contract contains duplicate criteria"
        );
        ensure!(
            contract.explicit_ambiguities.len() <= 6
                && contract
                    .explicit_ambiguities
                    .iter()
                    .all(|item| !item.is_empty()),
            "debate contract contains an empty ambiguity"
        );
        if contract.general_kind.is_some() {
            ensure!(
                (2..=8).contains(&contract.candidate_answers.len()),
                "general debate contract must contain 2-8 candidate answers"
            );
            ensure!(
                !contract.answer_format.is_empty(),
                "general debate answer format is empty"
            );
            let mut candidates = HashSet::new();
            ensure!(
                contract
                    .candidate_answers
                    .iter()
                    .all(|item| candidates.insert(item.to_lowercase())),
                "general debate contract contains duplicate candidate answers"
            );
        }
        Ok(contract)
    }

    pub fn render(&self) -> String {
        format!(
            "Scope: {}\nPRO burden: {}\nCON burden: {}\nCriteria: {}\nExplicit ambiguities: {}",
            self.scope.trim(),
            self.pro_burden.trim(),
            self.con_burden.trim(),
            self.decision_criteria.join("; "),
            if self.explicit_ambiguities.is_empty() {
                "None identified".into()
            } else {
                self.explicit_ambiguities.join("; ")
            }
        )
    }
}

impl Ballot {
    pub fn parse(text: &str, reversed: bool) -> Result<Self> {
        let value = parse_json_object(text)?;
        Self::from_value(value, reversed, None)
    }

    pub fn parse_response(response: &CallResult, reversed: bool) -> Result<Self> {
        Self::parse_response_for_criteria(response, reversed, None)
    }

    fn parse_response_for_criteria(
        response: &CallResult,
        reversed: bool,
        expected_criteria: Option<usize>,
    ) -> Result<Self> {
        ensure!(
            response.finish_reason != "length",
            "jury response was truncated"
        );
        let ballot_calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| call.name == "submit_debate_ballot")
            .collect();
        ensure!(
            ballot_calls.len() <= 1,
            "jury returned multiple ballot tool calls"
        );
        if let Some(call) = ballot_calls.first() {
            return Self::from_value(call.arguments.clone(), reversed, expected_criteria);
        }
        ensure!(
            response.tool_calls.is_empty(),
            "jury returned an unexpected tool call"
        );
        let value = parse_json_object(&response.text)?;
        Self::from_value(value, reversed, expected_criteria)
    }

    fn from_value(value: Value, reversed: bool, expected_criteria: Option<usize>) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            a: Vec<u8>,
            #[serde(default)]
            b: Vec<u8>,
            #[serde(default)]
            candidates: Vec<Vec<u8>>,
            reason: String,
        }
        let raw: Raw = serde_json::from_value(value)?;
        let mut candidate_scores = if raw.candidates.is_empty() {
            ensure!(
                !raw.a.is_empty() && !raw.b.is_empty(),
                "jury ballot must contain A/B or candidate scores"
            );
            vec![raw.a, raw.b]
        } else {
            raw.candidates
        };
        ensure!(
            (2..=8).contains(&candidate_scores.len()),
            "jury ballot must contain 2-8 candidates"
        );
        let criteria_count = candidate_scores.first().map(Vec::len).unwrap_or(0);
        ensure!(
            (2..=6).contains(&criteria_count),
            "jury score arrays must contain 2-6 criteria"
        );
        ensure!(
            candidate_scores
                .iter()
                .all(|scores| scores.len() == criteria_count),
            "jury candidates must share one criterion rubric"
        );
        if let Some(expected) = expected_criteria {
            ensure!(
                criteria_count == expected,
                "jury returned {criteria_count} criterion scores; contract requires {expected}"
            );
        }
        ensure!(
            candidate_scores.iter().flatten().all(|n| *n <= 10),
            "jury scores must be 0–10"
        );
        ensure!(!raw.reason.trim().is_empty(), "jury must provide reasons");
        if reversed {
            candidate_scores.reverse();
        }
        let pro = candidate_scores.first().cloned().unwrap_or_default();
        let con = candidate_scores.get(1).cloned().unwrap_or_default();
        Ok(Self {
            pro,
            con,
            candidate_scores,
            reason: raw.reason,
        })
    }
}

fn parse_json_object(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }

    if let Some(rest) = trimmed.strip_prefix("```")
        && let Some((_, fenced)) = rest.split_once('\n')
        && let Some(body) = fenced.strip_suffix("```")
    {
        return Ok(serde_json::from_str::<Value>(body.trim())?);
    }
    Ok(serde_json::from_str::<Value>(trimmed)?)
}

fn compact_text(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let mut compact: String = value.chars().take(maximum).collect();
    compact.push_str("\n[truncated for debate packet]");
    compact
}

impl DebateState {
    fn effective_contract(&self) -> DebateContract {
        self.contract
            .clone()
            .unwrap_or_else(|| DebateContract::minimal(&self.topic))
            .bound_to_topic(&self.topic)
    }

    pub fn upsert_research_record(&mut self, record: ResearchRecord) {
        if let Some(existing) = self
            .research
            .iter_mut()
            .find(|existing| existing.stage == record.stage && existing.pro == record.pro)
        {
            *existing = record;
        } else {
            self.research.push(record);
        }
    }

    pub fn research_record(&self, stage: usize, pro: bool) -> Option<&ResearchRecord> {
        self.research
            .iter()
            .find(|record| record.stage == stage && record.pro == pro)
    }

    pub fn research_records(&self) -> impl Iterator<Item = &ResearchRecord> {
        self.research.iter()
    }

    pub fn has_reusable_evidence(&self) -> bool {
        let source_ids = self
            .research
            .iter()
            .flat_map(|record| record.evidence.iter())
            .flat_map(|item| item.source_ids.iter());
        if self.subject.workspace_bound {
            source_ids.into_iter().any(|source| {
                source.starts_with("file:")
                    || source.starts_with("analysis:")
                    || source.starts_with("artifact:")
            })
        } else {
            source_ids.into_iter().next().is_some()
        }
    }

    /// Workspace-bound debates are only meaningful if both isolated advocates
    /// independently inspected the concrete workspace before opening. A model
    /// or provider failure must not silently degrade such a debate into generic
    /// prose that the jury later scores as if it were project evidence.
    pub fn initial_workspace_grounding_gap(&self) -> Option<String> {
        if !self.subject.workspace_bound {
            return None;
        }

        let mut missing = Vec::new();
        for (pro, label) in [(true, "PRO"), (false, "CON")] {
            let grounded = self.research_record(0, pro).is_some_and(|record| {
                record
                    .evidence
                    .iter()
                    .flat_map(|item| item.source_ids.iter())
                    .any(|source| source.starts_with("file:") || source.starts_with("analysis:"))
            });
            if !grounded {
                missing.push(label);
            }
        }

        (!missing.is_empty()).then(|| {
            format!(
                "{} initial research did not retain concrete workspace source evidence",
                missing.join(" and ")
            )
        })
    }

    pub fn evidence_source_ids(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.research
            .iter()
            .flat_map(|record| record.evidence.iter())
            .flat_map(|item| item.source_ids.iter().cloned())
            .filter(|source| seen.insert(source.clone()))
            .collect()
    }

    pub fn stage_label(&self, stage: usize) -> String {
        if self.closing_stage == Some(stage) {
            return "Closing".into();
        }
        match stage {
            0 => "Opening".into(),
            1 => "Rebuttal".into(),
            _ => format!("Strengthening {}", stage - 1),
        }
    }

    pub fn record_speech(&mut self, speech: Speech) -> Result<()> {
        let stage = speech.stage;
        ensure!(
            !self
                .speeches
                .iter()
                .any(|existing| existing.stage == speech.stage && existing.pro == speech.pro),
            "duplicate debate speech for stage {stage} and side {}",
            if speech.pro { "Pro" } else { "Con" }
        );
        self.speeches.push(speech);
        if stage >= 2 && self.closing_stage.is_none() {
            let latest = self.speeches.last().unwrap();
            let index = if latest.pro { 0 } else { 1 };
            let found_new_source_evidence = self.research.iter().any(|record| {
                record.stage == stage && record.pro == latest.pro && record.successful_reads > 0
            });
            if latest.ready_to_close {
                self.ready_to_close[index] = true;
            } else if latest.continue_research || found_new_source_evidence {
                // An explicit continue marker is stronger than stale readiness.
                // Missing control lines preserve the prior state, while genuinely
                // new source evidence also reopens a previously-ready case.
                self.ready_to_close[index] = false;
            }
            let both_spoke = [true, false].iter().all(|pro| {
                self.speeches
                    .iter()
                    .any(|s| s.stage == stage && s.pro == *pro)
            });
            let both_ready = self.ready_to_close.iter().all(|ready| *ready);
            if both_spoke {
                let reason = if both_ready {
                    Some(
                        "Both advocates declared that further research would add little."
                            .to_owned(),
                    )
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.closing_stage = Some(stage + 1);
                    self.closing_reason.get_or_insert(reason);
                }
            }
        }
        Ok(())
    }

    pub fn record_checkpoint(&mut self, mut checkpoint: DebateCheckpoint) -> Result<()> {
        ensure!(
            checkpoint.stage >= 2,
            "checkpoint is only valid after strengthening"
        );
        ensure!(
            !self
                .checkpoints
                .iter()
                .any(|existing| existing.stage == checkpoint.stage),
            "duplicate debate checkpoint for stage {}",
            checkpoint.stage
        );
        if let Some(previous) = self.checkpoints.last() {
            let prior_ids: HashSet<_> = previous
                .issues
                .iter()
                .map(|issue| canonical_issue_id(&issue.id))
                .collect();
            // A brand-new issue with no issue-local material change is just
            // relabeling or bookkeeping churn. Do not let it grow the ledger.
            checkpoint.issues.retain(|issue| {
                prior_ids.contains(&canonical_issue_id(&issue.id)) || issue.material_change
            });
            for prior_issue in &previous.issues {
                if let Some(current_issue) = checkpoint.issues.iter_mut().find(|issue| {
                    canonical_issue_id(&issue.id) == canonical_issue_id(&prior_issue.id)
                }) {
                    current_issue.id = prior_issue.id.clone();
                    // Issue IDs name a stable dispute. A model may update the
                    // rationale, but not silently redefine what that ID means.
                    current_issue.issue = prior_issue.issue.clone();
                    if (!checkpoint.material_progress || !current_issue.material_change)
                        && current_issue.status != prior_issue.status
                    {
                        current_issue.status = prior_issue.status.clone();
                        current_issue.reason = format!(
                            "No issue-local material change was established, so the prior status is preserved. {}",
                            current_issue.reason.trim()
                        );
                    }
                } else {
                    let mut carried = prior_issue.clone();
                    carried.material_change = false;
                    checkpoint.issues.push(carried);
                }
            }

            let issue_local_progress = checkpoint.issues.iter().any(|issue| issue.material_change);
            checkpoint.material_progress &= issue_local_progress;
        }
        let has_unresolved = checkpoint
            .issues
            .iter()
            .any(|issue| issue.status == DebateIssueStatus::Unresolved);
        checkpoint.should_continue &= checkpoint.material_progress && has_unresolved;
        let should_close = !checkpoint.should_continue;
        let stage = checkpoint.stage;
        let reason = checkpoint.reason.clone();
        self.checkpoints.push(checkpoint);
        if should_close && self.closing_stage.is_none() {
            self.closing_stage = Some(stage + 1);
            self.closing_reason = Some(format!("Progress checkpoint: {}", reason.trim()));
        }
        Ok(())
    }

    fn argument_ledger(
        &self,
        before_stage: Option<usize>,
        reversed: bool,
        blinded: bool,
    ) -> Vec<Value> {
        let mut speeches: Vec<_> = self
            .speeches
            .iter()
            .filter(|speech| before_stage.is_none_or(|stage| speech.stage < stage))
            .collect();
        if blinded {
            // Reversing a ballot must reverse presentation order, not merely
            // rename the same first speaker from A to B.
            speeches.sort_by_key(|speech| {
                (
                    speech.stage,
                    if speech.pro != reversed {
                        0usize
                    } else {
                        1usize
                    },
                )
            });
        }
        speeches
            .into_iter()
            .map(|speech| {
                json!({
                    "stage": self.stage_label(speech.stage),
                    "side": if blinded {
                        if speech.pro != reversed { "A" } else { "B" }
                    } else if speech.pro {
                        "PRO"
                    } else {
                        "CON"
                    },
                    "argument": compact_text(&speech.text, ARGUMENT_PACKET_CHARS),
                })
            })
            .collect()
    }

    fn stage_argument_ledger(&self, stage: usize) -> Vec<Value> {
        self.speeches
            .iter()
            .filter(|speech| speech.stage == stage)
            .map(|speech| {
                json!({
                    "stage": self.stage_label(speech.stage),
                    "side": if speech.pro { "PRO" } else { "CON" },
                    "argument": compact_text(&speech.text, ARGUMENT_PACKET_CHARS),
                })
            })
            .collect()
    }

    fn stage_evidence_packet(&self, stage: usize) -> Vec<Value> {
        self.research
            .iter()
            .filter(|record| record.stage == stage)
            .map(|record| {
                let source_reads: Vec<_> = record
                    .evidence
                    .iter()
                    .take(EVIDENCE_ITEMS_PER_RECORD)
                    .map(|item| {
                        json!({
                            "tool": item.tool,
                            "input": item.input,
                            "kind": if item.kind.is_empty() { "source-read" } else { item.kind.as_str() },
                            "source_ids": item.source_ids,
                            "excerpt": compact_text(&item.excerpt, EVIDENCE_PACKET_EXCERPT_CHARS),
                        })
                    })
                    .collect();
                json!({
                    "stage": self.stage_label(record.stage),
                    "research_side": if record.pro { "PRO" } else { "CON" },
                    "research_status": record.status,
                    "successful_new_source_reads": record.successful_reads,
                    "source_reads": source_reads,
                    "research_synthesis_non_evidence": compact_text(&record.notes, DOSSIER_PACKET_CHARS),
                })
            })
            .collect()
    }

    fn evidence_packet(
        &self,
        through_stage: Option<usize>,
        reversed: bool,
        blinded: bool,
        side: Option<bool>,
    ) -> Vec<Value> {
        self.evidence_packet_entries(through_stage, reversed, blinded, side)
            .into_iter()
            .map(|(_, packet)| packet)
            .collect()
    }

    /// Evidence packets paired with their research stage, in presentation order.
    fn evidence_packet_entries(
        &self,
        through_stage: Option<usize>,
        reversed: bool,
        blinded: bool,
        side: Option<bool>,
    ) -> Vec<(usize, Value)> {
        let mut records: Vec<_> = self
            .research
            .iter()
            .filter(|record| through_stage.is_none_or(|stage| record.stage <= stage))
            .filter(|record| side.is_none_or(|pro| record.pro == pro))
            .collect();
        if blinded {
            records.sort_by_key(|record| {
                (
                    record.stage,
                    if record.pro != reversed {
                        0usize
                    } else {
                        1usize
                    },
                )
            });
        }
        // Deduplicate within each advocate's evidence stream, not across both
        // advocates. The same source may contain different retained excerpts,
        // and first-writer-wins would otherwise privilege PRO because research
        // is collected PRO-first in the backend.
        let mut seen_sources: HashSet<(bool, String)> = HashSet::new();
        records
            .into_iter()
            .map(|record| {
                let source_reads: Vec<_> = record
                    .evidence
                    .iter()
                    .filter_map(|item| {
                        let source_ids = if item.source_ids.is_empty() {
                            vec![format!("legacy:{}:{}", item.tool, item.input)]
                        } else {
                            item.source_ids.clone()
                        };
                        let mut has_new_source = false;
                        for source in &source_ids {
                            has_new_source |= seen_sources.insert((record.pro, source.clone()));
                        }
                        has_new_source.then(|| {
                            json!({
                                "tool": item.tool,
                                "input": item.input,
                                "kind": if item.kind.is_empty() { "source-read" } else { item.kind.as_str() },
                                "source_ids": source_ids,
                                "excerpt": compact_text(&item.excerpt, EVIDENCE_PACKET_EXCERPT_CHARS),
                            })
                        })
                    })
                    .take(EVIDENCE_ITEMS_PER_RECORD)
                    .collect();
                let mut packet = json!({
                    "stage": self.stage_label(record.stage),
                    "research_side": if blinded {
                        if record.pro != reversed { "A" } else { "B" }
                    } else if record.pro {
                        "PRO"
                    } else {
                        "CON"
                    },
                    "research_status": record.status,
                    "successful_reads": record.successful_reads,
                    "source_reads": source_reads,
                });
                // Private research synthesis is useful to its own advocate,
                // but it is not source evidence. Neutral checkpoints/jury and
                // retained knowledge receive only provenance-bearing reads.
                if side.is_some() {
                    packet["research_synthesis_non_evidence"] =
                        compact_text(&record.notes, DOSSIER_PACKET_CHARS).into();
                }
                (record.stage, packet)
            })
            .collect()
    }

    pub fn next_jury_reversed(&self) -> bool {
        let accepted_forward = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted && !attempt.reversed)
            .count();
        let accepted_reversed = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted && attempt.reversed)
            .count();
        let per_orientation = JURY_TARGET_BALLOTS / 2;
        if accepted_forward >= per_orientation {
            return true;
        }
        if accepted_reversed >= per_orientation {
            return false;
        }
        if let Some(last) = self.jury_attempts.last()
            && !last.accepted
        {
            // A malformed/truncated ballot did not fill an orientation slot.
            // Retry the missing orientation instead of flipping sides and
            // accidentally spending the next attempt on an already-covered
            // presentation order.
            return last.reversed;
        }
        if accepted_forward != accepted_reversed {
            return accepted_forward > accepted_reversed;
        }
        let attempted_forward = self
            .jury_attempts
            .iter()
            .filter(|attempt| !attempt.reversed)
            .count();
        let attempted_reversed = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.reversed)
            .count();
        attempted_forward > attempted_reversed
    }

    pub fn record_jury_response(&mut self, reversed: bool, response: &CallResult) -> bool {
        let raw = serde_json::to_string(&json!({
            "text": response.text,
            "tool_calls": response.tool_calls,
            "finish_reason": response.finish_reason,
        }))
        .unwrap_or_else(|_| response.text.clone());
        let contract = self.effective_contract();
        let expected_criteria = contract.decision_criteria.len();
        match Ballot::parse_response_for_criteria(response, reversed, Some(expected_criteria)) {
            Ok(ballot) => {
                if contract.general_kind.is_some()
                    && ballot.candidate_scores.len() != contract.candidate_answers.len()
                {
                    let error = format!(
                        "jury returned {} candidates; contract requires {}",
                        ballot.candidate_scores.len(),
                        contract.candidate_answers.len()
                    );
                    self.jury_attempts.push(JuryAttempt {
                        reversed,
                        accepted: false,
                        raw,
                        error: Some(error),
                    });
                    return false;
                }
                self.ballots.push(ballot);
                self.jury_attempts.push(JuryAttempt {
                    reversed,
                    accepted: true,
                    raw,
                    error: None,
                });
                true
            }
            Err(error) => {
                self.jury_attempts.push(JuryAttempt {
                    reversed,
                    accepted: false,
                    raw,
                    error: Some(error.to_string()),
                });
                false
            }
        }
    }

    pub fn record_jury_error(&mut self, reversed: bool, error: impl Into<String>) {
        let error = error.into();
        self.jury_attempts.push(JuryAttempt {
            reversed,
            accepted: false,
            raw: String::new(),
            error: Some(error),
        });
    }

    pub fn jury_done(&self) -> bool {
        self.jury_early_consensus_met() || self.jury_balanced_target_met()
    }

    pub fn jury_unavailable_reason(&self) -> Option<String> {
        (!self.jury_done() && self.jury_attempts.len() >= JURY_MAX_ATTEMPTS).then(|| {
            let last_error = self
                .jury_attempts
                .iter()
                .rev()
                .find_map(|attempt| attempt.error.as_deref())
                .unwrap_or("no error recorded");
            format!(
                "The jury produced {} valid ballot(s) in {} attempts, short of the balanced target; no verdict is issued. Last error: {}",
                self.ballots.len(),
                self.jury_attempts.len(),
                last_error.chars().take(400).collect::<String>()
            )
        })
    }

    /// Whether the jury phase has finished, with or without a valid verdict.
    pub fn jury_settled(&self) -> bool {
        self.jury_done() || self.jury_unavailable_reason().is_some()
    }

    fn jury_early_consensus_met(&self) -> bool {
        if self.effective_contract().general_kind.is_some() {
            return false;
        }
        if self.ballots.len() != JURY_EARLY_BALLOTS {
            return false;
        }
        let forward = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted && !attempt.reversed)
            .count();
        let reversed = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted && attempt.reversed)
            .count();
        if forward < 1 || reversed < 1 {
            return false;
        }
        let totals: Vec<_> = self
            .ballots
            .iter()
            .map(|ballot| {
                let pro: u32 = ballot.pro.iter().map(|n| *n as u32).sum();
                let con: u32 = ballot.con.iter().map(|n| *n as u32).sum();
                (pro, con)
            })
            .collect();
        let outcomes: std::collections::BTreeSet<_> =
            totals.iter().map(|(pro, con)| pro.cmp(con)).collect();
        let minimum_margin = self
            .ballots
            .first()
            .map(|ballot| ballot.pro.len() as u32)
            .unwrap_or(1);
        outcomes.len() == 1
            && totals
                .iter()
                .all(|(pro, con)| pro.abs_diff(*con) >= minimum_margin)
    }

    fn jury_balanced_target_met(&self) -> bool {
        let per_orientation = JURY_TARGET_BALLOTS / 2;
        let forward = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted && !attempt.reversed)
            .count();
        let reversed = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted && attempt.reversed)
            .count();
        forward >= per_orientation && reversed >= per_orientation
    }

    fn finish_general(&mut self, criteria_count: usize) -> Result<()> {
        let contract = self.effective_contract();
        let candidate_count = contract.candidate_answers.len();
        ensure!(
            (2..=8).contains(&candidate_count),
            "general verdict has no valid candidate set"
        );
        ensure!(
            self.ballots
                .iter()
                .all(|ballot| ballot.candidate_scores.len() == candidate_count
                    && ballot
                        .candidate_scores
                        .iter()
                        .all(|scores| scores.len() == criteria_count)),
            "general jury ballots do not cover the framed candidate set"
        );
        let mut totals = vec![0u32; candidate_count];
        for ballot in &self.ballots {
            for (index, scores) in ballot.candidate_scores.iter().enumerate() {
                totals[index] += scores.iter().map(|score| *score as u32).sum::<u32>();
            }
        }
        let maximum = self.ballots.len() as u32 * criteria_count as u32 * 10;
        let mut order: Vec<_> = (0..candidate_count).collect();
        order.sort_by(|left, right| {
            totals[*right]
                .cmp(&totals[*left])
                .then_with(|| left.cmp(right))
        });
        let ranking = order
            .iter()
            .enumerate()
            .map(|(rank, index)| {
                format!(
                    "{}. {} — {}/{}",
                    rank + 1,
                    contract.candidate_answers[*index],
                    totals[*index],
                    maximum
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let best = order
            .first()
            .map(|index| contract.candidate_answers[*index].as_str())
            .unwrap_or("No candidate");
        self.verdict = Some(format!(
            "General answer · Jury {}/{} valid\n{}\nRecommendation: {}\nThis ranking is relative to the collected evidence packet; unresolved ambiguity and unsupported assumptions remain explicit.",
            self.ballots.len(),
            self.jury_attempts.len(),
            ranking,
            best
        ));
        self.status = "Completed".into();
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        let accepted_orientations: Vec<bool> = self
            .jury_attempts
            .iter()
            .filter(|attempt| attempt.accepted)
            .map(|attempt| attempt.reversed)
            .collect();
        ensure!(
            accepted_orientations.len() == self.ballots.len(),
            "jury state is inconsistent: {} accepted attempts for {} ballots",
            accepted_orientations.len(),
            self.ballots.len()
        );
        let criteria_count = self
            .ballots
            .first()
            .map(|ballot| ballot.pro.len())
            .unwrap_or(0);
        ensure!(
            (2..=6).contains(&criteria_count)
                && self.ballots.iter().all(|ballot| {
                    ballot.pro.len() == criteria_count && ballot.con.len() == criteria_count
                }),
            "jury ballots do not share one valid 2-6 criterion rubric"
        );
        if self.effective_contract().general_kind.is_some() {
            return self.finish_general(criteria_count);
        }
        ensure!(
            self.jury_early_consensus_met() || self.jury_balanced_target_met(),
            "jury incomplete: target {JURY_TARGET_BALLOTS} balanced valid ballots (or an early-consensus result) has not been reached; got {} ballots after {} attempts",
            self.ballots.len(),
            self.jury_attempts.len()
        );
        let totals: Vec<(u32, u32)> = self
            .ballots
            .iter()
            .map(|b| {
                (
                    b.pro.iter().map(|n| *n as u32).sum(),
                    b.con.iter().map(|n| *n as u32).sum(),
                )
            })
            .collect();
        let orientation_mean = |wanted_reversed: bool| -> Option<(f64, f64)> {
            let mut count = 0usize;
            let mut pro = 0u32;
            let mut con = 0u32;
            for ((pro_score, con_score), reversed) in totals.iter().zip(&accepted_orientations) {
                if *reversed == wanted_reversed {
                    count += 1;
                    pro += *pro_score;
                    con += *con_score;
                }
            }
            (count > 0).then(|| (pro as f64 / count as f64, con as f64 / count as f64))
        };
        let (mean_pro, mean_con) = match (orientation_mean(false), orientation_mean(true)) {
            (Some((forward_pro, forward_con)), Some((reversed_pro, reversed_con))) => (
                (forward_pro + reversed_pro) / 2.0,
                (forward_con + reversed_con) / 2.0,
            ),
            (Some(mean), None) | (None, Some(mean)) => mean,
            (None, None) => (0.0, 0.0),
        };
        // Equalize A-first and B-first orientations before aggregating the verdict.
        let scale = self.ballots.len() as f64;
        let pro = mean_pro * scale;
        let con = mean_con * scale;
        let comparisons: std::collections::BTreeSet<_> =
            totals.iter().map(|(pro, con)| pro.cmp(con)).collect();
        let disagreement = comparisons.len() > 1;
        let maximum = self.ballots.len() as u32 * criteria_count as u32 * 10;
        let early_consensus = self.jury_early_consensus_met();
        let attempt_count = self.jury_attempts.len().max(self.ballots.len());
        let display_score = |score: f64| {
            if (score - score.round()).abs() < 0.000_001 {
                format!("{}", score.round() as u32)
            } else {
                format!("{score:.1}")
            }
        };
        let pro_display = display_score(pro);
        let con_display = display_score(con);
        self.verdict = Some(format!(
            "{} · Pro {pro_display}/{maximum} · Con {con_display}/{maximum} · Jury {}/{} valid{}{}\nModel bias may remain. Claims were judged only against the debate's collected evidence packet; no outside fact verification was performed.",
            if (pro - con).abs() < 0.000_001 {
                "Tie"
            } else if pro > con {
                "Pro wins"
            } else {
                "Con wins"
            },
            self.ballots.len(),
            attempt_count,
            if early_consensus {
                " · early consensus"
            } else {
                ""
            },
            if disagreement {
                " · Jury disagreement"
            } else {
                ""
            }
        ));
        self.status = "Completed".into();
        Ok(())
    }
}
