//! Main-Agent facing Group Agent lifecycle operations.

use crate::core::ToolDefinition;
use serde_json::json;

pub(crate) const CREATE_TOOL: &str = "create_agent_group";
pub(crate) const START_TOOL: &str = "start_agent_group";
pub(crate) const RESUME_TOOL: &str = "resume_agent_group";
pub(crate) const CANCEL_TOOL: &str = "cancel_agent_group";
pub(crate) const STOP_TOOL: &str = "stop_agent_group";
pub(crate) const INSPECT_TOOL: &str = "inspect_agent_group";

pub(crate) const TOOL_NAMES: [&str; 6] = [
    CREATE_TOOL,
    START_TOOL,
    RESUME_TOOL,
    CANCEL_TOOL,
    STOP_TOOL,
    INSPECT_TOOL,
];

pub(crate) fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            CREATE_TOOL,
            "Create a Group Agent for one shared objective. The group owns decomposition, role-specific delegation, shared findings, and the integrated result.",
            json!({
                "type":"object",
                "properties":{"objective":{"type":"string","minLength":1,"maxLength":12000}},
                "required":["objective"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            START_TOOL,
            "Start the Group Agent coordinator for a created objective. Member work and findings remain inside the group; this returns one integrated group result.",
            group_id_schema(),
        ),
        ToolDefinition::new(
            RESUME_TOOL,
            "Resume a paused Group Agent from its saved coordinator context and member checkpoints.",
            group_id_schema(),
        ),
        ToolDefinition::new(
            CANCEL_TOOL,
            "Cancel the Group Agent coordinator and its active member work.",
            group_id_schema(),
        ),
        ToolDefinition::new(
            STOP_TOOL,
            "Stop the Group Agent and retire its member execution contexts.",
            group_id_schema(),
        ),
        ToolDefinition::new(
            INSPECT_TOOL,
            "Inspect the Group Agent lifecycle, member states, shared findings, and event stream.",
            group_id_schema(),
        ),
    ]
}

fn group_id_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "properties":{"group_id":{"type":"string","format":"uuid"}},
        "required":["group_id"],
        "additionalProperties":false
    })
}
