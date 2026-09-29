use anyhow::Result;
use serde_json::{Map, Value, json};

use super::{BridgeClient, McpServerConfiguration, McpTool, field};

impl BridgeClient {
    pub fn remove_runtime_mcp_server(&self, server: &str) -> Result<bool> {
        let frame = self.request("mcp-remove-runtime-server", field("server", server))?;
        Ok(frame
            .get("removed")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    pub fn set_external_mcp_server(
        &self,
        server: &McpServerConfiguration,
    ) -> Result<McpServerConfiguration> {
        let mut fields = Map::new();
        fields.insert("server".into(), serde_json::to_value(server)?);
        self.request_typed("external-mcp-set-runtime-server", fields, "server")
    }

    pub fn remove_external_mcp_server(&self, server: &str) -> Result<bool> {
        let frame = self.request(
            "external-mcp-remove-runtime-server",
            field("server", server),
        )?;
        Ok(frame
            .get("removed")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    pub fn list_external_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>> {
        self.request_typed("external-mcp-list-tools", field("server", server), "tools")
    }

    pub fn call_external_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: &Map<String, Value>,
    ) -> Result<Value> {
        let mut fields = field("server", server);
        fields.insert("tool".into(), json!(tool));
        fields.insert("arguments".into(), Value::Object(arguments.clone()));
        let frame = self.request("external-mcp-call-tool", fields)?;
        Ok(frame.get("toolResult").cloned().unwrap_or(Value::Null))
    }
}
