//! The model-facing delegation tool: its schema and argument parsing.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agents::{member::AgentRole, task::ProposedTask},
    core::ToolDefinition,
};

pub(super) const MAX_PROPOSED_TASKS: usize = 4;

pub(super) fn tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "propose_agent_tasks",
        "Propose 1-4 bounded, independent tasks that could benefit from parallel workers. The runtime scheduler decides admission, enforces concurrency/budget/write policy, and returns structured findings. Use researcher for read-only inspection/research, implementer for a bounded code change, and verifier for tests or independent validation. Workers cannot spawn workers.",
        json!({
            "type":"object",
            "properties":{
                "tasks":{
                    "type":"array",
                    "minItems":1,
                    "maxItems":MAX_PROPOSED_TASKS,
                    "items":{
                        "type":"object",
                        "properties":{
                            "role":{"type":"string","enum":["researcher","implementer","verifier"]},
                            "task":{"type":"string","minLength":1,"maxLength":12000}
                        },
                        "required":["role","task"],
                        "additionalProperties":false
                    }
                }
            },
            "required":["tasks"],
            "additionalProperties":false
        }),
    )
}

pub(super) fn parse_proposed_tasks(arguments: &Map<String, Value>) -> Result<Vec<ProposedTask>> {
    let tasks = arguments
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("tasks must be an array"))?;
    if tasks.is_empty() || tasks.len() > MAX_PROPOSED_TASKS {
        bail!("tasks must contain between 1 and {MAX_PROPOSED_TASKS} items");
    }
    tasks
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| anyhow!("each task must be an object"))?;
            let role = AgentRole::parse(
                object
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("task role is required"))?,
            )?;
            let objective = object
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if objective.is_empty() {
                bail!("task objective must not be empty");
            }
            Ok(ProposedTask { role, objective })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_agent_task_proposals() {
        let arguments = json!({
            "tasks": [
                {"role":"researcher","task":"  inspect the parser  "},
                {"role":"verifier","task":"run focused tests"}
            ]
        });
        let tasks = parse_proposed_tasks(arguments.as_object().unwrap()).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].role, AgentRole::Researcher);
        assert_eq!(tasks[0].objective, "inspect the parser");
        assert_eq!(tasks[1].role, AgentRole::Verifier);
    }

    #[test]
    fn rejects_recursive_scale_agent_proposals() {
        let arguments = json!({
            "tasks": [
                {"role":"researcher","task":"a"},
                {"role":"researcher","task":"b"},
                {"role":"researcher","task":"c"},
                {"role":"researcher","task":"d"},
                {"role":"researcher","task":"e"}
            ]
        });
        assert!(parse_proposed_tasks(arguments.as_object().unwrap()).is_err());
    }
}
