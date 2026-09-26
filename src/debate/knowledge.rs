//! Evidence retained from a finished debate for reuse by later turns.
use super::{DebateSubject, parse_json_object};
use crate::core::CallResult;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebateKnowledgeSummary {
    #[serde(default)]
    pub supported_facts: Vec<String>,
    #[serde(default)]
    pub strong_inferences: Vec<String>,
    #[serde(default)]
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetainedKnowledgeClaim {
    pub text: String,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetainedDebateKnowledge {
    pub subject_ref: DebateSubject,
    pub source_debate_run_id: String,
    #[serde(default)]
    pub supported_claims: Vec<RetainedKnowledgeClaim>,
    #[serde(default)]
    pub strong_inferences: Vec<String>,
    #[serde(default)]
    pub unresolved: Vec<String>,
    pub confidence: String,
}

impl RetainedDebateKnowledge {
    pub fn from_summary(
        subject_ref: DebateSubject,
        source_debate_run_id: String,
        summary: &DebateKnowledgeSummary,
        evidence_ids: Vec<String>,
    ) -> Self {
        let supported_claims = summary
            .supported_facts
            .iter()
            .cloned()
            .map(|text| RetainedKnowledgeClaim {
                text,
                evidence_ids: evidence_ids.clone(),
            })
            .collect();
        Self {
            subject_ref,
            source_debate_run_id,
            supported_claims,
            strong_inferences: summary.strong_inferences.clone(),
            unresolved: summary.unresolved.clone(),
            confidence: if evidence_ids.len() >= 3 {
                "evidence_backed".into()
            } else {
                "limited_evidence".into()
            },
        }
    }
}

impl DebateKnowledgeSummary {
    pub fn parse_response(response: &CallResult) -> Result<Self> {
        ensure!(
            response.finish_reason != "length",
            "knowledge summary response was truncated"
        );
        let summary_calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| call.name == "submit_debate_knowledge")
            .collect();
        ensure!(
            summary_calls.len() <= 1,
            "knowledge synthesis returned multiple summary tool calls"
        );
        let value = if let Some(call) = summary_calls.first() {
            call.arguments.clone()
        } else {
            ensure!(
                response.tool_calls.is_empty(),
                "knowledge synthesis returned an unexpected tool call"
            );
            parse_json_object(&response.text)?
        };
        let summary: Self = serde_json::from_value(value)?;
        ensure!(
            summary
                .supported_facts
                .iter()
                .chain(&summary.strong_inferences)
                .chain(&summary.unresolved)
                .all(|item| !item.trim().is_empty()),
            "knowledge summary contains an empty item"
        );
        ensure!(
            !summary.supported_facts.is_empty()
                || !summary.strong_inferences.is_empty()
                || !summary.unresolved.is_empty(),
            "knowledge summary is empty"
        );
        Ok(summary)
    }

    pub fn render(&self) -> String {
        fn section(title: &str, items: &[String], output: &mut String) {
            output.push_str(title);
            output.push('\n');
            if items.is_empty() {
                output.push_str("- None\n");
            } else {
                for item in items {
                    output.push_str("- ");
                    output.push_str(item.trim());
                    output.push('\n');
                }
            }
        }

        let mut output = String::new();
        section(
            "Supported facts from collected evidence",
            &self.supported_facts,
            &mut output,
        );
        output.push('\n');
        section("Strong inferences", &self.strong_inferences, &mut output);
        output.push('\n');
        section("Still unresolved", &self.unresolved, &mut output);
        output.trim_end().to_owned()
    }
}
