//! Typed, advisory integration with TypeSafe's Jev System One model.
//!
//! Jev evaluates a state against constrained questions and returns choices and
//! probabilities. It is intentionally not represented as a chat provider.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

use serde_json::{Value, json};

use crate::core::{BridgeClient, Message, ToolDefinition};

use super::policy::ResearchLoopState;

const DEFAULT_MODEL: &str = "jev-latest";
const MIN_CONFIDENCE: f64 = 0.70;
const HARD_TOOL_FREE_CONFIDENCE: f64 = 0.86;
const HARD_TOOL_FREE_MAX_NEED: f64 = 0.20;
const READ_ONLY_FILTER_CONFIDENCE: f64 = 0.95;
const READ_ONLY_FILTER_MIN_RISK: f64 = 4.0;

#[derive(Debug, Clone)]
pub(super) struct Advice {
    pub decision: String,
    pub confidence: f64,
    pub probability: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopMode {
    Shadow,
    Enforce,
}

impl LoopMode {
    fn from_env() -> Option<Self> {
        let configured = std::env::var("JEV_LOOP_MODE")
            .or_else(|_| std::env::var("JEV_LOOP"))
            .ok()
            .or_else(|| crate::config::ConfigStore::default().jev_loop_mode().ok())?;
        match configured.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "off" | "false" | "disabled" | "none" => None,
            "1" | "on" | "true" | "enforce" | "apply" | "active" => Some(Self::Enforce),
            "shadow" | "observe" | "observed" | "log" | "advisory" => Some(Self::Shadow),
            _ => Some(Self::Shadow),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Enforce => "enforce",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // The suffix makes the tool-policy domain explicit at call sites.
enum LoopAction {
    AutoTools,
    ReadOnlyTools,
    NoTools,
}

impl LoopAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::AutoTools => "auto_tools",
            Self::ReadOnlyTools => "read_only_tools",
            Self::NoTools => "no_tools",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct LoopAdvice {
    decision: String,
    action: LoopAction,
    mode: LoopMode,
    confidence: f64,
    probability: f64,
    needs_tool: f64,
    destructive_risk: f64,
    stuck: f64,
}

impl LoopAdvice {
    fn enforced(&self) -> bool {
        self.mode == LoopMode::Enforce
    }

    fn force_tool_free(&self) -> bool {
        self.enforced()
            && self.action == LoopAction::NoTools
            && self.confidence >= HARD_TOOL_FREE_CONFIDENCE
            && self.probability >= HARD_TOOL_FREE_CONFIDENCE
            && self.needs_tool <= HARD_TOOL_FREE_MAX_NEED
    }

    fn filter_read_only(&self) -> bool {
        self.enforced()
            && self.action == LoopAction::ReadOnlyTools
            && self.confidence >= READ_ONLY_FILTER_CONFIDENCE
            && self.probability >= READ_ONLY_FILTER_CONFIDENCE
            && self.destructive_risk >= READ_ONLY_FILTER_MIN_RISK
    }

    fn instruction(&self) -> Option<String> {
        if !self.enforced() {
            return None;
        }
        Some(format!(
            "Jev loop state: decision={}; action={}; confidence={:.3}; probability={:.3}; needsTool={:.3}; destructiveRisk={:.3}.",
            self.decision,
            self.action.as_str(),
            self.confidence,
            self.probability,
            self.needs_tool,
            self.destructive_risk,
        ))
    }

    fn write_metadata(&self, metadata: &mut HashMap<String, String>) {
        metadata.insert("jevLoopMode".into(), self.mode.as_str().to_owned());
        metadata.insert("jevLoopDecision".into(), self.decision.clone());
        metadata.insert("jevLoopAction".into(), self.action.as_str().to_owned());
        metadata.insert(
            "jevLoopConfidence".into(),
            format!("{:.3}", self.confidence),
        );
        metadata.insert(
            "jevLoopProbability".into(),
            format!("{:.3}", self.probability),
        );
        metadata.insert("jevLoopNeedsTool".into(), format!("{:.3}", self.needs_tool));
        metadata.insert(
            "jevLoopRisk".into(),
            format!("{:.3}", self.destructive_risk),
        );
        metadata.insert("jevLoopStuck".into(), format!("{:.3}", self.stuck));
    }
}

pub(super) struct LoopInput<'a> {
    pub goal: &'a str,
    pub profile: &'a str,
    pub model_attempts: usize,
    pub tool_rounds: usize,
    pub consecutive_no_progress: usize,
    pub progressful_inspection_rounds: usize,
    pub implementation_requested: bool,
    pub successful_mutations: usize,
    pub verification_attempted: bool,
    pub verification_succeeded: bool,
    pub unresolved_failed_mutation: bool,
    pub planning_or_documentation: bool,
    pub bounded_analysis: bool,
    pub bounded_explanation: bool,
    pub local_file_lookup: bool,
    pub research: Option<ResearchLoopState>,
    pub retry_instruction: Option<&'a str>,
    pub recent_evidence: &'a [String],
    pub consult_jev: bool,
}

pub(super) struct LoopPolicy {
    pub selected_tools: Vec<ToolDefinition>,
    pub deferred_tools: Vec<ToolDefinition>,
    pub deferred_names: HashSet<String>,
    pub callable_names: HashSet<String>,
    pub instruction: Option<String>,
    pub advice: Option<LoopAdvice>,
    pub jev_attempted: bool,
    pub jev_request_chars: usize,
}

#[allow(clippy::too_many_arguments)] // Policy assembly combines bridge, catalog, filters, and turn input.
pub(super) fn apply_loop_policy<Defer, ReadOnly>(
    bridge: &BridgeClient,
    cancel: &AtomicBool,
    mut selected_tools: Vec<ToolDefinition>,
    tool_catalog: &[ToolDefinition],
    deferred_supported: bool,
    mut is_defer_candidate: Defer,
    mut is_read_only_extension: ReadOnly,
    input: LoopInput<'_>,
) -> LoopPolicy
where
    Defer: FnMut(&str) -> bool,
    ReadOnly: FnMut(&str) -> bool,
{
    let initial_attached_names = selected_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<HashSet<_>>();
    let mut deferred_tools = if deferred_supported {
        tool_catalog
            .iter()
            .filter(|tool| {
                !initial_attached_names.contains(&tool.name) && is_defer_candidate(&tool.name)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let available_tool_names = selected_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let deferred_tool_names = deferred_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let LoopAdviceEvaluation {
        advice,
        attempted: jev_attempted,
        request_chars: jev_request_chars,
    } = advise_loop(
        bridge,
        cancel,
        input,
        &available_tool_names,
        &deferred_tool_names,
    );
    let instruction = advice.as_ref().and_then(LoopAdvice::instruction);
    if let Some(advice) = &advice {
        if advice.force_tool_free() {
            selected_tools.clear();
            deferred_tools.clear();
        } else if advice.filter_read_only() {
            selected_tools = retain_read_only_tools(selected_tools, &mut is_read_only_extension);
            deferred_tools = retain_read_only_tools(deferred_tools, &mut is_read_only_extension);
        }
    }
    let attached_names = selected_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<HashSet<_>>();
    let deferred_names = deferred_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<HashSet<_>>();
    let callable_names = attached_names
        .iter()
        .chain(deferred_names.iter())
        .cloned()
        .collect();
    LoopPolicy {
        selected_tools,
        deferred_tools,
        deferred_names,
        callable_names,
        instruction,
        advice,
        jev_attempted,
        jev_request_chars,
    }
}

pub(super) fn append_loop_instruction(
    history: &mut Vec<Message>,
    instruction: Option<&str>,
    last_instruction: &mut Option<String>,
) {
    let Some(instruction) = instruction else {
        return;
    };
    if last_instruction.as_deref() == Some(instruction) {
        return;
    }
    history.push(Message::system(instruction.to_owned()).request_only());
    *last_instruction = Some(instruction.to_owned());
}

pub(super) fn write_loop_metadata(
    metadata: &mut HashMap<String, String>,
    advice: Option<&LoopAdvice>,
    jev_attempted: bool,
    jev_request_chars: usize,
) {
    metadata.insert("jevLoopConsulted".into(), jev_attempted.to_string());
    if jev_attempted {
        metadata.insert("jevLoopRequestChars".into(), jev_request_chars.to_string());
    }
    if let Some(advice) = advice {
        advice.write_metadata(metadata);
    }
}

pub(super) fn forces_tool_free(advice: Option<&LoopAdvice>) -> bool {
    advice.is_some_and(LoopAdvice::force_tool_free)
}

const LOOP_ADVICE_INTERVAL: usize = 4;

fn loop_advice_due(model_attempts: usize, consecutive_no_progress: usize) -> bool {
    consecutive_no_progress >= 2
        && model_attempts >= LOOP_ADVICE_INTERVAL
        && model_attempts.is_multiple_of(LOOP_ADVICE_INTERVAL)
}

#[derive(Default)]
struct LoopAdviceEvaluation {
    advice: Option<LoopAdvice>,
    attempted: bool,
    request_chars: usize,
}

fn advise_loop(
    bridge: &BridgeClient,
    cancel: &AtomicBool,
    input: LoopInput<'_>,
    available_tools: &[String],
    deferred_tools: &[String],
) -> LoopAdviceEvaluation {
    if !input.consult_jev {
        return LoopAdviceEvaluation::default();
    }
    let Some(mode) = LoopMode::from_env() else {
        return LoopAdviceEvaluation::default();
    };
    if !loop_advice_due(input.model_attempts, input.consecutive_no_progress) {
        return LoopAdviceEvaluation::default();
    }
    let Some((provider, model)) = provider_config(bridge) else {
        return LoopAdviceEvaluation::default();
    };
    let state = json!({
        "goal": truncate(input.goal, 2_000),
        "profile": input.profile,
        "model_attempts": input.model_attempts,
        "tool_rounds": input.tool_rounds,
        "consecutive_no_progress": input.consecutive_no_progress,
        "progressful_inspection_rounds": input.progressful_inspection_rounds,
        "implementation_requested": input.implementation_requested,
        "successful_mutations": input.successful_mutations,
        "verification_attempted": input.verification_attempted,
        "verification_succeeded": input.verification_succeeded,
        "unresolved_failed_mutation": input.unresolved_failed_mutation,
        "planning_or_documentation": input.planning_or_documentation,
        "bounded_analysis": input.bounded_analysis,
        "bounded_explanation": input.bounded_explanation,
        "local_file_lookup": input.local_file_lookup,
        "research": input.research,
        "retry_instruction": input.retry_instruction.map(|value| truncate(value, 1_200)),
        "available_tools": available_tools.iter().take(80).collect::<Vec<_>>(),
        "deferred_tools": deferred_tools.iter().take(80).collect::<Vec<_>>(),
        "recent_evidence": input.recent_evidence.iter().rev().take(6).map(|v| truncate(v, 1_000)).collect::<Vec<_>>(),
    });
    let questions = json!({
        "next_loop_action": {
            "type": "choice",
            "instructions": "Choose the next bare agent-loop action. Choose a loop mode only; do not invent a tool or tool arguments. Optimize for completing the user's requested outcome, not for minimizing tool calls. For research, use the supplied research evidence state: when evidence_ready=false, prefer the tool action that advances the unmet evidence requirement; when unread_sources>0, prefer reading those sources over more search. When evidence_ready=true, normally choose answer_now unless the state shows a concrete unresolved conflict or missing fact that another tool can resolve.",
            "criteria": {
                "answer_now": "The requested outcome is already supported well enough that another available tool is unlikely to materially improve correctness, completeness, execution, or verification.",
                "use_read_tool": "Read-only/context evidence from files, logs, web/docs, memory, or artifacts would materially improve the next step or resolve uncertainty.",
                "use_execute_tool": "A build, test, validation, status probe, benchmark, or other safe executable action would materially advance or verify the task.",
                "use_mutating_tool": "A persistent edit, file mutation, install, flash, deploy, or other state-changing action is required to fulfill the user's request under normal permission policy.",
                "ask_user": "A human-only decision, credential, physical action, or missing requirement blocks useful progress.",
                "stop": "Continuing is unsafe, impossible, unsupported, or would repeat the same loop without gaining evidence."
            }
        },
        "needs_tool": {
            "type": "noul",
            "instructions": "Would an available external tool materially improve the agent's ability to complete, execute, or verify the user's requested outcome before the next substantive assistant response? For research, evidence_ready=false is strong evidence that another tool is useful; evidence_ready=true means more tools need a concrete unresolved reason rather than general curiosity.",
            "criteria": {
                "true": "A tool result would materially improve correctness, completeness, execution, verification, or resolve a concrete uncertainty.",
                "false": "The requested outcome is already sufficiently supported, or only a genuinely human-only input can unblock it; another tool call would be redundant."
            }
        },
        "destructive_risk": {
            "type": "score",
            "instructions": "Rate the operational risk of the selected next action.",
            "criteria": [
                "No external side effects",
                "Read-only external observation",
                "Local reversible command or validation",
                "Persistent state change, hardware interaction, install, flash, or deploy",
                "Potentially destructive, wrong-target, hard-to-recover, or policy-sensitive action"
            ]
        },
        "stuck": {
            "type": "noul",
            "instructions": "Is the agent likely stuck in a loop or repeating actions without new information?",
            "criteria": {
                "true": "Recent attempts are repeating, low-progress, or structurally unlikely to improve.",
                "false": "The next action can make materially new progress."
            }
        }
    });

    let request_chars = serde_json::to_vec(&json!({
        "provider": &provider,
        "model": &model,
        "state": &state,
        "questions": &questions,
    }))
    .map(|serialized| serialized.len())
    .unwrap_or_default();
    match bridge.jev_evaluate_cancellable(Some(&provider), Some(&model), state, questions, cancel) {
        Ok(result) => LoopAdviceEvaluation {
            advice: parse_loop_advice(&result, mode),
            attempted: true,
            request_chars,
        },
        Err(_) => LoopAdviceEvaluation {
            advice: None,
            attempted: true,
            request_chars,
        },
    }
}

fn retain_read_only_tools<F>(
    tools: Vec<ToolDefinition>,
    mut extension_read_only: F,
) -> Vec<ToolDefinition>
where
    F: FnMut(&str) -> bool,
{
    tools
        .into_iter()
        .filter(|tool| builtin_read_only_tool(&tool.name) || extension_read_only(&tool.name))
        .collect()
}

fn builtin_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "search_tools"
            | "find_capabilities"
            | "activate_capability"
            | "list_files"
            | "read_file"
            | "read_files"
            | "search_workspace"
            | "web_search"
            | "web_read"
            | "read_document"
            | "analyze_data"
            | "artifact_info"
            | "read_artifact"
            | "search_artifact"
            | "list_sessions"
            | "context_status"
            | "context_history"
            | "project_memory_recall"
            | "project_memory_get"
            | "project_memory_connections"
    )
}

#[derive(Default)]
pub(super) struct AdviceEvaluation {
    pub advice: Option<Advice>,
    pub attempted: bool,
    pub request_chars: usize,
}

pub(super) fn advise(
    bridge: &BridgeClient,
    cancel: &AtomicBool,
    goal: &str,
    failed_tool: &str,
    failure: &str,
    recent_evidence: &[String],
) -> AdviceEvaluation {
    let Some((provider, model)) = provider_config(bridge) else {
        return AdviceEvaluation::default();
    };
    let state = json!({
        "goal": truncate(goal, 2_000),
        "failed_tool": failed_tool,
        "failure": truncate(failure, 3_000),
        "recent_evidence": recent_evidence.iter().rev().take(4).map(|v| truncate(v, 800)).collect::<Vec<_>>(),
    });
    let questions = json!({
        "next_step": {
            "type": "choice",
            "instructions": "What should the software agent do after this tool result? Choose the safest next step based only on the supplied state.",
            "criteria": {
                "retry": "Retry the same tool/action after correcting a transient or clearly fixable failure.",
                "change_plan": "Use a materially different approach or gather different evidence.",
                "continue": "Continue the current plan without retrying the failed action.",
                "ask_user": "The agent needs a user decision or missing information.",
                "stop": "Stop because continuing is unsafe, unsupported, or unnecessary."
            }
        },
        "should_retry": {
            "type": "noul",
            "instructions": "Should the agent retry the failed tool action immediately?",
            "criteria": {
                "true": "The failure is transient or can be fixed without changing the underlying plan.",
                "false": "Retrying is unlikely to help or could repeat an unsafe action."
            }
        }
    });

    let request_chars = serde_json::to_vec(&json!({
        "provider": &provider,
        "model": &model,
        "state": &state,
        "questions": &questions,
    }))
    .map(|serialized| serialized.len())
    .unwrap_or_default();
    let result = match bridge.jev_evaluate_cancellable(
        Some(&provider),
        Some(&model),
        state,
        questions,
        cancel,
    ) {
        Ok(result) => result,
        Err(_) => {
            return AdviceEvaluation {
                advice: None,
                attempted: true,
                request_chars,
            };
        }
    };
    AdviceEvaluation {
        advice: parse_failure_advice(&result),
        attempted: true,
        request_chars,
    }
}

fn parse_failure_advice(result: &Value) -> Option<Advice> {
    let answers = result.get("answers").unwrap_or(result);
    let next_step = answers.get("next_step")?;
    let decision = answer_choice(next_step)?.to_owned();
    let confidence = answer_f64(next_step, &["confidence"]).unwrap_or(0.0);
    let probability = answer_probability(next_step, &decision).unwrap_or(0.0);
    if confidence < MIN_CONFIDENCE || probability < MIN_CONFIDENCE {
        return None;
    }
    Some(Advice {
        decision,
        confidence,
        probability,
    })
}

fn provider_config(bridge: &BridgeClient) -> Option<(String, String)> {
    let explicit_provider = std::env::var("JEV_PROVIDER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let has_typesafe = std::env::var("TYPESAFE_API_KEY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let has_openrouter = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
        || bridge
            .auth_status("openrouter")
            .is_ok_and(|status| status.authenticated);

    let provider = match explicit_provider.as_deref() {
        Some("typesafe") if has_typesafe => "typesafe",
        Some("openrouter") if has_openrouter => "openrouter",
        Some("typesafe" | "openrouter") => return None,
        Some(_) => return None,
        None if has_typesafe => "typesafe",
        None if has_openrouter => "openrouter",
        None => return None,
    };
    let default_model = if provider == "openrouter" {
        "~typesafe/jev-latest"
    } else {
        DEFAULT_MODEL
    };
    let model = std::env::var("JEV_MODEL").unwrap_or_else(|_| default_model.to_owned());
    Some((provider.to_owned(), model))
}

fn parse_loop_advice(result: &Value, mode: LoopMode) -> Option<LoopAdvice> {
    let answers = result.get("answers").unwrap_or(result);
    let next = answers.get("next_loop_action")?;
    let decision = answer_choice(next)?.to_owned();
    let confidence = answer_f64(next, &["confidence"]).unwrap_or(0.0);
    let probability = answer_probability(next, &decision).unwrap_or(0.0);
    let action = match decision.as_str() {
        "answer_now" | "ask_user" | "stop" => LoopAction::NoTools,
        "use_read_tool" => LoopAction::ReadOnlyTools,
        "use_execute_tool" | "use_mutating_tool" => LoopAction::AutoTools,
        _ => return None,
    };
    Some(LoopAdvice {
        decision,
        action,
        mode,
        confidence,
        probability,
        needs_tool: answers
            .get("needs_tool")
            .and_then(noul_probability)
            .unwrap_or(0.5),
        destructive_risk: answers
            .get("destructive_risk")
            .and_then(|answer| answer_f64(answer, &["score", "value"]))
            .unwrap_or(0.0),
        stuck: answers
            .get("stuck")
            .and_then(noul_probability)
            .unwrap_or(0.0),
    })
}

fn answer_choice(answer: &Value) -> Option<&str> {
    answer
        .get("choice")
        .or_else(|| answer.get("value"))
        .or_else(|| answer.get("answer"))
        .and_then(Value::as_str)
        .or_else(|| answer.as_str())
}

fn answer_probability(answer: &Value, decision: &str) -> Option<f64> {
    answer
        .get("probabilities")
        .and_then(|value| value.get(decision))
        .and_then(Value::as_f64)
        .or_else(|| answer_f64(answer, &["probability", "confidence"]))
}

fn answer_f64(answer: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = answer.get(*key).and_then(Value::as_f64) {
            return Some(value);
        }
    }
    answer.as_f64()
}

fn noul_probability(answer: &Value) -> Option<f64> {
    answer_f64(
        answer,
        &[
            "noul",
            "probability",
            "confidence",
            "p_true",
            "true_probability",
        ],
    )
    .or_else(|| {
        answer
            .get("value")
            .and_then(Value::as_bool)
            .or_else(|| answer.as_bool())
            .map(|value| if value { 1.0 } else { 0.0 })
    })
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "test", json!({"type":"object"}))
    }

    #[test]
    fn parses_loop_advice_into_read_only_mode() {
        let result = json!({
            "answers": {
                "next_loop_action": {
                    "choice": "use_read_tool",
                    "confidence": 0.91,
                    "probabilities": { "use_read_tool": 0.88 }
                },
                "needs_tool": { "probability": 0.84 },
                "destructive_risk": { "score": 1.0 },
                "stuck": { "probability": 0.12 }
            }
        });
        let advice = parse_loop_advice(&result, LoopMode::Enforce).expect("advice");
        assert_eq!(advice.action, LoopAction::ReadOnlyTools);
        assert!(!advice.filter_read_only());
        assert!(!advice.force_tool_free());
    }

    #[test]
    fn maps_answer_now_to_tool_free_enforcement() {
        let result = json!({
            "answers": {
                "next_loop_action": {
                    "choice": "answer_now",
                    "confidence": 0.9,
                    "probabilities": { "answer_now": 0.90 }
                },
                "needs_tool": { "probability": 0.05 }
            }
        });
        let advice = parse_loop_advice(&result, LoopMode::Enforce).expect("advice");
        assert_eq!(advice.action, LoopAction::NoTools);
        assert!(advice.force_tool_free());
        let state = advice.instruction().unwrap();
        assert!(state.contains("decision=answer_now"));
        assert!(state.contains("action=no_tools"));
        assert!(!state.contains("without another tool call"));
    }

    #[test]
    fn answer_now_does_not_hide_tools_when_need_signal_disagrees() {
        let result = json!({
            "answers": {
                "next_loop_action": {
                    "choice": "answer_now",
                    "confidence": 0.94,
                    "probabilities": { "answer_now": 0.93 }
                },
                "needs_tool": { "probability": 0.72 }
            }
        });
        let advice = parse_loop_advice(&result, LoopMode::Enforce).expect("advice");
        assert!(!advice.force_tool_free());
        let state = advice.instruction().unwrap();
        assert!(state.contains("decision=answer_now"));
        assert!(state.contains("action=no_tools"));
        assert!(!state.contains("Prefer answering now"));
    }

    #[test]
    fn read_only_filter_requires_exceptionally_strong_high_risk_signal() {
        let result = json!({
            "answers": {
                "next_loop_action": {
                    "choice": "use_read_tool",
                    "confidence": 0.98,
                    "probabilities": { "use_read_tool": 0.98 }
                },
                "needs_tool": { "probability": 0.93 },
                "destructive_risk": { "score": 4.5 }
            }
        });
        let advice = parse_loop_advice(&result, LoopMode::Enforce).expect("advice");
        assert!(advice.filter_read_only());
    }

    #[test]
    fn low_confidence_loop_advice_remains_advisory_without_constraining_tools() {
        let result = json!({
            "answers": {
                "next_loop_action": {
                    "choice": "use_read_tool",
                    "confidence": 0.51,
                    "probabilities": { "use_read_tool": 0.59 }
                },
                "needs_tool": { "noul": 0.69 },
                "destructive_risk": { "score": 0.91 },
                "stuck": { "noul": 0.48 }
            }
        });
        let advice = parse_loop_advice(&result, LoopMode::Enforce).expect("advice");
        assert_eq!(advice.action, LoopAction::ReadOnlyTools);
        assert_eq!(advice.needs_tool, 0.69);
        assert_eq!(advice.stuck, 0.48);
        assert!(!advice.filter_read_only());
        assert!(!advice.force_tool_free());
    }

    #[test]
    fn read_only_filter_removes_shell_and_mutation_tools() {
        let tools = vec![
            tool("read_file"),
            tool("run_shell"),
            tool("apply_file_edits"),
            tool("context_history"),
            tool("task_notes"),
            tool("custom_readonly_extension"),
        ];
        let filtered = retain_read_only_tools(tools, |name| name == "custom_readonly_extension")
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            filtered,
            vec!["read_file", "context_history", "custom_readonly_extension"]
        );
    }
    #[test]
    fn loop_advice_is_sparse_until_progress_stalls() {
        assert!(!loop_advice_due(1, 0));
        assert!(!loop_advice_due(2, 2));
        assert!(!loop_advice_due(4, 0));
        assert!(!loop_advice_due(4, 1));
        assert!(loop_advice_due(4, 2));
        assert!(!loop_advice_due(6, 3));
        assert!(loop_advice_due(8, 2));
    }
}
