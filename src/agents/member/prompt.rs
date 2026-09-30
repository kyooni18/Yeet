//! Prompt and reasoning defaults for delegated worker members.

use super::AgentRole;

pub(crate) fn worker_prompt(role: AgentRole, objective: &str) -> String {
    format!(
        "Role: {} worker. Complete this bounded delegated task and return concise findings with concrete evidence for the primary agent.\n\nTask:\n{}",
        role.as_str(),
        objective.trim()
    )
}

pub(crate) fn worker_reasoning_level(model: &str) -> &'static str {
    let lower = model.to_ascii_lowercase();
    if lower.contains("luna") {
        "low"
    } else {
        "medium"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_reasoning_defaults_are_cost_aware() {
        assert_eq!(worker_reasoning_level("openai/gpt-5.6-sol"), "medium");
        assert_eq!(worker_reasoning_level("openai/gpt-5.6-luna"), "low");
    }
}
