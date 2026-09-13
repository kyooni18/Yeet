//! Compact, argument-free diagnostics for MCP runtime failures.

use std::path::Path;

use serde_json::Value;

pub(super) fn runtime_request_summary(payload: &Value, default_workspace: &Path) -> String {
    let mut ids = Vec::new();
    let mut methods = Vec::new();
    let mut tools = Vec::new();
    let mut workspaces = Vec::new();
    let mut pending = vec![payload];
    while let Some(value) = pending.pop() {
        if let Value::Array(items) = value {
            pending.extend(items.iter().rev());
            continue;
        }
        let Some(object) = value.as_object() else {
            continue;
        };
        if let Some(id) = object.get("id") {
            ids.push(trace_json_id(id));
        }
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        methods.push(method.to_owned());
        if method != "tools/call" {
            continue;
        }
        let Some(params) = object.get("params").and_then(Value::as_object) else {
            continue;
        };
        if let Some(name) = params.get("name").and_then(Value::as_str) {
            tools.push(name.to_owned());
        }
        let workspace = params
            .get("arguments")
            .and_then(Value::as_object)
            .and_then(|arguments| arguments.get("workspace"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| default_workspace.display().to_string());
        workspaces.push(workspace);
    }
    ids.sort();
    ids.dedup();
    methods.sort();
    methods.dedup();
    tools.sort();
    tools.dedup();
    workspaces.sort();
    workspaces.dedup();
    format!(
        "id={} method={} tool={} workspace={}",
        trace_list(&ids, "notification"),
        trace_list(&methods, "unknown"),
        trace_list(&tools, "none"),
        trace_list(&workspaces, default_workspace.to_string_lossy().as_ref())
    )
}

fn trace_json_id(value: &Value) -> String {
    match value {
        Value::String(value) => value.chars().take(96).collect(),
        Value::Number(value) => value.to_string(),
        Value::Null => "null".into(),
        _ => "non-scalar".into(),
    }
}

fn trace_list(values: &[String], fallback: &str) -> String {
    if values.is_empty() {
        return fallback.to_owned();
    }
    let mut joined = values.iter().take(4).cloned().collect::<Vec<_>>().join(",");
    if values.len() > 4 {
        joined.push_str(",...");
    }
    if joined.len() > 320 {
        joined.truncate(320);
        joined.push_str("...");
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn summary_identifies_request_without_logging_arguments() {
        let payload = json!({
            "jsonrpc":"2.0",
            "id":"call-42",
            "method":"tools/call",
            "params":{
                "name":"run_shell",
                "arguments":{
                    "command":"printf SUPER_SECRET_VALUE",
                    "workspace":"/tmp/trace-workspace",
                    "timeoutSeconds":5
                }
            }
        });
        let summary = runtime_request_summary(&payload, &PathBuf::from("/fallback"));
        assert!(summary.contains("id=call-42"));
        assert!(summary.contains("tool=run_shell"));
        assert!(summary.contains("workspace=/tmp/trace-workspace"));
        assert!(!summary.contains("SUPER_SECRET_VALUE"));
        assert!(!summary.contains("command="));
    }
}
