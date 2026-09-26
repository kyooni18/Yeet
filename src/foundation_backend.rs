use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

use crate::{
    core::{BridgeClient, McpTool},
    project_settings::ServiceBackend,
};

pub(crate) const BUILTIN_FOUNDATION_SERVER: &str = "yeet.foundation.builtin";
pub(crate) const MODEL_TOOLS: [&str; 5] = [
    "memory_recall",
    "memory_remember",
    "memory_update",
    "memory_replace",
    "memory_forget",
];

pub(crate) fn is_model_tool(name: &str) -> bool {
    MODEL_TOOLS.contains(&name)
}

pub(crate) fn project_scoped_model_tool<'a>(
    tools: &'a [McpTool],
    name: &str,
) -> Option<&'a McpTool> {
    tools.iter().find(|tool| {
        tool.name == name
            && is_model_tool(&tool.name)
            && tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key("project"))
    })
}

pub(crate) fn project_scoped_model_tool_names(tools: &[McpTool]) -> Vec<String> {
    MODEL_TOOLS
        .iter()
        .filter_map(|name| project_scoped_model_tool(tools, name).map(|tool| tool.name.clone()))
        .collect()
}

pub(crate) fn has_required_project_scoped_tools(tools: &[McpTool]) -> bool {
    MODEL_TOOLS
        .iter()
        .all(|name| project_scoped_model_tool(tools, name).is_some())
}

pub(crate) fn validate_project_scoped_tools(tools: &[McpTool], label: &str) -> Result<()> {
    for name in MODEL_TOOLS {
        ensure!(
            project_scoped_model_tool(tools, name).is_some(),
            "{label} is missing project-scoped {name}"
        );
    }
    Ok(())
}

pub(crate) fn server_for_backend(
    _bridge: &BridgeClient,
    backend: ServiceBackend,
    external_server: &str,
) -> Result<String> {
    match backend {
        ServiceBackend::Builtin => {
            bail!(
                "Yeet's built-in Foundation runtime is not available yet; configure an external Foundation MCP server"
            )
        }
        ServiceBackend::Mcp => Ok(external_server.to_owned()),
    }
}

pub(crate) fn ensure_builtin_foundation(_bridge: &BridgeClient) -> Result<Vec<McpTool>> {
    bail!(
        "Yeet's built-in Foundation runtime is not available yet; configure memory.backend=mcp and point memory.server at an external Foundation MCP server"
    )
}

pub(crate) fn builtin_status(_bridge: &BridgeClient) -> Result<Value> {
    Ok(json!({
        "storage": "foundation",
        "runtime": "not-yet-embedded",
        "connected": false,
        "message": "Configure an external Foundation MCP server; Yeet does not start Foundation through Docker Compose.",
    }))
}
