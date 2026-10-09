//! The model-facing agent tools: schemas and argument parsing.
//!
//! The Group Agent coordinator's member delegation tools.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agents::{member::AgentRole, task::SpawnRequest},
    core::ToolDefinition,
};

pub(crate) const AGENT_TOOL: &str = "delegate_task";
/// Hidden compatibility alias for restored sessions created before the
/// agent tool; never exposed as a schema.
pub(crate) const LEGACY_PROPOSE_TOOL: &str = "propose_agent_tasks";

pub(crate) const TOOL_NAMES: [&str; 1] = [AGENT_TOOL];

const LEGACY_DESCRIPTION_CHARS: usize = 60;

pub(crate) fn tool_definitions() -> Vec<ToolDefinition> {
    vec![ToolDefinition::new(
        AGENT_TOOL,
        "Delegate one bounded, role-specific task to a member of this Group Agent. Include only the context that member needs; the group supplies its shared objective and relevant promoted findings. The call returns a member checkpoint to the coordinator. Multiple delegations in one response run concurrently and are all integrated into group state.",
        json!({
            "type":"object",
            "properties":{
                "description":{"type":"string","minLength":1,"maxLength":120,"description":"A short (3-8 word) label for the task."},
                "prompt":{"type":"string","minLength":1,"maxLength":12000,"description":"The complete, self-contained task for the agent."},
                "role":{"type":"string","enum":["researcher","implementer","verifier"]},
            },
            "required":["description","prompt","role"],
            "additionalProperties":false
        }),
    )]
}

pub(crate) fn parse_spawn(arguments: &Map<String, Value>) -> Result<SpawnRequest> {
    Ok(SpawnRequest {
        role: AgentRole::parse(required_str(arguments, "role")?)?,
        description: required_str(arguments, "description")?.to_owned(),
        prompt: required_str(arguments, "prompt")?.to_owned(),
        background: false,
    })
}

/// Parses the legacy `propose_agent_tasks` batch as foreground spawns.
pub(crate) fn parse_legacy_proposals(arguments: &Map<String, Value>) -> Result<Vec<SpawnRequest>> {
    let tasks = arguments
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("tasks must be an array"))?;
    if tasks.is_empty() {
        bail!("tasks must not be empty");
    }
    tasks
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| anyhow!("each task must be an object"))?;
            let prompt = required_str(object, "task")?;
            Ok(SpawnRequest {
                role: AgentRole::parse(required_str(object, "role")?)?,
                description: prompt.chars().take(LEGACY_DESCRIPTION_CHARS).collect(),
                prompt: prompt.to_owned(),
                background: false,
            })
        })
        .collect()
}

fn required_str<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    let value = arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if value.is_empty() {
        bail!("{key} is required");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_tasks_are_awaited_by_the_group() {
        let arguments = json!({
            "description":"inspect parser",
            "prompt":"  read src/parser.rs  ",
            "role":"researcher",
            "run_in_background":true
        });
        let request = parse_spawn(arguments.as_object().unwrap()).unwrap();
        assert_eq!(request.role, AgentRole::Researcher);
        assert_eq!(request.prompt, "read src/parser.rs");
        assert!(!request.background);
    }

    #[test]
    fn legacy_proposals_allow_arbitrary_group_size() {
        let tasks = (0..8)
            .map(|_| json!({"role":"researcher","task":"a"}))
            .collect::<Vec<_>>();
        let arguments = json!({"tasks":tasks});
        let parsed = parse_legacy_proposals(arguments.as_object().unwrap()).unwrap();
        assert_eq!(parsed.len(), 8);
        assert!(!parsed[0].background);
    }
}
