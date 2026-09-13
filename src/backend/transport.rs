//! Bridge-envelope creation and concise tool-activity presentation.
//!
//! Transport serialization and UI-facing labels are kept out of the backend
//! dispatcher so execution logic does not depend on presentation details.

use serde_json::Value;

use crate::{
    core::{ToolCall, Usage},
    model::{BridgeEnvelope, BridgeState},
};

/// Wraps a full bridge state in the standard state envelope.
pub(super) fn state_envelope(state: &BridgeState) -> BridgeEnvelope {
    BridgeEnvelope {
        kind: "state".into(),
        state: Some(state.clone()),
        message: None,
    }
}

/// Wraps a compact state update that deliberately omits conversation history.
pub(super) fn state_envelope_without_conversation(state: &BridgeState) -> BridgeEnvelope {
    BridgeEnvelope {
        kind: "state".into(),
        state: Some(state.without_conversation()),
        message: None,
    }
}

/// Renders JSON arguments for transcript-visible tool calls.
pub(super) fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Formats cache usage without treating unreported provider telemetry as misses.
pub(super) fn cache_status_line(usage: &Usage) -> String {
    let cache = usage.cache_measurement();
    if let Some(hit_rate) = cache.hit_rate {
        let coverage = cache.measurement_coverage_rate.unwrap_or(0.0) * 100.0;
        format!(
            "Cache: read {}/{} measured tokens ({:.0}% hit) · write {} tokens · {:.0}% input measured",
            cache.cached_input_tokens,
            cache.measured_input_tokens,
            hit_rate * 100.0,
            cache.cache_write_input_tokens,
            coverage,
        )
    } else if cache.unclassified_input_tokens > 0 {
        format!(
            "Cache: read {} tokens · write {} tokens · hit rate unavailable ({} legacy/unclassified input tokens)",
            cache.cached_input_tokens,
            cache.cache_write_input_tokens,
            cache.unclassified_input_tokens,
        )
    } else if cache.unreported_input_tokens > 0 {
        format!(
            "Cache: read {} tokens · write {} tokens · hit rate unavailable (cache telemetry unreported for {} input tokens)",
            cache.cached_input_tokens,
            cache.cache_write_input_tokens,
            cache.unreported_input_tokens,
        )
    } else {
        format!(
            "Cache: read {} tokens · write {} tokens",
            cache.cached_input_tokens, cache.cache_write_input_tokens
        )
    }
}

/// Maps a tool name to the short activity title shown while it runs.
pub(super) fn tool_activity_title(name: &str) -> String {
    match name {
        "read_file" | "read_files" => "Reading",
        "list_files" => "Scanning",
        "search_workspace" => "Searching",
        "web_search" => "Searching web",
        "web_read" => "Reading web source",
        "read_artifact" => "Reading output",
        "search_artifact" => "Searching output",
        "apply_file_edits" => "Editing",
        "find_capabilities" => "Finding tools",
        "activate_capability" => "Loading",
        "run_shell" => "Running command",
        "request_shell_permission" => "Requesting permission",
        _ if name.starts_with("mcp_") => "Using MCP",
        _ if name.starts_with("worker_") => "Running worker",
        _ if name.starts_with("skill_") => "Reading skill",
        _ => "Running",
    }
    .into()
}

/// Extracts the most useful short argument for a running tool activity row.
pub(super) fn tool_detail(call: &ToolCall) -> Option<String> {
    let args = &call.arguments;
    match call.name.as_str() {
        "apply_file_edits" => {
            let changes = args.get("changes")?.as_array()?;
            let first = changes.first()?.get("path")?.as_str()?;
            Some(if changes.len() > 1 {
                let extra = changes.len() - 1;
                format!(
                    "{first} +{extra} {}",
                    if extra == 1 { "file" } else { "files" }
                )
            } else {
                first.to_owned()
            })
        }
        "read_file" | "read_files" if args.get("requests").is_some() => {
            let requests = args.get("requests")?.as_array()?;
            let first = requests.first()?.get("path")?.as_str()?;
            Some(if requests.len() > 1 {
                let extra = requests.len() - 1;
                format!(
                    "{first} +{extra} {}",
                    if extra == 1 { "file" } else { "files" }
                )
            } else {
                first.to_owned()
            })
        }
        "search_workspace" => {
            let query = args.get("query")?.as_str()?;
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            Some(match path {
                Some(path) => format!("{query} · {path}"),
                None => query.to_owned(),
            })
        }
        "web_search" => args.get("query").and_then(Value::as_str).map(str::to_owned),
        "web_read" => args.get("url").and_then(Value::as_str).map(str::to_owned),
        "list_files" => args.get("path").and_then(Value::as_str).map(|value| {
            if value.is_empty() {
                ".".to_owned()
            } else {
                value.to_owned()
            }
        }),
        "read_artifact" | "search_artifact" => {
            args.get("id").and_then(Value::as_str).map(str::to_owned)
        }
        _ => args
            .get("path")
            .and_then(Value::as_str)
            .or_else(|| args.get("command").and_then(Value::as_str))
            .or_else(|| args.get("capability").and_then(Value::as_str))
            .map(str::to_owned)
            .or_else(|| Some(call.name.replace('_', " "))),
    }
}
