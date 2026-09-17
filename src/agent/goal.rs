//! Strict completion judging for Goal mode.

use serde::Deserialize;

use super::*;

pub(super) const GOAL_JUDGE_SYSTEM_INSTRUCTION: &str = r#"You are Yeet's strict goal-success judge.

Judge only the evidence supplied in the request. Never infer that work is complete from a plan, intention, optimistic wording, or an unverified claim. Return exactly one JSON object and no markdown or extra text:
{"verdict":"success|failure","evidence":["short factual evidence"],"remaining":["unfinished requirement"]}

Use verdict "success" only when every explicit requirement is satisfied by the evidence and remaining is an empty array. If anything is missing, ambiguous, unverified, blocked, or contradicted, use verdict "failure" and list it in remaining. Evidence must be concrete and must not repeat the model's claim as proof."#;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalJudgeResponse {
    verdict: String,
    evidence: Vec<String>,
    remaining: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GoalVerdict {
    pub(super) passed: bool,
    pub(super) reason: String,
}

pub(super) fn parse_goal_verdict(text: &str) -> GoalVerdict {
    let parsed = match serde_json::from_str::<GoalJudgeResponse>(text.trim()) {
        Ok(value) => value,
        Err(error) => {
            return GoalVerdict {
                passed: false,
                reason: format!("strict goal judge returned invalid JSON: {error}"),
            };
        }
    };

    let evidence = normalized_items(&parsed.evidence);
    let remaining = normalized_items(&parsed.remaining);
    if parsed.verdict == "success"
        && evidence.as_ref().is_some_and(|items| !items.is_empty())
        && remaining.as_ref().is_some_and(Vec::is_empty)
    {
        let evidence = evidence.unwrap_or_default();
        return GoalVerdict {
            passed: true,
            reason: format!("strict judge accepted: {}", evidence.join("; ")),
        };
    }

    let reason = if let Some(remaining) = remaining.filter(|items| !items.is_empty()) {
        remaining.join("; ")
    } else if parsed.verdict != "failure" {
        format!(
            "strict judge returned unsupported verdict {:?}",
            parsed.verdict
        )
    } else if evidence.as_ref().is_none_or(Vec::is_empty) {
        "strict judge rejected the goal without concrete evidence".to_owned()
    } else {
        "strict judge rejected the goal".to_owned()
    };
    GoalVerdict {
        passed: false,
        reason,
    }
}

fn normalized_items(items: &[String]) -> Option<Vec<String>> {
    const MAX_ITEMS: usize = 8;
    const MAX_ITEM_CHARS: usize = 500;
    if items.len() > MAX_ITEMS {
        return None;
    }
    let normalized = items
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    normalized
        .iter()
        .all(|value| value.chars().count() <= MAX_ITEM_CHARS)
        .then_some(normalized)
}

pub(super) fn render_goal_evidence(history: &[Message]) -> String {
    const MAX_CHARS: usize = 24_000;
    let mut rendered = String::new();
    for message in history.iter().skip(1) {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let content = message.content.as_deref().unwrap_or_default().trim();
        let tool_calls = message
            .tool_calls
            .as_ref()
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| call.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let line = if tool_calls.is_empty() {
            format!("{role}: {content}\n")
        } else {
            format!("{role} tool_calls=[{tool_calls}]: {content}\n")
        };
        if rendered.chars().count() + line.chars().count() > MAX_CHARS {
            rendered.push_str("[earlier evidence omitted for judge budget]\n");
            break;
        }
        rendered.push_str(&line);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_exact_success_contract() {
        let verdict = parse_goal_verdict(
            r#"{"verdict":"success","evidence":["cargo test passed"],"remaining":[]}"#,
        );
        assert!(verdict.passed);
    }

    #[test]
    fn rejects_claims_without_evidence_or_with_remaining_work() {
        assert!(
            !parse_goal_verdict(r#"{"verdict":"success","evidence":[],"remaining":[]}"#,).passed
        );
        assert!(
            !parse_goal_verdict(
                r#"{"verdict":"success","evidence":["looks done"],"remaining":["verify it"]}"#,
            )
            .passed
        );
    }

    #[test]
    fn rejects_markdown_and_unknown_fields() {
        assert!(
            !parse_goal_verdict(
                "```json\n{\"verdict\":\"success\",\"evidence\":[\"x\"],\"remaining\":[]}\n```"
            )
            .passed
        );
        assert!(
            !parse_goal_verdict(
                r#"{"verdict":"success","evidence":["x"],"remaining":[],"confidence":"high"}"#,
            )
            .passed
        );
    }
}
