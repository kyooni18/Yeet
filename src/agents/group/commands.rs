//! The model-facing agent tools: schemas and argument parsing.
//!
//! The surface follows the subagent model of Claude Code: one call launches
//! one agent (foreground by default, or in the background with a completion
//! notification), agents are continued by message, and stopped explicitly.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agents::{AgentId, member::AgentRole, task::SpawnRequest},
    core::ToolDefinition,
};

pub(crate) const AGENT_TOOL: &str = "agent";
pub(crate) const SEND_TOOL: &str = "send_agent_message";
pub(crate) const STOP_TOOL: &str = "stop_agent";
/// Hidden compatibility alias for restored sessions created before the
/// agent tool; never exposed as a schema.
pub(crate) const LEGACY_PROPOSE_TOOL: &str = "propose_agent_tasks";

pub(crate) const TOOL_NAMES: [&str; 3] = [AGENT_TOOL, SEND_TOOL, STOP_TOOL];

const MAX_LEGACY_TASKS: usize = 4;
const LEGACY_DESCRIPTION_CHARS: usize = 60;

pub(crate) fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            AGENT_TOOL,
            "Launch a worker agent with its own isolated context for a bounded task. Roles: researcher (read-only inspection and research), implementer (a bounded code change; only one works at a time), verifier (tests and independent validation; no file writes). By default the call blocks and returns the agent's final report; several agent calls in one response run concurrently. Set run_in_background=true to return immediately: you will be notified with the result when it finishes, so do not poll or wait for it. The agent cannot see this conversation, so write a self-contained prompt. Workers cannot launch agents. The result includes an agentId for send_agent_message.",
            json!({
                "type":"object",
                "properties":{
                    "description":{"type":"string","minLength":1,"maxLength":120,"description":"A short (3-8 word) label for the task."},
                    "prompt":{"type":"string","minLength":1,"maxLength":12000,"description":"The complete, self-contained task for the agent."},
                    "role":{"type":"string","enum":["researcher","implementer","verifier"]},
                    "run_in_background":{"type":"boolean","description":"Return immediately and deliver the result as a notification."}
                },
                "required":["description","prompt","role"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            SEND_TOOL,
            "Send a follow-up message to an agent launched earlier, continuing it with its context intact. The message runs in the background and you will be notified when it finishes; if the agent is busy the message is queued.",
            json!({
                "type":"object",
                "properties":{
                    "to":{"type":"string","description":"The agentId returned by agent."},
                    "message":{"type":"string","minLength":1,"maxLength":12000}
                },
                "required":["to","message"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            STOP_TOOL,
            "Stop an agent. Its current and queued work is cancelled and it cannot be messaged again.",
            json!({
                "type":"object",
                "properties":{
                    "agent_id":{"type":"string"}
                },
                "required":["agent_id"],
                "additionalProperties":false
            }),
        ),
    ]
}

pub(crate) fn parse_spawn(arguments: &Map<String, Value>) -> Result<SpawnRequest> {
    Ok(SpawnRequest {
        role: AgentRole::parse(required_str(arguments, "role")?)?,
        description: required_str(arguments, "description")?.to_owned(),
        prompt: required_str(arguments, "prompt")?.to_owned(),
        background: arguments
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub(crate) fn parse_send(arguments: &Map<String, Value>) -> Result<(AgentId, String)> {
    Ok((
        parse_agent_id(required_str(arguments, "to")?)?,
        required_str(arguments, "message")?.to_owned(),
    ))
}

pub(crate) fn parse_stop(arguments: &Map<String, Value>) -> Result<AgentId> {
    parse_agent_id(required_str(arguments, "agent_id")?)
}

/// Parses the legacy `propose_agent_tasks` batch as foreground spawns.
pub(crate) fn parse_legacy_proposals(arguments: &Map<String, Value>) -> Result<Vec<SpawnRequest>> {
    let tasks = arguments
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("tasks must be an array"))?;
    if tasks.is_empty() || tasks.len() > MAX_LEGACY_TASKS {
        bail!("tasks must contain between 1 and {MAX_LEGACY_TASKS} items");
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

fn parse_agent_id(value: &str) -> Result<AgentId> {
    value
        .parse()
        .map_err(|_| anyhow!("invalid agent id: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_background_spawn() {
        let arguments = json!({
            "description":"inspect parser",
            "prompt":"  read src/parser.rs  ",
            "role":"researcher",
            "run_in_background":true
        });
        let request = parse_spawn(arguments.as_object().unwrap()).unwrap();
        assert_eq!(request.role, AgentRole::Researcher);
        assert_eq!(request.prompt, "read src/parser.rs");
        assert!(request.background);
    }

    #[test]
    fn legacy_proposals_stay_bounded() {
        let task = json!({"role":"researcher","task":"a"});
        let five = json!({"tasks":[task, task, task, task, task]});
        assert!(parse_legacy_proposals(five.as_object().unwrap()).is_err());
        let one = json!({"tasks":[task]});
        let parsed = parse_legacy_proposals(one.as_object().unwrap()).unwrap();
        assert!(!parsed[0].background);
    }
}
