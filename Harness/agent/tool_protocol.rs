//! Normalization and recovery for model/tool protocol messages.
//!
//! Providers and MCP servers can stream partial calls, embed image resources,
//! or emit malformed textual tool calls. This module converts those variants
//! into Yeet's canonical `ToolCall` and model-visible output forms.

use std::collections::HashMap;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::core::{ImageAttachment, ToolCall};

/// Incrementally assembled tool call received from a streaming provider.
#[derive(Debug, Clone)]
pub(super) struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl PartialToolCall {
    /// Creates an empty partial call using deterministic fallbacks for missing fields.
    pub(super) fn new(_index: usize, id: Option<String>, name: Option<String>) -> Self {
        Self {
            id: id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("tool-{}", Uuid::new_v4().simple())),
            name: name
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "tool".into()),
            arguments: String::new(),
        }
    }

    /// Applies one streamed update to the partial call.
    pub(super) fn apply(
        &mut self,
        id: Option<String>,
        name: Option<String>,
        delta: Option<String>,
    ) {
        if let Some(id) = id.filter(|value| !value.trim().is_empty()) {
            self.id = id;
        }
        if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
            self.name = name;
        }
        if let Some(delta) = delta {
            self.arguments.push_str(&delta);
        }
    }

    /// Finalizes streamed JSON arguments without fabricating an empty object when
    /// the provider stopped mid-call. Invalid raw arguments are preserved so the
    /// coordinator can repair the model turn instead of executing the wrong call.
    fn finish(self) -> ToolCall {
        let raw = self.arguments.trim();
        let arguments = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw).unwrap_or(Value::String(self.arguments))
        };
        ToolCall {
            id: self.id,
            name: self.name,
            arguments,
        }
    }
}

/// Converts structured tool output into compact model text plus supported images.
///
/// Raw tool results remain untouched for execution bookkeeping, evidence, replay, and UI
/// events. This is the model-facing projection boundary: transport metadata, duplicated
/// MCP structured payloads, and opaque cache handles are removed here.
pub(super) fn normalize_tool_output_for_model(
    content: &str,
    vision_enabled: bool,
) -> (String, Vec<ImageAttachment>) {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return (content.to_owned(), Vec::new());
    };
    let Some(object) = value.as_object() else {
        return (render_model_value(&value), Vec::new());
    };
    let Some(items) = object.get("content").and_then(Value::as_array) else {
        return (render_model_value(&value), Vec::new());
    };

    let mut lines = Vec::new();
    let mut images = Vec::new();
    for item in items {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => {
                if let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    let rendered = serde_json::from_str::<Value>(text)
                        .map(|value| render_model_value(&value))
                        .unwrap_or_else(|_| text.to_owned());
                    if !rendered.is_empty() {
                        lines.push(rendered);
                    }
                }
            }
            "image" => {
                let media_type = item
                    .get("mimeType")
                    .or_else(|| item.get("mediaType"))
                    .or_else(|| item.get("mime_type"))
                    .and_then(Value::as_str)
                    .unwrap_or("image/png");
                let data = item.get("data").and_then(Value::as_str).unwrap_or_default();
                if vision_enabled && is_supported_image_media_type(media_type) && !data.is_empty() {
                    images.push(ImageAttachment {
                        media_type: media_type.to_owned(),
                        data: data.to_owned(),
                        name: None,
                    });
                    lines.push(format!("[image output: {media_type}]"));
                } else {
                    lines.push(format!("[image output: {media_type}; vision unavailable]"));
                }
            }
            "resource" => {
                let resource = item.get("resource").and_then(Value::as_object);
                if let Some(text) = resource
                    .and_then(|value| value.get("text"))
                    .and_then(Value::as_str)
                {
                    lines.push(text.to_owned());
                } else if let Some(uri) = resource
                    .and_then(|value| value.get("uri"))
                    .and_then(Value::as_str)
                {
                    lines.push(format!("[resource output: {uri}]"));
                }
            }
            other if !other.is_empty() => lines.push(format!("[{other} output]")),
            _ => {}
        }
    }

    // content is the canonical model-facing representation. MCP structuredContent
    // is frequently the same payload wrapped in workspace/transport metadata, so it
    // is only a fallback when the server supplied no usable content at all.
    if lines.is_empty()
        && let Some(structured) = object.get("structuredContent")
    {
        let rendered = render_model_value(structured);
        if !rendered.is_empty() {
            lines.push(rendered);
        }
    }
    if lines.is_empty() && object.get("isError").and_then(Value::as_bool) == Some(true) {
        lines.push("Tool reported an error.".into());
    }

    if lines.is_empty() {
        (render_model_value(&value), images)
    } else {
        (lines.join("\n"), images)
    }
}

/// Produces a semantic model-facing JSON projection without mutating the raw result.
fn project_tool_json_for_model(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(project_tool_json_for_model)
                .filter(|value| !value.is_null())
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(payload) = mcp_structured_payload(object) {
                return project_tool_json_for_model(payload);
            }

            let synchronous_shell = looks_like_synchronous_shell_result(object);
            let mut projected = serde_json::Map::new();
            for (key, value) in object {
                if value.is_null()
                    || matches!(
                        key.as_str(),
                        "_meta"
                            | "resultType"
                            | "route"
                            | "durationMilliseconds"
                            | "stdoutBytes"
                            | "stderrBytes"
                            | "requestedStartLine"
                            | "requestedEndLine"
                            | "refreshReplay"
                            | "snapshot"
                            | "duplicateReadBytesAvoided"
                            | "contentAlreadyReturned"
                    )
                    || (synchronous_shell
                        && matches!(key.as_str(), "command" | "workingDirectory" | "succeeded"))
                    || (matches!(key.as_str(), "stdoutTruncated" | "stderrTruncated")
                        && value.as_bool() == Some(false))
                    || (key == "data" && value.as_str().is_some_and(|text| text.len() > 512))
                {
                    continue;
                }

                let projected_value = project_tool_json_for_model(value);
                if projected_value.is_null()
                    || projected_value
                        .as_array()
                        .is_some_and(|values| values.is_empty())
                    || projected_value
                        .as_object()
                        .is_some_and(|values| values.is_empty())
                {
                    continue;
                }
                projected.insert(key.clone(), projected_value);
            }
            Value::Object(projected)
        }
        _ => value.clone(),
    }
}

/// Unwraps Yeet's legacy MCP structuredContent envelope when it contains no
/// information beyond the actual result plus transport metadata.
fn mcp_structured_payload<'a>(object: &'a serde_json::Map<String, Value>) -> Option<&'a Value> {
    let result = object.get("result")?;
    object
        .keys()
        .all(|key| {
            matches!(
                key.as_str(),
                "result" | "workspace" | "status" | "resultType" | "_meta"
            )
        })
        .then_some(result)
}

fn looks_like_synchronous_shell_result(object: &serde_json::Map<String, Value>) -> bool {
    !object.contains_key("status")
        && object.contains_key("exitCode")
        && (object.contains_key("stdout")
            || object.contains_key("stderr")
            || object.contains_key("stdoutBytes")
            || object.contains_key("durationMilliseconds")
            || object.contains_key("route"))
}

fn render_model_value(value: &Value) -> String {
    let projected = project_tool_json_for_model(value);
    match projected {
        Value::Null => String::new(),
        Value::String(text) => text,
        Value::Object(ref object) if object.is_empty() => String::new(),
        Value::Array(ref values) if values.is_empty() => String::new(),
        other => other.to_string(),
    }
}

/// Returns whether a media type can be passed through the model image interface.
fn is_supported_image_media_type(value: &str) -> bool {
    matches!(
        value,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    )
}

/// Merges decoded and streamed calls while preserving provider call order.
/// Calls whose arguments are not JSON objects are returned separately so the
/// coordinator can retry them without sending a malformed invocation to tools.
pub(super) fn collect_tool_calls(
    decoded: HashMap<usize, ToolCall>,
    partial: HashMap<usize, PartialToolCall>,
) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut all = decoded;
    for (index, partial) in partial {
        all.entry(index).or_insert_with(|| partial.finish());
    }
    let mut indexes: Vec<_> = all.keys().copied().collect();
    indexes.sort_unstable();
    let mut valid = Vec::new();
    let mut malformed = Vec::new();
    for index in indexes {
        let Some(call) = all.remove(&index) else {
            continue;
        };
        if call.arguments.is_object() {
            valid.push(call);
        } else {
            malformed.push(call);
        }
    }
    (valid, malformed)
}

/// Recovers structured calls from providers that emitted textual tool-call markup.
pub(super) fn recover_text_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = text[cursor..].find("<tool_call>") {
        let start = cursor + relative_start + "<tool_call>".len();
        let Some(relative_end) = text[start..].find("</tool_call>") else {
            break;
        };
        let end = start + relative_end;
        blocks.push(&text[start..end]);
        cursor = end + "</tool_call>".len();
    }
    if blocks.is_empty() && text.contains("<function=") {
        blocks.push(text);
    }
    blocks
        .into_iter()
        .filter_map(parse_text_tool_call_block)
        .collect()
}

/// Parses one JSON or XML-like textual tool-call block.
fn parse_text_tool_call_block(block: &str) -> Option<ToolCall> {
    let trimmed = block.trim().trim_matches('`').trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && let Some(call) = tool_call_from_json(&value)
    {
        return Some(call);
    }

    let function_marker = "<function=";
    let function_start = trimmed.find(function_marker)? + function_marker.len();
    let function_name_end = function_start + trimmed[function_start..].find('>')?;
    let name = trimmed[function_start..function_name_end].trim();
    if name.is_empty() {
        return None;
    }
    let body_start = function_name_end + 1;
    let body_end = trimmed[body_start..]
        .find("</function>")
        .map(|index| body_start + index)
        .unwrap_or(trimmed.len());
    let body = &trimmed[body_start..body_end];
    let mut arguments = serde_json::Map::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = body[cursor..].find("<parameter=") {
        let parameter_start = cursor + relative_start + "<parameter=".len();
        let Some(relative_name_end) = body[parameter_start..].find('>') else {
            break;
        };
        let parameter_name_end = parameter_start + relative_name_end;
        let parameter_name = body[parameter_start..parameter_name_end].trim();
        if parameter_name.is_empty() {
            break;
        }
        let value_start = parameter_name_end + 1;
        let Some(relative_value_end) = body[value_start..].find("</parameter>") else {
            break;
        };
        let value_end = value_start + relative_value_end;
        let raw = body[value_start..value_end].trim();
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
        arguments.insert(parameter_name.to_owned(), value);
        cursor = value_end + "</parameter>".len();
    }
    if arguments.is_empty()
        && let Ok(Value::Object(object)) = serde_json::from_str::<Value>(body.trim())
    {
        arguments = object;
    }
    Some(ToolCall {
        id: format!("recovered-{}", Uuid::new_v4().simple()),
        name: name.to_owned(),
        arguments: Value::Object(arguments),
    })
}

/// Converts common provider JSON call shapes into Yeet's canonical call type.
fn tool_call_from_json(value: &Value) -> Option<ToolCall> {
    let object = value.as_object()?;
    let function = object.get("function").and_then(Value::as_object);
    let name = object.get("name").and_then(Value::as_str).or_else(|| {
        function
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
    })?;
    let raw_arguments = object
        .get("arguments")
        .or_else(|| function.and_then(|value| value.get("arguments")))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arguments = match raw_arguments {
        Value::String(raw) => {
            let parsed = serde_json::from_str::<Value>(&raw).ok()?;
            parsed.is_object().then_some(parsed)?
        }
        other if other.is_object() => other,
        _ => return None,
    };
    Some(ToolCall {
        id: object
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("recovered-{}", Uuid::new_v4().simple())),
        name: name.to_owned(),
        arguments,
    })
}

/// Detects provider text that looks like an unexecuted textual tool call.
pub(super) fn looks_like_malformed_tool_call(text: &str) -> bool {
    let value = text.trim().to_ascii_lowercase();
    value.starts_with("<tool_call")
        || value.starts_with("<function=")
        || value.starts_with("```tool_call")
        || value.starts_with("<|tool_call|>")
        || (value.contains("<tool_call>") && value.contains("<function="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_stream_call_ids_are_unique_across_rounds() {
        let first = PartialToolCall::new(0, None, Some("lookup".into())).finish();
        let second = PartialToolCall::new(0, None, Some("lookup".into())).finish();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn mcp_content_is_not_repeated_from_structured_content() {
        let payload = json!({
            "content": [{
                "type": "text",
                "text": "{\"value\":42,\"durationMilliseconds\":8}"
            }],
            "structuredContent": {
                "workspace": "/tmp/project",
                "result": {"value":42,"durationMilliseconds":8}
            },
            "isError": false
        })
        .to_string();

        let (text, images) = normalize_tool_output_for_model(&payload, false);

        assert!(images.is_empty());
        assert_eq!(text, "{\"value\":42}");
        assert!(!text.contains("Structured output"));
        assert!(!text.contains("/tmp/project"));
    }

    #[test]
    fn shell_projection_keeps_output_and_drops_execution_bookkeeping() {
        let payload = json!({
            "route": "actor",
            "command": "printf ok",
            "workingDirectory": "/tmp/project",
            "exitCode": 0,
            "succeeded": true,
            "durationMilliseconds": 12,
            "stdoutBytes": 3,
            "stderrBytes": 0,
            "stdoutTruncated": false,
            "stderrTruncated": false,
            "stdout": "ok\n",
            "stderr": null
        })
        .to_string();

        let (text, _) = normalize_tool_output_for_model(&payload, false);
        let projected: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(projected["exitCode"], json!(0));
        assert_eq!(projected["stdout"], json!("ok\n"));
        for key in [
            "route",
            "command",
            "workingDirectory",
            "succeeded",
            "durationMilliseconds",
            "stdoutBytes",
            "stderrBytes",
            "stdoutTruncated",
            "stderrTruncated",
            "stderr",
        ] {
            assert!(
                projected.get(key).is_none(),
                "{key} leaked into model output"
            );
        }
    }

    #[test]
    fn structured_only_result_unwraps_yeet_transport_envelope() {
        let payload = json!({
            "content": [],
            "structuredContent": {
                "workspace": "/tmp/project",
                "result": {
                    "path": "src/main.rs",
                    "snapshot": "opaque-cache-handle",
                    "requestedStartLine": 1,
                    "requestedEndLine": 5,
                    "startLine": 1,
                    "endLine": 5,
                    "lines": "1:abcd|fn main() {}"
                }
            },
            "isError": false
        })
        .to_string();

        let (text, _) = normalize_tool_output_for_model(&payload, false);
        let projected: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(projected["path"], json!("src/main.rs"));
        assert_eq!(projected["lines"], json!("1:abcd|fn main() {}"));
        assert!(projected.get("workspace").is_none());
        assert!(projected.get("snapshot").is_none());
        assert!(projected.get("requestedStartLine").is_none());
        assert!(projected.get("requestedEndLine").is_none());
    }
}
