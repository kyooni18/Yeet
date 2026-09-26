//! Open-ended questions answered by ranking candidate answers.
use super::parse_json_object;
use crate::core::CallResult;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// General question support for subjects that cannot be faithfully reduced to
/// a yes/no proposition. This is intentionally separate from the legacy
/// `DebateContract`: persisted binary debates keep their old wire shape while
/// callers can adopt this contract incrementally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneralQuestionKind {
    #[default]
    Recommendation,
    Explanation,
    Ranking,
    Diagnosis,
    Forecast,
    Design,
    Plan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebateAnswerCandidate {
    pub id: String,
    pub answer: String,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeneralDebateContract {
    pub question: String,
    pub kind: GeneralQuestionKind,
    pub answer_format: String,
    pub candidates: Vec<DebateAnswerCandidate>,
    pub criteria: Vec<String>,
    #[serde(default)]
    pub ambiguities: Vec<String>,
}

impl GeneralDebateContract {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.question.trim().is_empty(), "question is empty");
        ensure!(
            self.question.chars().count() <= 4000,
            "question must contain at most 4000 characters"
        );
        ensure!(
            !self.answer_format.trim().is_empty(),
            "answer format is empty"
        );
        ensure!(
            (2..=8).contains(&self.candidates.len()),
            "general question requires 2-8 candidate answers"
        );
        ensure!(
            (2..=6).contains(&self.criteria.len()),
            "general question requires 2-6 evaluation criteria"
        );
        let mut ids = HashSet::new();
        ensure!(
            self.candidates.iter().all(|candidate| {
                !candidate.id.trim().is_empty()
                    && !candidate.answer.trim().is_empty()
                    && ids.insert(candidate.id.trim().to_lowercase())
            }),
            "candidate IDs and answers must be non-empty and IDs must be unique"
        );
        let mut criteria = HashSet::new();
        ensure!(
            self.criteria.iter().all(|criterion| {
                !criterion.trim().is_empty() && criteria.insert(criterion.trim().to_lowercase())
            }),
            "criteria must be non-empty and unique"
        );
        ensure!(
            self.ambiguities.len() <= 8
                && self.ambiguities.iter().all(|item| !item.trim().is_empty()),
            "ambiguities must contain at most 8 non-empty items"
        );
        Ok(())
    }

    pub fn candidate_index(&self, id: &str) -> Option<usize> {
        self.candidates
            .iter()
            .position(|candidate| candidate.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeneralDebateBallot {
    /// `scores[candidate_index][criterion_index]`, in contract candidate order.
    pub scores: Vec<Vec<u8>>,
    pub reason: String,
}

impl GeneralDebateBallot {
    pub fn parse(text: &str, candidate_count: usize, criteria_count: usize) -> Result<Self> {
        let value = parse_json_object(text)?;
        Self::from_value(value, candidate_count, criteria_count)
    }

    pub fn parse_response(
        response: &CallResult,
        candidate_count: usize,
        criteria_count: usize,
    ) -> Result<Self> {
        ensure!(
            response.finish_reason != "length",
            "general jury response was truncated"
        );
        let calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| call.name == "submit_general_debate_ballot")
            .collect();
        ensure!(
            calls.len() <= 1,
            "general jury returned multiple ballot tool calls"
        );
        let value = if let Some(call) = calls.first() {
            call.arguments.clone()
        } else {
            ensure!(
                response.tool_calls.is_empty(),
                "general jury returned an unexpected tool call"
            );
            parse_json_object(&response.text)?
        };
        Self::from_value(value, candidate_count, criteria_count)
    }

    fn from_value(value: Value, candidate_count: usize, criteria_count: usize) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            scores: Vec<Vec<u8>>,
            reason: String,
        }
        let raw: Raw = serde_json::from_value(value)?;
        ensure!(
            raw.scores.len() == candidate_count,
            "general ballot must score every candidate"
        );
        ensure!(
            raw.scores
                .iter()
                .all(|scores| scores.len() == criteria_count),
            "general ballot criteria counts do not match the contract"
        );
        ensure!(
            raw.scores.iter().flatten().all(|score| *score <= 10),
            "general ballot scores must be 0-10"
        );
        ensure!(
            !raw.reason.trim().is_empty(),
            "general jury must provide a reason"
        );
        Ok(Self {
            scores: raw.scores,
            reason: raw.reason.trim().to_owned(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedDebateAnswer {
    pub candidate: DebateAnswerCandidate,
    pub mean_score: f64,
    pub total_score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralDebateVerdict {
    pub ranking: Vec<RankedDebateAnswer>,
    pub synthesis: String,
    pub confidence: f64,
}

impl GeneralDebateVerdict {
    pub fn from_ballots(
        contract: &GeneralDebateContract,
        ballots: &[GeneralDebateBallot],
    ) -> Result<Self> {
        contract.validate()?;
        ensure!(
            !ballots.is_empty(),
            "cannot build a verdict without ballots"
        );
        let candidate_count = contract.candidates.len();
        let criteria_count = contract.criteria.len();
        ensure!(
            ballots.iter().all(|ballot| {
                ballot.scores.len() == candidate_count
                    && ballot
                        .scores
                        .iter()
                        .all(|scores| scores.len() == criteria_count)
            }),
            "ballots do not match the question contract"
        );

        let mut totals = vec![0u32; candidate_count];
        for ballot in ballots {
            for (candidate_index, scores) in ballot.scores.iter().enumerate() {
                totals[candidate_index] += scores.iter().map(|score| *score as u32).sum::<u32>();
            }
        }
        let denominator = (ballots.len() * criteria_count) as f64;
        let mut order: Vec<usize> = (0..candidate_count).collect();
        order.sort_by(|left, right| {
            totals[*right].cmp(&totals[*left]).then_with(|| {
                contract.candidates[*left]
                    .id
                    .cmp(&contract.candidates[*right].id)
            })
        });
        let ranking = order
            .into_iter()
            .map(|index| RankedDebateAnswer {
                candidate: contract.candidates[index].clone(),
                mean_score: totals[index] as f64 / denominator,
                total_score: totals[index],
            })
            .collect::<Vec<_>>();
        let top = ranking.first().map(|item| item.mean_score).unwrap_or(0.0);
        let second = ranking.get(1).map(|item| item.mean_score).unwrap_or(0.0);
        let confidence = ((top - second) / 10.0).clamp(0.0, 1.0);
        let synthesis = format!(
            "Best-supported answer: {}. It scored {:.1}/10 across {} ballot(s) and {} criterion(s). The ranking is evidence-relative; unresolved ambiguities and unsupported assumptions remain open.",
            ranking[0].candidate.answer,
            ranking[0].mean_score,
            ballots.len(),
            criteria_count
        );
        Ok(Self {
            ranking,
            synthesis,
            confidence,
        })
    }
}

#[cfg(test)]
mod general_question_tests {
    use super::*;

    fn contract() -> GeneralDebateContract {
        GeneralDebateContract {
            question: "Which storage design should we use?".into(),
            kind: GeneralQuestionKind::Recommendation,
            answer_format: "Rank the alternatives and recommend one with caveats.".into(),
            candidates: vec![
                DebateAnswerCandidate {
                    id: "a".into(),
                    answer: "Relational storage".into(),
                    evidence_ids: vec![],
                },
                DebateAnswerCandidate {
                    id: "b".into(),
                    answer: "Document storage".into(),
                    evidence_ids: vec![],
                },
                DebateAnswerCandidate {
                    id: "c".into(),
                    answer: "Embedded key-value storage".into(),
                    evidence_ids: vec![],
                },
            ],
            criteria: vec!["Evidence".into(), "Fit".into(), "Risk".into()],
            ambiguities: vec![],
        }
    }

    #[test]
    fn validates_and_ranks_multiple_answers() {
        let contract = contract();
        let ballots = vec![
            GeneralDebateBallot {
                scores: vec![vec![9, 8, 8], vec![6, 6, 6], vec![7, 7, 7]],
                reason: "A is best supported.".into(),
            },
            GeneralDebateBallot {
                scores: vec![vec![8, 9, 8], vec![7, 6, 6], vec![7, 8, 7]],
                reason: "A remains best.".into(),
            },
        ];
        let verdict = GeneralDebateVerdict::from_ballots(&contract, &ballots).unwrap();
        assert_eq!(verdict.ranking[0].candidate.id, "a");
        assert!(verdict.confidence > 0.0);
    }

    #[test]
    fn rejects_duplicate_candidates() {
        let mut contract = contract();
        contract.candidates[1].id = "A".into();
        assert!(contract.validate().is_err());
    }

    #[test]
    fn parses_structured_general_ballot() {
        let ballot = GeneralDebateBallot::parse(
            r#"{"scores":[[8,7],[6,5]],"reason":"Evidence favors the first answer."}"#,
            2,
            2,
        )
        .unwrap();
        assert_eq!(ballot.scores[0][0], 8);
    }
}
